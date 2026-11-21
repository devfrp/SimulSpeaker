mod common;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use speaker_sim::app::{HeadphoneSummary, HrtfFactory, SpeakerSimApp, changes};
use speaker_sim::audio_engine::HrtfProvider;
use speaker_sim::config::{HeadphoneEntry, Paths};
use speaker_sim::download::Downloader;
use speaker_sim::system::{self, CmdOutput, NODE_NAME, Runner, SystemError, node_name};

use common::{FakeHrtf, FakeHttp, FakeRunner, make_cea_text, temp_paths};

const INDEX: &str = "# Index
- [1MORE Aero (ANC Off)](./HypetheSonics/GRAS%20RA0045%20in-ear/1MORE%20Aero%20(ANC%20Off)) by HypetheSonics on GRAS RA0045
- [Sennheiser HD 650](./oratory1990/over-ear/Sennheiser%20HD%20650) by oratory1990
- [AKG K371](./crinacle/GRAS%2043AG-7%20over-ear/AKG%20K371) by crinacle on GRAS 43AG-7
";

/// Graphe PipeWire : deux sorties physiques, les deux instances du simulateur.
fn dump(configured: &str, effective: &str) -> String {
    let sink = |id: i64, name: &str| {
        json!({"id": id, "type": "PipeWire:Interface:Node",
               "info": {"props": {"media.class": "Audio/Sink", "node.name": name}}})
    };
    json!([
        sink(55, "alsa_output.analog"),
        sink(60, "bluez_output.hp"),
        sink(88, &node_name("a")),
        sink(89, &node_name("b")),
        {"id": 31, "type": "PipeWire:Interface:Metadata", "props": {"metadata.name": "default"},
         "metadata": [
             {"subject": 0, "key": "default.configured.audio.sink", "value": {"name": configured}},
             {"subject": 0, "key": "default.audio.sink", "value": {"name": effective}},
         ]},
    ])
    .to_string()
}

fn default_runner() -> FakeRunner {
    FakeRunner::new(&[
        ("pw-dump", 0, &dump(&node_name("a"), "alsa_output.analog")),
        ("pw-metadata", 0, "key:'clock.rate' value:'48000'"),
        ("systemctl --user is-active", 3, ""),
    ])
}

fn http() -> FakeHttp {
    FakeHttp::new(&[
        ("INDEX.md", INDEX.as_bytes()),
        ("CEA2034.txt", make_cea_text(60).as_bytes()),
        ("ParametricEQ.txt", b"Preamp: -3 dB\nFilter 1: ON PK Fc 1000 Hz Gain 3 dB Q 1\n"),
    ])
}

fn make_app_with(paths: &Paths, runner: Runner, http: &FakeHttp) -> SpeakerSimApp {
    let mut d = Downloader::with_client(paths.cache_dir.clone(), Arc::new(http.clone()));
    d.retry_delay = Duration::ZERO;
    let sofa = paths.cache_dir.join("x.sofa");
    std::fs::write(&sofa, b"fake").unwrap();
    d.system_sofa_paths = vec![sofa];
    let factory: HrtfFactory = Arc::new(|_p, fs| Ok(Box::new(FakeHrtf::new(fs)) as Box<dyn HrtfProvider>));
    let mut app = SpeakerSimApp::with(paths.clone(), runner, d, Some(factory)).unwrap();
    app.supports_live = true;
    app
}

fn make_app(paths: &Paths) -> SpeakerSimApp {
    make_app_with(paths, default_runner().runner(), &http())
}

fn short(app: &SpeakerSimApp) {
    app.update_settings(&changes(json!({"ir_length_ms": 100, "room_enabled": false}))).unwrap();
}

fn measurement() -> String {
    let rows: Vec<String> = (0..61)
        .map(|i| {
            let f = 20.0 * 1.12f64.powi(i);
            format!("{f:.1} {:.2} 0", 80.0 + if f > 3000.0 { 3.0 } else { 0.0 })
        })
        .collect();
    format!("* REW export\nFreq(Hz) SPL(dB) Phase\n{}\n", rows.join("\n"))
}

fn target_csv() -> String {
    let rows: Vec<String> = (0..61).map(|i| format!("{:.1},0.0", 20.0 * 1.12f64.powi(i))).collect();
    format!("frequency,raw\n{}\n", rows.join("\n"))
}

fn write(path: PathBuf, text: &str) -> PathBuf {
    std::fs::write(&path, text).unwrap();
    path
}

// --- Application ---------------------------------------------------------------------

#[test]
fn build_filters_end_to_end() {
    let (_tmp, paths) = temp_paths();
    let app = make_app(&paths);
    let hp = app.search_headphones("hd 650", None).unwrap().remove(0);
    app.select_headphone(Some(&hp)).unwrap();
    app.update_settings(&changes(json!({"ir_length_ms": 150, "rt60": 0.15}))).unwrap();
    let progress = Mutex::new(Vec::new());
    let report = |f: f64, _m: &str| progress.lock().unwrap().push(f);
    let result = app.build_filters(Some(&report)).unwrap();
    assert_eq!(result.fs, 48000);
    assert_eq!(*progress.lock().unwrap().last().unwrap(), 1.0);
    for slot in ["a", "b"] {
        for key in ["LL", "LR", "RL", "RR"] {
            assert!(paths.slot_dir(slot).join(format!("ir_{key}.wav")).is_file());
        }
    }
    assert!(std::fs::read_to_string(paths.slot_conf("b")).unwrap().contains(&node_name("b")));
    let conf: Value = serde_json::from_str(&std::fs::read_to_string(paths.filter_conf()).unwrap()).unwrap();
    let args = &conf["context.modules"].as_array().unwrap().last().unwrap()["args"];
    // Sortie cible : la sortie effective par défaut (le simulateur est exclu).
    assert_eq!(args["playback.props"]["target.object"], json!("alsa_output.analog"));
    assert!(args["node.description"].as_str().unwrap().contains("KEF R3"));
}

#[test]
fn settings_persist_between_instances() {
    let (_tmp, paths) = temp_paths();
    let app = make_app(&paths);
    app.select_speaker("Genelec 8030C").unwrap();
    app.select_headphone(Some(&HeadphoneEntry::new("A", "B", "over-ear", "B/over-ear/A"))).unwrap();
    let again = make_app(&paths);
    assert_eq!(again.settings().speaker_id, "Genelec 8030C");
    assert_eq!(again.headphone().unwrap().name, "A");
}

#[test]
fn unknown_speaker() {
    let (_tmp, paths) = temp_paths();
    assert!(make_app(&paths).select_speaker("Nope 9000").is_err());
}

#[test]
fn explicit_output_sink() {
    let (_tmp, paths) = temp_paths();
    let app = make_app(&paths);
    app.update_settings(&changes(json!({"output_sink": "bluez_output.hp"}))).unwrap();
    assert_eq!(app.target_sink(), "bluez_output.hp");
    app.update_settings(&changes(json!({"output_sink": "disparu"}))).unwrap();
    assert_eq!(app.target_sink(), "alsa_output.analog");
}

#[test]
fn status() {
    let (_tmp, paths) = temp_paths();
    let st = make_app(&paths).status();
    assert_eq!(st["running"], json!(false));
    assert_eq!(st["speaker"], json!("KEF R3"));
    assert_eq!(st["sim_is_default"], json!(false));
    assert_eq!(NODE_NAME, "devfrp_speaker_sim_py");
}

// --- Pièce ---------------------------------------------------------------------------

#[test]
fn preset_changes_rt60_and_layout() {
    let (_tmp, paths) = temp_paths();
    let app = make_app(&paths);
    let living = app.room_spec(None).rt60();
    app.apply_room_preset("studio").unwrap();
    assert_eq!(app.settings().room_preset, "studio");
    assert!(app.room_spec(None).rt60() < living / 2.0);
    let layout = app.layout().unwrap();
    assert!((layout.left[0] - (5.5 - 0.6)).abs() < 1e-9);
}

#[test]
fn manual_rt60() {
    let (_tmp, paths) = temp_paths();
    let app = make_app(&paths);
    app.update_settings(&changes(json!({"rt60_auto": false, "rt60": 0.8}))).unwrap();
    assert!((app.room_spec(None).rt60() - 0.8).abs() < 1e-6);
}

// --- Bascule A/B des instances ---------------------------------------------------------

/// systemd et PipeWire simulés avec état : unités actives, sortie configurée.
#[derive(Clone)]
struct World {
    active: Arc<Mutex<BTreeSet<String>>>,
    configured: Arc<Mutex<String>>,
    events: Arc<Mutex<Vec<String>>>,
}

impl World {
    fn new(active: &[&str], configured: &str) -> Self {
        World {
            active: Arc::new(Mutex::new(active.iter().map(|s| system::unit_name(s)).collect())),
            configured: Arc::new(Mutex::new(configured.into())),
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn runner(&self) -> Runner {
        let w = self.clone();
        Arc::new(move |cmd: &[String]| -> Result<CmdOutput, SystemError> {
            let ok = |stdout: String| Ok(CmdOutput { code: 0, stdout, stderr: String::new() });
            let c: Vec<&str> = cmd.iter().map(String::as_str).collect();
            match c.as_slice() {
                ["pw-dump"] => {
                    w.events.lock().unwrap().push("dump".into());
                    let conf = w.configured.lock().unwrap().clone();
                    ok(dump(&conf, &conf))
                }
                ["systemctl", "--user", "is-active", "--quiet", unit] => {
                    let code = if w.active.lock().unwrap().contains(*unit) { 0 } else { 3 };
                    Ok(CmdOutput { code, ..CmdOutput::default() })
                }
                ["systemctl", "--user", "stop", unit] => {
                    w.events.lock().unwrap().push(format!("stop {unit}"));
                    w.active.lock().unwrap().remove(*unit);
                    ok(String::new())
                }
                ["systemd-run", ..] => {
                    let unit = c.iter().find_map(|a| a.strip_prefix("--unit=")).unwrap().to_string();
                    w.events.lock().unwrap().push(format!("start {unit}"));
                    w.active.lock().unwrap().insert(unit.clone());
                    // Le hook « started » fait de la nouvelle instance la sortie choisie.
                    let slot = unit.trim_end_matches(".service").rsplit('-').next().unwrap().to_string();
                    *w.configured.lock().unwrap() = node_name(&slot);
                    ok(String::new())
                }
                ["wpctl", "set-default", id] => {
                    w.events.lock().unwrap().push(format!("set-default {id}"));
                    ok(String::new())
                }
                _ => ok(String::new()),
            }
        })
    }

    /// Événements hors lectures du graphe, avec « dump » regroupé.
    fn timeline(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for e in self.events.lock().unwrap().iter() {
            if e == "dump" && out.last().is_some_and(|l| l == "dump") {
                continue;
            }
            out.push(e.clone());
        }
        out
    }
}

#[test]
fn switch_from_a_to_b_without_gap() {
    let (_tmp, paths) = temp_paths();
    let world = World::new(&["a"], &node_name("a"));
    let app = make_app_with(&paths, world.runner(), &http());
    assert_eq!(app.switch(&|_, _| {}).unwrap(), "b");
    // B démarre, on attend que les flux aient quitté A (lecture du graphe), puis A s'arrête.
    assert_eq!(
        world.timeline(),
        ["start devfrp-speaker-sim-py-b.service", "dump", "stop devfrp-speaker-sim-py-a.service"]
    );
}

#[test]
fn first_start_uses_a() {
    let (_tmp, paths) = temp_paths();
    let world = World::new(&[], "alsa_output.analog");
    let app = make_app_with(&paths, world.runner(), &http());
    assert_eq!(app.switch(&|_, _| {}).unwrap(), "a");
    let starts: Vec<String> = world.timeline().into_iter().filter(|e| e.starts_with("start")).collect();
    assert_eq!(starts, ["start devfrp-speaker-sim-py-a.service"]);
    assert!(!world.timeline().iter().any(|e| e.starts_with("stop")));
}

// --- Comparaison A/B des réglages ---------------------------------------------------------

#[test]
fn ab_switch_keeps_both_configurations() {
    let (_tmp, paths) = temp_paths();
    let app = make_app(&paths);
    app.update_settings(&changes(json!({"speaker_id": "KEF R3", "bass_db": 2.0}))).unwrap();
    assert_eq!(app.ab_slot(), "A");
    app.ab_switch("B").unwrap(); // B vide : copie de A
    assert_eq!(app.ab_slot(), "B");
    assert_eq!(app.settings().bass_db, 2.0);
    app.apply_room_preset("studio").unwrap();
    app.update_settings(&changes(json!({"bass_db": -4.0, "output_sink": "casque"}))).unwrap();
    app.ab_switch("A").unwrap();
    let s = app.settings();
    assert_eq!(s.bass_db, 2.0);
    assert_eq!(s.room_preset, "salon");
    assert_eq!(s.output_sink, "casque"); // matériel commun
    app.ab_switch("B").unwrap();
    let s = app.settings();
    assert_eq!((s.bass_db, s.room_preset.as_str()), (-4.0, "studio"));
    assert_eq!(make_app(&paths).ab_slot(), "B");
    assert!(app.ab_switch("C").is_err());
}

// --- Imports -----------------------------------------------------------------------------

#[test]
fn import_parametric_eq() {
    let (_tmp, paths) = temp_paths();
    let app = make_app(&paths);
    let src = write(
        paths.cache_dir.parent().unwrap().join("mon eq.txt"),
        "Preamp: -2 dB\nFilter 1: ON PK Fc 3000 Hz Gain -3 dB Q 2\n",
    );
    let entry = app.import_headphone(&src, "Mon casque", "").unwrap();
    assert!(entry.is_import());
    assert_eq!(entry.rig, "EQ paramétrique");
    assert_eq!(app.headphone().unwrap().name, "Mon casque");
    let names: Vec<String> = app.imported_headphones().into_iter().map(|e| e.name).collect();
    assert_eq!(names, ["Mon casque"]);
    assert_eq!(app.search_headphones("mon casque", None).unwrap()[0].path, entry.path);
    short(&app);
    app.build_filters(None).unwrap();
}

#[test]
fn import_raw_measurement_uses_target() {
    let (_tmp, paths) = temp_paths();
    let web = http();
    web.route("Harman%20in-ear%202019.csv", target_csv().as_bytes());
    let app = make_app_with(&paths, default_runner().runner(), &web);
    let src = write(paths.cache_dir.parent().unwrap().join("mesure.txt"), &measurement());
    let entry = app.import_headphone(&src, "", "Harman in-ear 2019").unwrap();
    assert_eq!(entry.name, "mesure");
    assert_eq!(entry.target, "Harman in-ear 2019");
    let eq = app.headphone_eq(&entry).unwrap();
    let (f, g) = eq.graphic.unwrap();
    let band = |lo: f64, hi: f64| {
        let v: Vec<f64> = f.iter().zip(&g).filter(|(x, _)| **x > lo && **x < hi).map(|(_, y)| *y).collect();
        v.iter().sum::<f64>() / v.len() as f64
    };
    // +3 dB mesurés au-dessus de 3 kHz par rapport à une cible plate → correction ≈ −3 dB
    assert!((band(5000.0, 9000.0) + 3.0).abs() < 0.6, "{}", band(5000.0, 9000.0));
    assert!(band(300.0, 1500.0).abs() < 0.4, "{}", band(300.0, 1500.0));
}

#[test]
fn import_rejects_garbage_and_unknown_target() {
    let (_tmp, paths) = temp_paths();
    let app = make_app(&paths);
    let dir = paths.cache_dir.parent().unwrap();
    assert!(app.import_headphone(&write(dir.join("x.txt"), "bonjour\n1 2\n"), "", "").is_err());
    assert!(app.import_headphone(&write(dir.join("m.txt"), &measurement()), "", "Inexistante").is_err());
}

#[test]
fn change_import_target() {
    let (_tmp, paths) = temp_paths();
    let app = make_app(&paths);
    let src = write(paths.cache_dir.parent().unwrap().join("mesure.txt"), &measurement());
    let entry = app.import_headphone(&src, "M", "Harman in-ear 2019").unwrap();
    let updated = app.set_import_target(&entry, "Harman over-ear 2018").unwrap();
    assert_eq!(app.headphone().unwrap().target, "Harman over-ear 2018");
    assert_eq!(app.imported_headphones()[0].rig, "mesure → Harman over-ear 2018");
    assert_eq!(updated.path, entry.path);
    assert!(app.set_import_target(&entry, "Inexistante").is_err());
}

#[test]
fn summary_and_alternatives() {
    let (_tmp, paths) = temp_paths();
    let app = make_app(&paths);
    let entries = app.headphones(None).unwrap();
    let hd650 = entries.iter().find(|e| e.name == "Sennheiser HD 650").unwrap().clone();
    app.select_headphone(Some(&hd650)).unwrap();
    assert_eq!(
        app.headphone_summary().unwrap(),
        Some(HeadphoneSummary { filters: 1, points: 0, preamp_db: -3.0 })
    );
    let alt: Vec<String> = app.alternatives(Some(&hd650)).into_iter().map(|e| e.path).collect();
    assert_eq!(alt, [hd650.path]);
}

#[test]
fn remove_import() {
    let (_tmp, paths) = temp_paths();
    let app = make_app(&paths);
    let src = write(paths.cache_dir.parent().unwrap().join("eq.txt"), "GraphicEQ: 20 0; 1000 0; 20000 -3");
    let entry = app.import_headphone(&src, "", "").unwrap();
    app.remove_import(&entry).unwrap();
    assert!(app.imported_headphones().is_empty());
    assert!(app.headphone().is_none());
}

// --- Fonctions multiplateformes -------------------------------------------------------------

#[test]
fn export_files() {
    let (_tmp, paths) = temp_paths();
    let app = make_app(&paths);
    short(&app);
    let out = app.export(&paths.cache_dir.parent().unwrap().join("export"), None).unwrap();
    let names: Vec<String> = out.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert!(names.contains(&"speaker-sim-true-stereo.wav".to_string()));
    assert!(names.contains(&"speaker-sim.txt".to_string()));
    let stereo = out.iter().find(|p| p.to_string_lossy().ends_with("true-stereo.wav")).unwrap();
    assert_eq!(hound::WavReader::open(stereo).unwrap().spec().channels, 4);
    let apo = std::fs::read_to_string(out.iter().find(|p| p.ends_with("speaker-sim.txt")).unwrap()).unwrap();
    assert!(apo.contains("Copy: VLL=L VLR=L VRL=R VRR=R"));
    assert_eq!(apo.matches("Convolution:").count(), 4);
}

#[test]
fn non_linux_mode() {
    let (_tmp, paths) = temp_paths();
    let mut app = make_app(&paths);
    app.supports_live = false;
    assert!(!app.is_running());
    assert_eq!(app.sample_rate(), 48000);
    assert!(app.start(None).is_err());
    assert!(app.set_autostart(true).is_err());
    short(&app);
    app.build_filters(None).unwrap();
    assert!(!paths.filter_conf().exists());
}

#[test]
fn sample_rate_follows_pipewire_clock() {
    let (_tmp, paths) = temp_paths();
    let runner = default_runner();
    runner.respond("pw-metadata", 0, "update: id:0 key:'clock.rate' value:'44100' type:''");
    let app = make_app_with(&paths, runner.runner(), &http());
    assert_eq!(app.sample_rate(), 44100);
    app.update_settings(&changes(json!({"sample_rate": 96000}))).unwrap();
    assert_eq!(app.sample_rate(), 96000);
}

