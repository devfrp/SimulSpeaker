import json
import unittest

from tests.helpers import FakeHrtf, FakeRunner, make_cea_text, temp_paths

import system
from app import SpeakerSimApp
from download import Downloader, HeadphoneEntry
from tests.test_download import INDEX, FakeSession
from tests.test_system import DUMP, NODE_A, NODE_B, make_dump


def make_app(paths, runner=None):
    session = FakeSession({
        "INDEX.md": INDEX.encode(),
        "CEA2034.txt": make_cea_text().encode(),
        "ParametricEQ.txt": b"Preamp: -3 dB\nFilter 1: ON PK Fc 1000 Hz Gain 3 dB Q 1\n",
        ".sofa": b"fake",
    })
    runner = runner or FakeRunner({"pw-dump": (0, json.dumps(DUMP)),
                                   "pw-metadata": (0, "key:'clock.rate' value:'48000'"),
                                   "systemctl --user is-active": (3, "")})
    downloader = Downloader(paths.cache_dir, session=session)
    downloader.ensure_hrtf = lambda preferred="", progress=None: paths.cache_dir / "x.sofa"
    return SpeakerSimApp(paths, runner=runner, downloader=downloader,
                         hrtf_factory=lambda _p, fs: FakeHrtf(fs=fs))


class AppTests(unittest.TestCase):
    def test_build_filters_end_to_end(self):
        tmp, paths = temp_paths()
        with tmp:
            app = make_app(paths)
            hp = app.search_headphones("hd 650")[0]
            app.select_headphone(hp)
            app.update_settings(ir_length_ms=150, rt60=0.15)
            progress = []
            result = app.build_filters(lambda f, m: progress.append(f))
            self.assertEqual(result.fs, 48000)
            self.assertEqual(progress[-1], 1.0)
            for slot in ("a", "b"):
                for key in ("LL", "LR", "RL", "RR"):
                    self.assertTrue((paths.slot_dir(slot) / f"ir_{key}.wav").is_file())
            self.assertIn(NODE_B, paths.slot_conf("b").read_text())
            conf = json.loads(paths.filter_conf.read_text())
            args = conf["context.modules"][-1]["args"]
            # Sortie cible : la sortie effective par défaut (le simulateur est exclu).
            self.assertEqual(args["playback.props"]["target.object"], "alsa_output.analog")
            self.assertIn("KEF R3", args["node.description"])

    def test_settings_persist_between_instances(self):
        tmp, paths = temp_paths()
        with tmp:
            app = make_app(paths)
            app.select_speaker("Genelec 8030C")
            app.select_headphone(HeadphoneEntry("A", "B", "over-ear", "B/over-ear/A"))
            again = make_app(paths)
            self.assertEqual(again.settings.speaker_id, "Genelec 8030C")
            self.assertEqual(again.headphone.name, "A")

    def test_unknown_speaker(self):
        tmp, paths = temp_paths()
        with tmp:
            from app import AppError
            with self.assertRaises(AppError):
                make_app(paths).select_speaker("Nope 9000")

    def test_explicit_output_sink(self):
        tmp, paths = temp_paths()
        with tmp:
            app = make_app(paths)
            app.update_settings(output_sink="bluez_output.hp")
            self.assertEqual(app.target_sink(), "bluez_output.hp")
            app.update_settings(output_sink="disparu")
            self.assertEqual(app.target_sink(), "alsa_output.analog")

    def test_status(self):
        tmp, paths = temp_paths()
        with tmp:
            st = make_app(paths).status()
            self.assertFalse(st["running"])
            self.assertEqual(st["speaker"], "KEF R3")
            self.assertFalse(st["sim_is_default"])
            self.assertEqual(system.NODE_NAME, "devfrp_speaker_sim_py")



class RoomSettingsTests(unittest.TestCase):
    def test_preset_changes_rt60_and_layout(self):
        tmp, paths = temp_paths()
        with tmp:
            app = make_app(paths)
            living = app.room_spec().rt60
            app.apply_room_preset("studio")
            self.assertEqual(app.settings.room_preset, "studio")
            self.assertLess(app.room_spec().rt60, living / 2)
            layout = app.layout()
            self.assertAlmostEqual(layout.speakers["L"][0], 5.5 - 0.6)

    def test_manual_rt60(self):
        tmp, paths = temp_paths()
        with tmp:
            app = make_app(paths)
            app.update_settings(rt60_auto=False, rt60=0.8)
            self.assertAlmostEqual(app.room_spec().rt60, 0.8, places=6)


class SwitchTests(unittest.TestCase):
    """Bascule A→B : démarrer B, en faire la sortie, attendre les flux, arrêter A."""

    def _app(self, paths, active):
        app = make_app(paths)
        calls = []
        sd = app.systemd
        sd.active_slots = lambda: list(active)
        sd.legacy_active = lambda: False

        def start(slot):
            calls.append(("start", slot))
            active.append(slot)

        def stop(slot=None):
            calls.append(("stop", slot))
            if slot in active:
                active.remove(slot)

        sd.start, sd.stop = start, stop
        app.pw.wait_for_sink = lambda name, timeout=8.0: object()
        app.pw.default_sink = lambda configured_only=False: NODE_B
        app.pw.wait_streams_left = lambda name, timeout=3.0: calls.append(("wait", name)) or True
        return app, calls

    def test_switch_from_a_to_b_without_gap(self):
        tmp, paths = temp_paths()
        with tmp:
            app, calls = self._app(paths, ["a"])
            self.assertEqual(app._switch(lambda *_: None), "b")
            self.assertEqual(calls, [("start", "b"), ("wait", NODE_A), ("stop", "a")])

    def test_first_start_uses_a(self):
        tmp, paths = temp_paths()
        with tmp:
            app, calls = self._app(paths, [])
            app.pw.default_sink = lambda configured_only=False: NODE_A
            self.assertEqual(app._switch(lambda *_: None), "a")
            self.assertEqual(calls, [("start", "a")])


MEASUREMENT = "* REW export\nFreq(Hz) SPL(dB) Phase\n" + "\n".join(
    f"{f:.1f} {80 + (3 if f > 3000 else 0):.2f} 0" for f in
    [20 * 1.12 ** i for i in range(61)]) + "\n"
TARGET_CSV = "frequency,raw\n" + "\n".join(f"{20 * 1.12 ** i:.1f},0.0" for i in range(61)) + "\n"


class ABTests(unittest.TestCase):
    def test_switch_keeps_both_configurations(self):
        tmp, paths = temp_paths()
        with tmp:
            app = make_app(paths)
            app.update_settings(speaker_id="KEF R3", bass_db=2.0)
            self.assertEqual(app.ab_slot(), "A")
            app.ab_switch("B")                       # B vide : copie de A
            self.assertEqual(app.ab_slot(), "B")
            self.assertEqual(app.settings.bass_db, 2.0)
            app.apply_room_preset("studio")
            app.update_settings(bass_db=-4.0, output_sink="casque")
            app.ab_switch("A")
            self.assertEqual(app.settings.bass_db, 2.0)
            self.assertEqual(app.settings.room_preset, "salon")
            self.assertEqual(app.settings.output_sink, "casque")   # matériel commun
            app.ab_switch("B")
            self.assertEqual((app.settings.bass_db, app.settings.room_preset), (-4.0, "studio"))
            reloaded = make_app(paths)
            self.assertEqual(reloaded.ab_slot(), "B")


class ImportTests(unittest.TestCase):
    def test_import_parametric_eq(self):
        tmp, paths = temp_paths()
        with tmp:
            app = make_app(paths)
            src = paths.cache_dir.parent / "mon eq.txt"
            src.write_text("Preamp: -2 dB\nFilter 1: ON PK Fc 3000 Hz Gain -3 dB Q 2\n")
            entry = app.import_headphone(src, "Mon casque")
            self.assertTrue(entry.is_import)
            self.assertEqual(entry.rig, "EQ paramétrique")
            self.assertEqual(app.headphone.name, "Mon casque")
            self.assertEqual([e.name for e in app.imported_headphones()], ["Mon casque"])
            self.assertEqual(app.search_headphones("mon casque")[0].path, entry.path)
            app.update_settings(ir_length_ms=100, room_enabled=False)
            app.build_filters()

    def test_import_raw_measurement_uses_target(self):
        tmp, paths = temp_paths()
        with tmp:
            app = make_app(paths)
            app.downloader.session.routes["Harman%20in-ear%202019.csv"] = TARGET_CSV.encode()
            src = paths.cache_dir.parent / "mesure.txt"
            src.write_text(MEASUREMENT)
            entry = app.import_headphone(src, "", "Harman in-ear 2019")
            self.assertEqual(entry.name, "mesure")
            self.assertEqual(entry.target, "Harman in-ear 2019")
            eq = app._headphone_eq(entry)
            f, g = eq.graphic
            # +3 dB mesurés au-dessus de 3 kHz par rapport à une cible plate → correction ≈ −3 dB
            self.assertAlmostEqual(float(g[(f > 5000) & (f < 9000)].mean()), -3.0, delta=0.6)
            self.assertAlmostEqual(float(g[(f > 300) & (f < 1500)].mean()), 0.0, delta=0.4)

    def test_import_rejects_garbage_and_unknown_target(self):
        tmp, paths = temp_paths()
        with tmp:
            from app import AppError
            app = make_app(paths)
            bad = paths.cache_dir.parent / "x.txt"
            bad.write_text("bonjour\n1 2\n")
            with self.assertRaises(AppError):
                app.import_headphone(bad)
            good = paths.cache_dir.parent / "m.txt"
            good.write_text(MEASUREMENT)
            with self.assertRaises(AppError):
                app.import_headphone(good, target="Inexistante")

    def test_change_import_target(self):
        tmp, paths = temp_paths()
        with tmp:
            from app import AppError
            app = make_app(paths)
            src = paths.cache_dir.parent / "mesure.txt"
            src.write_text(MEASUREMENT)
            entry = app.import_headphone(src, "M", "Harman in-ear 2019")
            updated = app.set_import_target(entry, "Harman over-ear 2018")
            self.assertEqual(app.headphone.target, "Harman over-ear 2018")
            self.assertEqual(app.imported_headphones()[0].rig, "mesure → Harman over-ear 2018")
            self.assertEqual(updated.path, entry.path)
            with self.assertRaises(AppError):
                app.set_import_target(entry, "Inexistante")

    def test_summary_and_alternatives(self):
        tmp, paths = temp_paths()
        with tmp:
            app = make_app(paths)
            entries = app.headphones()
            hd650 = next(e for e in entries if e.name == "Sennheiser HD 650")
            app.select_headphone(hd650)
            self.assertEqual(app.headphone_summary(),
                             {"filters": 1, "points": 0, "preamp_db": -3.0})
            self.assertEqual([e.path for e in app.alternatives(hd650)], [hd650.path])

    def test_remove_import(self):
        tmp, paths = temp_paths()
        with tmp:
            app = make_app(paths)
            src = paths.cache_dir.parent / "eq.txt"
            src.write_text("GraphicEQ: 20 0; 1000 0; 20000 -3")
            entry = app.import_headphone(src)
            app.remove_import(entry)
            self.assertEqual(app.imported_headphones(), [])
            self.assertIsNone(app.headphone)


if __name__ == "__main__":
    unittest.main()
