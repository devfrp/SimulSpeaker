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
@dataclass(frozen=True)
class RoomSpec:
    width: float = 4.5
    length: float = 6.0
    height: float = 2.6
    rt60: float = 0.35
    order: int = 2

    @property
    def volume(self) -> float:
        return self.width * self.length * self.height

    @property
    def surface(self) -> float:
        w, l, h = self.width, self.length, self.height
        return 2 * (w * l + w * h + l * h)

    def reflection_coefficient(self) -> float:
        """β des parois depuis le RT60 (Sabine), identique pour toutes les parois."""
        alpha = 0.161 * self.volume / (self.surface * self.rt60)
        alpha = min(max(alpha, 0.02), 0.99)
        return math.sqrt(1 - alpha)

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
    front_gap = min(1.0, max(0.3, (room.length - distance * math.cos(a)) / 3))
    sx = room.length - front_gap
    lx = sx - distance * math.cos(a)
    if lx < 0.3:
        raise AudioEngineError("La pièce est trop courte pour cette distance d'écoute")
    cy = room.width / 2
    dy = distance * math.sin(a)
    if cy + dy > room.width - 0.2:
        raise AudioEngineError("La pièce est trop étroite pour cet angle/distance")
    return Layout(listener=np.array([lx, cy, EAR_HEIGHT]),
                  speakers={"L": np.array([sx, cy + dy, EAR_HEIGHT]),
                            "R": np.array([sx, cy - dy, EAR_HEIGHT])})


@dataclass(frozen=True)
class ImageSource:
    position: np.ndarray
    reflections: int
    gain: float  # β^reflections


def image_sources(source: np.ndarray, room: RoomSpec, order: int, beta: float) -> list[ImageSource]:
    """Sources-images (Allen & Berkley) jusqu'à ``order`` réflexions incluses."""
    dims = np.array([room.length, room.width, room.height])
    out = []
    rng = range(-order, order + 1)
    for nx in rng:
        for ny in rng:
            for nz in rng:
                n = np.array([nx, ny, nz])
                for p in np.ndindex(2, 2, 2):
                    p = np.array(p)
                    refl = int(np.sum(np.abs(n - p) + np.abs(n)))
                    if refl > order:
                        continue
                    pos = (1 - 2 * p) * source + 2 * n * dims
                    out.append(ImageSource(pos, refl, beta ** refl))
    return out


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
    seed: int = 20240601


@dataclass
class RenderResult:
    fs: int
    irs: dict[str, np.ndarray]          # "LL", "LR", "RL", "RR"
    gain_db: float
    plot_freqs: np.ndarray
    plot_curves: dict[str, np.ndarray]
    notes: list[str] = field(default_factory=list)


ProgressFn = Callable[[float, str], None]


def _band_rt_noise(n: int, fs: int, rt60: float, rng: np.random.Generator) -> np.ndarray:
    """Bruit gaussien à décroissance exponentielle, RT60 plus court dans les aigus."""
    t = np.arange(n) / fs
    noise = rng.standard_normal(n)
    bands = [(signal.butter(4, 500, "lowpass", fs=fs, output="sos"), 1.15),
             (signal.butter(4, [500, 4000], "bandpass", fs=fs, output="sos"), 1.0),
             (signal.butter(4, 4000, "highpass", fs=fs, output="sos"), 0.7)]
    out = np.zeros(n)
    for sos, factor in bands:
        out += signal.sosfiltfilt(sos, noise) * np.exp(-6.9078 * t / (rt60 * factor))
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
    ir_ms = params.ir_length_ms if not use_room else max(params.ir_length_ms, 900 * room.rt60)
    ir_len = int(min(ir_ms, 1500) * fs / 1000)
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
    beta = room.reflection_coefficient()
    order = room.order if use_room else 0
    pre_delay = 0.0005  # marge avant le son direct (s)

    rng = np.random.default_rng(params.seed)
    late_energy = 0.0
    t_start = 0.0
    if use_room:
        rc = room.critical_distance()
        late_energy = (params.distance / rc) ** 2 * params.room_amount
        t_start = max(2e-3 * math.sqrt(room.volume), 0.012)
        notes.append(f"Distance critique {rc:.2f} m, β={beta:.2f}, "
                     f"{len(image_sources(layout.speakers['L'], room, order, beta))} sources-images/enceinte")

    spectra: dict[str, np.ndarray] = {}
    for si, side in enumerate(("L", "R")):
        src = layout.speakers[side]
        direct = [np.zeros(len(freqs), complex), np.zeros(len(freqs), complex)]
        early = [np.zeros(len(freqs), complex), np.zeros(len(freqs), complex)]
        for img in image_sources(src, room, order, beta):
            vec = img.position - layout.listener
            dist = float(np.linalg.norm(vec))
            tau = (dist - params.distance) / SPEED_OF_SOUND + pre_delay
            if tau * fs >= ir_len:
                continue
            amp = img.gain * params.distance / dist
            if img.reflections:
                amp *= params.room_amount
            hl, hr, dl, dr = hrtf.hrir(vec / dist)
            target = direct if img.reflections == 0 else early
            for ei, (h, d) in enumerate(((hl, dl), (hr, dr))):
                target[ei] += amp * np.fft.rfft(h, nfft) * np.exp(-1j * w * (tau + d))
        report(0.3 + 0.3 * si, f"Haut-parleur {side} : sons directs et réflexions")

        for ei, ear in enumerate(("L", "R")):
            total = h_df_inv * (h_lw * direct[ei] + h_er * early[ei])
            if late_energy > 0:
                tail = _band_rt_noise(ir_len, fs, room.rt60, rng)
                tail *= math.sqrt(late_energy / np.sum(tail ** 2))
                t = np.arange(ir_len) / fs
                onset = np.clip((t - pre_delay - t_start) / 0.01, 0, 1)
                tail *= 0.5 - 0.5 * np.cos(np.pi * onset)
                total += np.fft.rfft(tail, nfft) * h_sp * late_color
            spectra[side + ear] = total * h_hp
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
                        plot_curves=curves, notes=notes)


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
