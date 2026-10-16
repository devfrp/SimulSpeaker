"""Outils communs aux tests : chemins temporaires, HRTF factice, exécuteur simulé."""

from __future__ import annotations

import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from config import Paths  # noqa: E402

CEA_SAMPLE = '''"CEA2034"
"On Axis"\t\t"Listening Window"\t\t"Early Reflections"\t\t"Sound Power"\t\t"Sound Power DI"
"Frequency [Hz]"\t"SPL"\t"Frequency [Hz]"\t"SPL"\t"Frequency [Hz]"\t"SPL"\t"Frequency [Hz]"\t"SPL"\t"Frequency [Hz]"\t"SPL"
'''


def make_cea_text(n: int = 60) -> str:
    """Mesure synthétique : LW plate à 86 dB, ER −2 dB, SP −6 dB, avec milliers « 1,000 »."""
    rows = []
    for f in np.geomspace(20, 20000, n):
        fs = f"{f:,.2f}"
        rows.append("\t".join([fs, "86.5", fs, "86.0", fs, "84.0", fs, "80.0", fs, "6.0"]))
    # Ligne incomplète (colonne DI absente) comme dans certains exports Klippel.
    return CEA_SAMPLE + "\n".join(rows) + "\n"


def temp_paths() -> tuple[tempfile.TemporaryDirectory, Paths]:
    tmp = tempfile.TemporaryDirectory()
    root = Path(tmp.name)
    paths = Paths(state_dir=root / "state" / "devfrp-speaker-sim",
                  cache_dir=root / "cache" / "devfrp-speaker-sim")
    paths.ensure()
    return tmp, paths


class FakeHrtf:
    """HRTF minimal : impulsion retardée selon l'azimut, oreille opposée atténuée."""

    def __init__(self, _path=None, fs: int = 48000):
        self.fs = fs
        self.length = 64

    def hrir(self, direction):
        x, y, z = direction
        az = np.arctan2(y, x)
        itd = 0.0003 * np.sin(az)  # > 0 : source à gauche
        left = np.zeros(self.length)
        right = np.zeros(self.length)
        left[8 + max(0, int(round(-itd * self.fs)))] = 1.0 if y >= 0 else 0.5
        right[8 + max(0, int(round(itd * self.fs)))] = 1.0 if y <= 0 else 0.5
        return left, right, 0.0, 0.0

    def close(self):
        pass


class FakeRunner:
    """Enregistre les commandes et renvoie des réponses programmées."""

    def __init__(self, responses: dict | None = None):
        self.calls: list[list[str]] = []
        self.responses = responses or {}

    def __call__(self, cmd):
        cmd = list(cmd)
        self.calls.append(cmd)
        key = " ".join(cmd)
        for prefix, (code, out) in self.responses.items():
            if key.startswith(prefix):
                return subprocess.CompletedProcess(cmd, code, out, "")
        return subprocess.CompletedProcess(cmd, 0, "", "")
