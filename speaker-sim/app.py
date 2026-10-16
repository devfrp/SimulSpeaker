"""Classe principale : orchestre configuration, téléchargements, rendu et système.

``SpeakerSimApp`` ne dépend pas de Qt ; l'interface graphique (``ui_panel``) et
la ligne de commande (``main``) l'utilisent de la même façon. Les méthodes
longues (téléchargement, rendu, systemctl) sont bloquantes : l'interface les
exécute dans un thread de travail.
"""

from __future__ import annotations

import hashlib
import json
import logging
import logging.handlers
import re
import shutil
from dataclasses import replace
from pathlib import Path
from typing import Callable

import audio_engine as ae
from config import APP_NAME, IS_LINUX, ConfigStore, Paths, Settings, load_speaker_catalog
from download import (AUTOEQ_TARGETS, IMPORT_PREFIX, Downloader, HeadphoneEntry,
                      search_headphones)
from system import (SLOTS, PipeWire, Systemd, SystemError_, default_runner, is_sim_node,
                    missing_tools, node_name, other_slot, write_filter_config)

log = logging.getLogger("speaker_sim")
Progress = Callable[[float, str], None]


class AppError(RuntimeError):
    """Erreur présentable à l'utilisateur."""


def setup_logging(paths: Paths, verbose: bool = False) -> None:
    if log.handlers:
        return
    paths.ensure()
    handler = logging.handlers.RotatingFileHandler(paths.log_file, maxBytes=512_000,
                                                   backupCount=1, encoding="utf-8")
    handler.setFormatter(logging.Formatter("%(asctime)s %(levelname)s %(message)s"))
    log.addHandler(handler)
    if verbose:
        log.addHandler(logging.StreamHandler())
    log.setLevel(logging.DEBUG if verbose else logging.INFO)


class SpeakerSimApp:
    def __init__(self, paths: Paths | None = None, runner=default_runner,
                 downloader: Downloader | None = None, hrtf_factory=None):
        self.paths = paths or Paths.default()
        self.paths.ensure()
        self.store = ConfigStore(self.paths)
        self.settings: Settings = self.store.load()
        self.downloader = downloader or Downloader(self.paths.cache_dir)
        self.pw = PipeWire(runner)
        self.systemd = Systemd(self.paths, runner)
        self.speakers: list[dict] = load_speaker_catalog()
        self._headphones: list[HeadphoneEntry] | None = None
        self._hrtf_factory = hrtf_factory or ae.open_hrtf
        # Simulation en direct : PipeWire + systemd (Linux). Ailleurs : export des filtres.
        self.supports_live = IS_LINUX
        self.last_result: ae.RenderResult | None = None

    # -- catalogue ------------------------------------------------------
    def speaker(self, speaker_id: str | None = None) -> dict:
        sid = speaker_id or self.settings.speaker_id
        for spk in self.speakers:
            if spk["id"] == sid:
                return spk
        raise AppError(f"Haut-parleur inconnu : {sid}")

    def headphones(self, progress: Progress | None = None) -> list[HeadphoneEntry]:
        if self._headphones is None:
            cb = None
            if progress:
                cb = lambda got, total: progress(got / total if total else 0.0,
                                                 "Index AutoEq…")
            self._headphones = self.downloader.load_headphones(cb)
            log.info("Index AutoEq : %d mesures", len(self._headphones))
        return self._headphones

    def search_headphones(self, query: str, limit: int | None = 200) -> list[HeadphoneEntry]:
        return search_headphones(self.imported_headphones() + self.headphones(), query, limit)

    # -- mesures importées ------------------------------------------------
    @property
    def _imports_index(self) -> Path:
        return self.paths.imports_dir / "index.json"

    def imported_headphones(self) -> list[HeadphoneEntry]:
        try:
            data = json.loads(self._imports_index.read_text(encoding="utf-8"))
            return [HeadphoneEntry.from_dict(d) for d in data
                    if (self.paths.imports_dir / d["path"][len(IMPORT_PREFIX):]).is_file()]
        except (OSError, ValueError, KeyError, TypeError):
            return []

    def _save_imports(self, entries: list[HeadphoneEntry]) -> None:
        from config import atomic_write_json

        atomic_write_json(self._imports_index, [e.to_dict() for e in entries])

    def import_headphone(self, source: Path | str, name: str = "",
                         target: str = "") -> HeadphoneEntry:
        """Importe une mesure ou un EQ de casque, le valide, le copie et le sélectionne.

        * EQ paramétrique / graphique (AutoEq, Equalizer APO) : appliqué tel quel ;
        * mesure brute (fréquence, dB) : corrigée vers ``target`` (cible AutoEq).
        """
        src = Path(source).expanduser()
        try:
            text = src.read_text(encoding="utf-8", errors="replace")
        except OSError as exc:
            raise AppError(f"Lecture impossible : {exc}") from exc
        kind = ae.detect_headphone_file_kind(text)
        try:
            if kind == "measurement":
                ae.parse_measurement(text)
                target = target or AUTOEQ_TARGETS[0]
                if target not in AUTOEQ_TARGETS:
                    raise AppError(f"Cible inconnue : {target}")
                rig = f"mesure → {target}"
            else:
                eq = ae.parse_graphic_eq(text) if kind == "graphic" else ae.parse_parametric_eq(text)
                if kind == "parametric" and not eq.filters:
                    raise AppError("Aucun filtre « Filter N: ON … » reconnu dans ce fichier")
                target = ""
                rig = "EQ graphique" if kind == "graphic" else "EQ paramétrique"
        except ae.AudioEngineError as exc:
            raise AppError(str(exc)) from exc
        digest = hashlib.sha256(text.encode("utf-8", "replace")).hexdigest()[:12]
        safe = re.sub(r"[^A-Za-z0-9._-]+", "_", src.name)[:80] or "mesure.txt"
        self.paths.imports_dir.mkdir(parents=True, exist_ok=True)
        dest = self.paths.imports_dir / f"{digest}-{safe}"
        shutil.copyfile(src, dest)
        entry = HeadphoneEntry(name=(name or src.stem).strip(), source="Import", rig=rig,
                               path=IMPORT_PREFIX + dest.name, target=target)
        others = [e for e in self.imported_headphones() if e.path != entry.path]
        self._save_imports([entry] + others)
        self.select_headphone(entry)
        log.info("Casque importé : %s (%s)", entry.name, rig)
        return entry

    def remove_import(self, entry: HeadphoneEntry) -> None:
        remaining = [e for e in self.imported_headphones() if e.path != entry.path]
        self._save_imports(remaining)
        (self.paths.imports_dir / entry.path[len(IMPORT_PREFIX):]).unlink(missing_ok=True)
        if self.headphone and self.headphone.path == entry.path:
            self.select_headphone(None)

    def _headphone_eq(self, hp: HeadphoneEntry) -> ae.HeadphoneEq:
        if not hp.is_import:
            return ae.load_headphone_eq(self.downloader.ensure_headphone_eq(hp))
        path = self.paths.imports_dir / hp.path[len(IMPORT_PREFIX):]
        if not path.is_file():
            raise AppError(f"Fichier importé introuvable : {path}")
        if not hp.target:
            return ae.load_headphone_eq(path)
        text = path.read_text(encoding="utf-8", errors="replace")
        target_text = self.downloader.ensure_autoeq_target(hp.target).read_text(encoding="utf-8")
        return ae.measurement_eq(ae.parse_measurement(text), ae.parse_measurement(target_text))

    @property
    def headphone(self) -> HeadphoneEntry | None:
        hp = self.settings.headphone
        return HeadphoneEntry.from_dict(hp) if hp else None

    # -- paramètres -----------------------------------------------------
    def update_settings(self, **changes) -> Settings:
        self.settings = replace(self.settings, **changes).validated()
        self.store.save(self.settings)
        return self.settings

    def select_headphone(self, entry: HeadphoneEntry | None) -> None:
        self.update_settings(headphone=entry.to_dict() if entry else None)

    def select_speaker(self, speaker_id: str) -> None:
        self.speaker(speaker_id)  # valide
        self.update_settings(speaker_id=speaker_id)

    # -- rendu ----------------------------------------------------------
    def sample_rate(self) -> int:
        if self.settings.sample_rate:
            return self.settings.sample_rate
        if not self.supports_live:
            return 48000
        try:
            return self.pw.clock_rate()
        except SystemError_:
            return 48000

    def room_spec(self, settings: Settings | None = None) -> ae.RoomSpec:
        s = settings or self.settings
        return ae.RoomSpec(
            width=s.room_width, length=s.room_length, height=s.room_height,
            order=s.reflection_order, walls=s.room_walls, treatment=s.room_treatment,
            bass_traps=s.room_bass_traps, floor=s.room_floor, ceiling=s.room_ceiling,
            furniture=s.room_furniture, diffusion=s.room_diffusion, front_gap=s.front_gap,
            ear_height=s.ear_height, rt60_override=0.0 if s.rt60_auto else s.rt60,
            absorber=s.room_absorber, per_wall=s.treatment_per_wall,
            treatment_front=s.treatment_front, treatment_sides=s.treatment_sides,
            treatment_rear=s.treatment_rear, treatment_ceiling=s.treatment_ceiling,
            speaker_height=s.speaker_height, listener_offset=s.listener_offset)

    def layout(self) -> ae.Layout | None:
        """Positions auditeur / enceintes (vue en plan de l'interface)."""
        try:
            return ae.stereo_layout(self.room_spec(), self.settings.speaker_angle,
                                    self.settings.speaker_distance)
        except ae.AudioEngineError:
            return None

    def apply_room_preset(self, key: str) -> Settings:
        from config import preset_settings

        return self.update_settings(**preset_settings(key))

    def render_params(self, fs: int) -> ae.RenderParams:
        s = self.settings
        return ae.RenderParams(
            fs=fs, angle_deg=s.speaker_angle, distance=s.speaker_distance,
            room=self.room_spec(),
            room_enabled=s.room_enabled, room_amount=s.room_amount,
            diffuse_field_eq=s.diffuse_field_eq, ir_length_ms=s.ir_length_ms,
            preamp_db=s.preamp_db,
            balance_db=s.balance_db if s.tone_enabled else 0.0,
            bass_db=s.bass_db if s.tone_enabled else 0.0,
            treble_db=s.treble_db if s.tone_enabled else 0.0)

    def build_filters(self, progress: Progress | None = None) -> ae.RenderResult:
        """Télécharge ce qui manque, calcule et écrit les IR + la config filter-chain."""
        report = progress or (lambda _f, _m: None)
        s = self.settings
        spk = self.speaker()
        fs = self.sample_rate()

        report(0.02, f"Mesure CEA-2034 : {spk['brand']} {spk['model']}")
        speaker = ae.load_speaker(self.downloader.ensure_speaker(spk))

        hp_fir = None
        hp = self.headphone
        if hp and s.headphone_eq_enabled:
            report(0.06, f"Correction du casque : {hp.name}")
            hp_fir = ae.headphone_eq_fir(self._headphone_eq(hp), fs)

        report(0.1, "HRTF KEMAR")
        hrtf = self._hrtf_factory(self.downloader.ensure_hrtf(s.sofa_file), fs)
        try:
            result = ae.render(speaker, hp_fir, hrtf, self.render_params(fs),
                               lambda f, m: report(0.1 + 0.8 * f, m))
        finally:
            close = getattr(hrtf, "close", None)
            if close:
                close()

        report(0.92, "Écriture des filtres")
        # Les deux instances reçoivent les mêmes filtres : l'instance active a déjà
        # chargé les siens en mémoire, l'autre démarrera avec les nouveaux.
        target = self.target_sink() if self.supports_live else ""
        for slot in SLOTS:
            ir_paths = ae.write_irs(result, self.paths.slot_dir(slot))
            if self.supports_live:
                write_filter_config(self.paths, ir_paths, target, self.description(), slot)
        if not hp:
            result.notes.append("Aucun casque sélectionné : pas de correction AutoEq")
        self.last_result = result
        log.info("Filtres générés (%d Hz, gain %.1f dB) : %s / %s", fs, result.gain_db,
                 spk["id"], hp.name if hp else "-")
        report(1.0, "Filtres prêts")
        return result

    def export(self, dest: Path | str, progress: Progress | None = None) -> list[Path]:
        """Génère les filtres et les copie vers ``dest`` dans des formats portables :
        4 WAV mono, un WAV 4 canaux « true stereo » et une config Equalizer APO."""
        result = self.build_filters(progress)
        dest = Path(dest).expanduser()
        dest.mkdir(parents=True, exist_ok=True)
        ir_paths = ae.write_irs(result, dest)
        stereo = ae.write_true_stereo(result, dest / "speaker-sim-true-stereo.wav")
        apo = dest / "speaker-sim.txt"
        apo.write_text(ae.equalizer_apo_config(ir_paths), encoding="utf-8")
        info = dest / "LISEZ-MOI.txt"
        hp = self.headphone
        info.write_text(
            f"{self.description()} — casque : {hp.label if hp else 'aucune correction'}\n"
            f"{result.fs} Hz, gain {result.gain_db:+.1f} dB\n\n"
            "ir_XY.wav : enceinte X vers oreille Y (mono, float32).\n"
            "speaker-sim-true-stereo.wav : 4 canaux LL, LR, RL, RR.\n"
            "speaker-sim.txt : Equalizer APO ≥ 1.2 (Windows) ; ajoutez\n"
            "  « Include: speaker-sim.txt » dans config.txt après avoir copié ce dossier\n"
            "  dans C:\\Program Files\\EqualizerAPO\\config\\ (chemins absolus inclus).\n",
            encoding="utf-8")
        log.info("Filtres exportés vers %s", dest)
        return [*ir_paths.values(), stereo, apo, info]

    def description(self) -> str:
        spk = self.speaker()
        return f"{APP_NAME} ({spk['brand']} {spk['model']})"

    def target_sink(self) -> str:
        """Sortie physique (casque) vers laquelle le filtre joue."""
        try:
            sinks = {s.name for s in self.pw.sinks()}
            if self.settings.output_sink in sinks:
                return self.settings.output_sink
            for candidate in (self.store.load_runtime().get("previous_sink", ""),
                              self.pw.default_sink(),
                              self.pw.default_sink(configured_only=True)):
                if candidate in sinks and not is_sim_node(candidate):
                    return candidate
            return sorted(sinks)[0] if sinks else ""
        except SystemError_ as exc:
            log.warning("Sortie cible indéterminée : %s", exc)
            return self.settings.output_sink

    # -- cycle de vie ---------------------------------------------------
    def is_running(self) -> bool:
        if not self.supports_live:
            return False
        try:
            return self.systemd.is_active()
        except SystemError_:
            return False

    def start(self, progress: Progress | None = None) -> ae.RenderResult:
        if not self.supports_live:
            raise AppError("La simulation en direct nécessite Linux + PipeWire. "
                           "Utilisez « Exporter » pour un convolveur (Equalizer APO…).")
        missing = missing_tools()
        if missing:
            raise AppError("Outils manquants : " + ", ".join(missing))
        report = progress or (lambda *_: None)
        result = self.build_filters(progress)
        try:
            self._switch(report)
        except SystemError_ as exc:
            raise AppError(f"Démarrage impossible : {exc}\n{self._logs()}") from exc
        log.info("Simulation démarrée (%s)", self.description())
        return result

    def _switch(self, report: Progress) -> str:
        """Démarre l'instance inactive puis arrête l'ancienne, sans coupure.

        La nouvelle instance devient la sortie par défaut (hook) ; WirePlumber y
        déplace les flux qui suivent la sortie par défaut ; on attend qu'ils aient
        quitté l'ancienne instance avant de l'arrêter. La sortie virtuelle n'est donc
        jamais absente et les lecteurs ne se mettent pas en pause.
        """
        active = self.systemd.active_slots()
        legacy = self.systemd.legacy_active()
        new = other_slot(active[0]) if active else "a"
        report(0.95, "Bascule sans coupure" if active or legacy else "Démarrage du service")
        self.systemd.start(new)
        node = node_name(new)
        if self.pw.wait_for_sink(node) is None:
            self.systemd.stop(new)
            raise SystemError_("la nouvelle instance n'est pas apparue dans PipeWire")
        if self.pw.default_sink(configured_only=True) != node:
            self.pw.set_default_sink(node)
        old_nodes = [node_name(s) for s in active if s != new] + ([node_name("")] if legacy else [])
        for old in old_nodes:
            if not self.pw.wait_streams_left(old, timeout=3.0):
                log.warning("Des flux restent liés à %s (cible explicite) ; arrêt quand même", old)
        for slot in active:
            if slot != new:
                self.systemd.stop(slot)
        if legacy:
            self.systemd.stop_legacy()
            self.systemd.cleanup_legacy()
        if not self.systemd.active_slots():
            raise SystemError_("le service s'est arrêté aussitôt")
        return new

    apply = start  # reconstruire + (re)démarrer

    def stop(self) -> None:
        try:
            self.systemd.stop()
        except SystemError_ as exc:
            raise AppError(f"Arrêt impossible : {exc}") from exc
        log.info("Simulation arrêtée")

    def set_autostart(self, enabled: bool) -> None:
        if not self.supports_live:
            raise AppError("Auto-démarrage disponible uniquement sous Linux")
        try:
            if enabled and not self.paths.filter_conf.exists():
                self.build_filters()
            if enabled and "a" in self.systemd.active_slots() and not self.systemd.is_linked("a"):
                # L'instance A tourne en transitoire : basculer sur B (sans coupure)
                # libère son nom pour l'unité persistante.
                self._switch(lambda *_: None)
            self.systemd.set_autostart(enabled)
        except SystemError_ as exc:
            raise AppError(f"Auto-démarrage : {exc}") from exc
        self.update_settings(autostart=enabled)

    def status(self) -> dict:
        rt = self.store.load_runtime()
        spk = self.speaker()
        hp = self.headphone
        slots = self.systemd.active_slots() if self.supports_live else []
        default = ""
        if self.supports_live:
            try:
                default = self.pw.default_sink()
            except SystemError_:
                pass
        return {
            "running": self.is_running(),
            "autostart": self.supports_live and self.systemd.is_enabled(),
            "speaker": f"{spk['brand']} {spk['model']}",
            "headphone": hp.label if hp else None,
            "default_sink": default,
            "instance": slots[0].upper() if slots else None,
            "sim_is_default": is_sim_node(default),
            "previous_sink": rt.get("previous_sink"),
            "filters_ready": self.paths.filter_conf.exists(),
        }

    def _logs(self) -> str:
        try:
            return self.systemd.recent_logs(15)
        except SystemError_:
            return ""


def ir_files(paths: Paths, slot: str = "a") -> list[Path]:
    return sorted(paths.slot_dir(slot).glob("ir_*.wav"))
