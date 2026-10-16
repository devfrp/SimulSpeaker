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
    "speaker_angle": (10.0, 80.0),
    "speaker_distance": (0.5, 10.0),
    "room_width": (2.0, 20.0),
    "room_length": (2.0, 30.0),
    "room_height": (2.0, 8.0),
    "rt60": (0.1, 4.0),
    "room_treatment": (0.0, 1.0),
    "room_bass_traps": (0.0, 1.0),
    "room_diffusion": (0.0, 1.0),
    "front_gap": (0.2, 5.0),
    "ear_height": (0.6, 2.0),
    "room_amount": (0.0, 2.0),
    "reflection_order": (0, 4),
    "preamp_db": (-30.0, 20.0),
    "ir_length_ms": (50, 2500),
    "treatment_front": (0.0, 1.0),
    "treatment_sides": (0.0, 1.0),
    "treatment_rear": (0.0, 1.0),
    "treatment_ceiling": (0.0, 1.0),
    "speaker_height": (0.0, 3.0),
    "listener_offset": (-3.0, 3.0),
    "balance_db": (-6.0, 6.0),
    "bass_db": (-12.0, 12.0),
    "treble_db": (-12.0, 12.0),
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
    rt60: float = 0.35                   # secondes, utilisé si rt60_auto est faux
    rt60_auto: bool = True               # RT60 calculé depuis les matériaux (par octave)
    room_preset: str = "salon"           # "" = réglages personnalisés
    room_walls: str = "masonry"          # masonry | drywall
    room_treatment: float = 0.0          # part des murs couverte de panneaux absorbants
    room_bass_traps: float = 0.0         # pièges à basses d'angle (0 = aucun, 1 = complet)
    room_floor: str = "rug"              # tile | wood | rug | carpet
    room_ceiling: str = "plaster"        # plaster | cloud
    room_furniture: str = "furnished"    # empty | furnished | full
    room_diffusion: float = 0.1          # diffusion du mur arrière
    front_gap: float = 0.8               # enceintes – mur avant (m)
    ear_height: float = 1.2              # hauteur des oreilles (m)
    speaker_height: float = 0.0          # hauteur des tweeters (m), 0 = à hauteur d'oreille
    listener_offset: float = 0.0         # décalage latéral de l'auditeur (m, + = gauche)
    room_absorber: str = "wool10"        # foam5 | wool10 | wool20
    treatment_per_wall: bool = False     # traitement réglé paroi par paroi
    treatment_front: float = 0.0
    treatment_sides: float = 0.0
    treatment_rear: float = 0.0
    treatment_ceiling: float = 0.0
    balance_db: float = 0.0              # + = enceinte droite plus forte
    bass_db: float = 0.0                 # tonalité : plateau grave 105 Hz
    treble_db: float = 0.0               # tonalité : plateau aigu 10 kHz
    tone_enabled: bool = True            # tonalité et balance actives
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
        for name, allowed in CHOICES.items():
            if getattr(s, name) not in allowed:
                setattr(s, name, getattr(Settings(), name))
        if s.room_preset not in ROOM_PRESETS:
            s.room_preset = ""
        s.ear_height = min(s.ear_height, s.room_height - 0.3)
        if s.speaker_height:
            s.speaker_height = min(max(s.speaker_height, 0.2), s.room_height - 0.2)
        s.listener_offset = max(-(s.room_width / 2 - 0.3), min(s.listener_offset, s.room_width / 2 - 0.3))
        # La pièce doit contenir le triangle stéréo (0,5 m de marge).
        import math
        a = math.radians(s.speaker_angle)
        s.room_width = max(s.room_width, 2 * s.speaker_distance * math.sin(a) + 1.0)
        s.room_length = max(s.room_length, s.front_gap + s.speaker_distance * math.cos(a) + 0.5)
        return s


# Valeurs autorisées des réglages à choix.
CHOICES: dict[str, tuple[str, ...]] = {
    "room_walls": ("masonry", "drywall"),
    "room_floor": ("tile", "wood", "rug", "carpet"),
    "room_ceiling": ("plaster", "cloud"),
    "room_furniture": ("empty", "furnished", "full"),
    "room_absorber": ("foam5", "wool10", "wool20"),
}

# Préréglages de salle : chaque entrée remplace les réglages de pièce correspondants.
ROOM_PRESETS: dict[str, dict[str, Any]] = {
    "salon": {"label": "Salon", "room_width": 4.5, "room_length": 6.0, "room_height": 2.6,
              "room_walls": "masonry", "room_treatment": 0.0, "room_bass_traps": 0.0,
              "room_floor": "rug", "room_ceiling": "plaster", "room_furniture": "furnished",
              "room_diffusion": 0.1, "front_gap": 0.8},
    "studio": {"label": "Studio traité", "room_width": 4.2, "room_length": 5.5,
               "room_height": 2.7, "room_walls": "masonry", "room_treatment": 0.25,
               "room_bass_traps": 1.0, "room_floor": "rug", "room_ceiling": "plaster",
               "room_furniture": "furnished", "room_diffusion": 0.6, "front_gap": 0.6},
    "regie": {"label": "Régie mate", "room_width": 3.8, "room_length": 4.6,
              "room_height": 2.6, "room_walls": "drywall", "room_treatment": 0.35,
              "room_bass_traps": 1.0, "room_floor": "rug", "room_ceiling": "cloud",
              "room_furniture": "furnished", "room_diffusion": 0.3, "front_gap": 0.5},
    "home_cinema": {"label": "Home-cinéma", "room_width": 4.0, "room_length": 6.0,
                    "room_height": 2.5, "room_walls": "drywall", "room_treatment": 0.3,
                    "room_bass_traps": 1.0, "room_floor": "carpet", "room_ceiling": "plaster",
                    "room_furniture": "furnished", "room_diffusion": 0.5, "front_gap": 0.7},
    "chambre": {"label": "Chambre", "room_width": 3.5, "room_length": 4.0, "room_height": 2.5,
                "room_walls": "masonry", "room_treatment": 0.0, "room_bass_traps": 0.0,
                "room_floor": "carpet", "room_ceiling": "plaster", "room_furniture": "full",
                "room_diffusion": 0.0, "front_gap": 0.4},
    "nue": {"label": "Pièce nue", "room_width": 4.0, "room_length": 5.0, "room_height": 2.5,
            "room_walls": "masonry", "room_treatment": 0.0, "room_bass_traps": 0.0,
            "room_floor": "tile", "room_ceiling": "plaster", "room_furniture": "empty",
            "room_diffusion": 0.0, "front_gap": 0.8},
}
ROOM_KEYS = ("room_width", "room_length", "room_height", "room_walls", "room_treatment",
             "room_bass_traps", "room_floor", "room_ceiling", "room_furniture",
             "room_diffusion", "front_gap", "room_absorber", "treatment_per_wall",
             "treatment_front", "treatment_sides", "treatment_rear", "treatment_ceiling")


def preset_settings(key: str) -> dict[str, Any]:
    """Réglages à appliquer pour un préréglage (RT60 recalculé automatiquement)."""
    preset = {k: v for k, v in ROOM_PRESETS[key].items() if k != "label"}
    return {**preset, "room_preset": key, "rt60_auto": True, "room_enabled": True,
            "room_absorber": "wool10", "treatment_per_wall": False}


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
