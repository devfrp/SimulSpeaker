#!/usr/bin/env python3
"""Ajoute des moniteurs de studio à ``data/speakers.json`` (outil développeur).

Ne régénère pas le catalogue existant : ajoute les modèles listés ci-dessous
(mesure Klippel NFS, ASR ou Erin's Audio Corner), vérifie la présence de
``CEA2034.txt`` au commit Spinorama figé, et renseigne ``category``
(``studio`` / ``hifi``) pour toutes les entrées.

Usage : python3 tools/add_studio_monitors.py metadata.json
"""

from __future__ import annotations

import json
import sys
import urllib.parse
from pathlib import Path

import requests

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from download import SPINORAMA_COMMIT, SPINORAMA_RAW  # noqa: E402

ORIGINS = {"asr": "ASR", "asr-horizontal": "ASR", "eac": "Erin's Audio Corner"}

NEW_STUDIO = [
    "Adam A4V", "Adam D3V", "Adam S2V",
    "Audio Smile Little British Monitor",
    "Edifier MR4",
    "Eve Audio SC305",
    "Fluid Audio FX50", "Fluid Audio FX50v2",
    "Focal Alpha 65 Evo", "Focal Solo6 ST6",
    "Genelec 8050B", "Genelec 8331A", "Genelec 8341A", "Genelec 8351B", "Genelec 8361A",
    "Genelec M040", "Genelec S360",
    "IK Multimedia iLoud MTM MKII",
    "JBL 104", "JBL 305P Mark ii", "JBL 306P Mark ii", "JBL 705P", "JBL 708P",
    "JBL 708i active", "JBL M2",
    "KRK Systems Classic 5",
    "Kali IN-5", "Kali IN-8v2", "Kali LP-6v2", "Kali LP-8v2", "Kali LP-UNF", "Kali SM-5",
    "Kii Audio Three",
    "M-Audio BX3 D3", "M-Audio BX8 D3",
    "Mackie CR3-X",
    "Neumann KH 120 II", "Neumann KH 150", "Neumann KH 420G", "Neumann KH 80",
    "PMC Result6",
    "Pioneer DJ VM-50",
    "Presonus Eris E3.5", "Presonus Eris E5 XT",
    "Yamaha HS8",
]

# Entrées déjà au catalogue qui sont des moniteurs de studio.
EXISTING_STUDIO = {
    "Adam A5X", "Adam T5V", "Adam T8V", "Barefoot Footprint 01", "Behringer B2030p",
    "Dynaudio Core 47", "Dynaudio LYD 5", "Emotiva Airmotiv 6s", "Emotiva Airmotiv B1+",
    "Fluid Audio Image 2", "Focal Solo6 Be", "Fostex PM03", "Genelec 8010A",
    "Genelec 8030C", "Genelec 8320A", "Genelec 8330A", "IK Multimedia iLoud MTM",
    "JBL 308P Mark ii", "KRK Systems RoKit 5 G4", "Kali IN-8", "Kali LP-6",
    "Neumann KH 310A", "Yamaha HS5", "Yamaha HS7",
}


def main() -> int:
    meta = json.loads(Path(sys.argv[1]).read_text())
    out = Path(__file__).resolve().parent.parent / "data" / "speakers.json"
    catalog = json.loads(out.read_text())
    speakers = catalog["speakers"]
    have = {s["id"] for s in speakers}
    for s in speakers:
        s["category"] = "studio" if s["id"] in EXISTING_STUDIO else "hifi"
        s.setdefault("origin", "ASR")
        s.setdefault("format", "cea2034")

    session = requests.Session()
    added = 0
    for sid in NEW_STUDIO:
        if sid in have:
            continue
        spk = meta[sid]
        klippel = [(k, m) for k, m in spk["measurements"].items()
                   if m.get("format") == "klippel" and k in ORIGINS]
        klippel.sort(key=lambda km: list(ORIGINS).index(km[0]))
        def exists(rel: str) -> bool:
            url = f"{SPINORAMA_RAW}/{SPINORAMA_COMMIT}/{urllib.parse.quote(rel)}"
            return session.head(url, timeout=30).status_code == 200

        for key, m in klippel:
            folder = f"datas/measurements/{sid}/{key}"
            if exists(f"{folder}/CEA2034.txt"):
                rel, fmt = f"{folder}/CEA2034.txt", "cea2034"
            elif exists(f"{folder}/SPL Horizontal.txt") and exists(f"{folder}/SPL Vertical.txt"):
                rel, fmt = folder, "polar"      # CEA-2034 calculé localement
            else:
                print(f"  absent : {sid} [{key}]", file=sys.stderr)
                continue
            rating = m.get("pref_rating", {})
            speakers.append({
                "id": sid, "brand": spk["brand"], "model": spk["model"],
                "type": spk.get("type", ""), "shape": spk.get("shape", ""),
                "price": spk.get("price", ""),
                "pref_score": rating.get("pref_score"),
                "pref_score_wsub": rating.get("pref_score_wsub"),
                "lfx_hz": rating.get("lfx_hz"),
                "path": rel, "format": fmt, "category": "studio", "origin": ORIGINS[key],
            })
            added += 1
            print(f"+ {sid} [{key}, {fmt}]", file=sys.stderr)
            break
        else:
            print(f"! ignoré (pas de CEA2034 Klippel) : {sid}", file=sys.stderr)

    speakers.sort(key=lambda s: (s["brand"].lower(), s["model"].lower()))
    out.write_text(json.dumps(catalog, indent=1, ensure_ascii=False) + "\n")
    n_studio = sum(s["category"] == "studio" for s in speakers)
    print(f"{added} ajoutés ; {len(speakers)} au total dont {n_studio} moniteurs de studio",
          file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
