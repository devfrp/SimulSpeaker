mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use speaker_sim::config::Speaker;
use speaker_sim::download::{self, AUTOEQ_COMMIT, Downloader, HeadphoneEntry, SPINORAMA_COMMIT};

use common::{FakeHttp, temp_paths};

const INDEX: &str = "# Index
This is a list of all equalization profiles.

- [1MORE Aero (ANC Off)](./HypetheSonics/GRAS%20RA0045%20in-ear/1MORE%20Aero%20(ANC%20Off)) by HypetheSonics on GRAS RA0045
- [Sennheiser HD 650](./oratory1990/over-ear/Sennheiser%20HD%20650) by oratory1990
- [AKG K371](./crinacle/GRAS%2043AG-7%20over-ear/AKG%20K371) by crinacle on GRAS 43AG-7
not a list line
";

fn downloader(cache: &Path, http: &FakeHttp) -> Downloader {
    let mut d = Downloader::with_client(cache.to_path_buf(), Arc::new(http.clone()));
    d.retry_delay = Duration::ZERO;
    d.system_sofa_paths.clear();
    d
}

fn speaker(json: serde_json::Value) -> Speaker {
    let mut v = serde_json::json!({"brand": "", "model": "", "pref_score": null,
                                   "pref_score_wsub": null, "lfx_hz": null});
    v.as_object_mut().unwrap().extend(json.as_object().unwrap().clone());
    serde_json::from_value(v).unwrap()
}

fn files_under(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(files_under(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

#[test]
fn parse_index() {
    let entries = download::parse_autoeq_index(INDEX);
    assert_eq!(entries.len(), 3);
    let aero = &entries[0];
    assert_eq!(aero.name, "1MORE Aero (ANC Off)");
    assert_eq!(aero.path, "HypetheSonics/GRAS RA0045 in-ear/1MORE Aero (ANC Off)");
    assert_eq!(aero.source, "HypetheSonics");
    assert_eq!(aero.rig, "GRAS RA0045 in-ear");
    assert_eq!(entries[1].rig, "over-ear");
}

#[test]
fn search_all_words() {
    let entries = download::parse_autoeq_index(INDEX);
    let names: Vec<String> =
        download::search_headphones(&entries, "hd ORATORY", None).into_iter().map(|e| e.name).collect();
    assert_eq!(names, ["Sennheiser HD 650"]);
    assert_eq!(download::search_headphones(&entries, "", None).len(), 3);
    assert_eq!(download::search_headphones(&entries, "", Some(1)).len(), 1);
}

#[test]
fn search_word_start() {
    let e = [
        HeadphoneEntry::new("Beyerdynamic DT 990 (600 Ohm)", "oratory1990", "over-ear", "a"),
        HeadphoneEntry::new("Beyerdynamic DT 1990 Pro", "oratory1990", "over-ear", "b"),
        HeadphoneEntry::new("Sennheiser HD 650", "crinacle", "over-ear", "c"),
    ];
    let paths = |q: &str| -> Vec<String> {
        download::search_headphones(&e, q, None).into_iter().map(|x| x.path).collect()
    };
    assert_eq!(paths("dt 990"), ["a"]);
    assert_eq!(paths("hd 65"), ["c"]);
    assert_eq!(paths("(600"), ["a"]);
}

#[test]
fn entry_roundtrip() {
    let e = HeadphoneEntry::new("A", "B", "C", "B/C/A");
    assert_eq!(HeadphoneEntry::from_json(&e.to_json()), Some(e));
}

#[test]
fn pinned_url_quoting() {
    let d = downloader(Path::new("/nonexistent"), &FakeHttp::default());
    let url = d.autoeq_url("oratory1990/over-ear/Sennheiser HD 650/Sennheiser HD 650 ParametricEQ.txt");
    assert!(url.contains(AUTOEQ_COMMIT));
    assert!(url.contains("Sennheiser%20HD%20650"));
    assert!(!url.contains(' '));
}

#[test]
fn cache_hit_skips_network() {
    let (_tmp, paths) = temp_paths();
    let http = FakeHttp::new(&[("INDEX.md", INDEX.as_bytes())]);
    let d = downloader(&paths.cache_dir, &http);
    assert_eq!(d.load_headphones(None).unwrap().len(), 3);
    assert_eq!(d.load_headphones(None).unwrap().len(), 3);
    assert_eq!(http.urls().len(), 1);
    assert!(d.autoeq_dir().join("INDEX.md").is_file());
}

#[test]
fn parametric_then_graphic_fallback() {
    let (_tmp, paths) = temp_paths();
    let http = FakeHttp::new(&[("AKG%20K371%20GraphicEQ.txt", b"GraphicEQ: 20 0; 20000 0")]);
    let d = downloader(&paths.cache_dir, &http);
    let entry = &download::parse_autoeq_index(INDEX)[2];
    let path = d.ensure_headphone_eq(entry, None).unwrap();
    assert!(path.file_name().unwrap().to_string_lossy().ends_with("GraphicEQ.txt"));
    assert_eq!(http.urls().len(), 2);
}

#[test]
fn not_found_raises_without_partial_files() {
    let (_tmp, paths) = temp_paths();
    let d = downloader(&paths.cache_dir, &FakeHttp::default());
    let entry = &download::parse_autoeq_index(INDEX)[1];
    assert!(d.ensure_headphone_eq(entry, None).is_err());
    assert_eq!(files_under(&paths.cache_dir), Vec::<std::path::PathBuf>::new());
}

#[test]
fn network_error_retries() {
    let (_tmp, paths) = temp_paths();
    let http = FakeHttp::default();
    http.fail("INDEX.md", "offline");
    let mut d = downloader(&paths.cache_dir, &http);
    d.retries = 2;
    assert!(d.ensure_autoeq_index(None).is_err());
    assert_eq!(http.urls().len(), 2);
}

#[test]
fn speaker_and_hrtf() {
    let (_tmp, paths) = temp_paths();
    let http = FakeHttp::new(&[("CEA2034.txt", b"x"), (".sofa", b"sofa")]);
    let d = downloader(&paths.cache_dir, &http);
    let spk = speaker(serde_json::json!({"id": "KEF R3", "path": "datas/measurements/KEF R3/asr/CEA2034.txt"}));
    let p = d.ensure_speaker(&spk, None).unwrap();
    assert_eq!(p, d.spinorama_dir().join("KEF R3").join("CEA2034.txt"));
    assert!(http.urls()[0].contains(SPINORAMA_COMMIT));
    let polar = speaker(serde_json::json!({"id": "Neumann KH 150", "format": "polar",
                                           "path": "datas/measurements/Neumann KH 150/asr"}));
    http.route("SPL%20Horizontal.txt", b"h");
    http.route("SPL%20Vertical.txt", b"v");
    let folder = d.ensure_speaker(&polar, None).unwrap();
    assert!(folder.join("SPL Horizontal.txt").is_file());
    assert!(folder.join("SPL Vertical.txt").is_file());
    let custom = paths.cache_dir.join("my.sofa");
    std::fs::write(&custom, b"s").unwrap();
    assert_eq!(d.ensure_hrtf(custom.to_str().unwrap(), None).unwrap(), custom);
    // Sans fichier choisi ni système : KEMAR téléchargé dans le cache.
    let kemar = d.ensure_hrtf("", None).unwrap();
    assert_eq!(kemar, paths.cache_dir.join("hrtf").join("mit_kemar_normal_pinna.sofa"));
}
