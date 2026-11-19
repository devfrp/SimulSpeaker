//! Intégration système : PipeWire (filter-chain), WirePlumber (sortie par défaut), systemd.
//!
//! Principes :
//!
//! * Tout s'exécute en tant qu'utilisateur ; aucune configuration de PipeWire,
//!   WirePlumber ou Omarchy n'est modifiée. Le filtre tourne dans un processus
//!   `pipewire -c <filters/filter-chain.conf>` séparé, lancé comme unité
//!   systemd **transitoire** (`systemd-run --user`).
//! * Deux instances alternées (A/B) : appliquer un réglage démarre l'autre
//!   instance, en fait la sortie par défaut, attend que les flux l'aient rejointe,
//!   puis arrête l'ancienne. La sortie virtuelle ne disparaît jamais : aucune
//!   application ne se met en pause.
//! * La seule action sur WirePlumber est le choix de la sortie par défaut
//!   (`wpctl set-default`), exactement comme le ferait le sélecteur de volume ;
//!   la sortie précédente est mémorisée dans `runtime.json` et restaurée à l'arrêt
//!   (hook `ExecStopPost`, donc aussi si le service est arrêté par ailleurs).
//! * Auto-démarrage (option) : le fichier d'unité est écrit dans le répertoire
//!   d'état puis lié par `systemctl --user link` + `enable` ; systemd ne crée
//!   que des liens symboliques, supprimés par `disable`.

use std::collections::BTreeMap;
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::config::{APP_NAME, ConfigStore, Paths, atomic_write_json};

pub const SLOTS: [&str; 2] = ["a", "b"];
/// Unité/nœud uniques de la version 1.0.x, encore gérés pour la migration.
pub const UNIT_NAME: &str = "devfrp-speaker-sim-py.service";
pub const NODE_NAME: &str = "devfrp_speaker_sim_py";
/// Préfixe commun aux nœuds de simulation (celui-ci et le greffon Omarchy) :
/// ils ne doivent jamais servir de sortie « casque ».
pub const SIM_NODE_PREFIX: &str = "devfrp_speaker_sim";

pub fn unit_name(slot: &str) -> String {
    if SLOTS.contains(&slot) { format!("devfrp-speaker-sim-py-{slot}.service") } else { UNIT_NAME.into() }
}

pub fn node_name(slot: &str) -> String {
    if SLOTS.contains(&slot) { format!("{NODE_NAME}_{slot}") } else { NODE_NAME.into() }
}

pub fn other_slot(slot: &str) -> &'static str {
    if slot == "a" { "b" } else { "a" }
}

pub fn is_sim_node(name: &str) -> bool {
    name.starts_with(SIM_NODE_PREFIX)
}

/// Commande système en échec.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemError(pub String);

impl fmt::Display for SystemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SystemError {}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CmdOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Exécuteur de commandes (remplaçable dans les tests).
pub type Runner = Arc<dyn Fn(&[String]) -> Result<CmdOutput, SystemError> + Send + Sync>;

/// Chemin d'un exécutable du `PATH`.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT").unwrap_or(".EXE;.CMD;.BAT".into()).split(';').map(str::to_string).collect()
    } else {
        vec![String::new()]
    };
    std::env::split_paths(&path).find_map(|dir| {
        exts.iter().map(|ext| dir.join(format!("{name}{ext}"))).find(|p| p.is_file() && is_executable(p))
    })
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata().is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(_p: &Path) -> bool {
    true
}

/// Exécute `cmd` (15 s maximum) et capture ses sorties.
pub fn run_command(cmd: &[String], timeout: Duration) -> Result<CmdOutput, SystemError> {
    let mut child = Command::new(&cmd[0])
        .args(&cmd[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => SystemError(format!("Commande introuvable : {}", cmd[0])),
            _ => SystemError(format!("{} : {e}", cmd[0])),
        })?;
    let mut out = child.stdout.take().expect("stdout");
    let mut err = child.stderr.take().expect("stderr");
    let t_out = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = out.read_to_end(&mut s);
        s
    });
    let t_err = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = err.read_to_end(&mut s);
        s
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(SystemError(format!("Délai dépassé : {}", cmd.join(" "))));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => return Err(SystemError(format!("{} : {e}", cmd[0]))),
        }
    };
    let text = |v: Vec<u8>| String::from_utf8_lossy(&v).into_owned();
    Ok(CmdOutput {
        code: status.code().unwrap_or(-1),
        stdout: text(t_out.join().unwrap_or_default()),
        stderr: text(t_err.join().unwrap_or_default()),
    })
}

pub fn default_runner() -> Runner {
    Arc::new(|cmd: &[String]| run_command(cmd, Duration::from_secs(15)))
}

pub fn missing_tools() -> Vec<&'static str> {
    ["pipewire", "pw-dump", "wpctl", "systemctl", "systemd-run"].into_iter().filter(|t| which(t).is_none()).collect()
}

pub fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

// ---------------------------------------------------------------------------
// PipeWire : lecture du graphe
// ---------------------------------------------------------------------------
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sink {
    pub id: i64,
    pub name: String,
    pub description: String,
}

impl Sink {
    pub fn label(&self) -> &str {
        if self.description.is_empty() { &self.name } else { &self.description }
    }
}

fn props(obj: &Value) -> Option<&serde_json::Map<String, Value>> {
    obj.get("info")?.get("props")?.as_object()
}

fn text(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

pub fn parse_sinks(dump: &[Value]) -> Vec<Sink> {
    dump.iter()
        .filter(|o| o.get("type").and_then(Value::as_str) == Some("PipeWire:Interface:Node"))
        .filter_map(|o| {
            let p = props(o)?;
            if p.get("media.class").and_then(Value::as_str) != Some("Audio/Sink") {
                return None;
            }
            let description = [p.get("node.description"), p.get("node.nick")]
                .into_iter()
                .map(text)
                .find(|s| !s.is_empty())
                .unwrap_or_default();
            Some(Sink { id: o.get("id")?.as_i64()?, name: text(p.get("node.name")), description })
        })
        .collect()
}

/// Nom de la sortie par défaut (métadonnée `default`).
///
/// Par défaut : la sortie effective. `configured_only` : le choix de
/// l'utilisateur, qui persiste même quand le nœud correspondant a disparu.
pub fn parse_default_sink(dump: &[Value], configured_only: bool) -> String {
    let mut configured = String::new();
    for obj in dump {
        if obj.get("type").and_then(Value::as_str) != Some("PipeWire:Interface:Metadata") {
            continue;
        }
        if obj.get("props").and_then(|p| p.get("metadata.name")).and_then(Value::as_str) != Some("default") {
            continue;
        }
        for entry in obj.get("metadata").and_then(Value::as_array).into_iter().flatten() {
            let value = match entry.get("value") {
                Some(Value::String(s)) => serde_json::from_str::<Value>(s).unwrap_or_else(|_| json!({"name": s})),
                Some(v) => v.clone(),
                None => Value::Null,
            };
            let name = value.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
            let key = entry.get("key").and_then(Value::as_str);
            if key == Some("default.audio.sink") && !configured_only {
                return name;
            }
            if key == Some("default.configured.audio.sink") {
                configured = name;
            }
        }
    }
    configured
}

#[derive(Clone)]
pub struct PipeWire {
    pub run: Runner,
}

impl PipeWire {
    pub fn new(run: Runner) -> Self {
        PipeWire { run }
    }

    pub fn dump(&self) -> Result<Vec<Value>, SystemError> {
        let res = (self.run)(&args(&["pw-dump"]))?;
        if res.code != 0 {
            return Err(SystemError(format!("pw-dump a échoué : {}", res.stderr.trim())));
        }
        let out = if res.stdout.trim().is_empty() { "[]" } else { &res.stdout };
        serde_json::from_str(out).map_err(|_| SystemError("Sortie de pw-dump illisible".into()))
    }

    pub fn sinks(&self, include_sim: bool) -> Result<Vec<Sink>, SystemError> {
        let sinks = parse_sinks(&self.dump()?);
        Ok(if include_sim { sinks } else { sinks.into_iter().filter(|s| !is_sim_node(&s.name)).collect() })
    }

    pub fn default_sink(&self, configured_only: bool) -> Result<String, SystemError> {
        Ok(parse_default_sink(&self.dump()?, configured_only))
    }

    pub fn find_sink(&self, name: &str) -> Result<Option<Sink>, SystemError> {
        Ok(parse_sinks(&self.dump()?).into_iter().find(|s| s.name == name))
    }

    pub fn set_default_sink(&self, name: &str) -> Result<(), SystemError> {
        if let Some(sink) = self.find_sink(name)? {
            if (self.run)(&args(&["wpctl", "set-default", &sink.id.to_string()]))?.code == 0 {
                return Ok(());
            }
        }
        if which("pactl").is_some() && (self.run)(&args(&["pactl", "set-default-sink", name]))?.code == 0 {
            return Ok(());
        }
        Err(SystemError(format!("Impossible de définir « {name} » comme sortie par défaut")))
    }

    pub fn wait_for_sink(&self, name: &str, timeout: Duration) -> Result<Option<Sink>, SystemError> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(sink) = self.find_sink(name)? {
                return Ok(Some(sink));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        Ok(None)
    }

    /// Nombre de liens entrant dans le nœud `name` (flux qui y jouent).
    pub fn input_links(&self, name: &str) -> Result<usize, SystemError> {
        let dump = self.dump()?;
        let ids: Vec<i64> = parse_sinks(&dump).into_iter().filter(|s| s.name == name).map(|s| s.id).collect();
        if ids.is_empty() {
            return Ok(0);
        }
        Ok(dump
            .iter()
            .filter(|o| o.get("type").and_then(Value::as_str) == Some("PipeWire:Interface:Link"))
            .filter(|o| {
                o.get("info").and_then(|i| i.get("input-node-id")).and_then(Value::as_i64).is_some_and(|id| ids.contains(&id))
            })
            .count())
    }

    /// Attend que plus aucun flux ne joue vers `name` (déplacés par WirePlumber).
    pub fn wait_streams_left(&self, name: &str, timeout: Duration) -> Result<bool, SystemError> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.input_links(name)? == 0 {
                return Ok(true);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(false)
    }

    pub fn clock_rate(&self, fallback: u32) -> u32 {
        let Ok(res) = (self.run)(&args(&["pw-metadata", "-n", "settings", "0", "clock.rate"])) else {
            return fallback;
        };
        for line in res.stdout.lines() {
            if line.contains("clock.rate") {
                if let Some((_, rest)) = line.split_once("value:'") {
                    let value = rest.split('\'').next().unwrap_or_default();
                    if let Ok(v) = value.parse::<u32>() {
                        if v >= 8000 {
                            return v;
                        }
                    }
                }
            }
        }
        fallback
    }
}

// ---------------------------------------------------------------------------
// Configuration filter-chain
// ---------------------------------------------------------------------------
/// Configuration (JSON valide, accepté par PipeWire) d'un processus filter-chain.
///
/// Graphe : copie de chaque canal d'entrée → 4 convolveurs (haut-parleur→oreille)
/// → deux mélangeurs (oreille gauche/droite).
pub fn filter_chain_config(
    ir_paths: &BTreeMap<String, PathBuf>,
    target_sink: &str,
    description: &str,
    slot: &str,
) -> Value {
    let node = node_name(slot);
    let mut nodes = vec![
        json!({"type": "builtin", "label": "copy", "name": "inL"}),
        json!({"type": "builtin", "label": "copy", "name": "inR"}),
    ];
    for key in ["LL", "LR", "RL", "RR"] {
        nodes.push(json!({"type": "builtin", "label": "convolver", "name": format!("conv{key}"),
                          "config": {"filename": ir_paths[key].display().to_string()}}));
    }
    nodes.push(json!({"type": "builtin", "label": "mixer", "name": "mixL"}));
    nodes.push(json!({"type": "builtin", "label": "mixer", "name": "mixR"}));
    let links = json!([
        {"output": "inL:Out", "input": "convLL:In"},
        {"output": "inL:Out", "input": "convLR:In"},
        {"output": "inR:Out", "input": "convRL:In"},
        {"output": "inR:Out", "input": "convRR:In"},
        {"output": "convLL:Out", "input": "mixL:In 1"},
        {"output": "convRL:Out", "input": "mixL:In 2"},
        {"output": "convLR:Out", "input": "mixR:In 1"},
        {"output": "convRR:Out", "input": "mixR:In 2"},
    ]);
    let mut playback = json!({"node.name": format!("{node}.output"), "node.passive": true,
                              "audio.channels": 2, "audio.position": ["FL", "FR"],
                              "stream.dont-remix": true});
    if !target_sink.is_empty() {
        playback["target.object"] = json!(target_sink);
    }
    json!({
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
    })
}

pub fn write_filter_config(
    paths: &Paths,
    ir_paths: &BTreeMap<String, PathBuf>,
    target_sink: &str,
    description: &str,
    slot: &str,
) -> std::io::Result<PathBuf> {
    let conf = paths.slot_conf(slot);
    atomic_write_json(&conf, &filter_chain_config(ir_paths, target_sink, description, slot))?;
    Ok(conf)
}

// ---------------------------------------------------------------------------
// systemd
// ---------------------------------------------------------------------------
/// Argument pour une ligne Exec* de systemd (guillemets doubles, `%` et `$` échappés).
fn systemd_quote(arg: &str) -> String {
    let arg = arg.replace('%', "%%").replace('$', "$$");
    if !arg.is_empty() && arg.chars().all(|c| c.is_alphanumeric() || "/._-+=:,".contains(c)) {
        return arg;
    }
    format!("\"{}\"", arg.replace('\\', "\\\\").replace('"', "\\\""))
}

pub fn exec_line(items: &[String]) -> String {
    items.iter().map(|a| systemd_quote(a)).collect::<Vec<_>>().join(" ")
}

/// Exécutable courant (appelé par les hooks de l'unité).
pub fn current_exe() -> String {
    std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "speaker-sim".into())
}

pub fn hook_command(hook: &str, slot: &str) -> Vec<String> {
    vec![current_exe(), hook.into(), slot.into()]
}

fn pipewire_path() -> String {
    which("pipewire").map(|p| p.display().to_string()).unwrap_or_else(|| "/usr/bin/pipewire".into())
}

pub fn unit_file_content(paths: &Paths, slot: &str) -> String {
    let parent = |p: &Path| p.parent().unwrap_or(p).display().to_string();
    let exec = exec_line(&[pipewire_path(), "-c".into(), paths.slot_conf(slot).display().to_string()]);
    format!(
        "[Unit]
Description={APP_NAME} – simulation de haut-parleurs au casque (devfrp, instance {upper})
After=pipewire.service wireplumber.service
Wants=wireplumber.service
PartOf=pipewire.service

[Service]
Type=simple
Environment=XDG_STATE_HOME={state}
Environment=XDG_CACHE_HOME={cache}
ExecStart={exec}
ExecStartPost=-{started}
ExecStopPost=-{stopped}
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
",
        upper = slot.to_uppercase(),
        state = systemd_quote(&parent(&paths.state_dir)),
        cache = systemd_quote(&parent(&paths.cache_dir)),
        started = exec_line(&hook_command("--hook-started", slot)),
        stopped = exec_line(&hook_command("--hook-stopped", slot)),
    )
}

/// Deux unités alternées (A/B). Seule l'instance A peut être liée (auto-démarrage).
#[derive(Clone)]
pub struct Systemd {
    pub paths: Paths,
    pub run: Runner,
}

impl Systemd {
    pub const AUTOSTART_SLOT: &'static str = "a";

    pub fn new(paths: Paths, run: Runner) -> Self {
        Systemd { paths, run }
    }

    pub fn unit_path(&self, slot: &str) -> PathBuf {
        self.paths.state_dir.join(unit_name(slot))
    }

    fn ctl(&self, items: &[&str], check: bool) -> Result<CmdOutput, SystemError> {
        let mut cmd = args(&["systemctl", "--user"]);
        cmd.extend(items.iter().map(|s| (*s).to_string()));
        let res = (self.run)(&cmd)?;
        if check && res.code != 0 {
            let detail = if res.stderr.trim().is_empty() { &res.stdout } else { &res.stderr };
            return Err(SystemError(format!("systemctl --user {} : {}", items.join(" "), detail.trim())));
        }
        Ok(res)
    }

    fn unit_active(&self, unit: &str) -> bool {
        self.ctl(&["is-active", "--quiet", unit], false).is_ok_and(|r| r.code == 0)
    }

    pub fn active_slots(&self) -> Vec<String> {
        SLOTS.iter().filter(|s| self.unit_active(&unit_name(s))).map(|s| (*s).to_string()).collect()
    }

    pub fn legacy_active(&self) -> bool {
        self.unit_active(UNIT_NAME)
    }

    pub fn is_active(&self) -> bool {
        !self.active_slots().is_empty() || self.legacy_active()
    }

    pub fn is_enabled(&self) -> bool {
        self.ctl(&["is-enabled", &unit_name(Self::AUTOSTART_SLOT)], false)
            .is_ok_and(|r| r.code == 0 && ["enabled", "linked", "enabled-runtime"].contains(&r.stdout.trim()))
    }

    pub fn is_linked(&self, slot: &str) -> bool {
        self.ctl(&["show", "-p", "FragmentPath", "--value", &unit_name(slot)], false)
            .is_ok_and(|r| r.stdout.trim() == self.unit_path(slot).display().to_string())
    }

    pub fn write_unit(&self, slot: &str) -> Result<PathBuf, SystemError> {
        self.paths.ensure().map_err(|e| SystemError(e.to_string()))?;
        let path = self.unit_path(slot);
        std::fs::write(&path, unit_file_content(&self.paths, slot)).map_err(|e| SystemError(e.to_string()))?;
        Ok(path)
    }

    pub fn start(&self, slot: &str) -> Result<(), SystemError> {
        let unit = unit_name(slot);
        if self.unit_active(&unit) {
            self.ctl(&["restart", &unit], true)?;
            return Ok(());
        }
        self.ctl(&["reset-failed", &unit], false)?;
        if self.is_linked(slot) {
            self.write_unit(slot)?;
            self.ctl(&["daemon-reload"], true)?;
            self.ctl(&["start", &unit], true)?;
            return Ok(());
        }
        let parent = |p: &Path| p.parent().unwrap_or(p).display().to_string();
        let cmd = vec![
            "systemd-run".into(),
            "--user".into(),
            format!("--unit={unit}"),
            "--collect".into(),
            format!("--description={APP_NAME} (devfrp, instance {})", slot.to_uppercase()),
            format!("--setenv=XDG_STATE_HOME={}", parent(&self.paths.state_dir)),
            format!("--setenv=XDG_CACHE_HOME={}", parent(&self.paths.cache_dir)),
            "-p".into(),
            "After=pipewire.service wireplumber.service".into(),
            "-p".into(),
            "Restart=on-failure".into(),
            "-p".into(),
            format!("ExecStartPost=-{}", exec_line(&hook_command("--hook-started", slot))),
            "-p".into(),
            format!("ExecStopPost=-{}", exec_line(&hook_command("--hook-stopped", slot))),
            pipewire_path(),
            "-c".into(),
            self.paths.slot_conf(slot).display().to_string(),
        ];
        let res = (self.run)(&cmd)?;
        if res.code != 0 {
            let detail = if res.stderr.trim().is_empty() { &res.stdout } else { &res.stderr };
            return Err(SystemError(format!("systemd-run a échoué : {}", detail.trim())));
        }
        Ok(())
    }

    /// Arrête une instance, ou toutes (y compris l'unité unique de la v1.0).
    pub fn stop(&self, slot: Option<&str>) -> Result<(), SystemError> {
        let units: Vec<String> = match slot {
            Some(s) => vec![unit_name(s)],
            None => SLOTS.iter().map(|s| unit_name(s)).chain([UNIT_NAME.to_string()]).collect(),
        };
        for unit in units {
            if self.unit_active(&unit) {
                self.ctl(&["stop", &unit], true)?;
            }
        }
        Ok(())
    }

    pub fn stop_legacy(&self) -> Result<(), SystemError> {
        if self.legacy_active() {
            self.ctl(&["stop", UNIT_NAME], true)?;
        }
        Ok(())
    }

    /// Retire l'ancienne unité liée de la v1.0 (remplacée par l'instance A).
    pub fn cleanup_legacy(&self) -> Result<(), SystemError> {
        let res = self.ctl(&["show", "-p", "FragmentPath", "--value", UNIT_NAME], false)?;
        if res.stdout.trim() == self.paths.state_dir.join(UNIT_NAME).display().to_string() {
            self.ctl(&["disable", UNIT_NAME], false)?;
            self.ctl(&["daemon-reload"], false)?;
        }
        Ok(())
    }

    pub fn set_autostart(&self, enabled: bool) -> Result<(), SystemError> {
        let slot = Self::AUTOSTART_SLOT;
        let unit = unit_name(slot);
        self.cleanup_legacy()?;
        if enabled {
            if self.unit_active(&unit) && !self.is_linked(slot) {
                return Err(SystemError("l'instance A transitoire est active ; basculez d'abord sur B".into()));
            }
            self.write_unit(slot)?;
            if !self.is_linked(slot) {
                self.ctl(&["link", &self.unit_path(slot).display().to_string()], true)?;
            }
            self.ctl(&["daemon-reload"], true)?;
            self.ctl(&["enable", &unit], true)?;
        } else if self.is_linked(slot) || self.is_enabled() {
            self.ctl(&["disable", &unit], false)?;
            self.ctl(&["daemon-reload"], false)?;
        }
        Ok(())
    }

    pub fn recent_logs(&self, lines: usize) -> Result<String, SystemError> {
        let mut cmd = args(&["journalctl", "--user", "-n"]);
        cmd.push(lines.to_string());
        cmd.extend(args(&["--no-pager", "-o", "cat"]));
        for unit in SLOTS.iter().map(|s| unit_name(s)).chain([UNIT_NAME.to_string()]) {
            cmd.push("-u".into());
            cmd.push(unit);
        }
        Ok((self.run)(&cmd)?.stdout)
    }
}

// ---------------------------------------------------------------------------
// Hooks ExecStartPost / ExecStopPost : bascule et restauration de la sortie
// ---------------------------------------------------------------------------
fn now_seconds() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// Après démarrage : mémorise la sortie actuelle puis bascule sur cette instance.
pub fn hook_started(store: &ConfigStore, pw: &PipeWire, slot: &str, log: &dyn Fn(&str)) -> Result<(), SystemError> {
    hook_started_with(store, pw, slot, log, Duration::from_secs(8))
}

pub fn hook_started_with(
    store: &ConfigStore,
    pw: &PipeWire,
    slot: &str,
    log: &dyn Fn(&str),
    wait: Duration,
) -> Result<(), SystemError> {
    let node = node_name(slot);
    let mut rt = store.load_runtime();
    if pw.wait_for_sink(&node, wait)?.is_none() {
        log("Nœud du simulateur introuvable : sortie par défaut inchangée");
        return Ok(());
    }
    let current = pw.default_sink(false)?;
    // Basculement A↔B : la sortie courante est l'autre instance, on garde la mémoire.
    if !current.is_empty() && !is_sim_node(&current) {
        rt.insert("previous_sink".into(), json!(current));
    }
    rt.insert("started_at".into(), json!(now_seconds()));
    rt.insert("active_slot".into(), json!(slot));
    rt.remove("restarting");
    store.save_runtime(&rt).map_err(|e| SystemError(e.to_string()))?;
    if let Err(e) = pw.set_default_sink(&node) {
        log(&e.0);
    }
    Ok(())
}

/// Après arrêt : restaure la sortie précédente si cette instance était la sortie choisie.
///
/// Lors d'un basculement A↔B, la sortie choisie est déjà l'autre instance : rien à faire.
pub fn hook_stopped(store: &ConfigStore, pw: &PipeWire, slot: &str, log: &dyn Fn(&str)) -> Result<(), SystemError> {
    let node = node_name(slot);
    let mut rt = store.load_runtime();
    if rt.get("restarting").is_some_and(|v| !v.is_null() && v != &json!(false)) {
        return Ok(());
    }
    let previous = text(rt.get("previous_sink"));
    let restore = || -> Result<(), SystemError> {
        // WirePlumber a déjà basculé la sortie effective sur un repli ; seul le
        // choix configuré indique si l'utilisateur écoutait cette instance.
        let configured = pw.default_sink(true)?;
        if !previous.is_empty() && (configured.is_empty() || configured == node) {
            if pw.find_sink(&previous)?.is_some() {
                pw.set_default_sink(&previous)?;
            } else {
                log(&format!("Sortie précédente « {previous} » absente : WirePlumber choisira"));
            }
        }
        Ok(())
    };
    if let Err(e) = restore() {
        log(&e.0);
    }
    let active = rt.get("active_slot").cloned().unwrap_or(Value::Null);
    if active.is_null() || active.as_str() == Some(slot) {
        rt.remove("started_at");
        rt.remove("active_slot");
    }
    store.save_runtime(&rt).map_err(|e| SystemError(e.to_string()))?;
    Ok(())
}
