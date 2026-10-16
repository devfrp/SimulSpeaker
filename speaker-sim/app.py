"""Classe principale : orchestre configuration, téléchargements, rendu et système.

``SpeakerSimApp`` ne dépend pas de Qt ; l'interface graphique (``ui_panel``) et
la ligne de commande (``main``) l'utilisent de la même façon. Les méthodes
longues (téléchargement, rendu, systemctl) sont bloquantes : l'interface les
exécute dans un thread de travail.
"""

from __future__ import annotations

import logging
import logging.handlers
from dataclasses import replace
from pathlib import Path
from typing import Callable

import audio_engine as ae
from config import APP_NAME, IS_LINUX, ConfigStore, Paths, Settings, load_speaker_catalog
from download import Downloader, HeadphoneEntry, search_headphones
from system import (NODE_NAME, PipeWire, Systemd, SystemError_, default_runner,
                    is_sim_node, missing_tools, write_filter_config)

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
        return search_headphones(self.headphones(), query, limit)

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

    def render_params(self, fs: int) -> ae.RenderParams:
        s = self.settings
        return ae.RenderParams(
            fs=fs, angle_deg=s.speaker_angle, distance=s.speaker_distance,
            room=ae.RoomSpec(width=s.room_width, length=s.room_length, height=s.room_height,
                             rt60=s.rt60, order=s.reflection_order),
            room_enabled=s.room_enabled, room_amount=s.room_amount,
            diffuse_field_eq=s.diffuse_field_eq, ir_length_ms=s.ir_length_ms,
            preamp_db=s.preamp_db)

    def build_filters(self, progress: Progress | None = None) -> ae.RenderResult:
        """Télécharge ce qui manque, calcule et écrit les IR + la config filter-chain."""
        report = progress or (lambda _f, _m: None)
        s = self.settings
        spk = self.speaker()
        fs = self.sample_rate()

        report(0.02, f"Mesure CEA-2034 : {spk['brand']} {spk['model']}")
        speaker = ae.SpeakerResponse.load(self.downloader.ensure_speaker(spk))

        hp_fir = None
        hp = self.headphone
        if hp and s.headphone_eq_enabled:
            report(0.06, f"Correction AutoEq : {hp.name}")
            eq = ae.load_headphone_eq(self.downloader.ensure_headphone_eq(hp))
            hp_fir = ae.headphone_eq_fir(eq, fs)

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
        ir_paths = ae.write_irs(result, self.paths.filters_dir)
        if self.supports_live:
            write_filter_config(self.paths, ir_paths, self.target_sink(), self.description())
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
        result = self.build_filters(progress)
        running = self.is_running()
        if running:
            self.store.update_runtime(restarting=True)
        try:
            (progress or (lambda *_: None))(0.96, "Démarrage du service")
            self.systemd.start()
        except SystemError_ as exc:
            self.store.update_runtime(restarting=None)
            raise AppError(f"Démarrage impossible : {exc}\n{self._logs()}") from exc
        if running:
            self.store.update_runtime(restarting=None)
        if not self.is_running():
            raise AppError("Le service s'est arrêté aussitôt.\n" + self._logs())
        log.info("Simulation démarrée (%s)", self.description())
        return result

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
            self.systemd.set_autostart(enabled)
        except SystemError_ as exc:
            raise AppError(f"Auto-démarrage : {exc}") from exc
        self.update_settings(autostart=enabled)

    def status(self) -> dict:
        rt = self.store.load_runtime()
        spk = self.speaker()
        hp = self.headphone
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
            "sim_is_default": default == NODE_NAME,
            "previous_sink": rt.get("previous_sink"),
            "filters_ready": self.paths.filter_conf.exists(),
        }

    def _logs(self) -> str:
        try:
            return self.systemd.recent_logs(15)
        except SystemError_:
            return ""


def ir_files(paths: Paths) -> list[Path]:
    return sorted(paths.filters_dir.glob("ir_*.wav"))
