import unittest
from pathlib import Path

from tests.helpers import temp_paths

import download
from download import Downloader, DownloadError, HeadphoneEntry

INDEX = """# Index
This is a list of all equalization profiles.

- [1MORE Aero (ANC Off)](./HypetheSonics/GRAS%20RA0045%20in-ear/1MORE%20Aero%20(ANC%20Off)) by HypetheSonics on GRAS RA0045
- [Sennheiser HD 650](./oratory1990/over-ear/Sennheiser%20HD%20650) by oratory1990
- [AKG K371](./crinacle/GRAS%2043AG-7%20over-ear/AKG%20K371) by crinacle on GRAS 43AG-7
not a list line
"""


class FakeResponse:
    def __init__(self, status=200, body=b"data"):
        self.status_code = status
        self.body = body
        self.headers = {"Content-Length": str(len(body))}

    def iter_content(self, chunk_size):
        yield self.body

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False


class FakeSession:
    def __init__(self, routes=None, default=404):
        self.routes = routes or {}
        self.default = default
        self.urls = []

    def get(self, url, stream=True, timeout=None):
        self.urls.append(url)
        for suffix, body in self.routes.items():
            if url.endswith(suffix):
                if isinstance(body, Exception):
                    raise body
                return FakeResponse(200, body)
        return FakeResponse(self.default, b"")


class IndexTests(unittest.TestCase):
    def test_parse_index(self):
        entries = download.parse_autoeq_index(INDEX)
        self.assertEqual(len(entries), 3)
        aero = entries[0]
        self.assertEqual(aero.name, "1MORE Aero (ANC Off)")
        self.assertEqual(aero.path, "HypetheSonics/GRAS RA0045 in-ear/1MORE Aero (ANC Off)")
        self.assertEqual(aero.source, "HypetheSonics")
        self.assertEqual(aero.rig, "GRAS RA0045 in-ear")
        self.assertEqual(entries[1].rig, "over-ear")

    def test_search_all_words(self):
        entries = download.parse_autoeq_index(INDEX)
        self.assertEqual([e.name for e in download.search_headphones(entries, "hd ORATORY")],
                         ["Sennheiser HD 650"])
        self.assertEqual(len(download.search_headphones(entries, "")), 3)
        self.assertEqual(len(download.search_headphones(entries, "", limit=1)), 1)

    def test_search_word_start(self):
        e = [HeadphoneEntry("Beyerdynamic DT 990 (600 Ohm)", "oratory1990", "over-ear", "a"),
             HeadphoneEntry("Beyerdynamic DT 1990 Pro", "oratory1990", "over-ear", "b"),
             HeadphoneEntry("Sennheiser HD 650", "crinacle", "over-ear", "c")]
        self.assertEqual([x.path for x in download.search_headphones(e, "dt 990")], ["a"])
        self.assertEqual([x.path for x in download.search_headphones(e, "hd 65")], ["c"])
        self.assertEqual([x.path for x in download.search_headphones(e, "(600")], ["a"])

    def test_entry_roundtrip(self):
        e = HeadphoneEntry("A", "B", "C", "B/C/A")
        self.assertEqual(HeadphoneEntry.from_dict(e.to_dict()), e)


class DownloaderTests(unittest.TestCase):
    def test_pinned_url_quoting(self):
        d = Downloader(Path("/nonexistent"), session=FakeSession())
        url = d.autoeq_url("oratory1990/over-ear/Sennheiser HD 650/Sennheiser HD 650 ParametricEQ.txt")
        self.assertIn(download.AUTOEQ_COMMIT, url)
        self.assertIn("Sennheiser%20HD%20650", url)
        self.assertNotIn(" ", url)

    def test_cache_hit_skips_network(self):
        tmp, paths = temp_paths()
        with tmp:
            session = FakeSession({"INDEX.md": INDEX.encode()})
            d = Downloader(paths.cache_dir, session=session)
            self.assertEqual(len(d.load_headphones()), 3)
            self.assertEqual(len(d.load_headphones()), 3)
            self.assertEqual(len(session.urls), 1)
            self.assertTrue((d.autoeq_dir / "INDEX.md").is_file())

    def test_parametric_then_graphic_fallback(self):
        tmp, paths = temp_paths()
        with tmp:
            session = FakeSession({"AKG%20K371%20GraphicEQ.txt": b"GraphicEQ: 20 0; 20000 0"})
            d = Downloader(paths.cache_dir, session=session)
            entry = download.parse_autoeq_index(INDEX)[2]
            path = d.ensure_headphone_eq(entry)
            self.assertTrue(path.name.endswith("GraphicEQ.txt"))
            self.assertEqual(len(session.urls), 2)

    def test_not_found_raises_without_partial_files(self):
        tmp, paths = temp_paths()
        with tmp:
            d = Downloader(paths.cache_dir, session=FakeSession())
            entry = download.parse_autoeq_index(INDEX)[1]
            with self.assertRaises(DownloadError):
                d.ensure_headphone_eq(entry)
            leftovers = [p for p in paths.cache_dir.rglob("*") if p.is_file()]
            self.assertEqual(leftovers, [])

    def test_network_error_retries(self):
        tmp, paths = temp_paths()
        with tmp:
            session = FakeSession({"INDEX.md": ConnectionError("offline")})
            d = Downloader(paths.cache_dir, session=session, retries=2)
            with self.assertRaises(DownloadError):
                d.ensure_autoeq_index()
            self.assertEqual(len(session.urls), 2)

    def test_speaker_and_hrtf(self):
        tmp, paths = temp_paths()
        with tmp:
            session = FakeSession({"CEA2034.txt": b"x", ".sofa": b"sofa"})
            d = Downloader(paths.cache_dir, session=session)
            spk = {"id": "KEF R3", "path": "datas/measurements/KEF R3/asr/CEA2034.txt"}
            p = d.ensure_speaker(spk)
            self.assertEqual(p, d.spinorama_dir / "KEF R3" / "CEA2034.txt")
            self.assertIn(download.SPINORAMA_COMMIT, session.urls[0])
            polar = {"id": "Neumann KH 150", "format": "polar",
                     "path": "datas/measurements/Neumann KH 150/asr"}
            session.routes["SPL%20Horizontal.txt"] = b"h"
            session.routes["SPL%20Vertical.txt"] = b"v"
            folder = d.ensure_speaker(polar)
            self.assertTrue((folder / "SPL Horizontal.txt").is_file())
            self.assertTrue((folder / "SPL Vertical.txt").is_file())
            custom = paths.cache_dir / "my.sofa"
            custom.write_bytes(b"s")
            self.assertEqual(d.ensure_hrtf(str(custom)), custom)


if __name__ == "__main__":
    unittest.main()
