//! Outils développeur de Speaker Sim.
//!
//! ```text
//! cargo run -p speaker-sim-tools -- catalog [metadata.json]          # régénère data/speakers.json
//! cargo run -p speaker-sim-tools -- studio-monitors metadata.json    # ajoute les moniteurs de studio
//! cargo run -p speaker-sim-tools -- icons                            # icon.svg → icon.png, icon.ico
//! ```
//!
//! `metadata.json` : <https://www.spinorama.org/json/metadata.json> (téléchargé si absent).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use serde_json::{Map, Value, json};
use speaker_sim::download::{SPINORAMA_COMMIT, SPINORAMA_RAW, quote_path, user_agent};

const METADATA_URL: &str = "https://www.spinorama.org/json/metadata.json";
const TARGET_COUNT: usize = 85;
const MAX_PER_BRAND: usize = 4;

/// Mesures Klippel retenues, par ordre de préférence.
const ORIGINS: &[(&str, &str)] = &[("asr", "ASR"), ("asr-horizontal", "ASR"), ("eac", "Erin's Audio Corner")];

const NEW_STUDIO: &[&str] = &[
    "Adam A4V", "Adam D3V", "Adam S2V",
    "Audio Smile Little British Monitor",
    "Edifier MR4",
    "Eve Audio SC305",
    "Fluid Audio FX50", "Fluid Audio FX50v2",
    "Focal Alpha 65 Evo", "Focal Solo6 ST6",
    "Genelec 8050B", "Genelec 8331A", "Genelec 8341A", "Genelec 8351B", "Genelec 8361A",
    "Genelec M040", "Genelec S360",
    "IK Multimedia iLoud MTM MKII",
    "JBL 104", "JBL 305P Mark ii", "JBL 306P Mark ii", "JBL 705P", "JBL 708P",
    "JBL 708i active", "JBL M2",
    "KRK Systems Classic 5",
    "Kali IN-5", "Kali IN-8v2", "Kali LP-6v2", "Kali LP-8v2", "Kali LP-UNF", "Kali SM-5",
    "Kii Audio Three",
    "M-Audio BX3 D3", "M-Audio BX8 D3",
    "Mackie CR3-X",
    "Neumann KH 120 II", "Neumann KH 150", "Neumann KH 420G", "Neumann KH 80",
    "PMC Result6",
    "Pioneer DJ VM-50",
    "Presonus Eris E3.5", "Presonus Eris E5 XT",
    "Yamaha HS8",
];

/// Entrées déjà au catalogue qui sont des moniteurs de studio.
const EXISTING_STUDIO: &[&str] = &[
    "Adam A5X", "Adam T5V", "Adam T8V", "Barefoot Footprint 01", "Behringer B2030p",
    "Dynaudio Core 47", "Dynaudio LYD 5", "Emotiva Airmotiv 6s", "Emotiva Airmotiv B1+",
    "Fluid Audio Image 2", "Focal Solo6 Be", "Fostex PM03", "Genelec 8010A",
    "Genelec 8030C", "Genelec 8320A", "Genelec 8330A", "IK Multimedia iLoud MTM",
    "JBL 308P Mark ii", "KRK Systems RoKit 5 G4", "Kali IN-8", "Kali LP-6",
    "Neumann KH 310A", "Yamaha HS5", "Yamaha HS7",
];

#[derive(Parser)]
#[command(name = "speaker-sim-tools", about = "Outils développeur de Speaker Sim")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Régénère data/speakers.json : 85 enceintes mesurées par ASR (Klippel NFS), classées
    /// par score de préférence avec caisson, 4 par marque au plus.
    Catalog {
        /// metadata.json de Spinorama (téléchargé si absent)
        metadata: Option<PathBuf>,
    },
    /// Ajoute les moniteurs de studio listés à data/speakers.json et renseigne `category`.
    StudioMonitors {
        /// metadata.json de Spinorama (téléchargé si absent)
        metadata: Option<PathBuf>,
    },
    /// Rend packaging/icon.svg en icon.png (512 px) et icon.ico (256, 48, 32, 16 px).
    Icons,
}

/// Dossier `speaker-sim/` (parent de ce crate).
fn project_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("dossier parent").to_path_buf()
}

fn catalog_path() -> PathBuf {
    project_dir().join("data").join("speakers.json")
}

type Res<T> = Result<T, String>;

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .http_status_as_error(false)
        .user_agent(user_agent())
        .build()
        .into()
}

fn load_metadata(path: Option<&Path>) -> Res<Map<String, Value>> {
    let text = match path {
        Some(p) => fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?,
        None => {
            eprintln!("téléchargement de {METADATA_URL}");
            agent()
                .get(METADATA_URL)
                .call()
                .map_err(|e| e.to_string())?
                .into_body()
                .read_to_string()
                .map_err(|e| e.to_string())?
        }
    };
    match serde_json::from_str(&text).map_err(|e| e.to_string())? {
        Value::Object(m) => Ok(m),
        _ => Err("metadata.json : objet attendu".into()),
    }
}

/// Vrai si `rel` existe au commit Spinorama figé.
fn exists_at_pinned_commit(agent: &ureq::Agent, rel: &str) -> bool {
    let url = format!("{SPINORAMA_RAW}/{SPINORAMA_COMMIT}/{}", quote_path(rel));
    agent.head(&url).call().is_ok_and(|r| r.status() == 200)
}

fn text(v: &Value, key: &str) -> Value {
    v.get(key).cloned().filter(|x| !x.is_null()).unwrap_or_else(|| json!(""))
}

fn rating(m: &Value, key: &str) -> Value {
    m.get("pref_rating").and_then(|r| r.get(key)).cloned().unwrap_or(Value::Null)
}

fn sort_speakers(speakers: &mut [Value]) {
    let key = |s: &Value| {
        let f = |k: &str| s.get(k).and_then(Value::as_str).unwrap_or("").to_lowercase();
        (f("brand"), f("model"))
    };
    speakers.sort_by_key(key);
}

/// JSON comme `json.dumps(indent=1, ensure_ascii=False)` suivi d'un saut de ligne.
fn write_catalog(path: &Path, catalog: &Value) -> Res<()> {
    let mut out = Vec::new();
    let fmt = serde_json::ser::PrettyFormatter::with_indent(b" ");
    let mut ser = serde_json::Serializer::with_formatter(&mut out, fmt);
    serde::Serialize::serialize(catalog, &mut ser).map_err(|e| e.to_string())?;
    out.push(b'\n');
    fs::write(path, out).map_err(|e| format!("{}: {e}", path.display()))
}

fn catalog(metadata: Option<&Path>) -> Res<bool> {
    let meta = load_metadata(metadata)?;
    let mut candidates: Vec<(f64, &String, &Value, &Value)> = meta
        .iter()
        .filter_map(|(key, spk)| {
            let m = spk.get("measurements")?.get("asr")?;
            if m.get("format").and_then(Value::as_str) != Some("klippel") {
                return None;
            }
            let shape = spk.get("shape").and_then(Value::as_str)?;
            if !["bookshelves", "floorstanders", "center"].contains(&shape) {
                return None;
            }
            let score = rating(m, "pref_score_wsub").as_f64()?;
            Some((score, key, spk, m))
        })
        .collect();
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0));

    let agent = agent();
    let mut per_brand: HashMap<String, usize> = HashMap::new();
    let mut chosen = Vec::new();
    for (score, key, spk, m) in candidates {
        if chosen.len() >= TARGET_COUNT {
            break;
        }
        let brand = spk.get("brand").and_then(Value::as_str).unwrap_or_default().to_string();
        if per_brand.get(&brand).copied().unwrap_or(0) >= MAX_PER_BRAND {
            continue;
        }
        let rel = format!("datas/measurements/{key}/asr/CEA2034.txt");
        if !exists_at_pinned_commit(&agent, &rel) {
            continue;
        }
        *per_brand.entry(brand.clone()).or_default() += 1;
        chosen.push(json!({
            "id": key,
            "brand": brand,
            "model": text(spk, "model"),
            "type": text(spk, "type"),
            "shape": text(spk, "shape"),
            "price": text(spk, "price"),
            "pref_score": rating(m, "pref_score"),
            "pref_score_wsub": score,
            "lfx_hz": rating(m, "lfx_hz"),
            "path": rel,
        }));
        eprintln!("{:3} {key}", chosen.len());
    }
    sort_speakers(&mut chosen);
    let out = catalog_path();
    let n = chosen.len();
    write_catalog(&out, &json!({"spinorama_commit": SPINORAMA_COMMIT, "speakers": chosen}))?;
    eprintln!("{n} enceintes écrites dans {}", out.display());
    Ok(n == TARGET_COUNT)
}

fn studio_monitors(metadata: Option<&Path>) -> Res<bool> {
    let meta = load_metadata(metadata)?;
    let out = catalog_path();
    let mut catalog: Value =
        serde_json::from_str(&fs::read_to_string(&out).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let speakers = catalog["speakers"].as_array_mut().ok_or("speakers.json : liste « speakers » attendue")?;
    let have: Vec<String> = speakers.iter().filter_map(|s| s["id"].as_str().map(str::to_string)).collect();
    for s in speakers.iter_mut() {
        let id = s["id"].as_str().unwrap_or_default().to_string();
        let obj = s.as_object_mut().ok_or("entrée invalide")?;
        let category = if EXISTING_STUDIO.contains(&id.as_str()) { "studio" } else { "hifi" };
        obj.insert("category".into(), json!(category));
        obj.entry("origin").or_insert(json!("ASR"));
        obj.entry("format").or_insert(json!("cea2034"));
    }

    let agent = agent();
    let mut added = 0;
    for &sid in NEW_STUDIO {
        if have.iter().any(|h| h == sid) {
            continue;
        }
        let spk = meta.get(sid).ok_or_else(|| format!("absent de metadata.json : {sid}"))?;
        let mut klippel: Vec<(usize, &str, &Value)> = spk
            .get("measurements")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter(|(_, m)| m.get("format").and_then(Value::as_str) == Some("klippel"))
            .filter_map(|(k, m)| ORIGINS.iter().position(|(o, _)| o == k).map(|i| (i, k.as_str(), m)))
            .collect();
        klippel.sort_by_key(|(i, _, _)| *i);
        let mut done = false;
        for (i, key, m) in klippel {
            let folder = format!("datas/measurements/{sid}/{key}");
            let (rel, fmt) = if exists_at_pinned_commit(&agent, &format!("{folder}/CEA2034.txt")) {
                (format!("{folder}/CEA2034.txt"), "cea2034")
            } else if exists_at_pinned_commit(&agent, &format!("{folder}/SPL Horizontal.txt"))
                && exists_at_pinned_commit(&agent, &format!("{folder}/SPL Vertical.txt"))
            {
                (folder.clone(), "polar") // CEA-2034 calculé localement
            } else {
                eprintln!("  absent : {sid} [{key}]");
                continue;
            };
            speakers.push(json!({
                "id": sid, "brand": text(spk, "brand"), "model": text(spk, "model"),
                "type": text(spk, "type"), "shape": text(spk, "shape"),
                "price": text(spk, "price"),
                "pref_score": rating(m, "pref_score"),
                "pref_score_wsub": rating(m, "pref_score_wsub"),
                "lfx_hz": rating(m, "lfx_hz"),
                "path": rel, "format": fmt, "category": "studio", "origin": ORIGINS[i].1,
            }));
            added += 1;
            eprintln!("+ {sid} [{key}, {fmt}]");
            done = true;
            break;
        }
        if !done {
            eprintln!("! ignoré (pas de CEA2034 Klippel) : {sid}");
        }
    }
    sort_speakers(speakers);
    let total = speakers.len();
    let n_studio = speakers.iter().filter(|s| s["category"] == "studio").count();
    write_catalog(&out, &catalog)?;
    eprintln!("{added} ajoutés ; {total} au total dont {n_studio} moniteurs de studio");
    Ok(true)
}

fn render_svg(tree: &resvg::usvg::Tree, size: u32) -> Res<resvg::tiny_skia::Pixmap> {
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).ok_or("taille invalide")?;
    let s = tree.size();
    let scale = (size as f32 / s.width()).min(size as f32 / s.height());
    resvg::render(tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    Ok(pixmap)
}

/// Pixels RGBA non prémultipliés (format attendu par l'ICO).
fn straight_rgba(pixmap: &resvg::tiny_skia::Pixmap) -> Vec<u8> {
    pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect()
}

fn icons() -> Res<bool> {
    let dir = project_dir().join("packaging");
    let svg = fs::read(dir.join("icon.svg")).map_err(|e| e.to_string())?;
    let tree = resvg::usvg::Tree::from_data(&svg, &resvg::usvg::Options::default()).map_err(|e| e.to_string())?;
    render_svg(&tree, 512)?.save_png(dir.join("icon.png")).map_err(|e| e.to_string())?;
    let mut icon = ico::IconDir::new(ico::ResourceType::Icon);
    for size in [256, 48, 32, 16] {
        let image = ico::IconImage::from_rgba_data(size, size, straight_rgba(&render_svg(&tree, size)?));
        icon.add_entry(ico::IconDirEntry::encode(&image).map_err(|e| e.to_string())?);
    }
    let file = fs::File::create(dir.join("icon.ico")).map_err(|e| e.to_string())?;
    icon.write(file).map_err(|e| e.to_string())?;
    eprintln!("icon.png, icon.ico");
    Ok(true)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let res = match &cli.command {
        Command::Catalog { metadata } => catalog(metadata.as_deref()),
        Command::StudioMonitors { metadata } => studio_monitors(metadata.as_deref()),
        Command::Icons => icons(),
    };
    match res {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("erreur : {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Le format d'écriture reproduit exactement le fichier existant (pas de diff parasite).
    #[test]
    fn catalog_format_roundtrip() {
        let path = catalog_path();
        let original = fs::read_to_string(&path).unwrap();
        let value: Value = serde_json::from_str(&original).unwrap();
        let tmp = std::env::temp_dir().join(format!("speakers-{}.json", std::process::id()));
        write_catalog(&tmp, &value).unwrap();
        let written = fs::read_to_string(&tmp).unwrap();
        let _ = fs::remove_file(&tmp);
        assert!(written == original, "format différent de data/speakers.json");
    }
}
