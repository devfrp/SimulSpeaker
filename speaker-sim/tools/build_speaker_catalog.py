#!/usr/bin/env python3
"""Regenerate ``data/speakers.json`` from Spinorama metadata (developer tool).

Selects 85 speakers measured by ASR on a Klippel NFS (uniform CEA-2034 text
format), ranked by Olive preference score with subwoofer, max 4 per brand,
and checks that each ``CEA2034.txt`` exists at the pinned Spinorama commit.

Usage: python3 tools/build_speaker_catalog.py [metadata.json]
"""

from __future__ import annotations

import json
import sys
import urllib.parse
from collections import Counter
from pathlib import Path

import requests

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from download import SPINORAMA_COMMIT, SPINORAMA_RAW  # noqa: E402

METADATA_URL = "https://www.spinorama.org/json/metadata.json"
TARGET_COUNT = 85
MAX_PER_BRAND = 4


def main() -> int:
    if len(sys.argv) > 1:
        meta = json.loads(Path(sys.argv[1]).read_text())
    else:
        meta = requests.get(METADATA_URL, timeout=60).json()

    candidates = []
    for key, spk in meta.items():
        m = spk.get("measurements", {}).get("asr")
        if not m or m.get("format") != "klippel":
            continue
        if spk.get("shape") not in ("bookshelves", "floorstanders", "center"):
            continue
        score = m.get("pref_rating", {}).get("pref_score_wsub")
        if score is None:
            continue
        candidates.append((score, key, spk, m))
    candidates.sort(key=lambda c: -c[0])

    per_brand: Counter[str] = Counter()
    chosen = []
    session = requests.Session()
    for score, key, spk, m in candidates:
        if len(chosen) >= TARGET_COUNT:
            break
        if per_brand[spk["brand"]] >= MAX_PER_BRAND:
            continue
        rel = f"datas/measurements/{key}/asr/CEA2034.txt"
        url = f"{SPINORAMA_RAW}/{SPINORAMA_COMMIT}/{urllib.parse.quote(rel)}"
        if session.head(url, timeout=30).status_code != 200:
            continue
        per_brand[spk["brand"]] += 1
        chosen.append({
            "id": key,
            "brand": spk["brand"],
            "model": spk["model"],
            "type": spk.get("type", ""),
            "shape": spk.get("shape", ""),
            "price": spk.get("price", ""),
            "pref_score": m.get("pref_rating", {}).get("pref_score"),
            "pref_score_wsub": score,
            "lfx_hz": m.get("pref_rating", {}).get("lfx_hz"),
            "path": rel,
        })
        print(f"{len(chosen):3d} {key}", file=sys.stderr)

    chosen.sort(key=lambda s: (s["brand"].lower(), s["model"].lower()))
    out = Path(__file__).resolve().parent.parent / "data" / "speakers.json"
    out.write_text(json.dumps({"spinorama_commit": SPINORAMA_COMMIT, "speakers": chosen},
                              indent=1, ensure_ascii=False) + "\n")
    print(f"wrote {len(chosen)} speakers to {out}", file=sys.stderr)
    return 0 if len(chosen) == TARGET_COUNT else 1


if __name__ == "__main__":
    sys.exit(main())
