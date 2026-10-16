"""Téléchargement et cache des mesures (AutoEq, Spinorama, HRTF KEMAR).

Toutes les sources sont figées sur un commit Git : un fichier présent dans le
cache est donc immuable et n'est jamais retéléchargé. Le cache vit dans
``~/.cache/devfrp-speaker-sim/`` avec la disposition suivante ::

    autoeq/<commit12>/INDEX.md
    autoeq/<commit12>/<source>/<rig>/<modèle>/<modèle> ParametricEQ.txt
    spinorama/<commit12>/<haut-parleur>/CEA2034.txt
    hrtf/mit_kemar_normal_pinna.sofa          (seulement si libmysofa n'en fournit pas)

Ce module ne dépend pas de Qt : les callbacks de progression sont de simples
fonctions ``(octets_reçus, octets_totaux_ou_None) -> None``.
"""

from __future__ import annotations

import os
import re
import tempfile
import time
import urllib.parse
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Iterable

AUTOEQ_COMMIT = "7ae0f56d53074872b028649617a22bbb4232feb7"
AUTOEQ_RAW = "https://raw.githubusercontent.com/jaakkopasanen/AutoEq"
SPINORAMA_COMMIT = "acc757bb98d63327092ee537bde25d9c227811f3"
SPINORAMA_RAW = "https://raw.githubusercontent.com/pierreaubert/spinorama"
KEMAR_URL = "https://sofacoustics.org/data/database/mit/mit_kemar_normal_pinna.sofa"

# Emplacements où libmysofa installe généralement le HRTF MIT KEMAR.
SYSTEM_SOFA_PATHS = (
    "/usr/share/libmysofa/MIT_KEMAR_normal_pinna.sofa",
    "/usr/share/libmysofa/default.sofa",
    "/usr/local/share/libmysofa/MIT_KEMAR_normal_pinna.sofa",
)

USER_AGENT = "devfrp-speaker-sim/1.0 (+https://github.com/devfrp)"
IMPORT_PREFIX = "import:"

# Courbes cibles AutoEq proposées pour corriger une mesure brute importée.
AUTOEQ_TARGETS = (
    "Harman over-ear 2018",
    "Harman in-ear 2019",
    "Diffuse field ISO 11904-1",
    "Diffuse field GRAS KEMAR",
)
ProgressCallback = Callable[[int, "int | None"], None]


class DownloadError(RuntimeError):
    """Échec réseau ou fichier introuvable à la source."""


@dataclass(frozen=True)
class HeadphoneEntry:
    """Une ligne de l'index AutoEq (``results/INDEX.md``)."""

    name: str
    source: str
    rig: str
    path: str  # chemin relatif à results/, décodé (ex. "oratory1990/over-ear/Sennheiser HD 650")
    target: str = ""  # import d'une mesure brute : courbe cible AutoEq

    @property
    def is_import(self) -> bool:
        return self.path.startswith(IMPORT_PREFIX)

    @property
    def label(self) -> str:
        rig = f" · {self.rig}" if self.rig else ""
        return f"{self.name} — {self.source}{rig}"

    def to_dict(self) -> dict:
        return {"name": self.name, "source": self.source, "rig": self.rig, "path": self.path,
                "target": self.target}

    @classmethod
    def from_dict(cls, data: dict) -> "HeadphoneEntry":
        return cls(name=str(data["name"]), source=str(data.get("source", "")),
                   rig=str(data.get("rig", "")), path=str(data["path"]),
                   target=str(data.get("target", "") or ""))


def parse_autoeq_index(text: str) -> list[HeadphoneEntry]:
    """Analyse ``INDEX.md`` d'AutoEq.

    Format d'une ligne : ``- [NOM](./CHEMIN%20ENCODÉ) by SOURCE[ on BANC]``.
    Le nom et le chemin peuvent contenir des parenthèses, d'où l'analyse par
    séparateurs plutôt que par expression régulière gourmande.
    """
    entries: list[HeadphoneEntry] = []
    for raw in text.splitlines():
        line = raw.strip()
        if not line.startswith("- [") or "](./" not in line:
            continue
        name_part, _, rest = line[3:].partition("](./")
        path_enc, sep, by_part = rest.rpartition(") by ")
        if not sep:
            continue
        source, _, rig = by_part.partition(" on ")
        path = urllib.parse.unquote(path_enc)
        # Le "rig" de l'index est le banc de mesure ; on garde plutôt le dossier
        # (ex. "711 in-ear") qui distingue aussi intra/circum-auriculaire.
        parts = path.split("/")
        rig_dir = parts[1] if len(parts) >= 3 else rig
        entries.append(HeadphoneEntry(name=name_part, source=source.strip(),
                                      rig=rig_dir.strip(), path=path))
    return entries


def query_patterns(query: str) -> list["re.Pattern[str]"]:
    """Un motif par mot, ancré en début de mot : « 990 » ne trouve pas « 1990 »,
    mais « hd 65 » trouve « HD 650 »."""
    return [re.compile(r"(?<![0-9a-z])" + re.escape(w)) for w in query.lower().split()]


def matches(patterns: list["re.Pattern[str]"], haystack: str) -> bool:
    hay = haystack.lower()
    return all(p.search(hay) for p in patterns)


def search_headphones(entries: Iterable[HeadphoneEntry], query: str,
                      limit: int | None = None) -> list[HeadphoneEntry]:
    """Recherche insensible à la casse : chaque mot doit commencer un mot de l'entrée."""
    patterns = query_patterns(query)
    out = []
    for e in entries:
        if matches(patterns, f"{e.name} {e.source} {e.rig}"):
            out.append(e)
            if limit is not None and len(out) >= limit:
                break
    return out


def _quote_path(path: str) -> str:
    return urllib.parse.quote(path, safe="/()")


class Downloader:
    """Téléchargements atomiques avec cache et nouvelles tentatives."""

    def __init__(self, cache_dir: Path, session=None, retries: int = 3, timeout: float = 30.0):
        self.cache_dir = Path(cache_dir)
        self._session = session
        self.retries = max(1, retries)
        self.timeout = timeout

    # -- infrastructure -------------------------------------------------
    @property
    def session(self):
        if self._session is None:
            import requests  # import paresseux : inutile hors téléchargement

            self._session = requests.Session()
            self._session.headers["User-Agent"] = USER_AGENT
        return self._session

    @property
    def autoeq_dir(self) -> Path:
        return self.cache_dir / "autoeq" / AUTOEQ_COMMIT[:12]

    @property
    def spinorama_dir(self) -> Path:
        return self.cache_dir / "spinorama" / SPINORAMA_COMMIT[:12]

    def fetch(self, url: str, dest: Path, progress: ProgressCallback | None = None) -> Path:
        """Télécharge ``url`` vers ``dest`` sauf si le fichier existe déjà."""
        dest = Path(dest)
        if dest.is_file() and dest.stat().st_size > 0:
            return dest
        dest.parent.mkdir(parents=True, exist_ok=True)
        last_exc: Exception | None = None
        for attempt in range(self.retries):
            try:
                self._fetch_once(url, dest, progress)
                return dest
            except DownloadError:
                raise  # 404 & co : inutile de réessayer
            except Exception as exc:  # erreurs réseau transitoires
                last_exc = exc
                time.sleep(min(2 ** attempt, 5) * 0.5)
        raise DownloadError(f"Échec du téléchargement de {url} : {last_exc}") from last_exc

    def _fetch_once(self, url: str, dest: Path, progress: ProgressCallback | None) -> None:
        with self.session.get(url, stream=True, timeout=self.timeout) as resp:
            if resp.status_code == 404:
                raise DownloadError(f"Introuvable (404) : {url}")
            if resp.status_code >= 400:
                raise DownloadError(f"HTTP {resp.status_code} : {url}")
            total = resp.headers.get("Content-Length")
            total_i = int(total) if total and total.isdigit() else None
            fd, tmp = tempfile.mkstemp(dir=dest.parent, prefix=".part-")
            try:
                received = 0
                with os.fdopen(fd, "wb") as fh:
                    for chunk in resp.iter_content(chunk_size=65536):
                        if chunk:
                            fh.write(chunk)
                            received += len(chunk)
                            if progress:
                                progress(received, total_i)
                if received == 0:
                    raise DownloadError(f"Réponse vide : {url}")
                os.replace(tmp, dest)
            except BaseException:
                Path(tmp).unlink(missing_ok=True)
                raise

    # -- AutoEq ---------------------------------------------------------
    def autoeq_url(self, rel_path: str) -> str:
        return f"{AUTOEQ_RAW}/{AUTOEQ_COMMIT}/results/{_quote_path(rel_path)}"

    def ensure_autoeq_index(self, progress: ProgressCallback | None = None) -> Path:
        return self.fetch(self.autoeq_url("INDEX.md"), self.autoeq_dir / "INDEX.md", progress)

    def load_headphones(self, progress: ProgressCallback | None = None) -> list[HeadphoneEntry]:
        path = self.ensure_autoeq_index(progress)
        return parse_autoeq_index(path.read_text(encoding="utf-8"))

    def ensure_headphone_eq(self, entry: HeadphoneEntry,
                            progress: ProgressCallback | None = None) -> Path:
        """Renvoie le fichier d'EQ du casque (ParametricEQ, sinon GraphicEQ)."""
        leaf = entry.path.rstrip("/").split("/")[-1]
        errors = []
        for suffix in ("ParametricEQ.txt", "GraphicEQ.txt"):
            rel = f"{entry.path}/{leaf} {suffix}"
            try:
                return self.fetch(self.autoeq_url(rel), self.autoeq_dir / rel, progress)
            except DownloadError as exc:
                errors.append(str(exc))
        raise DownloadError("Aucune correction AutoEq pour « %s » :\n%s"
                            % (entry.name, "\n".join(errors)))

    def ensure_autoeq_target(self, name: str, progress: ProgressCallback | None = None) -> Path:
        """Courbe cible AutoEq (``targets/<nom>.csv``, commit figé)."""
        rel = f"targets/{name}.csv"
        url = f"{AUTOEQ_RAW}/{AUTOEQ_COMMIT}/{_quote_path(rel)}"
        return self.fetch(url, self.autoeq_dir / rel, progress)

    # -- Spinorama ------------------------------------------------------
    def spinorama_url(self, rel_path: str) -> str:
        return f"{SPINORAMA_RAW}/{SPINORAMA_COMMIT}/{_quote_path(rel_path)}"

    def ensure_speaker(self, speaker: dict, progress: ProgressCallback | None = None) -> Path:
        """Télécharge la mesure d'un haut-parleur du catalogue (format Klippel).

        ``CEA2034.txt`` → chemin du fichier ; mesures polaires seules
        (``format: polar``) → dossier contenant ``SPL Horizontal/Vertical.txt``,
        dont le CEA-2034 est calculé localement.
        """
        rel = speaker["path"]
        folder = self.spinorama_dir / speaker["id"]
        if speaker.get("format") == "polar":
            for name in ("SPL Horizontal.txt", "SPL Vertical.txt"):
                self.fetch(self.spinorama_url(f"{rel}/{name}"), folder / name, progress)
            return folder
        return self.fetch(self.spinorama_url(rel), folder / "CEA2034.txt", progress)

    # -- HRTF -----------------------------------------------------------
    def ensure_hrtf(self, preferred: str = "", progress: ProgressCallback | None = None) -> Path:
        """Chemin d'un fichier SOFA KEMAR : celui choisi, celui du système, ou téléchargé."""
        if preferred:
            p = Path(preferred).expanduser()
            if p.is_file():
                return p
        for candidate in SYSTEM_SOFA_PATHS:
            if Path(candidate).is_file():
                return Path(candidate)
        return self.fetch(KEMAR_URL, self.cache_dir / "hrtf" / "mit_kemar_normal_pinna.sofa", progress)

    def cache_size(self) -> int:
        if not self.cache_dir.exists():
            return 0
        return sum(f.stat().st_size for f in self.cache_dir.rglob("*") if f.is_file())
