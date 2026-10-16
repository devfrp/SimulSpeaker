import json
import unittest

from tests.helpers import FakeHrtf, FakeRunner, make_cea_text, temp_paths

import system
from app import SpeakerSimApp
from download import Downloader, HeadphoneEntry
from tests.test_download import INDEX, FakeSession
from tests.test_system import DUMP


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
            for key in ("LL", "LR", "RL", "RR"):
                self.assertTrue((paths.filters_dir / f"ir_{key}.wav").is_file())
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


if __name__ == "__main__":
    unittest.main()
