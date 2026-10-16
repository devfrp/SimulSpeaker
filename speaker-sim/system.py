"""Intégration système : PipeWire (filter-chain), WirePlumber (sortie par défaut), systemd.

Principes :

* Tout s'exécute en tant qu'utilisateur ; aucune configuration de PipeWire,
  WirePlumber ou Omarchy n'est modifiée. Le filtre tourne dans un processus
  ``pipewire -c <filters/filter-chain.conf>`` séparé, lancé comme unité
  systemd **transitoire** (``systemd-run --user``).
* Deux instances alternées (A/B) : appliquer un réglage démarre l'autre
  instance, en fait la sortie par défaut, attend que les flux l'aient rejointe,
  puis arrête l'ancienne. La sortie virtuelle ne disparaît jamais : aucune
  application ne se met en pause.
* La seule action sur WirePlumber est le choix de la sortie par défaut
  (``wpctl set-default``), exactement comme le ferait le sélecteur de volume ;
  la sortie précédente est mémorisée dans ``runtime.json`` et restaurée à l'arrêt
  (hook ``ExecStopPost``, donc aussi si le service est arrêté par ailleurs).
* Auto-démarrage (option) : le fichier d'unité est écrit dans le répertoire
  d'état puis lié par ``systemctl --user link`` + ``enable`` ; systemd ne crée
  que des liens symboliques, supprimés par ``disable``.
"""

from __future__ import annotations

import json
import shlex
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Sequence

from config import APP_DIR, APP_NAME, ConfigStore, Paths

SLOTS = ("a", "b")
# Unité/nœud uniques de la version 1.0.x, encore gérés pour la migration.
UNIT_NAME = "devfrp-speaker-sim-py.service"
NODE_NAME = "devfrp_speaker_sim_py"


def unit_name(slot: str) -> str:
    return f"devfrp-speaker-sim-py-{slot}.service" if slot in SLOTS else UNIT_NAME


def node_name(slot: str) -> str:
    return f"{NODE_NAME}_{slot}" if slot in SLOTS else NODE_NAME


def other_slot(slot: str) -> str:
    return "b" if slot == "a" else "a"

# Préfixe commun aux nœuds de simulation (celui-ci et le greffon Omarchy) :
# ils ne doivent jamais servir de sortie « casque ».
SIM_NODE_PREFIX = "devfrp_speaker_sim"
MAIN_SCRIPT = APP_DIR / "main.py"


class SystemError_(RuntimeError):
    """Commande système en échec (nom évitant de masquer ``SystemError``)."""


@dataclass(frozen=True)
class Sink:
    id: int
    name: str
    description: str

    @property
    def label(self) -> str:
        return self.description or self.name


Runner = Callable[[Sequence[str]], subprocess.CompletedProcess]


def default_runner(cmd: Sequence[str], timeout: float = 15.0) -> subprocess.CompletedProcess:
    try:
        return subprocess.run(list(cmd), capture_output=True, text=True, timeout=timeout)
    except FileNotFoundError as exc:
        raise SystemError_(f"Commande introuvable : {cmd[0]}") from exc
    except subprocess.TimeoutExpired as exc:
        raise SystemError_(f"Délai dépassé : {' '.join(cmd)}") from exc


def missing_tools() -> list[str]:
    tools = ("pipewire", "pw-dump", "wpctl", "systemctl", "systemd-run")
    return [t for t in tools if shutil.which(t) is None]


# ---------------------------------------------------------------------------
# PipeWire : lecture du graphe
# ---------------------------------------------------------------------------
def parse_sinks(dump: list) -> list[Sink]:
    sinks = []
    for obj in dump:
        if obj.get("type") != "PipeWire:Interface:Node":
            continue
        props = (obj.get("info") or {}).get("props") or {}
        if props.get("media.class") != "Audio/Sink":
            continue
        sinks.append(Sink(id=int(obj["id"]), name=str(props.get("node.name", "")),
                          description=str(props.get("node.description")
                                          or props.get("node.nick") or "")))
    return sinks


def parse_default_sink(dump: list, configured_only: bool = False) -> str:
    """Nom de la sortie par défaut (métadonnée ``default``).

    Par défaut : la sortie effective. ``configured_only`` : le choix de
    l'utilisateur, qui persiste même quand le nœud correspondant a disparu.
    """
    configured = ""
    for obj in dump:
        if obj.get("type") != "PipeWire:Interface:Metadata":
            continue
        if (obj.get("props") or {}).get("metadata.name") != "default":
            continue
        for entry in obj.get("metadata") or []:
            value = entry.get("value")
            if isinstance(value, str):
                try:
                    value = json.loads(value)
                except ValueError:
                    value = {"name": value}
            name = value.get("name", "") if isinstance(value, dict) else ""
            if entry.get("key") == "default.audio.sink" and not configured_only:
                return name
            if entry.get("key") == "default.configured.audio.sink":
                configured = name
    return configured


def is_sim_node(name: str) -> bool:
    return name.startswith(SIM_NODE_PREFIX)


class PipeWire:
    def __init__(self, runner: Runner = default_runner):
        self.run = runner

    def dump(self) -> list:
        res = self.run(["pw-dump"])
        if res.returncode != 0:
            raise SystemError_(f"pw-dump a échoué : {res.stderr.strip()}")
        try:
            return json.loads(res.stdout or "[]")
        except ValueError as exc:
            raise SystemError_("Sortie de pw-dump illisible") from exc

    def sinks(self, include_sim: bool = False) -> list[Sink]:
        sinks = parse_sinks(self.dump())
        return sinks if include_sim else [s for s in sinks if not is_sim_node(s.name)]

    def default_sink(self, configured_only: bool = False) -> str:
        return parse_default_sink(self.dump(), configured_only)

    def find_sink(self, name: str) -> Sink | None:
        return next((s for s in parse_sinks(self.dump()) if s.name == name), None)

    def set_default_sink(self, name: str) -> None:
        sink = self.find_sink(name)
        if sink is not None:
            res = self.run(["wpctl", "set-default", str(sink.id)])
            if res.returncode == 0:
                return
        if shutil.which("pactl"):
            res = self.run(["pactl", "set-default-sink", name])
            if res.returncode == 0:
                return
        raise SystemError_(f"Impossible de définir « {name} » comme sortie par défaut")

    def wait_for_sink(self, name: str, timeout: float = 8.0) -> Sink | None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            sink = self.find_sink(name)
            if sink is not None:
                return sink
            time.sleep(0.2)
        return None

    def input_links(self, name: str) -> int:
        """Nombre de liens entrant dans le nœud ``name`` (flux qui y jouent)."""
        dump = self.dump()
        ids = {s.id for s in parse_sinks(dump) if s.name == name}
        if not ids:
            return 0
        return sum(1 for o in dump if o.get("type") == "PipeWire:Interface:Link"
                   and ((o.get("info") or {}).get("input-node-id")) in ids)

    def wait_streams_left(self, name: str, timeout: float = 3.0) -> bool:
        """Attend que plus aucun flux ne joue vers ``name`` (déplacés par WirePlumber)."""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.input_links(name) == 0:
                return True
            time.sleep(0.1)
        return False

    def clock_rate(self, fallback: int = 48000) -> int:
        try:
            res = self.run(["pw-metadata", "-n", "settings", "0", "clock.rate"])
        except SystemError_:
            return fallback
        for line in res.stdout.splitlines():
            if "clock.rate" in line and "value:'" in line:
                value = line.split("value:'", 1)[1].split("'", 1)[0]
                if value.isdigit() and int(value) >= 8000:
                    return int(value)
        return fallback


# ---------------------------------------------------------------------------
# Configuration filter-chain
# ---------------------------------------------------------------------------
def filter_chain_config(ir_paths: dict[str, Path], target_sink: str, description: str,
                        slot: str = "a") -> dict:
    """Configuration (JSON valide, accepté par PipeWire) d'un processus filter-chain.

    Graphe : copie de chaque canal d'entrée → 4 convolveurs (haut-parleur→oreille)
    → deux mélangeurs (oreille gauche/droite).
    """
    node = node_name(slot)
    nodes = [{"type": "builtin", "label": "copy", "name": "inL"},
             {"type": "builtin", "label": "copy", "name": "inR"}]
    for key in ("LL", "LR", "RL", "RR"):
        nodes.append({"type": "builtin", "label": "convolver", "name": f"conv{key}",
                      "config": {"filename": str(ir_paths[key])}})
    nodes += [{"type": "builtin", "label": "mixer", "name": "mixL"},
              {"type": "builtin", "label": "mixer", "name": "mixR"}]
    links = [
        {"output": "inL:Out", "input": "convLL:In"},
        {"output": "inL:Out", "input": "convLR:In"},
        {"output": "inR:Out", "input": "convRL:In"},
        {"output": "inR:Out", "input": "convRR:In"},
        {"output": "convLL:Out", "input": "mixL:In 1"},
        {"output": "convRL:Out", "input": "mixL:In 2"},
        {"output": "convLR:Out", "input": "mixR:In 1"},
        {"output": "convRR:Out", "input": "mixR:In 2"},
    ]
    playback = {"node.name": f"{node}.output", "node.passive": True,
                "audio.channels": 2, "audio.position": ["FL", "FR"],
                "stream.dont-remix": True}
    if target_sink:
        playback["target.object"] = target_sink
    return {
        "context.properties": {"log.level": 2},
        "context.spa-libs": {"audio.convert.*": "audioconvert/libspa-audioconvert",
                             "support.*": "support/libspa-support"},
        "context.modules": [
            {"name": "libpipewire-module-rt", "flags": ["ifexists", "nofail"]},
            {"name": "libpipewire-module-protocol-native"},
            {"name": "libpipewire-module-client-node"},
            {"name": "libpipewire-module-adapter"},
            {"name": "libpipewire-module-filter-chain", "args": {
                "node.description": description,
                "media.name": description,
                "filter.graph": {"nodes": nodes, "links": links,
                                 "inputs": ["inL:In", "inR:In"],
                                 "outputs": ["mixL:Out", "mixR:Out"]},
                "capture.props": {"node.name": node, "media.class": "Audio/Sink",
                                  "audio.channels": 2, "audio.position": ["FL", "FR"]},
                "playback.props": playback,
            }},
        ],
    }


def write_filter_config(paths: Paths, ir_paths: dict[str, Path], target_sink: str,
                        description: str, slot: str = "a") -> Path:
    from config import atomic_write_json

    conf = paths.slot_conf(slot)
    atomic_write_json(conf, filter_chain_config(ir_paths, target_sink, description, slot))
    return conf


# ---------------------------------------------------------------------------
# systemd
# ---------------------------------------------------------------------------
def _systemd_quote(arg: str) -> str:
    """Argument pour une ligne Exec* de systemd (guillemets doubles, ``%`` et ``$`` échappés)."""
    arg = arg.replace("%", "%%").replace("$", "$$")
    if arg and all(c.isalnum() or c in "/._-+=:," for c in arg):
        return arg
    return '"' + arg.replace("\\", "\\\\").replace('"', '\\"') + '"'


def exec_line(args: Sequence[str]) -> str:
    return " ".join(_systemd_quote(a) for a in args)


def hook_command(hook: str, slot: str = "a") -> list[str]:
    return [sys.executable, str(MAIN_SCRIPT), hook, slot]


def unit_file_content(paths: Paths, slot: str = "a") -> str:
    pipewire = shutil.which("pipewire") or "/usr/bin/pipewire"
    return f"""[Unit]
Description={APP_NAME} – simulation de haut-parleurs au casque (devfrp, instance {slot.upper()})
After=pipewire.service wireplumber.service
Wants=wireplumber.service
PartOf=pipewire.service

[Service]
Type=simple
Environment=XDG_STATE_HOME={_systemd_quote(str(paths.state_dir.parent))}
Environment=XDG_CACHE_HOME={_systemd_quote(str(paths.cache_dir.parent))}
ExecStart={exec_line([pipewire, '-c', str(paths.slot_conf(slot))])}
ExecStartPost=-{exec_line(hook_command('--hook-started', slot))}
ExecStopPost=-{exec_line(hook_command('--hook-stopped', slot))}
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
"""


class Systemd:
    """Deux unités alternées (A/B). Seule l'instance A peut être liée (auto-démarrage)."""

    AUTOSTART_SLOT = "a"

    def __init__(self, paths: Paths, runner: Runner = default_runner):
        self.paths = paths
        self.run = runner

    def unit_path(self, slot: str = "a") -> Path:
        return self.paths.state_dir / unit_name(slot)

    def _ctl(self, *args: str, check: bool = True) -> subprocess.CompletedProcess:
        res = self.run(["systemctl", "--user", *args])
        if check and res.returncode != 0:
            raise SystemError_(f"systemctl --user {' '.join(args)} : "
                               f"{(res.stderr or res.stdout).strip()}")
        return res

    def _unit_active(self, unit: str) -> bool:
        return self._ctl("is-active", "--quiet", unit, check=False).returncode == 0

    def active_slots(self) -> list[str]:
        return [s for s in SLOTS if self._unit_active(unit_name(s))]

    def legacy_active(self) -> bool:
        return self._unit_active(UNIT_NAME)

    def is_active(self) -> bool:
        return bool(self.active_slots()) or self.legacy_active()

    def is_enabled(self) -> bool:
        res = self._ctl("is-enabled", unit_name(self.AUTOSTART_SLOT), check=False)
        return res.returncode == 0 and res.stdout.strip() in ("enabled", "linked", "enabled-runtime")

    def is_linked(self, slot: str = "a") -> bool:
        res = self._ctl("show", "-p", "FragmentPath", "--value", unit_name(slot), check=False)
        return res.stdout.strip() == str(self.unit_path(slot))

    def write_unit(self, slot: str = "a") -> Path:
        self.paths.ensure()
        self.unit_path(slot).write_text(unit_file_content(self.paths, slot), encoding="utf-8")
        return self.unit_path(slot)

    def start(self, slot: str = "a") -> None:
        unit = unit_name(slot)
        if self._unit_active(unit):
            self._ctl("restart", unit)
            return
        self._ctl("reset-failed", unit, check=False)
        if self.is_linked(slot):
            self.write_unit(slot)
            self._ctl("daemon-reload")
            self._ctl("start", unit)
            return
        pipewire = shutil.which("pipewire") or "/usr/bin/pipewire"
        env = [f"--setenv=XDG_STATE_HOME={self.paths.state_dir.parent}",
               f"--setenv=XDG_CACHE_HOME={self.paths.cache_dir.parent}"]
        cmd = ["systemd-run", "--user", f"--unit={unit}", "--collect",
               f"--description={APP_NAME} (devfrp, instance {slot.upper()})", *env,
               "-p", "After=pipewire.service wireplumber.service",
               "-p", "Restart=on-failure",
               "-p", f"ExecStartPost=-{exec_line(hook_command('--hook-started', slot))}",
               "-p", f"ExecStopPost=-{exec_line(hook_command('--hook-stopped', slot))}",
               pipewire, "-c", str(self.paths.slot_conf(slot))]
        res = self.run(cmd)
        if res.returncode != 0:
            raise SystemError_(f"systemd-run a échoué : {(res.stderr or res.stdout).strip()}")

    def stop(self, slot: str | None = None) -> None:
        """Arrête une instance, ou toutes (y compris l'unité unique de la v1.0)."""
        units = [unit_name(slot)] if slot else [unit_name(s) for s in SLOTS] + [UNIT_NAME]
        for unit in units:
            if self._unit_active(unit):
                self._ctl("stop", unit)

    def stop_legacy(self) -> None:
        if self.legacy_active():
            self._ctl("stop", UNIT_NAME)

    def cleanup_legacy(self) -> None:
        """Retire l'ancienne unité liée de la v1.0 (remplacée par l'instance A)."""
        res = self._ctl("show", "-p", "FragmentPath", "--value", UNIT_NAME, check=False)
        if res.stdout.strip() == str(self.paths.state_dir / UNIT_NAME):
            self._ctl("disable", UNIT_NAME, check=False)
            self._ctl("daemon-reload", check=False)

    def set_autostart(self, enabled: bool) -> None:
        slot = self.AUTOSTART_SLOT
        unit = unit_name(slot)
        self.cleanup_legacy()
        if enabled:
            if self._unit_active(unit) and not self.is_linked(slot):
                raise SystemError_("l'instance A transitoire est active ; basculez d'abord sur B")
            self.write_unit(slot)
            if not self.is_linked(slot):
                self._ctl("link", str(self.unit_path(slot)))
            self._ctl("daemon-reload")
            self._ctl("enable", unit)
        else:
            if self.is_linked(slot) or self.is_enabled():
                self._ctl("disable", unit, check=False)
                self._ctl("daemon-reload", check=False)

    def recent_logs(self, lines: int = 30) -> str:
        cmd = ["journalctl", "--user", "-n", str(lines), "--no-pager", "-o", "cat"]
        for unit in [unit_name(s) for s in SLOTS] + [UNIT_NAME]:
            cmd += ["-u", unit]
        return self.run(cmd).stdout


# ---------------------------------------------------------------------------
# Hooks ExecStartPost / ExecStopPost : bascule et restauration de la sortie
# ---------------------------------------------------------------------------
def hook_started(store: ConfigStore, pw: PipeWire, slot: str = "a", log=print) -> int:
    """Après démarrage : mémorise la sortie actuelle puis bascule sur cette instance."""
    node = node_name(slot)
    rt = store.load_runtime()
    sink = pw.wait_for_sink(node)
    if sink is None:
        log("Nœud du simulateur introuvable : sortie par défaut inchangée")
        return 0
    current = pw.default_sink()
    # Basculement A↔B : la sortie courante est l'autre instance, on garde la mémoire.
    if current and not is_sim_node(current):
        rt["previous_sink"] = current
    rt["started_at"] = time.time()
    rt["active_slot"] = slot
    rt.pop("restarting", None)
    store.save_runtime(rt)
    try:
        pw.set_default_sink(node)
    except SystemError_ as exc:
        log(str(exc))
    return 0


def hook_stopped(store: ConfigStore, pw: PipeWire, slot: str = "a", log=print) -> int:
    """Après arrêt : restaure la sortie précédente si cette instance était la sortie choisie.

    Lors d'un basculement A↔B, la sortie choisie est déjà l'autre instance : rien à faire.
    """
    node = node_name(slot)
    rt = store.load_runtime()
    if rt.get("restarting"):
        return 0
    previous = rt.get("previous_sink", "")
    try:
        # WirePlumber a déjà basculé la sortie effective sur un repli ; seul le
        # choix configuré indique si l'utilisateur écoutait cette instance.
        configured = pw.default_sink(configured_only=True)
        if previous and (not configured or configured == node):
            if pw.find_sink(previous) is not None:
                pw.set_default_sink(previous)
            else:
                log(f"Sortie précédente « {previous} » absente : WirePlumber choisira")
    except SystemError_ as exc:
        log(str(exc))
    if rt.get("active_slot") in (slot, None):
        rt.pop("started_at", None)
        rt.pop("active_slot", None)
    store.save_runtime(rt)
    return 0


def describe_command(cmd: Sequence[str]) -> str:
    return " ".join(shlex.quote(c) for c in cmd)
