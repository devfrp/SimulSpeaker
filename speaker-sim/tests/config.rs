mod common;

use std::collections::HashSet;
use std::path::PathBuf;

use serde_json::{Value, json};
use speaker_sim::config::{
    self, ConfigStore, DEFAULT_CONFIG_JSON, Json, Paths, ROOM_KEYS, ROOM_PRESETS, Settings, load_speaker_catalog,
};

use common::temp_paths;

fn obj(v: Value) -> Json {
    v.as_object().cloned().unwrap()
}

#[test]
fn defaults_roundtrip() {
    let (_tmp, paths) = temp_paths();
    let store = ConfigStore::new(paths.clone());
    let mut s = store.load();
    assert_eq!(s.speaker_id, "KEF R3");
    s.speaker_distance = 2.5;
    store.save(&s).unwrap();
    assert_eq!(store.load().speaker_distance, 2.5);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&paths.state_dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}

#[test]
fn coercion_and_unknown_keys() {
    let s = Settings::from_json(&obj(json!({
        "rt60": "0.5", "autostart": "oui", "reflection_order": 1.6,
        "bogus": 1, "headphone": {"name": "X"}
    })));
    assert_eq!(s.rt60, 0.5);
    assert!(s.autostart);
    assert_eq!(s.reflection_order, 2);
    assert!(s.headphone.is_none()); // dictionnaire incomplet rejeté
}

#[test]
fn bad_values_fall_back() {
    let s = Settings::from_json(&obj(json!({"rt60": "abc", "speaker_angle": "nan"})));
    assert_eq!(s.rt60, Settings::default().rt60);
    assert_eq!(s.speaker_angle, Settings::default().speaker_angle);
}

#[test]
fn validation_clamps_and_room_fits() {
    let s = Settings {
        rt60: 10.0,
        speaker_angle: 5.0,
        preamp_db: -99.0,
        sample_rate: 100,
        speaker_distance: 5.0,
        room_width: 2.0,
        room_length: 2.0,
        ..Settings::default()
    }
    .validated();
    assert_eq!(s.rt60, 4.0);
    assert_eq!(s.speaker_angle, 10.0);
    assert_eq!(s.preamp_db, -30.0);
    assert_eq!(s.sample_rate, 0);
    assert!(s.room_length >= 5.0);
}

#[test]
fn zero_room_amount_turns_room_off() {
    let s = Settings { room_enabled: true, room_amount: 0.0, ..Settings::default() }.validated();
    assert!(!s.room_enabled);
    assert_eq!(s.room_amount, 1.0);
    assert_eq!(Settings { room_amount: 0.01, ..Settings::default() }.validated().room_amount, 0.05);
}

#[test]
fn choices_and_presets() {
    let s = Settings { room_floor: "lave".into(), room_preset: "inconnu".into(), ..Settings::default() }.validated();
    assert_eq!(s.room_floor, Settings::default().room_floor);
    assert_eq!(s.room_preset, "");
    for preset in ROOM_PRESETS {
        let settings = config::preset_settings(preset.key);
        let applied = Settings::default().with(&settings).validated();
        assert_eq!(applied.room_preset, preset.key);
        assert!(applied.rt60_auto);
        let mut covered = preset.values();
        covered.extend(settings);
        for k in ROOM_KEYS {
            if k.starts_with("treatment_") && *k != "treatment_per_wall" {
                continue; // sans objet : un préréglage désactive le réglage par paroi
            }
            assert!(covered.contains_key(*k), "{}: {k}", preset.key);
        }
    }
}

#[test]
fn corrupted_file_is_set_aside() {
    let (_tmp, paths) = temp_paths();
    std::fs::write(paths.config_file(), "{ pas du json").unwrap();
    let s = ConfigStore::new(paths.clone()).load();
    assert_eq!(s, Settings::default().validated());
    assert!(paths.state_dir.join("config.json.corrupt").exists());
}

#[test]
fn runtime_update() {
    let (_tmp, paths) = temp_paths();
    let store = ConfigStore::new(paths.clone());
    store.update_runtime(&[("previous_sink", Some(json!("hp"))), ("restarting", Some(json!(true)))]).unwrap();
    let rt = store.update_runtime(&[("restarting", None)]).unwrap();
    assert_eq!(Value::Object(rt.clone()), json!({"previous_sink": "hp"}));
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(paths.runtime_file()).unwrap()).unwrap();
    assert_eq!(saved, Value::Object(rt));
}

#[test]
fn xdg_paths() {
    let home = PathBuf::from("/home/test");
    let env = |k: &str| match k {
        "XDG_STATE_HOME" => Some("/x/state".to_string()),
        "XDG_CACHE_HOME" => Some("rel".to_string()),
        _ => None,
    };
    let p = Paths::for_platform("linux", env, &home);
    assert_eq!(p.state_dir, PathBuf::from("/x/state/devfrp-speaker-sim"));
    assert_eq!(p.cache_dir, home.join(".cache/devfrp-speaker-sim")); // relatif ignoré
}

#[test]
fn platform_paths() {
    let home = PathBuf::from("/home/test");
    let win = Paths::for_platform("windows", |k| (k == "LOCALAPPDATA").then(|| "C:/L".to_string()), &home);
    assert_eq!(win.state_dir, PathBuf::from("C:/L/devfrp-speaker-sim/State"));
    assert_eq!(win.cache_dir, PathBuf::from("C:/L/devfrp-speaker-sim/Cache"));
    let mac = Paths::for_platform("macos", |_| None, &home);
    assert_eq!(mac.state_dir, home.join("Library/Application Support/devfrp-speaker-sim"));
    assert_eq!(mac.cache_dir, home.join("Library/Caches/devfrp-speaker-sim"));
}

#[test]
fn default_config_file_matches_struct() {
    let data: Json = serde_json::from_str(DEFAULT_CONFIG_JSON).unwrap();
    assert_eq!(Settings::from_json(&data), Settings::default());
}

#[test]
fn speaker_catalog() {
    let speakers = load_speaker_catalog();
    assert!(speakers.len() >= 130);
    let ids: HashSet<&str> = speakers.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids.len(), speakers.len());
    assert!(speakers.iter().filter(|s| s.category == "studio").count() >= 69);
    for s in &speakers {
        assert!(["studio", "hifi"].contains(&s.category.as_str()), "{}", s.id);
        if s.format == "polar" {
            assert!(!s.path.ends_with(".txt"), "{}", s.id);
        } else {
            assert!(s.path.ends_with("/CEA2034.txt"), "{}", s.id);
        }
    }
    for must in ["Neumann KH 150", "Genelec 8361A", "Kii Audio Three", "Yamaha HS8"] {
        assert!(ids.contains(must), "{must}");
    }
}

#[test]
fn theme_choices() {
    assert_eq!(Settings::default().theme, "auto");
    let s = Settings::from_json(&obj(json!({"theme": "dark"}))).validated();
    assert_eq!(s.theme, "dark");
    let s = Settings::from_json(&obj(json!({"theme": "fluo"}))).validated();
    assert_eq!(s.theme, "auto");
}
