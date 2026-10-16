"""Moteur audio : construction des filtres de simulation de haut-parleurs.

Chaîne simulée, pour chaque haut-parleur virtuel *s* (gauche/droite) et chaque
oreille *e* ::

    IR[s][e] = Casque⁻¹ · ( DF⁻¹ · ( LW · Direct[s][e] + ER · Réflexions[s][e] )
                             + SP · Réverbération[s][e] )

* ``Direct`` / ``Réflexions`` : HRIR KEMAR (libmysofa) pour la source et ses
  sources-images (modèle de pièce parallélépipédique d'Allen & Berkley), avec
  retards fractionnaires exacts (appliqués dans le domaine fréquentiel).
* ``LW``, ``ER``, ``SP`` : courbes CEA-2034 du haut-parleur (Listening Window,
  Early Reflections, Sound Power) converties en FIR à phase minimale.
* ``Réverbération`` : bruit décorrélé à décroissance exponentielle (RT60 par
  bandes), énergie fixée par la distance critique de la pièce.
* ``DF⁻¹`` : égalisation champ diffus du HRTF (AutoEq vise une cible proche
  du champ diffus, on évite ainsi de compter deux fois la résonance du pavillon).
* ``Casque⁻¹`` : correction AutoEq (EQ paramétrique → IIR → réponse impulsionnelle,
  phase minimale par construction).

Les quatre IR (LL, LR, RL, RR) sont normalisées pour éviter l'écrêtage puis
écrites en WAV float32 mono, lues par le convolveur de PipeWire.
Ce module ne dépend ni de Qt ni de PipeWire.
"""

from __future__ import annotations

import ctypes
import ctypes.util
import math
import os
import re
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable, Protocol

import numpy as np
from scipy import signal
from scipy.io import wavfile

SPEED_OF_SOUND = 343.0
EAR_HEIGHT = 1.2
PAIRS = ("LL", "LR", "RL", "RR")  # haut-parleur → oreille
FIR_TAPS = 8192
HEADROOM_DB = 1.5


class AudioEngineError(RuntimeError):
    pass


# ---------------------------------------------------------------------------
# Égaliseurs AutoEq
# ---------------------------------------------------------------------------
@dataclass(frozen=True)
class Biquad:
    kind: str       # PK, LSC, HSC, LP, HP
    fc: float
    gain_db: float = 0.0
    q: float = 0.7071


@dataclass
class HeadphoneEq:
    preamp_db: float = 0.0
    filters: list[Biquad] = field(default_factory=list)
    graphic: tuple[np.ndarray, np.ndarray] | None = None  # (fréquences, gains dB)


_FILTER_RE = re.compile(
    r"Filter\s+\d+\s*:\s*ON\s+(?P<kind>[A-Z]+)\s+Fc\s+(?P<fc>[\d.]+)\s*Hz"
    r"(?:\s+Gain\s+(?P<gain>[-+\d.]+)\s*dB)?(?:\s+Q\s+(?P<q>[\d.]+))?",
    re.IGNORECASE,
)
_KIND_ALIASES = {"PK": "PK", "PEQ": "PK", "LSC": "LSC", "LS": "LSC", "LSQ": "LSC",
                 "HSC": "HSC", "HS": "HSC", "HSQ": "HSC", "LP": "LP", "LPQ": "LP",
                 "HP": "HP", "HPQ": "HP"}


def parse_parametric_eq(text: str) -> HeadphoneEq:
    """Analyse un fichier ``ParametricEQ.txt`` (format Equalizer APO)."""
    eq = HeadphoneEq()
    for line in text.splitlines():
        line = line.strip()
        m = re.match(r"Preamp\s*:\s*([-+\d.]+)\s*dB", line, re.IGNORECASE)
        if m:
            eq.preamp_db = float(m.group(1))
            continue
        m = _FILTER_RE.match(line)
        if not m:
            continue
        kind = _KIND_ALIASES.get(m.group("kind").upper())
        if kind is None:
            continue
        eq.filters.append(Biquad(kind=kind, fc=float(m.group("fc")),
                                 gain_db=float(m.group("gain") or 0.0),
                                 q=float(m.group("q") or 0.7071)))
    return eq


def parse_graphic_eq(text: str) -> HeadphoneEq:
    """Analyse ``GraphicEQ: f1 g1; f2 g2; …``."""
    m = re.search(r"GraphicEQ\s*:\s*(.+)", text, re.IGNORECASE | re.DOTALL)
    if not m:
        raise AudioEngineError("Fichier GraphicEQ invalide")
    pts = []
    for item in m.group(1).split(";"):
        bits = item.split()
        if len(bits) >= 2:
            pts.append((float(bits[0]), float(bits[1])))
    if len(pts) < 2:
        raise AudioEngineError("GraphicEQ : pas assez de points")
    f, g = np.array(pts).T
    return HeadphoneEq(graphic=(f, g))


def load_headphone_eq(path: Path) -> HeadphoneEq:
    text = Path(path).read_text(encoding="utf-8", errors="replace")
    if "GraphicEQ" in text:
        return parse_graphic_eq(text)
    eq = parse_parametric_eq(text)
    if not eq.filters:
        raise AudioEngineError(f"Aucun filtre reconnu dans {path}")
    return eq


# ---------------------------------------------------------------------------
# Mesures de casque importées par l'utilisateur
# ---------------------------------------------------------------------------
def detect_headphone_file_kind(text: str) -> str:
    """``parametric`` (Equalizer APO / AutoEq), ``graphic`` (GraphicEQ) ou ``measurement``."""
    if re.search(r"GraphicEQ\s*:", text, re.IGNORECASE):
        return "graphic"
    if _FILTER_RE.search(text) or re.search(r"^\s*Preamp\s*:", text, re.IGNORECASE | re.MULTILINE):
        return "parametric"
    return "measurement"


def parse_measurement(text: str) -> tuple[np.ndarray, np.ndarray]:
    """Réponse en fréquence brute (fréquence, dB) : export REW, Squiglink, CSV AutoEq…

    Lignes de commentaires et en-têtes ignorées ; séparateurs virgule, point-virgule,
    tabulation ou espaces. Avec un en-tête AutoEq, la colonne ``raw`` est utilisée.
    """
    column = 1
    freqs, values = [], []
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line[0] in "*#;/":
            continue
        fields = [f for f in re.split(r"[,;\t ]+", line) if f]
        if fields and not re.match(r"^[-+]?\d", fields[0]):
            names = [f.strip().strip('"').lower() for f in fields]
            if "raw" in names:
                column = names.index("raw")
            continue
        try:
            f = float(fields[0])
            v = float(fields[column])
        except (ValueError, IndexError):
            continue
        if f > 0 and math.isfinite(v):
            freqs.append(f)
            values.append(v)
    if len(freqs) < 10:
        raise AudioEngineError("Mesure illisible : il faut au moins 10 lignes « fréquence, dB »")
    f = np.asarray(freqs)
    v = np.asarray(values)
    order = np.argsort(f)
    f, v = f[order], v[order]
    keep = np.concatenate([[True], np.diff(f) > 0])
    f, v = f[keep], v[keep]
    if f[0] > 200 or f[-1] < 5000:
        raise AudioEngineError("Mesure incomplète : elle doit couvrir au moins 200 Hz – 5 kHz")
    return f, v


def _smooth_log(values: np.ndarray, points: int) -> np.ndarray:
    from scipy.ndimage import uniform_filter1d

    return uniform_filter1d(values, size=max(1, points), mode="nearest")


def measurement_eq(measurement: tuple[np.ndarray, np.ndarray],
                   target: tuple[np.ndarray, np.ndarray],
                   max_boost_db: float = 10.0, max_cut_db: float = -20.0) -> HeadphoneEq:
    """Correction (EQ graphique) amenant ``measurement`` sur la courbe cible ``target``.

    Erreur normalisée sur 200 Hz – 2 kHz, lissée au 1/12 d'octave puis au 1/3
    d'octave au-dessus de 5–10 kHz (zone dépendant du placement), bornée.
    """
    grid = np.geomspace(20, 20000, 480)       # ≈ 48 points par octave
    m = interp_log(*measurement, grid)
    t = interp_log(*target, grid)
    err = m - t
    band = (grid >= 200) & (grid <= 2000)
    err -= np.mean(err[band])
    fine = _smooth_log(err, 4)                # 1/12 d'octave
    coarse = _smooth_log(err, 16)             # 1/3 d'octave
    w = np.clip((np.log(grid) - np.log(5000)) / (np.log(10000) - np.log(5000)), 0, 1)
    eq = np.clip(-((1 - w) * fine + w * coarse), max_cut_db, max_boost_db)
    return HeadphoneEq(graphic=(grid, eq))


def biquad_coefficients(bq: Biquad, fs: float) -> tuple[np.ndarray, np.ndarray] | None:
    """Coefficients RBJ (Audio EQ Cookbook). ``None`` si fc ≥ Nyquist."""
    if bq.fc <= 0 or bq.fc >= fs / 2 * 0.999:
        return None
    a_lin = 10 ** (bq.gain_db / 40)
    w0 = 2 * math.pi * bq.fc / fs
    cw, sw = math.cos(w0), math.sin(w0)
    alpha = sw / (2 * max(bq.q, 1e-3))
    sq = 2 * math.sqrt(a_lin) * alpha
    if bq.kind == "PK":
        b = [1 + alpha * a_lin, -2 * cw, 1 - alpha * a_lin]
        a = [1 + alpha / a_lin, -2 * cw, 1 - alpha / a_lin]
    elif bq.kind == "LSC":
        b = [a_lin * ((a_lin + 1) - (a_lin - 1) * cw + sq),
             2 * a_lin * ((a_lin - 1) - (a_lin + 1) * cw),
             a_lin * ((a_lin + 1) - (a_lin - 1) * cw - sq)]
        a = [(a_lin + 1) + (a_lin - 1) * cw + sq,
             -2 * ((a_lin - 1) + (a_lin + 1) * cw),
             (a_lin + 1) + (a_lin - 1) * cw - sq]
    elif bq.kind == "HSC":
        b = [a_lin * ((a_lin + 1) + (a_lin - 1) * cw + sq),
             -2 * a_lin * ((a_lin - 1) + (a_lin + 1) * cw),
             a_lin * ((a_lin + 1) + (a_lin - 1) * cw - sq)]
        a = [(a_lin + 1) - (a_lin - 1) * cw + sq,
             2 * ((a_lin - 1) - (a_lin + 1) * cw),
             (a_lin + 1) - (a_lin - 1) * cw - sq]
    elif bq.kind == "LP":
        b = [(1 - cw) / 2, 1 - cw, (1 - cw) / 2]
        a = [1 + alpha, -2 * cw, 1 - alpha]
    elif bq.kind == "HP":
        b = [(1 + cw) / 2, -(1 + cw), (1 + cw) / 2]
        a = [1 + alpha, -2 * cw, 1 - alpha]
    else:
        return None
    b_arr, a_arr = np.array(b), np.array(a)
    return b_arr / a_arr[0], a_arr / a_arr[0]


def headphone_eq_fir(eq: HeadphoneEq, fs: float, n_taps: int = FIR_TAPS,
                     include_preamp: bool = False) -> np.ndarray:
    """Réponse impulsionnelle (phase minimale) de la correction du casque.

    Le préampli AutoEq n'est pas inclus par défaut : la normalisation globale
    de :func:`render` gère déjà la marge anti-écrêtage.
    """
    if eq.graphic is not None:
        f, g = eq.graphic
        return minimum_phase_fir(f, g, fs, n_taps)
    sos = [np.concatenate([c[0], c[1]]) for c in
           (biquad_coefficients(bq, fs) for bq in eq.filters) if c is not None]
    impulse = np.zeros(n_taps)
    impulse[0] = 10 ** (eq.preamp_db / 20) if include_preamp else 1.0
    h = signal.sosfilt(np.array(sos), impulse) if sos else impulse
    return _fade_out(h, 0.2)


def eq_response_db(eq: HeadphoneEq, freqs: np.ndarray, fs: float) -> np.ndarray:
    """Réponse en amplitude (dB) de l'EQ du casque aux fréquences données."""
    if eq.graphic is not None:
        f, g = eq.graphic
        return np.interp(np.log(freqs), np.log(f), g)
    total = np.zeros(len(freqs))
    w = 2 * np.pi * np.asarray(freqs) / fs
    for bq in eq.filters:
        c = biquad_coefficients(bq, fs)
        if c is None:
            continue
        _, h = signal.freqz(c[0], c[1], worN=w)
        total += 20 * np.log10(np.maximum(np.abs(h), 1e-12))
    return total


# ---------------------------------------------------------------------------
# Mesures de haut-parleurs (CEA-2034, format Klippel exporté par Spinorama)
# ---------------------------------------------------------------------------
def _parse_number(token: str) -> float:
    """Nombre Klippel : ``1,034.91`` (séparateur de milliers) ou ``12,5`` (virgule décimale)."""
    tok = token.strip().strip('"')
    if "," in tok:
        if "." in tok or re.fullmatch(r"-?\d{1,3}(,\d{3})+", tok):
            tok = tok.replace(",", "")
        else:
            tok = tok.replace(",", ".")
    return float(tok)


def parse_cea2034(text: str) -> dict[str, tuple[np.ndarray, np.ndarray]]:
    """Renvoie ``{nom_courbe: (fréquences, dB)}``."""
    lines = [ln for ln in text.splitlines() if ln.strip()]
    if len(lines) < 4:
        raise AudioEngineError("Fichier CEA-2034 trop court")
    names = re.findall(r'"([^"]*)"', lines[1])
    if not names:
        raise AudioEngineError("En-têtes CEA-2034 introuvables")
    # Colonnes positionnelles (séparées par tabulations) : certaines courbes
    # (DI…) s'arrêtent avant les autres et laissent des champs vides.
    columns: list[list[float]] = [[] for _ in range(2 * len(names))]
    for line in lines[3:]:
        fields = line.split("\t") if "\t" in line else line.split()
        for i in range(min(len(fields), 2 * len(names))):
            try:
                columns[i].append(_parse_number(fields[i]))
            except ValueError:
                columns[i].append(float("nan"))
        for i in range(len(fields), 2 * len(names)):
            columns[i].append(float("nan"))
    curves = {}
    for i, name in enumerate(names):
        f = np.array(columns[2 * i])
        db = np.array(columns[2 * i + 1])
        ok = np.isfinite(f) & np.isfinite(db) & (f > 0)
        if ok.sum() >= 2:
            order = np.argsort(f[ok])
            curves[name] = (f[ok][order], db[ok][order])
    for required in ("Listening Window",):
        if required not in curves:
            raise AudioEngineError(f"Courbe « {required} » absente de la mesure")
    return curves


# Pondérations CEA-2034 de la puissance acoustique, par angle (0°…180°, pas de 10°).
_SP_WEIGHTS = (0.000604486, 0.004730189, 0.008955027, 0.012387354, 0.014989611,
               0.016868154, 0.018165946, 0.019006236, 0.019477078, 0.019629421,
               0.019477078, 0.019006236, 0.018165946, 0.016868154, 0.014989611,
               0.012387354, 0.008955027, 0.004730189, 0.000604486)


def parse_klippel_polar(text: str) -> tuple[np.ndarray, dict[int, np.ndarray]]:
    """``SPL Horizontal.txt`` / ``SPL Vertical.txt`` (Klippel) → (fréquences, {angle: dB})."""
    lines = [ln for ln in text.splitlines() if ln.strip()]
    if len(lines) < 4:
        raise AudioEngineError("Fichier polaire trop court")
    angles = []
    for name in re.findall(r'"([^"]*)"', lines[1]):
        m = re.search(r"(-?\d+)", name)
        angles.append(0 if "on" in name.lower() and "axis" in name.lower() else
                      int(m.group(1)) if m else None)
    if not angles or None in angles:
        raise AudioEngineError("Angles illisibles dans le fichier polaire")
    rows = []
    for line in lines[3:]:
        fields = line.split("\t") if "\t" in line else line.split()
        try:
            values = [_parse_number(f) for f in fields[:2 * len(angles)]]
        except ValueError:
            continue
        if len(values) == 2 * len(angles):
            rows.append(values)
    if len(rows) < 10:
        raise AudioEngineError("Pas assez de points dans le fichier polaire")
    data = np.array(rows)
    freqs = data[:, 0]
    return freqs, {a: data[:, 2 * i + 1] for i, a in enumerate(angles)}


def cea2034_from_polar(horizontal: tuple[np.ndarray, dict[int, np.ndarray]],
                       vertical: tuple[np.ndarray, dict[int, np.ndarray]]
                       ) -> dict[str, tuple[np.ndarray, np.ndarray]]:
    """Courbes CEA-2034 (ANSI/CTA-2034-A) calculées depuis les mesures polaires.

    Moyennes spatiales en pression quadratique : Listening Window (0°, ±10° V,
    ±10/20/30° H), Early Reflections (sol, plafond, murs avant, latéraux et
    arrière) et Sound Power (70 points pondérés). Identique au calcul Klippel.
    """
    f, h = horizontal
    fv, v = vertical
    if len(fv) != len(f) or not np.allclose(fv, f, rtol=1e-3):
        v = {a: np.interp(np.log(f), np.log(fv), db) for a, db in v.items()}
    h = {(180 if a == -180 else a): db for a, db in h.items()}
    v = {(180 if a == -180 else a): db for a, db in v.items()}

    def avg(curves):
        if not curves:
            raise AudioEngineError("Angles manquants pour le calcul CEA-2034")
        return 10 * np.log10(np.mean([10 ** (c / 10) for c in curves], axis=0))

    def pick(plane, angles):
        return [plane[a] for a in angles if a in plane]

    lw = avg(pick(h, (0, 10, -10, 20, -20, 30, -30)) + pick(v, (10, -10)))
    # Réflexions sol, plafond, murs avant, latéraux et arrière : moyenne en
    # puissance des 26 directions regroupées (convention du logiciel Klippel,
    # vérifiée à l'identique sur le CEA2034.txt du KEF R3).
    er = avg(pick(v, (-20, -30, -40)) + pick(v, (40, 50, 60))
             + pick(h, (0, 10, -10, 20, -20, 30, -30))
             + pick(h, (40, -40, 50, -50, 60, -60, 70, -70, 80, -80))
             + pick(h, (90, -90, 180)))
    num = np.zeros(len(f))
    den = 0.0
    for plane, skip in ((h, ()), (v, (0, 180))):   # 0° et 180° comptés une seule fois
        for a, db in plane.items():
            if a in skip:
                continue
            w = _SP_WEIGHTS[abs(a) // 10]
            num += w * 10 ** (db / 10)
            den += w
    sp = 10 * np.log10(num / den)
    return {"On Axis": (f, h[0]), "Listening Window": (f, lw),
            "Early Reflections": (f, er), "Sound Power": (f, sp)}


def load_speaker(path: Path) -> "SpeakerResponse":
    """Fichier ``CEA2034.txt``, ou dossier contenant ``SPL Horizontal/Vertical.txt``."""
    path = Path(path)
    if path.is_dir():
        read = lambda name: (path / name).read_text(encoding="utf-8", errors="replace")
        curves = cea2034_from_polar(parse_klippel_polar(read("SPL Horizontal.txt")),
                                    parse_klippel_polar(read("SPL Vertical.txt")))
        return SpeakerResponse.from_curves(curves)
    return SpeakerResponse.load(path)


@dataclass
class SpeakerResponse:
    """Courbes CEA-2034 normalisées sur la Listening Window (300 Hz – 3 kHz)."""

    freqs: np.ndarray
    listening_window: np.ndarray
    early_reflections: np.ndarray
    sound_power: np.ndarray

    @classmethod
    def from_curves(cls, curves: dict[str, tuple[np.ndarray, np.ndarray]]) -> "SpeakerResponse":
        f, lw = curves["Listening Window"]
        er = _resample_curve(curves.get("Early Reflections"), f, lw)
        sp = _resample_curve(curves.get("Sound Power"), f, er)
        band = (f >= 300) & (f <= 3000)
        ref = float(np.mean(lw[band])) if band.any() else float(np.median(lw))
        return cls(f, lw - ref, er - ref, sp - ref)

    @classmethod
    def load(cls, path: Path) -> "SpeakerResponse":
        return cls.from_curves(parse_cea2034(Path(path).read_text(encoding="utf-8", errors="replace")))

    def directivity_index(self, lo: float = 1000, hi: float = 4000) -> float:
        band = (self.freqs >= lo) & (self.freqs <= hi)
        if not band.any():
            return 0.0
        return float(np.mean(self.listening_window[band] - self.sound_power[band]))


def _resample_curve(curve, f: np.ndarray, fallback: np.ndarray) -> np.ndarray:
    if curve is None:
        return fallback.copy()
    cf, cdb = curve
    return np.interp(np.log(f), np.log(cf), cdb)


# ---------------------------------------------------------------------------
# Outils DSP
# ---------------------------------------------------------------------------
def next_pow2(n: int) -> int:
    return 1 << max(0, int(n - 1).bit_length())


def _fade_out(h: np.ndarray, fraction: float) -> np.ndarray:
    n = len(h)
    k = max(2, int(n * fraction))
    out = np.array(h, dtype=float, copy=True)
    out[-k:] *= 0.5 * (1 + np.cos(np.linspace(0, np.pi, k)))
    return out


def interp_log(freqs: np.ndarray, mag_db: np.ndarray, grid: np.ndarray) -> np.ndarray:
    """Interpolation en log-fréquence, valeurs maintenues aux extrémités."""
    g = np.maximum(grid, freqs[0] * 1e-3)
    return np.interp(np.log(g), np.log(freqs), mag_db)


def minimum_phase_spectrum(mag: np.ndarray, nfft: int) -> np.ndarray:
    """Spectre à phase minimale (méthode du cepstre réel) d'une amplitude sur ``rfftfreq(nfft)``."""
    log_mag = np.log(np.maximum(mag, 1e-6))
    cep = np.fft.irfft(log_mag, nfft)
    fold = np.zeros(nfft)
    fold[0] = 1.0
    fold[1:nfft // 2] = 2.0
    fold[nfft // 2] = 1.0
    return np.exp(np.fft.rfft(cep * fold, nfft))


def minimum_phase_fir(freqs: np.ndarray, mag_db: np.ndarray, fs: float,
                      n_taps: int = FIR_TAPS) -> np.ndarray:
    """FIR à phase minimale suivant la courbe ``mag_db`` (dB) aux fréquences ``freqs``."""
    nfft = next_pow2(4 * n_taps)
    grid = np.fft.rfftfreq(nfft, 1 / fs)
    mag = 10 ** (interp_log(np.asarray(freqs, float), np.asarray(mag_db, float), grid) / 20)
    h = np.fft.irfft(minimum_phase_spectrum(mag, nfft), nfft)[:n_taps]
    return _fade_out(h, 0.1)


def smooth_octave(freqs: np.ndarray, power: np.ndarray, fraction: float) -> np.ndarray:
    """Lissage 1/``fraction`` d'octave d'un spectre de puissance (grille linéaire)."""
    csum = np.concatenate([[0.0], np.cumsum(power)])
    df = freqs[1] - freqs[0]
    half = 2 ** (1 / (2 * fraction))
    lo = np.clip(np.floor(freqs / half / df).astype(int), 0, len(power) - 1)
    hi = np.clip(np.ceil(freqs * half / df).astype(int), 0, len(power) - 1)
    hi = np.maximum(hi, lo)
    return (csum[hi + 1] - csum[lo]) / (hi - lo + 1)


# ---------------------------------------------------------------------------
# HRTF
# ---------------------------------------------------------------------------
class HrtfProvider(Protocol):
    fs: int

    def hrir(self, direction: np.ndarray) -> tuple[np.ndarray, np.ndarray, float, float]:
        """HRIR (gauche, droite) et retards (s) pour une direction (x avant, y gauche, z haut)."""


class MysofaHrtf:
    """Accès à libmysofa via ctypes (interpolation + rééchantillonnage natifs)."""

    def __init__(self, sofa_path: Path | str, fs: int, library: str | None = None):
        self.fs = int(fs)
        self._lib = self._load_library(library)
        self._lib.mysofa_open.restype = ctypes.c_void_p
        self._lib.mysofa_open.argtypes = [ctypes.c_char_p, ctypes.c_float,
                                          ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_int)]
        self._lib.mysofa_getfilter_float.restype = None
        self._lib.mysofa_getfilter_float.argtypes = [
            ctypes.c_void_p, ctypes.c_float, ctypes.c_float, ctypes.c_float,
            ctypes.POINTER(ctypes.c_float), ctypes.POINTER(ctypes.c_float),
            ctypes.POINTER(ctypes.c_float), ctypes.POINTER(ctypes.c_float)]
        self._lib.mysofa_close.restype = None
        self._lib.mysofa_close.argtypes = [ctypes.c_void_p]
        n, err = ctypes.c_int(0), ctypes.c_int(0)
        handle = self._lib.mysofa_open(os.fsencode(str(sofa_path)), float(fs),
                                       ctypes.byref(n), ctypes.byref(err))
        if not handle or err.value != 0:
            raise AudioEngineError(f"libmysofa n'a pas pu ouvrir {sofa_path} (erreur {err.value})")
        self._handle = handle
        self.length = n.value

    @staticmethod
    def _load_library(library: str | None):
        candidates = [library] if library else []
        found = ctypes.util.find_library("mysofa")
        if found:
            candidates.append(found)
        candidates += ["libmysofa.so.1", "libmysofa.so", "libmysofa.dylib"]
        for name in candidates:
            try:
                return ctypes.CDLL(name)
            except OSError:
                continue
        raise AudioEngineError("libmysofa introuvable : installez le paquet libmysofa")

    def hrir(self, direction):
        x, y, z = (float(v) for v in direction)
        left = (ctypes.c_float * self.length)()
        right = (ctypes.c_float * self.length)()
        dl, dr = ctypes.c_float(0), ctypes.c_float(0)
        self._lib.mysofa_getfilter_float(self._handle, x, y, z, left, right,
                                         ctypes.byref(dl), ctypes.byref(dr))
        return (np.frombuffer(left, dtype=np.float32).astype(float),
                np.frombuffer(right, dtype=np.float32).astype(float), dl.value, dr.value)

    def close(self):
        if getattr(self, "_handle", None):
            self._lib.mysofa_close(self._handle)
            self._handle = None

    def __del__(self):
        try:
            self.close()
        except Exception:
            pass


class SofaH5Hrtf:
    """Lecteur SOFA en Python pur (h5py), pour Windows/macOS où libmysofa manque.

    Interpolation par pondération inverse de la distance angulaire des trois
    directions mesurées les plus proches ; rééchantillonnage polyphase.
    """

    def __init__(self, sofa_path: Path | str, fs: int):
        try:
            import h5py
        except ImportError as exc:
            raise AudioEngineError("Ni libmysofa ni h5py ne sont disponibles pour lire le HRTF") from exc
        self.fs = int(fs)
        with h5py.File(str(sofa_path), "r") as f:
            irs = np.asarray(f["Data.IR"], dtype=float)                    # (M, 2, N)
            sofa_fs = float(np.asarray(f["Data.SamplingRate"]).ravel()[0])
            pos = np.asarray(f["SourcePosition"], dtype=float)             # (M, 3)
            kind = f["SourcePosition"].attrs.get("Type", b"spherical")
            delay = np.asarray(f["Data.Delay"], dtype=float) if "Data.Delay" in f else np.zeros((1, 2))
        kind = kind.decode() if isinstance(kind, bytes) else str(kind)
        if irs.ndim != 3 or irs.shape[1] != 2:
            raise AudioEngineError("SOFA inattendu : Data.IR doit être (M, 2, N)")
        if kind.lower().startswith("cartesian"):
            vec = pos
        else:
            az, el = np.radians(pos[:, 0]), np.radians(pos[:, 1])
            vec = np.stack([np.cos(el) * np.cos(az), np.cos(el) * np.sin(az), np.sin(el)], axis=1)
        self._dirs = vec / np.maximum(np.linalg.norm(vec, axis=1, keepdims=True), 1e-12)
        if int(round(sofa_fs)) != self.fs:
            g = math.gcd(self.fs, int(round(sofa_fs)))
            irs = signal.resample_poly(irs, self.fs // g, int(round(sofa_fs)) // g, axis=2)
        self._irs = irs
        self._delays = np.broadcast_to(delay, (len(irs), 2)) / sofa_fs  # échantillons → s
        self.length = irs.shape[2]

    def hrir(self, direction):
        d = np.asarray(direction, dtype=float)
        d = d / max(float(np.linalg.norm(d)), 1e-12)
        ang = np.arccos(np.clip(self._dirs @ d, -1.0, 1.0))
        idx = np.argsort(ang)[:3]
        if ang[idx[0]] < 1e-6:
            w = np.array([1.0, 0.0, 0.0])
        else:
            w = 1.0 / ang[idx]
            w /= w.sum()
        ir = np.tensordot(w, self._irs[idx], axes=1)
        dl, dr = np.tensordot(w, self._delays[idx], axes=1)
        return ir[0], ir[1], float(dl), float(dr)

    def close(self):
        pass


def open_hrtf(sofa_path: Path | str, fs: int):
    """libmysofa si disponible, sinon lecteur h5py."""
    try:
        return MysofaHrtf(sofa_path, fs)
    except AudioEngineError:
        return SofaH5Hrtf(sofa_path, fs)


def fibonacci_sphere(n: int, min_elevation_deg: float = -40.0) -> np.ndarray:
    """Directions ~uniformes sur la sphère (sous l'élévation min. exclue : absente de KEMAR)."""
    i = np.arange(n) + 0.5
    z = 1 - 2 * i / n
    r = np.sqrt(1 - z * z)
    phi = np.pi * (1 + 5 ** 0.5) * i
    pts = np.stack([r * np.cos(phi), r * np.sin(phi), z], axis=1)
    return pts[np.degrees(np.arcsin(pts[:, 2])) >= min_elevation_deg]


def diffuse_field_db(hrtf: HrtfProvider, nfft: int, n_dirs: int = 200) -> np.ndarray:
    """Réponse champ diffus (moyenne en puissance des deux oreilles), dB, lissée 1/3 oct."""
    power = np.zeros(nfft // 2 + 1)
    dirs = fibonacci_sphere(n_dirs)
    for d in dirs:
        hl, hr, _, _ = hrtf.hrir(d)
        power += np.abs(np.fft.rfft(hl, nfft)) ** 2 + np.abs(np.fft.rfft(hr, nfft)) ** 2
    power /= 2 * len(dirs)
    freqs = np.fft.rfftfreq(nfft, 1 / hrtf.fs)
    db = 10 * np.log10(np.maximum(smooth_octave(freqs, power, 3), 1e-12))
    band = (freqs >= 200) & (freqs <= 2000)
    return db - np.mean(db[band])


# ---------------------------------------------------------------------------
# Pièce
# ---------------------------------------------------------------------------
# Bandes d'octave du modèle acoustique.
OCTAVES = np.array([125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0])

# Coefficients d'absorption par bande d'octave (valeurs usuelles de la littérature).
MATERIALS: dict[str, tuple[float, ...]] = {
    "drywall": (0.29, 0.10, 0.05, 0.04, 0.07, 0.09, 0.09),   # plaque de plâtre sur ossature
    "masonry": (0.02, 0.02, 0.03, 0.04, 0.05, 0.06, 0.06),   # enduit sur brique / béton
    "panel": (0.30, 0.75, 1.00, 1.00, 1.00, 1.00, 0.95),     # panneau laine minérale 10 cm
    "foam5": (0.08, 0.25, 0.60, 0.90, 0.95, 0.95, 0.95),     # mousse acoustique 5 cm
    "wool20": (0.65, 0.95, 1.00, 1.00, 1.00, 1.00, 0.95),    # laine 20 cm, large bande
    "tile": (0.01, 0.01, 0.015, 0.02, 0.02, 0.02, 0.02),     # carrelage / béton
    "wood": (0.15, 0.11, 0.10, 0.07, 0.06, 0.07, 0.07),      # parquet sur lambourdes
    "carpet": (0.08, 0.24, 0.57, 0.69, 0.71, 0.73, 0.73),    # moquette épaisse sur thibaude
}
ABSORBERS = {"foam5": "Mousse 5 cm", "wool10": "Laine 10 cm", "wool20": "Laine 20 cm"}
WALLS = {"masonry": "Maçonnerie (brique, béton)", "drywall": "Placo (plaque de plâtre)"}
FLOORS = {"tile": "Carrelage / béton", "wood": "Parquet", "rug": "Parquet + tapis",
          "carpet": "Moquette"}
CEILINGS = {"plaster": "Plâtre", "cloud": "Nuage acoustique"}
FURNITURE = {"empty": "Vide", "furnished": "Meublé", "full": "Très meublé"}
# Aire d'absorption équivalente du mobilier (m²) pour 25 m² de sol.
_FURNITURE_AREA = {"empty": np.zeros(7),
                   "furnished": np.array([2.0, 3.0, 4.0, 5.0, 5.0, 5.0, 5.0]),
                   "full": np.array([4.0, 6.0, 9.0, 11.0, 11.0, 11.0, 11.0])}
# Pièges à basses d'angle (jeu complet), aire équivalente (m²).
_BASS_TRAPS_AREA = np.array([6.0, 4.0, 2.0, 1.0, 1.0, 1.0, 1.0])
# Absorption de l'air, coefficient m (1/m) à 20 °C, 50 % HR.
_AIR_M = np.array([0.0001, 0.0003, 0.0006, 0.001, 0.0024, 0.0075, 0.024])

# Surfaces de la pièce, dans l'ordre utilisé par les sources-images :
# x=0 mur arrière, x=L mur avant (enceintes), y=0 mur droit, y=W mur gauche, sol, plafond.
SURFACES = ("rear", "front", "right", "left", "floor", "ceiling")


@dataclass(frozen=True)
class RoomSpec:
    width: float = 4.5
    length: float = 6.0
    height: float = 2.6
    order: int = 2
    walls: str = "masonry"
    treatment: float = 0.0        # fraction des murs couverte de panneaux absorbants
    bass_traps: float = 0.0       # 0 = aucun, 1 = jeu complet de pièges d'angle
    floor: str = "rug"
    ceiling: str = "plaster"
    furniture: str = "furnished"
    diffusion: float = 0.0        # diffusion du mur arrière (0–1)
    front_gap: float = 0.8        # distance enceintes – mur avant (m)
    ear_height: float = EAR_HEIGHT
    rt60_override: float = 0.0    # > 0 : RT60 imposé (moyenne 500 Hz–1 kHz)
    absorber: str = "wool10"      # foam5 | wool10 | wool20
    per_wall: bool = False        # traitement réglé paroi par paroi
    treatment_front: float = 0.0
    treatment_sides: float = 0.0
    treatment_rear: float = 0.0
    treatment_ceiling: float = 0.0
    speaker_height: float = 0.0   # 0 = à hauteur d'oreille
    listener_offset: float = 0.0  # décalage latéral de l'auditeur (m, + = vers la gauche)

    @property
    def volume(self) -> float:
        return self.width * self.length * self.height

    def areas(self) -> np.ndarray:
        w, l, h = self.width, self.length, self.height
        return np.array([w * h, w * h, l * h, l * h, l * w, l * w])

    @property
    def surface(self) -> float:
        return float(self.areas().sum())

    def surface_absorption(self) -> np.ndarray:
        """Coefficients d'absorption (6 surfaces × 7 bandes)."""
        m = {k: np.array(v) for k, v in MATERIALS.items()}
        t = min(max(self.treatment, 0.0), 1.0)
        bare = m["drywall"] if self.walls == "drywall" else m["masonry"]
        panel = m["panel"] if self.absorber == "wool10" else m.get(self.absorber, m["panel"])
        mix = lambda frac: (1 - min(max(frac, 0.0), 1.0)) * bare + min(max(frac, 0.0), 1.0) * panel
        wall = mix(t)
        floor = {"tile": m["tile"], "wood": m["wood"], "carpet": m["carpet"],
                 "rug": 0.5 * m["wood"] + 0.5 * m["carpet"]}.get(self.floor, m["wood"])
        ceiling = mix(0.5) if self.ceiling == "cloud" else bare
        if self.per_wall:
            sides = mix(self.treatment_sides)
            return np.stack([mix(self.treatment_rear), mix(self.treatment_front), sides, sides,
                             floor, mix(self.treatment_ceiling)])
        return np.stack([wall, wall, wall, wall, floor, ceiling])

    def wall_treatment(self) -> dict[str, float]:
        """Taux de traitement par paroi (pour l'affichage)."""
        if self.per_wall:
            return {"front": self.treatment_front, "rear": self.treatment_rear,
                    "left": self.treatment_sides, "right": self.treatment_sides,
                    "ceiling": self.treatment_ceiling}
        return {"front": self.treatment, "rear": self.treatment, "left": self.treatment,
                "right": self.treatment, "ceiling": 0.5 if self.ceiling == "cloud" else 0.0}

    def _auto_rt60(self) -> tuple[np.ndarray, np.ndarray]:
        alpha = self.surface_absorption()
        extra = (_FURNITURE_AREA.get(self.furniture, np.zeros(7)) * (self.length * self.width / 25)
                 + _BASS_TRAPS_AREA * min(max(self.bass_traps, 0.0), 1.0))
        a_total = (self.areas()[:, None] * alpha).sum(axis=0) + extra
        mean_alpha = np.clip(a_total / self.surface, 0.01, 0.95)
        v, s = self.volume, self.surface
        rt = 0.161 * v / (-s * np.log(1 - mean_alpha) + 4 * _AIR_M * v)   # Eyring
        return rt, mean_alpha

    def rt60_bands(self) -> np.ndarray:
        """RT60 (s) par bande d'octave."""
        rt, _ = self._auto_rt60()
        if self.rt60_override > 0:
            rt = rt * self.rt60_override / float(np.mean(rt[2:4]))
        return rt

    @property
    def rt60(self) -> float:
        """RT60 « moyen » (500 Hz – 1 kHz)."""
        return float(np.mean(self.rt60_bands()[2:4]))

    def reflection_bands(self) -> np.ndarray:
        """Coefficient de réflexion spéculaire β (6 surfaces × 7 bandes)."""
        alpha = self.surface_absorption()
        if self.rt60_override > 0:
            # RT imposé : absorption uniforme cohérente avec ce RT (Eyring inversé).
            v, s = self.volume, self.surface
            rt = self.rt60_bands()
            mean_alpha = 1 - np.exp(-(0.161 * v / rt - 4 * _AIR_M * v) / s)
            alpha = np.tile(np.clip(mean_alpha, 0.01, 0.95), (6, 1))
        beta = np.sqrt(np.clip(1 - alpha, 0.0, 1.0))
        beta[0] *= math.sqrt(1 - 0.7 * min(max(self.diffusion, 0.0), 1.0))  # mur arrière diffusant
        return beta

    def reflection_coefficient(self) -> float:
        """β moyen (rapport, compatibilité)."""
        return float(np.mean(self.reflection_bands()[:, 2:4]))

    def critical_distance_bands(self) -> np.ndarray:
        return 0.057 * np.sqrt(self.volume / self.rt60_bands())

    def critical_distance(self) -> float:
        return 0.057 * math.sqrt(self.volume / self.rt60)


@dataclass(frozen=True)
class Layout:
    """Position de l'auditeur et des enceintes ; repère pièce = repère auditeur
    (x vers l'avant / le mur avant, y vers la gauche, z vers le haut)."""

    listener: np.ndarray
    speakers: dict[str, np.ndarray]


def stereo_layout(room: RoomSpec, angle_deg: float, distance: float) -> Layout:
    a = math.radians(angle_deg)
    sx = room.length - room.front_gap
    lx = sx - distance * math.cos(a)
    if lx < 0.3:
        raise AudioEngineError("La pièce est trop courte pour cette distance d'écoute")
    cy = room.width / 2
    dy = distance * math.sin(a)
    if cy + dy > room.width - 0.2:
        raise AudioEngineError("La pièce est trop étroite pour cet angle/distance")
    z = min(max(room.ear_height, 0.3), room.height - 0.3)
    zs = z if room.speaker_height <= 0 else min(max(room.speaker_height, 0.2), room.height - 0.2)
    ly = min(max(cy + room.listener_offset, 0.3), room.width - 0.3)
    return Layout(listener=np.array([lx, ly, z]),
                  speakers={"L": np.array([sx, cy + dy, zs]),
                            "R": np.array([sx, cy - dy, zs])})


@dataclass(frozen=True)
class ImageSource:
    position: np.ndarray
    reflections: int
    counts: tuple[int, ...]  # réflexions par surface, ordre SURFACES

    def gain_bands(self, beta: np.ndarray) -> np.ndarray:
        """Atténuation par bande d'octave (produit des β des surfaces touchées)."""
        return np.prod(beta ** np.asarray(self.counts)[:, None], axis=0)


def image_sources(source: np.ndarray, room: RoomSpec, order: int) -> list[ImageSource]:
    """Sources-images (Allen & Berkley) jusqu'à ``order`` réflexions incluses.

    Pour chaque axe d'indice n et de parité p : |n − p| réflexions sur la paroi
    à 0 et |n| sur la paroi opposée.
    """
    dims = np.array([room.length, room.width, room.height])
    out = []
    rng = range(-order, order + 1)
    for nx in rng:
        for ny in rng:
            for nz in rng:
                n = np.array([nx, ny, nz])
                for p in np.ndindex(2, 2, 2):
                    p = np.array(p)
                    near = np.abs(n - p)
                    far = np.abs(n)
                    refl = int(near.sum() + far.sum())
                    if refl > order:
                        continue
                    pos = (1 - 2 * p) * source + 2 * n * dims
                    counts = (int(near[0]), int(far[0]), int(near[1]), int(far[1]),
                              int(near[2]), int(far[2]))
                    out.append(ImageSource(pos, refl, counts))
    return out


def bands_to_grid(values: np.ndarray, freqs: np.ndarray) -> np.ndarray:
    """Valeurs par bande d'octave → grille de fréquences (interpolation log, extrémités tenues)."""
    f = np.maximum(freqs, 1.0)
    return np.interp(np.log(f), np.log(OCTAVES), values)


# ---------------------------------------------------------------------------
# Rendu
# ---------------------------------------------------------------------------
@dataclass
class RenderParams:
    fs: int = 48000
    angle_deg: float = 30.0
    distance: float = 2.0
    room: RoomSpec = field(default_factory=RoomSpec)
    room_enabled: bool = True
    room_amount: float = 1.0
    diffuse_field_eq: bool = True
    ir_length_ms: int = 400
    preamp_db: float = 0.0
    balance_db: float = 0.0       # + = enceinte droite plus forte
    bass_db: float = 0.0          # plateau grave (105 Hz)
    treble_db: float = 0.0        # plateau aigu (10 kHz)
    late_enabled: bool = True     # réverbération tardive
    seed: int = 20240601


@dataclass
class RenderResult:
    fs: int
    irs: dict[str, np.ndarray]          # "LL", "LR", "RL", "RR"
    gain_db: float
    plot_freqs: np.ndarray
    plot_curves: dict[str, np.ndarray]
    notes: list[str] = field(default_factory=list)
    rt60_bands: np.ndarray | None = None


ProgressFn = Callable[[float, str], None]


def _late_tail(n: int, fs: int, rt60: np.ndarray, energy: np.ndarray,
               rng: np.random.Generator) -> np.ndarray:
    """Queue de réverbération : bruit par bande d'octave, décroissance RT60 propre à
    chaque bande, énergie ``energy[b]`` (relative à un son direct d'énergie 1)."""
    t = np.arange(n) / fs
    noise = rng.standard_normal(n)
    nyq = fs / 2
    edges = [0.0] + [f * math.sqrt(2) for f in OCTAVES[:-1]] + [nyq]
    out = np.zeros(n)
    for b in range(len(OCTAVES)):
        lo, hi = edges[b], min(edges[b + 1], nyq * 0.98)
        if lo == 0:
            sos = signal.butter(4, hi, "lowpass", fs=fs, output="sos")
        elif b == len(OCTAVES) - 1:
            sos = signal.butter(4, lo, "highpass", fs=fs, output="sos")
        else:
            sos = signal.butter(4, [lo, hi], "bandpass", fs=fs, output="sos")
        band = signal.sosfiltfilt(sos, noise) * np.exp(-6.9078 * t / rt60[b])
        e = float(np.sum(band ** 2))
        if e > 0:
            # Part du spectre d'un son direct blanc tombant dans cette bande.
            share = (min(edges[b + 1], nyq) - lo) / nyq
            out += band * math.sqrt(energy[b] * share / e)
    return out


def render(speaker: SpeakerResponse, hp_fir: np.ndarray | None, hrtf: HrtfProvider,
           params: RenderParams, progress: ProgressFn | None = None) -> RenderResult:
    """Construit les quatre réponses impulsionnelles binaurales."""
    fs = params.fs
    if hrtf.fs != fs:
        raise AudioEngineError("Le HRTF n'est pas à la fréquence d'échantillonnage du rendu")
    report = progress or (lambda _f, _m: None)
    notes: list[str] = []
    room = params.room
    use_room = params.room_enabled and params.room_amount > 0
    rt_bands = room.rt60_bands()
    ir_ms = params.ir_length_ms if not use_room else max(params.ir_length_ms,
                                                         900 * float(np.max(rt_bands[1:5])))
    ir_len = int(min(ir_ms, 2500) * fs / 1000)
    nfft = next_pow2(ir_len + 3 * FIR_TAPS)
    freqs = np.fft.rfftfreq(nfft, 1 / fs)
    w = 2 * np.pi * freqs

    def spectrum_of(curve_db):
        return np.fft.rfft(minimum_phase_fir(speaker.freqs, curve_db, fs), nfft)

    report(0.05, "Filtres du haut-parleur")
    h_lw = spectrum_of(speaker.listening_window)
    h_er = spectrum_of(speaker.early_reflections)
    h_sp = spectrum_of(speaker.sound_power)
    h_hp = np.fft.rfft(hp_fir, nfft) if hp_fir is not None else np.ones_like(h_lw)
    # Tonalité (plateaux grave / aigu, phase minimale) appliquée avec la correction du casque.
    for bq in (Biquad("LSC", 105.0, params.bass_db, 0.7), Biquad("HSC", 10000.0, params.treble_db, 0.7)):
        coeffs = biquad_coefficients(bq, fs) if abs(bq.gain_db) > 1e-6 else None
        if coeffs is not None:
            _, resp = signal.freqz(coeffs[0], coeffs[1], worN=w / fs)
            h_hp = h_hp * resp

    report(0.15, "Champ diffus du HRTF")
    df_db = diffuse_field_db(hrtf, nfft)
    df_mag = 10 ** (df_db / 20)
    if params.diffuse_field_eq:
        h_df_inv = minimum_phase_spectrum(10 ** (np.clip(-df_db, -12, 12) / 20), nfft)
        late_color = np.ones_like(h_lw)
    else:
        h_df_inv = np.ones_like(h_lw)
        late_color = minimum_phase_spectrum(df_mag, nfft)

    layout = stereo_layout(room, params.angle_deg, params.distance)
    beta = room.reflection_bands()
    order = room.order if use_room else 0
    pre_delay = 0.0005  # marge avant le son direct (s)

    rng = np.random.default_rng(params.seed)
    late_energy = np.zeros(len(OCTAVES))
    t_start = 0.0
    if use_room:
        rc = room.critical_distance_bands()
        late_energy = (params.distance / rc) ** 2 * params.room_amount
        # Sous la fréquence de Schroeder (≈ 2000·√(T/V)), le champ diffus statistique
        # surestime l'énergie (régime modal) : excès borné à +6 dB par rapport aux médiums.
        late_energy = np.minimum(late_energy, 4.0 * float(np.mean(late_energy[2:4])))
        if not params.late_enabled:
            late_energy = np.zeros(len(OCTAVES))
        t_start = max(2e-3 * math.sqrt(room.volume), 0.012)
        notes.append(f"RT60 {room.rt60:.2f} s (500 Hz–1 kHz), distance critique "
                     f"{room.critical_distance():.2f} m, "
                     f"{len(image_sources(layout.speakers['L'], room, order))} sources-images/enceinte")

    # Référence temporelle : l'enceinte la plus proche (auditeur décentré possible).
    d_ref = min(float(np.linalg.norm(layout.speakers[k] - layout.listener)) for k in ("L", "R"))
    side_gain = {"L": 10 ** (-params.balance_db / 40), "R": 10 ** (params.balance_db / 40)}
    spectra: dict[str, np.ndarray] = {}
    for si, side in enumerate(("L", "R")):
        src = layout.speakers[side]
        direct = [np.zeros(len(freqs), complex), np.zeros(len(freqs), complex)]
        early = [np.zeros(len(freqs), complex), np.zeros(len(freqs), complex)]
        for img in image_sources(src, room, order):
            vec = img.position - layout.listener
            dist = float(np.linalg.norm(vec))
            tau = (dist - d_ref) / SPEED_OF_SOUND + pre_delay
            if tau * fs >= ir_len:
                continue
            amp = params.distance / dist
            if img.reflections:
                # Réflexion filtrée par les matériaux de chaque paroi touchée.
                amp = amp * params.room_amount * bands_to_grid(img.gain_bands(beta), freqs)
            hl, hr, dl, dr = hrtf.hrir(vec / dist)
            target = direct if img.reflections == 0 else early
            for ei, (h, d) in enumerate(((hl, dl), (hr, dr))):
                target[ei] += amp * np.fft.rfft(h, nfft) * np.exp(-1j * w * (tau + d))
        report(0.3 + 0.3 * si, f"Haut-parleur {side} : sons directs et réflexions")

        for ei, ear in enumerate(("L", "R")):
            total = h_df_inv * (h_lw * direct[ei] + h_er * early[ei])
            if np.any(late_energy > 0):
                tail = _late_tail(ir_len, fs, rt_bands, late_energy, rng)
                t = np.arange(ir_len) / fs
                onset = np.clip((t - pre_delay - t_start) / 0.01, 0, 1)
                tail *= 0.5 - 0.5 * np.cos(np.pi * onset)
                total += np.fft.rfft(tail, nfft) * h_sp * late_color
            spectra[side + ear] = total * h_hp * side_gain[side]
        report(0.45 + 0.3 * si, f"Haut-parleur {side} : réverbération")

    # Normalisation : pire cas = signal mono identique sur les deux enceintes.
    report(0.85, "Normalisation")
    band = (freqs >= 20) & (freqs <= 20000)
    peak = -np.inf
    for ear in ("L", "R"):
        p = np.abs(spectra["L" + ear] + spectra["R" + ear]) ** 2
        peak = max(peak, float(np.max(10 * np.log10(smooth_octave(freqs, p, 6)[band] + 1e-20))))
    gain_db = -HEADROOM_DB - peak + params.preamp_db
    gain = 10 ** (gain_db / 20)

    irs = {}
    for key, spec in spectra.items():
        h = np.fft.irfft(spec, nfft)[:ir_len] * gain
        irs[key] = _fade_out(h, 0.05).astype(np.float32)
    if max(float(np.max(np.abs(h))) for h in irs.values()) > 1.0:
        notes.append("Attention : crête d'IR > 0 dBFS")

    plot_f = np.geomspace(20, min(20000, fs / 2 * 0.95), 300)

    def plot_db(spec):
        p = smooth_octave(freqs, np.abs(spec) ** 2, 6)
        return 10 * np.log10(np.interp(plot_f, freqs, p) + 1e-20)

    curves = {
        "Haut-parleur (Listening Window)": np.interp(np.log(plot_f), np.log(speaker.freqs),
                                                     speaker.listening_window),
        "Correction casque": plot_db(h_hp),
        "Oreille gauche (mono, total)": plot_db((spectra["LL"] + spectra["RL"]) * gain),
    }
    report(1.0, "Terminé")
    return RenderResult(fs=fs, irs=irs, gain_db=gain_db, plot_freqs=plot_f,
                        plot_curves=curves, notes=notes, rt60_bands=rt_bands)


def write_irs(result: RenderResult, directory: Path) -> dict[str, Path]:
    """Écrit les IR en WAV float32 mono (écriture atomique)."""
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    paths = {}
    for key in PAIRS:
        dest = directory / f"ir_{key}.wav"
        fd, tmp = tempfile.mkstemp(dir=directory, prefix=f".ir_{key}.", suffix=".wav")
        os.close(fd)
        try:
            wavfile.write(tmp, result.fs, result.irs[key].astype(np.float32))
            os.replace(tmp, dest)
        except BaseException:
            Path(tmp).unlink(missing_ok=True)
            raise
        paths[key] = dest
    return paths


def write_true_stereo(result: RenderResult, path: Path) -> Path:
    """WAV 4 canaux « true stereo » (ordre LL, LR, RL, RR) pour convolveurs génériques."""
    data = np.stack([result.irs[k] for k in PAIRS], axis=1).astype(np.float32)
    wavfile.write(str(path), result.fs, data)
    return Path(path)


def equalizer_apo_config(ir_paths: dict[str, Path]) -> str:
    """Configuration Equalizer APO (≥ 1.2, canaux virtuels) réalisant la matrice 2×2."""
    def q(p: Path) -> str:
        return str(Path(p).resolve())

    return "\n".join([
        "# Speaker Sim (devfrp) – à inclure depuis config.txt : Include: speaker-sim.txt",
        "Channel: all",
        "Copy: VLL=L VLR=L VRL=R VRR=R",
        *(f"Channel: V{k}\nConvolution: {q(ir_paths[k])}" for k in PAIRS),
        "Channel: all",
        "Copy: L=VLL+VRL R=VLR+VRR",
        "",
    ])
