mod common;

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};
use speaker_sim::config::{ConfigStore, Json};
use speaker_sim::system::{self, NODE_NAME, PipeWire, Systemd, UNIT_NAME, node_name};

use common::{FakeRunner, temp_paths};

fn node_a() -> String {
    node_name("a")
}
fn node_b() -> String {
    node_name("b")
}

fn make_dump(configured: &str, effective: &str, links_to: &[i64]) -> Vec<Value> {
    let sink = |id: i64, name: &str, desc: Option<&str>| {
        let mut props = json!({"media.class": "Audio/Sink", "node.name": name});
        if let Some(d) = desc {
            props["node.description"] = json!(d);
        }
        json!({"id": id, "type": "PipeWire:Interface:Node", "info": {"props": props}})
    };
    let mut dump = vec![
        sink(55, "alsa_output.analog", Some("Audio interne")),
        sink(60, "bluez_output.hp", None),
        sink(88, &node_a(), Some("Speaker Sim")),
        sink(89, &node_b(), Some("Speaker Sim")),
        json!({"id": 70, "type": "PipeWire:Interface:Node",
               "info": {"props": {"media.class": "Stream/Output/Audio", "node.name": "brave"}}}),
        json!({"id": 31, "type": "PipeWire:Interface:Metadata", "props": {"metadata.name": "default"},
               "metadata": [
                   {"subject": 0, "key": "default.configured.audio.sink", "value": {"name": configured}},
                   {"subject": 0, "key": "default.audio.sink", "value": {"name": effective}},
               ]}),
    ];
    for (i, target) in links_to.iter().enumerate() {
        dump.push(json!({"id": 200 + i, "type": "PipeWire:Interface:Link",
                         "info": {"output-node-id": 70, "input-node-id": target}}));
    }
    dump
}

fn default_dump() -> Vec<Value> {
    make_dump(&node_a(), "alsa_output.analog", &[])
}

fn dump_runner(dump: &[Value]) -> FakeRunner {
    FakeRunner::new(&[("pw-dump", 0, &serde_json::to_string(dump).unwrap())])
}

fn has_call(r: &FakeRunner, cmd: &[&str]) -> bool {
    r.calls().iter().any(|c| c == cmd)
}

fn no_wpctl(r: &FakeRunner) -> bool {
    !r.calls().iter().any(|c| c[0] == "wpctl")
}

fn runtime(v: Value) -> Json {
    v.as_object().cloned().unwrap()
}

fn irs(path: impl Fn(&str) -> PathBuf) -> BTreeMap<String, PathBuf> {
    ["LL", "LR", "RL", "RR"].iter().map(|k| ((*k).to_string(), path(k))).collect()
}

const QUIET: &dyn Fn(&str) = &|_m: &str| {};

// --- Instances ---------------------------------------------------------------------

#[test]
fn names() {
    assert_eq!(system::unit_name("a"), "devfrp-speaker-sim-py-a.service");
    assert_eq!(system::unit_name(""), UNIT_NAME); // unité v1.0
    assert_eq!(node_b(), "devfrp_speaker_sim_py_b");
    assert_eq!(system::other_slot("a"), "b");
    assert!(system::is_sim_node(&node_b()));
}

// --- PipeWire ----------------------------------------------------------------------

#[test]
fn sinks_exclude_simulators() {
    let pw = PipeWire::new(dump_runner(&default_dump()).runner());
    let sinks = pw.sinks(false).unwrap();
    let names: Vec<&str> = sinks.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["alsa_output.analog", "bluez_output.hp"]);
    assert_eq!(pw.sinks(true).unwrap().len(), 4);
    assert_eq!(sinks[1].label(), "bluez_output.hp");
    assert_eq!(sinks[0].label(), "Audio interne");
}

#[test]
fn default_effective_and_configured() {
    let dump = default_dump();
    assert_eq!(system::parse_default_sink(&dump, false), "alsa_output.analog");
    assert_eq!(system::parse_default_sink(&dump, true), node_a());
    assert_eq!(system::parse_default_sink(&[], false), "");
}

#[test]
fn set_default_uses_node_id() {
    let r = dump_runner(&default_dump());
    PipeWire::new(r.runner()).set_default_sink("bluez_output.hp").unwrap();
    assert!(has_call(&r, &["wpctl", "set-default", "60"]));
}

#[test]
fn input_links() {
    let pw = PipeWire::new(dump_runner(&make_dump(&node_a(), "alsa_output.analog", &[88, 88, 89])).runner());
    assert_eq!(pw.input_links(&node_a()).unwrap(), 2);
    assert_eq!(pw.input_links(&node_b()).unwrap(), 1);
    assert_eq!(pw.input_links("absent").unwrap(), 0);
    let idle = PipeWire::new(dump_runner(&default_dump()).runner());
    assert!(idle.wait_streams_left(&node_a(), Duration::from_millis(500)).unwrap());
    assert!(!pw.wait_streams_left(&node_a(), Duration::from_millis(300)).unwrap());
}

#[test]
fn clock_rate() {
    let r = FakeRunner::new(&[(
        "pw-metadata",
        0,
        "Found \"settings\" metadata 31\nupdate: id:0 key:'clock.rate' value:'44100' type:''\n",
    )]);
    assert_eq!(PipeWire::new(r.runner()).clock_rate(48000), 44100);
    assert_eq!(PipeWire::new(FakeRunner::default().runner()).clock_rate(48000), 48000);
}

// --- Configuration filter-chain ------------------------------------------------------

#[test]
fn filter_graph() {
    let conf = system::filter_chain_config(
        &irs(|k| PathBuf::from(format!("/s/ir_{k}.wav"))),
        "alsa_output.analog",
        "Speaker Sim (X)",
        "b",
    );
    let args = conf["context.modules"].as_array().unwrap().last().unwrap()["args"].clone();
    let names: HashSet<&str> =
        args["filter.graph"]["nodes"].as_array().unwrap().iter().map(|n| n["name"].as_str().unwrap()).collect();
    let want: HashSet<&str> = ["inL", "inR", "convLL", "convLR", "convRL", "convRR", "mixL", "mixR"].into();
    assert_eq!(names, want);
    let links: HashSet<(String, String)> = args["filter.graph"]["links"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| (l["output"].as_str().unwrap().into(), l["input"].as_str().unwrap().into()))
        .collect();
    assert!(links.contains(&("convRL:Out".into(), "mixL:In 2".into()))); // enceinte droite → oreille gauche
    assert!(links.contains(&("inL:Out".into(), "convLR:In".into())));
    assert_eq!(args["capture.props"]["node.name"], json!(node_b()));
    assert_eq!(args["playback.props"]["node.name"], json!(format!("{}.output", node_b())));
    assert_eq!(args["playback.props"]["target.object"], json!("alsa_output.analog"));
}

#[test]
fn filter_no_target() {
    let conf = system::filter_chain_config(&irs(|_| PathBuf::from("x")), "", "d", "a");
    let playback = &conf["context.modules"].as_array().unwrap().last().unwrap()["args"]["playback.props"];
    assert!(playback.get("target.object").is_none());
}

#[test]
fn filter_write_per_slot() {
    let (_tmp, paths) = temp_paths();
    let path = system::write_filter_config(&paths, &irs(|_| PathBuf::from("x")), "", "d", "b").unwrap();
    assert_eq!(path, paths.filters_dir().join("b").join("filter-chain.conf"));
    let _: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
}

// --- systemd -------------------------------------------------------------------------

#[test]
fn systemd_quote() {
    let line = system::exec_line(&system::args(&["/usr/bin/speaker-sim", "/a b/x", "50%", "$HOME"]));
    assert_eq!(line, r#"/usr/bin/speaker-sim "/a b/x" "50%%" "$$HOME""#);
}

#[test]
fn unit_file() {
    let (_tmp, paths) = temp_paths();
    let text = system::unit_file_content(&paths, "a");
    assert!(text.contains(&format!("-c {}", paths.slot_conf("a").display())));
    assert!(text.contains("--hook-started a"));
    assert!(text.contains("--hook-stopped a"));
    assert!(text.contains("WantedBy=default.target"));
}

#[test]
fn start_transient_when_not_linked() {
    let (_tmp, paths) = temp_paths();
    let r = FakeRunner::new(&[("systemctl --user is-active", 3, ""), ("systemctl --user show", 0, "")]);
    Systemd::new(paths.clone(), r.runner()).start("b").unwrap();
    let run = r.calls().into_iter().find(|c| c[0] == "systemd-run").unwrap();
    assert!(run.contains(&"--unit=devfrp-speaker-sim-py-b.service".to_string()));
    assert_eq!(run[run.len() - 2..], ["-c".to_string(), paths.slot_conf("b").display().to_string()]);
    assert!(run.iter().any(|a| a.ends_with("--hook-stopped b")));
}

#[test]
fn start_restarts_active_unit() {
    let (_tmp, paths) = temp_paths();
    let r = FakeRunner::new(&[("systemctl --user is-active", 0, "")]);
    Systemd::new(paths, r.runner()).start("a").unwrap();
    assert!(has_call(&r, &["systemctl", "--user", "restart", "devfrp-speaker-sim-py-a.service"]));
    assert!(!r.calls().iter().any(|c| c[0] == "systemd-run"));
}

#[test]
fn active_slots() {
    let (_tmp, paths) = temp_paths();
    let r = FakeRunner::new(&[
        ("systemctl --user is-active --quiet devfrp-speaker-sim-py-b", 0, ""),
        ("systemctl --user is-active", 3, ""),
    ]);
    let sd = Systemd::new(paths, r.runner());
    assert_eq!(sd.active_slots(), ["b"]);
    assert!(sd.is_active());
    assert!(!sd.legacy_active());
}

#[test]
fn stop_all_includes_legacy() {
    let (_tmp, paths) = temp_paths();
    let r = FakeRunner::new(&[("systemctl --user is-active", 0, "")]);
    Systemd::new(paths, r.runner()).stop(None).unwrap();
    let stopped: Vec<String> = r
        .calls()
        .into_iter()
        .filter(|c| c.len() > 3 && c[..3] == ["systemctl", "--user", "stop"])
        .map(|c| c.last().unwrap().clone())
        .collect();
    assert_eq!(stopped, ["devfrp-speaker-sim-py-a.service", "devfrp-speaker-sim-py-b.service", UNIT_NAME]);
}

#[test]
fn enable_autostart_links_slot_a() {
    let (_tmp, paths) = temp_paths();
    let r = FakeRunner::new(&[("systemctl --user is-active", 3, ""), ("systemctl --user show", 0, "")]);
    let sd = Systemd::new(paths.clone(), r.runner());
    sd.set_autostart(true).unwrap();
    let unit = sd.unit_path("a");
    assert!(unit.is_file());
    assert_eq!(unit.parent().unwrap(), paths.state_dir);
    assert!(has_call(&r, &["systemctl", "--user", "link", unit.to_str().unwrap()]));
    assert!(has_call(&r, &["systemctl", "--user", "enable", "devfrp-speaker-sim-py-a.service"]));
}

#[test]
fn enable_refused_while_transient_a_runs() {
    let (_tmp, paths) = temp_paths();
    let r = FakeRunner::new(&[("systemctl --user is-active", 0, ""), ("systemctl --user show", 0, "")]);
    assert!(Systemd::new(paths, r.runner()).set_autostart(true).is_err());
}

#[test]
fn disable_autostart() {
    let (_tmp, paths) = temp_paths();
    let r = FakeRunner::new(&[("systemctl --user is-enabled", 0, "linked\n")]);
    Systemd::new(paths, r.runner()).set_autostart(false).unwrap();
    assert!(has_call(&r, &["systemctl", "--user", "disable", "devfrp-speaker-sim-py-a.service"]));
}

// --- Hooks ---------------------------------------------------------------------------

#[test]
fn started_records_previous_and_switches() {
    let (_tmp, paths) = temp_paths();
    let store = ConfigStore::new(paths);
    store.save_runtime(&runtime(json!({"restarting": true}))).unwrap();
    let r = dump_runner(&make_dump("alsa_output.analog", "alsa_output.analog", &[]));
    system::hook_started(&store, &PipeWire::new(r.runner()), "a", QUIET).unwrap();
    let rt = store.load_runtime();
    assert_eq!(rt["previous_sink"], json!("alsa_output.analog"));
    assert_eq!(rt["active_slot"], json!("a"));
    assert!(!rt.contains_key("restarting"));
    assert!(has_call(&r, &["wpctl", "set-default", "88"]));
}

/// B démarre alors que A est la sortie : la sortie « casque » mémorisée reste.
#[test]
fn switch_keeps_previous_sink() {
    let (_tmp, paths) = temp_paths();
    let store = ConfigStore::new(paths);
    store.save_runtime(&runtime(json!({"previous_sink": "bluez_output.hp", "active_slot": "a"}))).unwrap();
    let r = dump_runner(&make_dump(&node_a(), &node_a(), &[]));
    system::hook_started(&store, &PipeWire::new(r.runner()), "b", QUIET).unwrap();
    let rt = store.load_runtime();
    assert_eq!(rt["previous_sink"], json!("bluez_output.hp"));
    assert_eq!(rt["active_slot"], json!("b"));
    assert!(has_call(&r, &["wpctl", "set-default", "89"]));
}

/// A s'arrête après la bascule : la sortie choisie est B, on ne touche à rien.
#[test]
fn switch_old_instance_stop_does_not_restore() {
    let (_tmp, paths) = temp_paths();
    let store = ConfigStore::new(paths);
    store.save_runtime(&runtime(json!({"previous_sink": "bluez_output.hp", "active_slot": "b"}))).unwrap();
    let r = dump_runner(&make_dump(&node_b(), &node_b(), &[]));
    system::hook_stopped(&store, &PipeWire::new(r.runner()), "a", QUIET).unwrap();
    assert!(no_wpctl(&r));
    assert_eq!(store.load_runtime()["active_slot"], json!("b"));
}

#[test]
fn stopped_restores_previous() {
    let (_tmp, paths) = temp_paths();
    let store = ConfigStore::new(paths);
    store
        .save_runtime(&runtime(json!({"previous_sink": "bluez_output.hp", "started_at": 1, "active_slot": "a"})))
        .unwrap();
    let r = dump_runner(&default_dump());
    system::hook_stopped(&store, &PipeWire::new(r.runner()), "a", QUIET).unwrap();
    assert!(has_call(&r, &["wpctl", "set-default", "60"]));
    let rt = store.load_runtime();
    assert!(!rt.contains_key("started_at"));
    assert!(!rt.contains_key("active_slot"));
}

#[test]
fn stopped_skips_when_user_chose_other_output() {
    let (_tmp, paths) = temp_paths();
    let store = ConfigStore::new(paths);
    store.save_runtime(&runtime(json!({"previous_sink": "bluez_output.hp"}))).unwrap();
    let r = dump_runner(&make_dump("alsa_output.analog", "alsa_output.analog", &[]));
    system::hook_stopped(&store, &PipeWire::new(r.runner()), "a", QUIET).unwrap();
    assert!(no_wpctl(&r));
}

#[test]
fn stopped_while_restarting_does_nothing() {
    let (_tmp, paths) = temp_paths();
    let store = ConfigStore::new(paths);
    store.save_runtime(&runtime(json!({"previous_sink": "bluez_output.hp", "restarting": true}))).unwrap();
    let r = dump_runner(&default_dump());
    system::hook_stopped(&store, &PipeWire::new(r.runner()), "a", QUIET).unwrap();
    assert!(r.calls().is_empty());
}

#[test]
fn legacy_hook() {
    let (_tmp, paths) = temp_paths();
    let store = ConfigStore::new(paths);
    store.save_runtime(&runtime(json!({"previous_sink": "bluez_output.hp"}))).unwrap();
    let r = dump_runner(&make_dump(NODE_NAME, "alsa_output.analog", &[]));
    system::hook_stopped(&store, &PipeWire::new(r.runner()), "", QUIET).unwrap();
    assert!(has_call(&r, &["wpctl", "set-default", "60"]));
}

#[test]
fn started_without_node_leaves_default() {
    let (_tmp, paths) = temp_paths();
    let store = ConfigStore::new(paths);
    let r = dump_runner(&default_dump()[..2]);
    system::hook_started_with(&store, &PipeWire::new(r.runner()), "a", QUIET, Duration::ZERO).unwrap();
    assert!(no_wpctl(&r));
}
