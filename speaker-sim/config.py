"""Paramètres, état d'exécution et chemins de l'application.

Fichiers gérés (aucun autre emplacement n'est écrit) ::

    ~/.local/state/devfrp-speaker-sim/config.json     paramètres utilisateur
    ~/.local/state/devfrp-speaker-sim/runtime.json    état d'exécution (sortie précédente…)
    ~/.local/state/devfrp-speaker-sim/filters/        IR générées + configuration filter-chain
    ~/.local/state/devfrp-speaker-sim/<unité>.service unité systemd (auto-démarrage)
    ~/.cache/devfrp-speaker-sim/                      mesures téléchargées

Les variables ``XDG_STATE_HOME`` / ``XDG_CACHE_HOME`` sont respectées.
Les noms de fichiers évitent volontairement ``settings.json`` et ``headphones/``
déjà utilisés par le greffon Omarchy du même nom.
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
from dataclasses import asdict, dataclass, field, fields
from pathlib import Path
from typing import Any

APP_ID = "devfrp-speaker-sim"
APP_NAME = "Speaker Sim"
APP_VERSION = "1.1.0"
# Exécutable PyInstaller : les données sont extraites dans sys._MEIPASS.
APP_DIR = Path(getattr(sys, "_MEIPASS", Path(__file__).resolve().parent))
IS_LINUX = sys.platform.startswith("linux")
DATA_DIR = APP_DIR / "data"
DEFAULT_CONFIG_FILE = DATA_DIR / "default_config.json"
SPEAKER_CATALOG_FILE = DATA_DIR / "speakers.json"


def _xdg(var: str, fallback: str) -> Path:
    value = os.environ.get(var, "")
    return Path(value) if value and os.path.isabs(value) else Path.home() / fallback


@dataclass(frozen=True)
class Paths:
    state_dir: Path
    cache_dir: Path

    @classmethod
    def default(cls) -> "Paths":
        """Linux : XDG. Windows : %LOCALAPPDATA%. macOS : ~/Library."""
        if sys.platform == "win32":
            base = Path(os.environ.get("LOCALAPPDATA") or Path.home() / "AppData" / "Local") / APP_ID
            return cls(state_dir=base / "State", cache_dir=base / "Cache")
        if sys.platform == "darwin":
            lib = Path.home() / "Library"
            return cls(state_dir=lib / "Application Support" / APP_ID, cache_dir=lib / "Caches" / APP_ID)
        return cls(state_dir=_xdg("XDG_STATE_HOME", ".local/state") / APP_ID,
                   cache_dir=_xdg("XDG_CACHE_HOME", ".cache") / APP_ID)

    @property
    def config_file(self) -> Path:
        return self.state_dir / "config.json"

    @property
    def runtime_file(self) -> Path:
        return self.state_dir / "runtime.json"

    @property
    def filters_dir(self) -> Path:
        return self.state_dir / "filters"

    def slot_dir(self, slot: str) -> Path:
        """IR et configuration d'une instance (A/B) ; chaque instance a ses fichiers."""
        return self.filters_dir / slot

    def slot_conf(self, slot: str) -> Path:
        return self.slot_dir(slot) / "filter-chain.conf"

    @property
    def filter_conf(self) -> Path:
        return self.slot_conf("a")

    @property
    def imports_dir(self) -> Path:
        """Mesures et EQ de casques importés par l'utilisateur."""
        return self.state_dir / "imports"

    @property
    def log_file(self) -> Path:
        return self.state_dir / "speaker-sim.log"

    def ensure(self) -> None:
        self.state_dir.mkdir(parents=True, exist_ok=True)
        if os.name == "posix":
            os.chmod(self.state_dir, 0o700)
        self.filters_dir.mkdir(parents=True, exist_ok=True)
        self.cache_dir.mkdir(parents=True, exist_ok=True)


# Bornes de validation : (min, max). Les valeurs hors bornes sont ramenées dedans.
LIMITS: dict[str, tuple[float, float]] = {
    "sample_rate": (0, 192000),
    "speaker_angle": (10.0, 60.0),
    "speaker_distance": (0.5, 6.0),
    "room_width": (2.0, 20.0),
    "room_length": (2.0, 30.0),
    "room_height": (2.0, 8.0),
    "rt60": (0.1, 2.0),
    "room_amount": (0.0, 1.5),
    "reflection_order": (0, 3),
    "preamp_db": (-24.0, 12.0),
    "ir_length_ms": (50, 1500),
}


@dataclass
class Settings:
    """Paramètres persistants (``config.json``)."""

    headphone: dict | None = None        # HeadphoneEntry.to_dict()
    speaker_id: str = "KEF R3"
    output_sink: str = ""                # nom du nœud PipeWire ; "" = sortie par défaut au démarrage
    sample_rate: int = 0                 # 0 = fréquence de l'horloge PipeWire
    speaker_angle: float = 30.0          # demi-angle du triangle stéréo (degrés)
    speaker_distance: float = 2.0        # mètres
    room_enabled: bool = True
    room_width: float = 4.5
    room_length: float = 6.0
    room_height: float = 2.6
    rt60: float = 0.35                   # secondes
    room_amount: float = 1.0             # 0 = anéchoïque, 1 = niveau physique
    reflection_order: int = 2
    diffuse_field_eq: bool = True
    headphone_eq_enabled: bool = True
    preamp_db: float = 0.0
    ir_length_ms: int = 400
    sofa_file: str = ""                  # "" = KEMAR fourni par libmysofa
    autostart: bool = False
    extra: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict) -> "Settings":
        known = {f.name: f for f in fields(cls)}
        kwargs: dict[str, Any] = {}
        defaults = cls()
        for name, value in data.items():
            if name not in known:
                continue
            default = getattr(defaults, name)
            kwargs[name] = _coerce(name, value, default)
        return cls(**kwargs)

    def to_dict(self) -> dict:
        return asdict(self)

    def validated(self) -> "Settings":
        """Copie avec valeurs ramenées dans les bornes et cohérence de la pièce."""
        s = Settings.from_dict(self.to_dict())
        for name, (lo, hi) in LIMITS.items():
            v = getattr(s, name)
            v = min(max(v, lo), hi)
            setattr(s, name, type(getattr(Settings(), name))(v))
        if s.sample_rate and s.sample_rate < 8000:
            s.sample_rate = 0
        # La pièce doit contenir le triangle stéréo (0,5 m de marge).
        import math
        a = math.radians(s.speaker_angle)
        s.room_width = max(s.room_width, 2 * s.speaker_distance * math.sin(a) + 1.0)
        s.room_length = max(s.room_length, s.speaker_distance * math.cos(a) + 1.5)
        return s


def _coerce(name: str, value: Any, default: Any) -> Any:
    if name == "headphone":
        if value is None:
            return None
        if isinstance(value, dict) and "path" in value and "name" in value:
            return {k: str(value.get(k, "")) for k in ("name", "source", "rig", "path", "target")}
        return None
    if name == "extra":
        return value if isinstance(value, dict) else {}
    try:
        if isinstance(default, bool):
            if isinstance(value, str):
                return value.strip().lower() in ("1", "true", "yes", "on", "oui")
            return bool(value)
        if isinstance(default, int):
            return int(round(float(value)))
        if isinstance(default, float):
            v = float(value)
            return v if v == v else default  # rejette NaN
        if isinstance(default, str):
            return str(value)
    except (TypeError, ValueError):
        return default
    return value


def atomic_write_json(path: Path, data: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=path.parent, prefix=f".{path.name}.")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            json.dump(data, fh, indent=2, ensure_ascii=False)
            fh.write("\n")
        os.replace(tmp, path)
    except BaseException:
        Path(tmp).unlink(missing_ok=True)
        raise


def _read_json(path: Path) -> dict | None:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        return data if isinstance(data, dict) else None
    except FileNotFoundError:
        return None
    except (OSError, ValueError):
        # Fichier corrompu : on le met de côté plutôt que de l'écraser silencieusement.
        try:
            path.replace(path.with_suffix(path.suffix + ".corrupt"))
        except OSError:
            pass
        return None


class ConfigStore:
    """Lecture/écriture de ``config.json`` et ``runtime.json``."""

    def __init__(self, paths: Paths | None = None):
        self.paths = paths or Paths.default()

    def load(self) -> Settings:
        base = _read_json(DEFAULT_CONFIG_FILE) or {}
        user = _read_json(self.paths.config_file) or {}
        return Settings.from_dict({**base, **user}).validated()

    def save(self, settings: Settings) -> None:
        atomic_write_json(self.paths.config_file, settings.validated().to_dict())

    # État d'exécution : sortie précédente, cible, horodatage…
    def load_runtime(self) -> dict:
        return _read_json(self.paths.runtime_file) or {}

    def save_runtime(self, runtime: dict) -> None:
        atomic_write_json(self.paths.runtime_file, runtime)

    def update_runtime(self, **changes: Any) -> dict:
        rt = self.load_runtime()
        for k, v in changes.items():
            if v is None:
                rt.pop(k, None)
            else:
                rt[k] = v
        self.save_runtime(rt)
        return rt


def load_speaker_catalog(path: Path = SPEAKER_CATALOG_FILE) -> list[dict]:
    data = json.loads(path.read_text(encoding="utf-8"))
    return list(data["speakers"])
