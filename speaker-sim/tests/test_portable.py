"""Fonctions multiplateformes : lecteur SOFA h5py, export, chemins Windows/macOS."""

import importlib.util
import unittest
from pathlib import Path
from unittest import mock

import numpy as np
from scipy.io import wavfile

from tests.helpers import temp_paths
from tests.test_app import make_app

import audio_engine as ae
import config

HAS_H5PY = importlib.util.find_spec("h5py") is not None
SYSTEM_SOFA = Path("/usr/share/libmysofa/MIT_KEMAR_normal_pinna.sofa")


def write_test_sofa(path: Path) -> None:
    import h5py

    with h5py.File(path, "w") as f:
        pos = np.array([[0, 0, 1.4], [90, 0, 1.4], [270, 0, 1.4], [0, 90, 1.4]], float)
        irs = np.zeros((4, 2, 32))
        for m in range(4):
            irs[m, 0, m] = 1.0
            irs[m, 1, m + 4] = 0.5
        f["Data.IR"] = irs
        f["Data.SamplingRate"] = np.array([48000.0])
        f["Data.Delay"] = np.zeros((1, 2))
        f["SourcePosition"] = pos
        f["SourcePosition"].attrs["Type"] = b"spherical"


@unittest.skipUnless(HAS_H5PY, "h5py absent")
class SofaH5Tests(unittest.TestCase):
    def test_exact_direction_and_interpolation(self):
        tmp, paths = temp_paths()
        with tmp:
            sofa = paths.cache_dir / "t.sofa"
            write_test_sofa(sofa)
            h = ae.SofaH5Hrtf(sofa, 48000)
            left, right, dl, dr = h.hrir([0, 1, 0])        # azimut 90° = gauche
            self.assertEqual(int(np.argmax(left)), 1)
            self.assertEqual((dl, dr), (0.0, 0.0))
            left, _, _, _ = h.hrir([1, 1, 0])               # entre 0° et 90°
            self.assertAlmostEqual(left[0], left[1], places=6)

    def test_resampling(self):
        tmp, paths = temp_paths()
        with tmp:
            sofa = paths.cache_dir / "t.sofa"
            write_test_sofa(sofa)
            self.assertEqual(ae.SofaH5Hrtf(sofa, 96000).length, 64)

    @unittest.skipUnless(SYSTEM_SOFA.is_file(), "KEMAR système absent")
    def test_matches_libmysofa_closely(self):
        try:
            ref = ae.MysofaHrtf(SYSTEM_SOFA, 48000)
        except ae.AudioEngineError:
            self.skipTest("libmysofa absent")
        h5 = ae.SofaH5Hrtf(SYSTEM_SOFA, 48000)
        for d in ([1, 0, 0], [0.866, 0.5, 0], [0, -1, 0]):
            a = np.abs(np.fft.rfft(ref.hrir(d)[0], 1024))
            b = np.abs(np.fft.rfft(h5.hrir(d)[0], 1024))
            # Même forme spectrale à un gain près (libmysofa normalise le niveau).
            band = slice(20, 400)
            diff = 20 * np.log10(b[band] / a[band])
            self.assertLess(np.std(diff), 1.5)


class ExportTests(unittest.TestCase):
    def test_equalizer_apo_config(self):
        paths = {k: Path(f"/x/ir_{k}.wav") for k in ae.PAIRS}
        text = ae.equalizer_apo_config(paths)
        self.assertIn("Copy: VLL=L VLR=L VRL=R VRR=R", text)
        self.assertIn("Copy: L=VLL+VRL R=VLR+VRR", text)
        self.assertEqual(text.count("Convolution:"), 4)

    def test_export_files(self):
        tmp, paths = temp_paths()
        with tmp:
            app = make_app(paths)
            app.update_settings(ir_length_ms=100, room_enabled=False)
            out = app.export(paths.cache_dir.parent / "export")
            names = sorted(p.name for p in out)
            self.assertIn("speaker-sim-true-stereo.wav", names)
            self.assertIn("speaker-sim.txt", names)
            fs, data = wavfile.read(next(p for p in out if p.name.endswith("true-stereo.wav")))
            self.assertEqual(data.shape[1], 4)

    def test_non_linux_mode(self):
        tmp, paths = temp_paths()
        with tmp:
            app = make_app(paths)
            app.supports_live = False
            self.assertFalse(app.is_running())
            self.assertEqual(app.sample_rate(), 48000)
            from app import AppError
            with self.assertRaises(AppError):
                app.start()
            app.update_settings(ir_length_ms=100, room_enabled=False)
            app.build_filters()
            self.assertFalse(paths.filter_conf.exists())


class PlatformPathTests(unittest.TestCase):
    def test_windows(self):
        with mock.patch.object(config.sys, "platform", "win32"), \
             mock.patch.dict(config.os.environ, {"LOCALAPPDATA": "C:/Users/a/AppData/Local"}):
            p = config.Paths.default()
        self.assertEqual(p.state_dir, Path("C:/Users/a/AppData/Local/devfrp-speaker-sim/State"))

    def test_macos(self):
        with mock.patch.object(config.sys, "platform", "darwin"):
            p = config.Paths.default()
        self.assertEqual(p.cache_dir, Path.home() / "Library/Caches/devfrp-speaker-sim")


if __name__ == "__main__":
    unittest.main()
