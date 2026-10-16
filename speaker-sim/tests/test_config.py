import json
import os
import unittest
from pathlib import Path
from unittest import mock

from tests.helpers import temp_paths

import config
from config import ConfigStore, Paths, Settings


class SettingsTests(unittest.TestCase):
    def test_defaults_roundtrip(self):
        tmp, paths = temp_paths()
        with tmp:
            store = ConfigStore(paths)
            s = store.load()
            self.assertEqual(s.speaker_id, "KEF R3")
            s.speaker_distance = 2.5
            store.save(s)
            self.assertEqual(store.load().speaker_distance, 2.5)
            if os.name == "posix":
                self.assertEqual(oct(paths.state_dir.stat().st_mode & 0o777), "0o700")

    def test_coercion_and_unknown_keys(self):
        s = Settings.from_dict({"rt60": "0.5", "autostart": "oui", "reflection_order": 1.6,
                                "bogus": 1, "headphone": {"name": "X"}})
        self.assertEqual(s.rt60, 0.5)
        self.assertTrue(s.autostart)
        self.assertEqual(s.reflection_order, 2)
        self.assertIsNone(s.headphone)  # dictionnaire incomplet rejeté

    def test_bad_values_fall_back(self):
        s = Settings.from_dict({"rt60": "abc", "speaker_angle": float("nan")})
        self.assertEqual(s.rt60, Settings().rt60)
        self.assertEqual(s.speaker_angle, Settings().speaker_angle)

    def test_validation_clamps_and_room_fits(self):
        s = Settings(rt60=10.0, speaker_angle=5, preamp_db=-99, sample_rate=100,
                     speaker_distance=5.0, room_width=2.0, room_length=2.0).validated()
        self.assertEqual(s.rt60, 2.0)
        self.assertEqual(s.speaker_angle, 10.0)
        self.assertEqual(s.preamp_db, -24.0)
        self.assertEqual(s.sample_rate, 0)
        self.assertGreaterEqual(s.room_length, 5.0)

    def test_choices_and_presets(self):
        s = Settings(room_floor="lave", room_preset="inconnu").validated()
        self.assertEqual(s.room_floor, Settings().room_floor)
        self.assertEqual(s.room_preset, "")
        for key in config.ROOM_PRESETS:
            applied = Settings.from_dict({**Settings().to_dict(),
                                          **config.preset_settings(key)}).validated()
            self.assertEqual(applied.room_preset, key)
            self.assertTrue(applied.rt60_auto)
            for k in config.ROOM_KEYS:
                self.assertIn(k, config.ROOM_PRESETS[key])

    def test_corrupted_file_is_set_aside(self):
        tmp, paths = temp_paths()
        with tmp:
            paths.config_file.write_text("{ pas du json")
            s = ConfigStore(paths).load()
            self.assertEqual(s, Settings().validated())
            self.assertTrue(paths.config_file.with_suffix(".json.corrupt").exists())

    def test_runtime_update(self):
        tmp, paths = temp_paths()
        with tmp:
            store = ConfigStore(paths)
            store.update_runtime(previous_sink="hp", restarting=True)
            rt = store.update_runtime(restarting=None)
            self.assertEqual(rt, {"previous_sink": "hp"})
            self.assertEqual(json.loads(paths.runtime_file.read_text()), rt)

    def test_xdg_paths(self):
        with mock.patch.dict(os.environ, {"XDG_STATE_HOME": "/x/state", "XDG_CACHE_HOME": "rel"}), \
             mock.patch.object(config.sys, "platform", "linux"):
            p = Paths.default()
        self.assertEqual(p.state_dir, Path("/x/state/devfrp-speaker-sim"))
        self.assertEqual(p.cache_dir, Path.home() / ".cache/devfrp-speaker-sim")  # relatif ignoré

    def test_default_config_file_matches_dataclass(self):
        data = json.loads(config.DEFAULT_CONFIG_FILE.read_text())
        self.assertEqual(Settings.from_dict(data), Settings())

    def test_speaker_catalog(self):
        speakers = config.load_speaker_catalog()
        self.assertGreaterEqual(len(speakers), 130)
        self.assertEqual(len({s["id"] for s in speakers}), len(speakers))
        self.assertGreaterEqual(sum(s["category"] == "studio" for s in speakers), 69)
        for s in speakers:
            self.assertIn(s["category"], ("studio", "hifi"))
            if s["format"] == "polar":
                self.assertFalse(s["path"].endswith(".txt"))
            else:
                self.assertTrue(s["path"].endswith("/CEA2034.txt"))
        ids = {s["id"] for s in speakers}
        for must in ("Neumann KH 150", "Genelec 8361A", "Kii Audio Three", "Yamaha HS8"):
            self.assertIn(must, ids)


if __name__ == "__main__":
    unittest.main()
