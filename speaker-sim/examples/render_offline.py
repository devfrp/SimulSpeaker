#!/usr/bin/env python3
"""Exemple : générer les IR binaurales sans PipeWire ni interface graphique.

    python3 examples/render_offline.py "Genelec 8030C" "oratory1990/over-ear/Sennheiser HD 650" /tmp/irs

Télécharge (une fois) les mesures dans ~/.cache/devfrp-speaker-sim, puis écrit
ir_LL.wav, ir_LR.wav, ir_RL.wav, ir_RR.wav dans le dossier indiqué : utilisables
dans n'importe quel convolveur (EasyEffects, Equalizer APO, REAPER…).
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import audio_engine as ae  # noqa: E402
from config import Paths, load_speaker_catalog  # noqa: E402
from download import Downloader  # noqa: E402


def main(speaker_id: str, headphone_path: str, out_dir: str, fs: int = 48000) -> None:
    dl = Downloader(Paths.default().cache_dir)
    speaker = next(s for s in load_speaker_catalog() if s["id"] == speaker_id)
    entry = next(e for e in dl.load_headphones() if e.path == headphone_path)

    response = ae.SpeakerResponse.load(dl.ensure_speaker(speaker))
    hp_fir = ae.headphone_eq_fir(ae.load_headphone_eq(dl.ensure_headphone_eq(entry)), fs)
    hrtf = ae.MysofaHrtf(dl.ensure_hrtf(), fs)

    params = ae.RenderParams(fs=fs, angle_deg=30, distance=2.0,
                             room=ae.RoomSpec(width=4.5, length=6.0, height=2.6, rt60=0.35))
    result = ae.render(response, hp_fir, hrtf, params,
                       progress=lambda f, m: print(f"{f * 100:5.1f} %  {m}"))
    for key, path in ae.write_irs(result, Path(out_dir)).items():
        print(key, path)
    print(f"Gain de normalisation : {result.gain_db:+.1f} dB", *result.notes, sep="\n")


if __name__ == "__main__":
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    main(*sys.argv[1:])
