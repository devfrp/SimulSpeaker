import unittest

import numpy as np
from scipy import signal
from scipy.io import wavfile

from tests.helpers import FakeHrtf, make_cea_text, temp_paths

import audio_engine as ae

PARAMETRIC = """Preamp: -6.1 dB
Filter 1: ON LSC Fc 105 Hz Gain 6.4 dB Q 0.70
Filter 2: ON PK Fc 8800 Hz Gain 5.1 dB Q 1.42
Filter 3: ON HSC Fc 10000 Hz Gain -2.1 dB Q 0.70
Filter 4: OFF PK Fc 1000 Hz Gain 9 dB Q 1
"""


class ParametricEqTests(unittest.TestCase):
    def test_parse(self):
        eq = ae.parse_parametric_eq(PARAMETRIC)
        self.assertAlmostEqual(eq.preamp_db, -6.1)
        self.assertEqual([f.kind for f in eq.filters], ["LSC", "PK", "HSC"])  # OFF ignoré
        self.assertAlmostEqual(eq.filters[1].fc, 8800)
        self.assertAlmostEqual(eq.filters[1].q, 1.42)

    def test_peaking_gain_at_center(self):
        for gain in (6.0, -4.5):
            bq = ae.Biquad("PK", 1000, gain, 1.0)
            b, a = ae.biquad_coefficients(bq, 48000)
            _, h = signal.freqz(b, a, worN=[2 * np.pi * 1000 / 48000])
            self.assertAlmostEqual(20 * np.log10(abs(h[0])), gain, places=3)

    def test_shelves_asymptotes(self):
        fs = 48000
        low = ae.biquad_coefficients(ae.Biquad("LSC", 200, 6, 0.707), fs)
        high = ae.biquad_coefficients(ae.Biquad("HSC", 2000, -6, 0.707), fs)
        w = 2 * np.pi * np.array([10, 20000]) / fs
        _, hl = signal.freqz(*low, worN=w)
        _, hh = signal.freqz(*high, worN=w)
        self.assertAlmostEqual(20 * np.log10(abs(hl[0])), 6, delta=0.1)
        self.assertAlmostEqual(20 * np.log10(abs(hl[1])), 0, delta=0.1)
        self.assertAlmostEqual(20 * np.log10(abs(hh[1])), -6, delta=0.2)

    def test_filter_above_nyquist_skipped(self):
        self.assertIsNone(ae.biquad_coefficients(ae.Biquad("PK", 30000, 3, 1), 48000))

    def test_fir_matches_response(self):
        fs = 48000
        eq = ae.parse_parametric_eq(PARAMETRIC)
        h = ae.headphone_eq_fir(eq, fs)
        freqs = np.array([50, 105, 1000, 8800, 15000])
        expected = ae.eq_response_db(eq, freqs, fs)
        _, hf = signal.freqz(h, worN=2 * np.pi * freqs / fs)
        np.testing.assert_allclose(20 * np.log10(np.abs(hf)), expected, atol=0.2)

    def test_graphic_eq(self):
        eq = ae.parse_graphic_eq("GraphicEQ: 20 -3; 1000 0; 20000 4")
        self.assertIsNotNone(eq.graphic)
        h = ae.headphone_eq_fir(eq, 48000)
        _, hf = signal.freqz(h, worN=[2 * np.pi * 1000 / 48000])
        self.assertAlmostEqual(20 * np.log10(abs(hf[0])), 0.0, delta=0.3)


class MinimumPhaseTests(unittest.TestCase):
    def test_magnitude_and_energy_front_loaded(self):
        fs = 48000
        f = np.array([20, 100, 1000, 5000, 20000])
        db = np.array([-10, -3, 0, 3, -6])
        h = ae.minimum_phase_fir(f, db, fs, 4096)
        probe = np.array([100, 1000, 5000])
        _, hf = signal.freqz(h, worN=2 * np.pi * probe / fs)
        np.testing.assert_allclose(20 * np.log10(np.abs(hf)), [-3, 0, 3], atol=0.5)
        energy = np.cumsum(h ** 2) / np.sum(h ** 2)
        self.assertGreater(energy[256], 0.95)  # phase minimale : énergie au début

    def test_octave_smoothing_flat(self):
        freqs = np.linspace(0, 24000, 1025)
        out = ae.smooth_octave(freqs, np.ones(1025), 3)
        np.testing.assert_allclose(out, 1.0)


class CeaTests(unittest.TestCase):
    def test_parse_with_thousands_and_normalisation(self):
        curves = ae.parse_cea2034(make_cea_text())
        self.assertIn("Sound Power", curves)
        f, _ = curves["Listening Window"]
        self.assertAlmostEqual(f[-1], 20000, delta=1)  # « 20,000.00 » bien lu
        spk = ae.SpeakerResponse.from_curves(curves)
        self.assertAlmostEqual(float(np.mean(spk.listening_window)), 0.0, places=6)
        self.assertAlmostEqual(spk.directivity_index(), 6.0, places=6)

    def test_missing_listening_window(self):
        bad = '"CEA2034"\n"On Axis"\n"f"\t"spl"\n100\t80\n200\t81\n'
        with self.assertRaises(ae.AudioEngineError):
            ae.parse_cea2034(bad)


def make_polar_text(plane: str, beam: float) -> str:
    """Fichier polaire Klippel synthétique : niveau qui baisse avec l'angle (« beam » dB/10°)."""
    angles = [0] + [a for k in range(10, 180, 10) for a in (k, -k)] + [180]
    names = "\t\t".join('"On-Axis"' if a == 0 else f'"{a}°"' for a in angles)
    rows = []
    for f in np.geomspace(20, 20000, 40):
        cells = []
        for a in angles:
            cells += [f"{f:.4f}", f"{86 - beam * abs(a) / 10:.3f}"]
        rows.append("\t".join(cells))
    return f'"SPL {plane}"\n{names}\n"Frequency"\t"SPL"\n' + "\n".join(rows) + "\n"


class PolarTests(unittest.TestCase):
    def test_omni_gives_identical_curves(self):
        h = ae.parse_klippel_polar(make_polar_text("Horizontal", 0))
        v = ae.parse_klippel_polar(make_polar_text("Vertical", 0))
        self.assertEqual(len(h[1]), 36)
        curves = ae.cea2034_from_polar(h, v)
        for name in ("Listening Window", "Early Reflections", "Sound Power"):
            np.testing.assert_allclose(curves[name][1], 86.0, atol=1e-9)

    def test_directional_speaker_has_positive_di(self):
        h = ae.parse_klippel_polar(make_polar_text("Horizontal", 1.5))
        v = ae.parse_klippel_polar(make_polar_text("Vertical", 1.5))
        spk = ae.SpeakerResponse.from_curves(ae.cea2034_from_polar(h, v))
        self.assertGreater(spk.directivity_index(), 3)
        self.assertTrue(np.all(spk.early_reflections < spk.listening_window))

    def test_load_speaker_from_folder(self):
        tmp, paths = temp_paths()
        with tmp:
            folder = paths.cache_dir / "spk"
            folder.mkdir()
            (folder / "SPL Horizontal.txt").write_text(make_polar_text("Horizontal", 1))
            (folder / "SPL Vertical.txt").write_text(make_polar_text("Vertical", 1))
            self.assertGreater(ae.load_speaker(folder).directivity_index(), 1)


class RoomTests(unittest.TestCase):
    def test_image_source_counts(self):
        room = ae.RoomSpec(4, 5, 2.5)
        src = np.array([1.0, 1.5, 1.2])
        counts = [len(ae.image_sources(src, room, k)) for k in (0, 1, 2)]
        self.assertEqual(counts, [1, 7, 25])

    def test_first_order_images_hit_one_named_surface(self):
        room = ae.RoomSpec(4, 5, 2.5)
        src = np.array([1.0, 1.5, 1.2])
        imgs = [i for i in ae.image_sources(src, room, 1) if i.reflections == 1]
        by_surface = {ae.SURFACES[i.counts.index(1)]: i.position for i in imgs}
        self.assertEqual(set(by_surface), set(ae.SURFACES))
        self.assertAlmostEqual(by_surface["floor"][2], -1.2)            # sol
        self.assertAlmostEqual(by_surface["ceiling"][2], 2 * 2.5 - 1.2)  # plafond
        self.assertAlmostEqual(by_surface["front"][0], 2 * 5 - 1.0)      # mur avant (x = L)
        self.assertAlmostEqual(by_surface["rear"][0], -1.0)

    def test_treatment_shortens_rt60_mostly_above_bass(self):
        bare = ae.RoomSpec(treatment=0.0).rt60_bands()
        treated = ae.RoomSpec(treatment=0.6).rt60_bands()
        self.assertTrue(np.all(treated < bare))
        ratio = treated / bare
        self.assertGreater(ratio[0], ratio[3])   # les panneaux agissent peu à 125 Hz

    def test_bass_traps_act_on_low_bands(self):
        base = ae.RoomSpec().rt60_bands()
        traps = ae.RoomSpec(bass_traps=1.0).rt60_bands()
        self.assertLess(traps[0], base[0] * 0.8)
        self.assertGreater(traps[4], base[4] * 0.9)

    def test_materials_and_furniture(self):
        tile = ae.RoomSpec(floor="tile", furniture="empty").rt60
        carpet = ae.RoomSpec(floor="carpet", furniture="full").rt60
        self.assertGreater(tile, 2 * carpet)
        self.assertLess(ae.RoomSpec(walls="drywall").rt60_bands()[0],
                        ae.RoomSpec(walls="masonry").rt60_bands()[0])

    def test_rt60_override(self):
        room = ae.RoomSpec(rt60_override=0.5)
        self.assertAlmostEqual(room.rt60, 0.5, places=6)
        # L'absorption uniforme déduite reproduit ce RT : β cohérent avec le Sabine/Eyring
        self.assertTrue(np.all((room.reflection_bands() > 0) & (room.reflection_bands() < 1)))

    def test_reflection_filtered_by_treated_walls(self):
        room = ae.RoomSpec(treatment=1.0)
        beta = room.reflection_bands()
        img = next(i for i in ae.image_sources(np.array([5.0, 2.0, 1.2]), room, 1)
                   if i.counts[ae.SURFACES.index("left")] == 1)
        g = img.gain_bands(beta)
        self.assertGreater(g[0], 3 * g[4])   # panneau : grave réfléchi, aigu absorbé

    def test_diffusion_reduces_rear_specular(self):
        self.assertLess(ae.RoomSpec(diffusion=1.0).reflection_bands()[0, 3],
                        ae.RoomSpec(diffusion=0.0).reflection_bands()[0, 3])

    def test_order_4_image_count(self):
        room = ae.RoomSpec(5, 6, 3)
        self.assertEqual(len(ae.image_sources(np.array([4.0, 2.0, 1.2]), room, 4)), 129)

    def test_per_wall_treatment_only_changes_that_wall(self):
        base = ae.RoomSpec(per_wall=True).reflection_bands()
        front = ae.RoomSpec(per_wall=True, treatment_front=1.0).reflection_bands()
        i_front = ae.SURFACES.index("front")
        self.assertLess(front[i_front, 3], base[i_front, 3] * 0.5)
        others = [i for i in range(6) if i != i_front]
        np.testing.assert_allclose(front[others], base[others])

    def test_absorber_types(self):
        lf = {a: ae.RoomSpec(treatment=0.6, absorber=a).rt60_bands()[1]
              for a in ("foam5", "wool10", "wool20")}
        self.assertGreater(lf["foam5"], lf["wool10"])
        self.assertGreater(lf["wool10"], lf["wool20"])

    def test_speaker_height_and_listener_offset(self):
        room = ae.RoomSpec(4.5, 6, 2.6, speaker_height=1.0, listener_offset=0.5)
        lay = ae.stereo_layout(room, 30, 2.0)
        self.assertAlmostEqual(lay.speakers["L"][2], 1.0)
        self.assertAlmostEqual(lay.listener[2], 1.2)
        self.assertAlmostEqual(lay.listener[1], 4.5 / 2 + 0.5)
        dl = np.linalg.norm(lay.speakers["L"] - lay.listener)
        dr = np.linalg.norm(lay.speakers["R"] - lay.listener)
        self.assertLess(dl, dr)   # décalé vers la gauche : enceinte gauche plus proche

    def test_layout_symmetry_front_gap_and_errors(self):
        room = ae.RoomSpec(4.5, 6, 2.6, front_gap=1.0, ear_height=1.1)
        layout = ae.stereo_layout(room, 30, 2.0)
        left, right = layout.speakers["L"], layout.speakers["R"]
        self.assertAlmostEqual(np.linalg.norm(left - layout.listener), 2.0)
        self.assertGreater(left[1], right[1])  # y vers la gauche
        self.assertAlmostEqual(left[0], 5.0)   # 1 m du mur avant
        self.assertAlmostEqual(layout.listener[2], 1.1)
        with self.assertRaises(ae.AudioEngineError):
            ae.stereo_layout(ae.RoomSpec(2.0, 6, 2.6), 60, 3.0)


class RenderTests(unittest.TestCase):
    def setUp(self):
        self.speaker = ae.SpeakerResponse.from_curves(ae.parse_cea2034(make_cea_text()))

    def _render(self, **kw):
        params = ae.RenderParams(fs=48000, ir_length_ms=200,
                                 room=ae.RoomSpec(rt60_override=0.2), **kw)
        return ae.render(self.speaker, None, FakeHrtf(fs=48000), params)

    def test_render_shapes_and_headroom(self):
        res = self._render()
        self.assertEqual(set(res.irs), set(ae.PAIRS))
        lengths = {len(v) for v in res.irs.values()}
        self.assertEqual(len(lengths), 1)
        for ear in "LR":
            mono = res.irs["L" + ear].astype(float) + res.irs["R" + ear].astype(float)
            spec = np.abs(np.fft.rfft(mono, 1 << 16))
            self.assertLess(20 * np.log10(spec.max()), 3.0)

    def test_ipsilateral_louder_and_symmetric(self):
        res = self._render(room_enabled=False)
        e = {k: float(np.sum(v.astype(float) ** 2)) for k, v in res.irs.items()}
        self.assertGreater(e["LL"], e["LR"])
        self.assertGreater(e["RR"], e["RL"])
        self.assertAlmostEqual(e["LL"], e["RR"], delta=e["LL"] * 0.02)

    def test_room_adds_tail(self):
        dry = self._render(room_enabled=False)
        wet = self._render(room_enabled=True)
        tail = slice(int(0.05 * 48000), None)
        self.assertGreater(np.sum(wet.irs["LL"][tail].astype(float) ** 2),
                           10 * np.sum(dry.irs["LL"][tail].astype(float) ** 2) + 1e-12)

    def test_balance_and_offset(self):
        e = lambda r, k: float(np.sum(r.irs[k].astype(float) ** 2))
        right = self._render(room_enabled=False, balance_db=6.0)
        self.assertGreater(e(right, "RR"), 3 * e(right, "LL"))
        left_seat = ae.RenderParams(fs=48000, ir_length_ms=200, room_enabled=False,
                                    room=ae.RoomSpec(rt60_override=0.2, listener_offset=0.6))
        res = ae.render(self.speaker, None, FakeHrtf(fs=48000), left_seat)
        self.assertGreater(e(res, "LL"), e(res, "RR"))
        for v in res.irs.values():     # aucun repliement circulaire : début silencieux
            self.assertLess(float(np.max(np.abs(v[-200:]))), 1e-3)

    def test_tone_controls(self):
        flat = self._render(room_enabled=False)
        bass = self._render(room_enabled=False, bass_db=6.0)
        n = 1 << 15
        f = np.fft.rfftfreq(n, 1 / 48000)
        spec = lambda r: np.abs(np.fft.rfft(r.irs["LL"].astype(float), n))
        ratio = 20 * np.log10(spec(bass) / spec(flat))
        low = ratio[(f > 30) & (f < 60)].mean()
        mid = ratio[(f > 2000) & (f < 4000)].mean()
        self.assertGreater(low - mid, 4.0)

    def test_rate_mismatch(self):
        with self.assertRaises(ae.AudioEngineError):
            ae.render(self.speaker, None, FakeHrtf(fs=44100), ae.RenderParams(fs=48000))

    def test_write_irs(self):
        tmp, paths = temp_paths()
        with tmp:
            res = self._render(room_enabled=False)
            out = ae.write_irs(res, paths.filters_dir)
            self.assertEqual(sorted(out), sorted(ae.PAIRS))
            fs, data = wavfile.read(out["LL"])
            self.assertEqual(fs, 48000)
            self.assertEqual(data.dtype, np.float32)


if __name__ == "__main__":
    unittest.main()
