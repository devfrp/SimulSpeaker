//! Orchestration : configuration, téléchargements, rendu et système.
//!
//! [`SpeakerSimApp`] ne dépend pas de l'interface graphique ; celle-ci et la ligne de
//! commande l'utilisent de la même façon. Les méthodes longues (téléchargement, rendu,
//! systemctl) sont bloquantes : l'interface les exécute dans un fil de travail. L'état
//! partagé (réglages, index AutoEq, dernier rendu) est protégé par des verrous courts,
//! jamais tenus pendant un calcul.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::audio_engine::{self as ae, AudioEngineError, HeadphoneFileKind, HrtfProvider, RenderResult};
use crate::config::{
    APP_NAME, ConfigStore, HeadphoneEntry, IMPORT_PREFIX, IS_LINUX, Json, Paths, Settings, Speaker, atomic_write_json,
    load_speaker_catalog, preset_settings, read_text_lossy,
};
use crate::download::{AUTOEQ_TARGETS, DownloadError, Downloader, expand_user, search_headphones};
use crate::system::{
    PipeWire, Runner, SLOTS, SystemError, Systemd, default_runner, is_sim_node, missing_tools, node_name, other_slot,
    write_filter_config,
};

/// Erreur présentable à l'utilisateur.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppError(pub String);

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AppError {}

macro_rules! into_app_error {
    ($($t:ty),*) => { $( impl From<$t> for AppError { fn from(e: $t) -> Self { AppError(e.to_string()) } } )* };
}
into_app_error!(AudioEngineError, DownloadError, SystemError, std::io::Error);

pub type Result<T> = std::result::Result<T, AppError>;
pub type Progress<'a> = &'a dyn Fn(f64, &str);
pub type HrtfFactory =
    Arc<dyn Fn(&Path, u32) -> std::result::Result<Box<dyn HrtfProvider>, AudioEngineError> + Send + Sync>;

/// Réglages qui ne font pas partie du son comparé (A/B) : matériel, système, interface.
pub const AB_EXCLUDED: &[&str] =
    &["headphone", "output_sink", "sample_rate", "autostart", "language", "sofa_file", "ir_length_ms", "extra"];

/// Résumé de la correction du casque courant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeadphoneSummary {
    pub filters: usize,
    pub points: usize,
    pub preamp_db: f64,
}

/// Construit un dictionnaire de modifications : `changes(json!({"bass_db": 2.0}))`.
pub fn changes(value: Value) -> Json {
    match value {
        Value::Object(m) => m,
        _ => Json::new(),
    }
}

fn noop(_: f64, _: &str) {}

pub struct SpeakerSimApp {
    pub paths: Paths,
    pub store: ConfigStore,
    settings: Mutex<Settings>,
    pub downloader: Downloader,
    pub pw: PipeWire,
    pub systemd: Systemd,
    pub speakers: Vec<Speaker>,
    headphones: Mutex<Option<Arc<Vec<HeadphoneEntry>>>>,
    hrtf_factory: HrtfFactory,
    /// Simulation en direct : PipeWire + systemd (Linux). Ailleurs : export des filtres.
    pub supports_live: bool,
    last_result: Mutex<Option<Arc<RenderResult>>>,
}

impl SpeakerSimApp {
    pub fn new() -> Result<Self> {
        let paths = Paths::default_paths();
        let downloader = Downloader::new(paths.cache_dir.clone());
        Self::with(paths, default_runner(), downloader, None)
    }

    /// Construction avec dépendances injectées (tests).
    pub fn with(paths: Paths, runner: Runner, downloader: Downloader, hrtf_factory: Option<HrtfFactory>) -> Result<Self> {
        paths.ensure()?;
        let store = ConfigStore::new(paths.clone());
        let settings = store.load();
        Ok(SpeakerSimApp {
            pw: PipeWire::new(runner.clone()),
            systemd: Systemd::new(paths.clone(), runner),
            store,
            settings: Mutex::new(settings),
            downloader,
            speakers: load_speaker_catalog(),
            headphones: Mutex::new(None),
            hrtf_factory: hrtf_factory.unwrap_or_else(|| Arc::new(|p: &Path, fs: u32| ae::open_hrtf(p, fs))),
            supports_live: IS_LINUX,
            last_result: Mutex::new(None),
            paths,
        })
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().expect("réglages").clone()
    }

    pub fn last_result(&self) -> Option<Arc<RenderResult>> {
        self.last_result.lock().expect("rendu").clone()
    }

    // -- catalogue ------------------------------------------------------
    pub fn speaker(&self, speaker_id: Option<&str>) -> Result<&Speaker> {
        let sid = speaker_id.map(str::to_string).unwrap_or_else(|| self.settings().speaker_id);
        self.speakers.iter().find(|s| s.id == sid).ok_or_else(|| AppError(format!("Haut-parleur inconnu : {sid}")))
    }

    /// Index AutoEq (téléchargé une fois, gardé en mémoire).
    pub fn headphones(&self, progress: Option<Progress>) -> Result<Arc<Vec<HeadphoneEntry>>> {
        if let Some(h) = self.headphones.lock().expect("index").clone() {
            return Ok(h);
        }
        let cb = |got: u64, total: Option<u64>| {
            if let Some(p) = progress {
                p(total.filter(|t| *t > 0).map_or(0.0, |t| got as f64 / t as f64), "Index AutoEq…");
            }
        };
        let entries = Arc::new(self.downloader.load_headphones(Some(&cb))?);
        log::info!("Index AutoEq : {} mesures", entries.len());
        *self.headphones.lock().expect("index") = Some(entries.clone());
        Ok(entries)
    }

    /// Index déjà chargé (sans téléchargement).
    pub fn cached_headphones(&self) -> Option<Arc<Vec<HeadphoneEntry>>> {
        self.headphones.lock().expect("index").clone()
    }

    pub fn search_headphones(&self, query: &str, limit: Option<usize>) -> Result<Vec<HeadphoneEntry>> {
        let imported = self.imported_headphones();
        let index = self.headphones(None)?;
        Ok(search_headphones(imported.iter().chain(index.iter()), query, limit))
    }

    // -- mesures importées ------------------------------------------------
    fn imports_index(&self) -> PathBuf {
        self.paths.imports_dir().join("index.json")
    }

    fn import_file(&self, entry: &HeadphoneEntry) -> PathBuf {
        self.paths.imports_dir().join(entry.path.strip_prefix(IMPORT_PREFIX).unwrap_or(&entry.path))
    }

    pub fn imported_headphones(&self) -> Vec<HeadphoneEntry> {
        let Ok(text) = fs::read_to_string(self.imports_index()) else { return vec![] };
        let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&text) else { return vec![] };
        let entries: Option<Vec<HeadphoneEntry>> = items.iter().map(HeadphoneEntry::from_json).collect();
        entries.unwrap_or_default().into_iter().filter(|e| self.import_file(e).is_file()).collect()
    }

    fn save_imports(&self, entries: &[HeadphoneEntry]) -> Result<()> {
        atomic_write_json(&self.imports_index(), &Value::Array(entries.iter().map(HeadphoneEntry::to_json).collect()))?;
        Ok(())
    }

    /// Importe une mesure ou un EQ de casque, le valide, le copie et le sélectionne.
    ///
    /// * EQ paramétrique / graphique (AutoEq, Equalizer APO) : appliqué tel quel ;
    /// * mesure brute (fréquence, dB) : corrigée vers `target` (cible AutoEq).
    pub fn import_headphone(&self, source: &Path, name: &str, target: &str) -> Result<HeadphoneEntry> {
        let src = expand_user(&source.to_string_lossy());
        let text = read_text_lossy(&src).map_err(|e| AppError(format!("Lecture impossible : {e}")))?;
        let (target, rig) = match ae::detect_headphone_file_kind(&text) {
            HeadphoneFileKind::Measurement => {
                ae::parse_measurement(&text)?;
                let target = if target.is_empty() { AUTOEQ_TARGETS[0] } else { target };
                if !AUTOEQ_TARGETS.contains(&target) {
                    return Err(AppError(format!("Cible inconnue : {target}")));
                }
                (target.to_string(), format!("mesure → {target}"))
            }
            HeadphoneFileKind::Graphic => {
                ae::parse_graphic_eq(&text)?;
                (String::new(), "EQ graphique".to_string())
            }
            HeadphoneFileKind::Parametric => {
                if ae::parse_parametric_eq(&text).filters.is_empty() {
                    return Err(AppError("Aucun filtre « Filter N: ON … » reconnu dans ce fichier".into()));
                }
                (String::new(), "EQ paramétrique".to_string())
            }
        };
        let digest = Sha256::digest(text.as_bytes());
        let hex: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
        let file_name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let unsafe_chars = Regex::new(r"[^A-Za-z0-9._-]+").expect("motif");
        let mut safe: String = unsafe_chars.replace_all(&file_name, "_").chars().take(80).collect();
        if safe.is_empty() {
            safe = "mesure.txt".into();
        }
        fs::create_dir_all(self.paths.imports_dir())?;
        let dest = self.paths.imports_dir().join(format!("{hex}-{safe}"));
        fs::copy(&src, &dest)?;
        let stem = src.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let display = if name.trim().is_empty() { stem } else { name.to_string() };
        let entry = HeadphoneEntry {
            name: display.trim().to_string(),
            source: "Import".into(),
            rig: rig.clone(),
            path: format!("{IMPORT_PREFIX}{hex}-{safe}"),
            target,
        };
        let mut all = vec![entry.clone()];
        all.extend(self.imported_headphones().into_iter().filter(|e| e.path != entry.path));
        self.save_imports(&all)?;
        self.select_headphone(Some(&entry))?;
        log::info!("Casque importé : {} ({rig})", entry.name);
        Ok(entry)
    }

    /// Change la cible d'une mesure brute importée (et du casque courant si c'est lui).
    pub fn set_import_target(&self, entry: &HeadphoneEntry, target: &str) -> Result<HeadphoneEntry> {
        if !entry.is_import() || entry.target.is_empty() {
            return Err(AppError("Seule une mesure brute importée a une cible".into()));
        }
        if !AUTOEQ_TARGETS.contains(&target) {
            return Err(AppError(format!("Cible inconnue : {target}")));
        }
        let updated = HeadphoneEntry { target: target.into(), rig: format!("mesure → {target}"), ..entry.clone() };
        let all: Vec<HeadphoneEntry> = self
            .imported_headphones()
            .into_iter()
            .map(|e| if e.path == entry.path { updated.clone() } else { e })
            .collect();
        self.save_imports(&all)?;
        if self.headphone().is_some_and(|h| h.path == entry.path) {
            self.select_headphone(Some(&updated))?;
        }
        Ok(updated)
    }

    /// Autres mesures AutoEq du même modèle (autres sources ou bancs).
    pub fn alternatives(&self, entry: Option<&HeadphoneEntry>) -> Vec<HeadphoneEntry> {
        let (Some(entry), Some(index)) = (entry, self.cached_headphones()) else { return vec![] };
        if entry.is_import() {
            return vec![];
        }
        let name = entry.name.to_lowercase();
        index.iter().filter(|e| e.name.to_lowercase() == name).cloned().collect()
    }

    /// Correction du casque courant : nombre de filtres (ou de points d'un EQ graphique)
    /// et préampli. `None` sans casque.
    pub fn headphone_summary(&self) -> Result<Option<HeadphoneSummary>> {
        let Some(hp) = self.headphone() else { return Ok(None) };
        let eq = self.headphone_eq(&hp)?;
        Ok(Some(HeadphoneSummary {
            filters: eq.filters.len(),
            points: eq.graphic.as_ref().map_or(0, |g| g.0.len()),
            preamp_db: eq.preamp_db,
        }))
    }

    pub fn remove_import(&self, entry: &HeadphoneEntry) -> Result<()> {
        let remaining: Vec<HeadphoneEntry> =
            self.imported_headphones().into_iter().filter(|e| e.path != entry.path).collect();
        self.save_imports(&remaining)?;
        let _ = fs::remove_file(self.import_file(entry));
        if self.headphone().is_some_and(|h| h.path == entry.path) {
            self.select_headphone(None)?;
        }
        Ok(())
    }

    pub fn headphone_eq(&self, hp: &HeadphoneEntry) -> Result<ae::HeadphoneEq> {
        if !hp.is_import() {
            return Ok(ae::load_headphone_eq(&self.downloader.ensure_headphone_eq(hp, None)?)?);
        }
        let path = self.import_file(hp);
        if !path.is_file() {
            return Err(AppError(format!("Fichier importé introuvable : {}", path.display())));
        }
        if hp.target.is_empty() {
            return Ok(ae::load_headphone_eq(&path)?);
        }
        let text = read_text_lossy(&path)?;
        let target = read_text_lossy(&self.downloader.ensure_autoeq_target(&hp.target, None)?)?;
        Ok(ae::measurement_eq(&ae::parse_measurement(&text)?, &ae::parse_measurement(&target)?, 10.0, -20.0))
    }

    pub fn headphone(&self) -> Option<HeadphoneEntry> {
        self.settings.lock().expect("réglages").headphone.clone()
    }

    // -- paramètres -----------------------------------------------------
    /// Applique des modifications (clés inconnues ignorées), valide et enregistre.
    pub fn update_settings(&self, changes: &Json) -> Result<Settings> {
        let mut guard = self.settings.lock().expect("réglages");
        let next = guard.with(changes).validated();
        self.store.save(&next)?;
        *guard = next.clone();
        Ok(next)
    }

    pub fn select_headphone(&self, entry: Option<&HeadphoneEntry>) -> Result<()> {
        let value = entry.map_or(Value::Null, HeadphoneEntry::to_json);
        self.update_settings(&changes(json!({"headphone": value})))?;
        Ok(())
    }

    pub fn select_speaker(&self, speaker_id: &str) -> Result<()> {
        self.speaker(Some(speaker_id))?; // valide
        self.update_settings(&changes(json!({"speaker_id": speaker_id})))?;
        Ok(())
    }

    // -- rendu ----------------------------------------------------------
    pub fn sample_rate(&self) -> u32 {
        let rate = self.effective(None).sample_rate;
        if rate > 0 {
            return rate as u32;
        }
        if !self.supports_live {
            return 48000;
        }
        self.pw.clock_rate(48000)
    }

    /// Réglages réellement utilisés : les onglets désactivés reprennent leurs valeurs
    /// neutres, sans effacer celles de l'utilisateur.
    pub fn effective(&self, settings: Option<&Settings>) -> Settings {
        let mut s = settings.cloned().unwrap_or_else(|| self.settings());
        if !s.treatment_enabled {
            s.room_treatment = 0.0;
            s.room_bass_traps = 0.0;
            s.treatment_per_wall = false;
            s.room_ceiling = "plaster".into();
        }
        if !s.placement_custom {
            let d = Settings::default();
            let a = d.speaker_angle.to_radians();
            let fit = d
                .speaker_distance
                .min((s.room_length - d.front_gap - 0.5) / a.cos())
                .min((s.room_width - 1.0) / (2.0 * a.sin()));
            s.speaker_angle = d.speaker_angle;
            s.speaker_distance = fit.max(0.5);
            s.front_gap = d.front_gap;
            s.listener_offset = 0.0;
            s.ear_height = d.ear_height;
            s.speaker_height = 0.0;
        }
        if !s.advanced_enabled {
            s.diffuse_field_eq = true;
            s.sample_rate = 0;
        }
        s
    }

    pub fn room_spec(&self, settings: Option<&Settings>) -> ae::RoomSpec {
        let s = self.effective(settings);
        ae::RoomSpec {
            width: s.room_width,
            length: s.room_length,
            height: s.room_height,
            order: s.reflection_order.max(0) as usize,
            walls: s.room_walls,
            treatment: s.room_treatment,
            bass_traps: s.room_bass_traps,
            floor: s.room_floor,
            ceiling: s.room_ceiling,
            furniture: s.room_furniture,
            diffusion: s.room_diffusion,
            front_gap: s.front_gap,
            ear_height: s.ear_height,
            rt60_override: if s.rt60_auto { 0.0 } else { s.rt60 },
            absorber: s.room_absorber,
            per_wall: s.treatment_per_wall,
            treatment_front: s.treatment_front,
            treatment_sides: s.treatment_sides,
            treatment_rear: s.treatment_rear,
            treatment_ceiling: s.treatment_ceiling,
            speaker_height: s.speaker_height,
            listener_offset: s.listener_offset,
        }
    }

    /// Positions auditeur / enceintes (vue en plan de l'interface).
    pub fn layout(&self) -> Option<ae::Layout> {
        let e = self.effective(None);
        ae::stereo_layout(&self.room_spec(Some(&e)), e.speaker_angle, e.speaker_distance).ok()
    }

    // -- comparaison A/B --------------------------------------------------
    pub fn ab_slot(&self) -> String {
        self.settings()
            .extra
            .get("ab")
            .and_then(|ab| ab.get("slot"))
            .and_then(Value::as_str)
            .unwrap_or("A")
            .to_string()
    }

    fn ab_snapshot(s: &Settings) -> Json {
        s.to_json().into_iter().filter(|(k, _)| !AB_EXCLUDED.contains(&k.as_str())).collect()
    }

    /// Bascule vers la configuration `slot` (A ou B) en gardant l'autre en mémoire.
    ///
    /// Une configuration encore vide part de la configuration courante.
    pub fn ab_switch(&self, slot: &str) -> Result<Settings> {
        if slot != "A" && slot != "B" {
            return Err(AppError(format!("Configuration inconnue : {slot}")));
        }
        let s = self.settings();
        let mut ab = s.extra.get("ab").and_then(Value::as_object).cloned().unwrap_or_default();
        let current = ab.get("slot").and_then(Value::as_str).unwrap_or("A").to_string();
        if slot == current {
            return Ok(s);
        }
        let snapshot = Self::ab_snapshot(&s);
        ab.insert(current.clone(), Value::Object(snapshot.clone()));
        let target = match ab.get(slot) {
            Some(Value::Object(m)) if !m.is_empty() => m.clone(),
            _ => snapshot,
        };
        ab.insert("slot".into(), json!(slot));
        let mut update: Json = target
            .into_iter()
            .filter(|(k, _)| Settings::FIELDS.contains(&k.as_str()) && !AB_EXCLUDED.contains(&k.as_str()))
            .collect();
        let mut extra = s.extra.clone();
        extra.insert("ab".into(), Value::Object(ab));
        update.insert("extra".into(), Value::Object(extra));
        self.update_settings(&update)
    }

    pub fn apply_room_preset(&self, key: &str) -> Result<Settings> {
        self.update_settings(&preset_settings(key))
    }

    /// Enregistre une préférence d'affichage dans `extra` (sans effet sur le son).
    pub fn set_extra(&self, key: &str, value: Value) -> Result<Settings> {
        let mut extra = self.settings().extra;
        extra.insert(key.into(), value);
        self.update_settings(&changes(json!({"extra": extra})))
    }

    pub fn render_params(&self, settings: &Settings, fs: u32) -> ae::RenderParams {
        let s = self.effective(Some(settings));
        let tone = |v: f64| if s.tone_enabled { v } else { 0.0 };
        ae::RenderParams {
            fs,
            angle_deg: s.speaker_angle,
            distance: s.speaker_distance,
            room: self.room_spec(Some(settings)),
            room_enabled: s.room_enabled,
            room_amount: s.room_amount,
            diffuse_field_eq: s.diffuse_field_eq,
            ir_length_ms: s.ir_length_ms as f64,
            preamp_db: s.preamp_db,
            balance_db: tone(s.balance_db),
            bass_db: tone(s.bass_db),
            treble_db: tone(s.treble_db),
            late_enabled: s.late_enabled,
            ..Default::default()
        }
    }

    /// Télécharge ce qui manque, calcule et écrit les IR + la config filter-chain.
    pub fn build_filters(&self, progress: Option<Progress>) -> Result<Arc<RenderResult>> {
        let report = |f: f64, m: &str| progress.unwrap_or(&noop)(f, m);
        let s = self.settings();
        let spk = self.speaker(None)?.clone();
        let fs = self.sample_rate();

        report(0.02, &format!("Mesure CEA-2034 : {} {}", spk.brand, spk.model));
        let speaker = ae::load_speaker(&self.downloader.ensure_speaker(&spk, None)?)?;

        let hp = s.headphone.clone();
        let mut hp_fir = None;
        if let Some(hp) = hp.as_ref().filter(|_| s.headphone_eq_enabled) {
            report(0.06, &format!("Correction du casque : {}", hp.name));
            hp_fir = Some(ae::headphone_eq_fir(&self.headphone_eq(hp)?, f64::from(fs), ae::FIR_TAPS, false));
        }

        report(0.1, "HRTF KEMAR");
        let hrtf = (self.hrtf_factory)(&self.downloader.ensure_hrtf(&s.sofa_file, None)?, fs)?;
        let sub = |f: f64, m: &str| report(0.1 + 0.8 * f, m);
        let mut result = ae::render(&speaker, hp_fir.as_deref(), hrtf.as_ref(), &self.render_params(&s, fs), Some(&sub))?;
        drop(hrtf);

        report(0.92, "Écriture des filtres");
        // Les deux instances reçoivent les mêmes filtres : l'instance active a déjà
        // chargé les siens en mémoire, l'autre démarrera avec les nouveaux.
        let target = if self.supports_live { self.target_sink() } else { String::new() };
        let description = self.description_for(&spk);
        for slot in SLOTS {
            let ir_paths = ae::write_irs(&result, &self.paths.slot_dir(slot))?;
            if self.supports_live {
                write_filter_config(&self.paths, &ir_paths, &target, &description, slot)?;
            }
        }
        if hp.is_none() {
            result.notes.push("Aucun casque sélectionné : pas de correction AutoEq".into());
        }
        let result = Arc::new(result);
        *self.last_result.lock().expect("rendu") = Some(result.clone());
        log::info!(
            "Filtres générés ({fs} Hz, gain {:.1} dB) : {} / {}",
            result.gain_db,
            spk.id,
            hp.as_ref().map_or("-", |h| h.name.as_str())
        );
        report(1.0, "Filtres prêts");
        Ok(result)
    }

    /// Génère les filtres et les copie vers `dest` dans des formats portables :
    /// 4 WAV mono, un WAV 4 canaux « true stereo » et une config Equalizer APO.
    pub fn export(&self, dest: &Path, progress: Option<Progress>) -> Result<Vec<PathBuf>> {
        let result = self.build_filters(progress)?;
        let dest = expand_user(&dest.to_string_lossy());
        fs::create_dir_all(&dest)?;
        let ir_paths = ae::write_irs(&result, &dest)?;
        let stereo = ae::write_true_stereo(&result, &dest.join("speaker-sim-true-stereo.wav"))?;
        let apo = dest.join("speaker-sim.txt");
        fs::write(&apo, ae::equalizer_apo_config(&ir_paths))?;
        let info = dest.join("LISEZ-MOI.txt");
        let hp = self.headphone().map_or("aucune correction".to_string(), |h| h.label());
        fs::write(
            &info,
            format!(
                "{} — casque : {hp}\n{} Hz, gain {:+.1} dB\n\n\
                 ir_XY.wav : enceinte X vers oreille Y (mono, float32).\n\
                 speaker-sim-true-stereo.wav : 4 canaux LL, LR, RL, RR.\n\
                 speaker-sim.txt : Equalizer APO ≥ 1.2 (Windows) ; ajoutez\n  \
                 « Include: speaker-sim.txt » dans config.txt après avoir copié ce dossier\n  \
                 dans C:\\Program Files\\EqualizerAPO\\config\\ (chemins absolus inclus).\n",
                self.description(),
                result.fs,
                result.gain_db
            ),
        )?;
        log::info!("Filtres exportés vers {}", dest.display());
        let mut out: Vec<PathBuf> = ir_paths.into_values().collect();
        out.extend([stereo, apo, info]);
        Ok(out)
    }

    fn description_for(&self, spk: &Speaker) -> String {
        format!("{APP_NAME} ({} {})", spk.brand, spk.model)
    }

    pub fn description(&self) -> String {
        self.speaker(None).map(|s| self.description_for(s)).unwrap_or_else(|_| APP_NAME.into())
    }

    /// Sortie physique (casque) vers laquelle le filtre joue.
    pub fn target_sink(&self) -> String {
        let configured = self.settings().output_sink;
        let pick = || -> std::result::Result<String, SystemError> {
            let mut names: Vec<String> = self.pw.sinks(false)?.into_iter().map(|s| s.name).collect();
            if names.contains(&configured) {
                return Ok(configured.clone());
            }
            let previous = self.store.load_runtime().get("previous_sink").and_then(Value::as_str).unwrap_or("").to_string();
            for candidate in [previous, self.pw.default_sink(false)?, self.pw.default_sink(true)?] {
                if names.contains(&candidate) && !is_sim_node(&candidate) {
                    return Ok(candidate);
                }
            }
            names.sort();
            Ok(names.into_iter().next().unwrap_or_default())
        };
        pick().unwrap_or_else(|e| {
            log::warn!("Sortie cible indéterminée : {e}");
            configured.clone()
        })
    }

    // -- cycle de vie ---------------------------------------------------
    pub fn is_running(&self) -> bool {
        self.supports_live && self.systemd.is_active()
    }

    pub fn start(&self, progress: Option<Progress>) -> Result<Arc<RenderResult>> {
        if !self.supports_live {
            return Err(AppError(
                "La simulation en direct nécessite Linux + PipeWire. \
                 Utilisez « Exporter » pour un convolveur (Equalizer APO…)."
                    .into(),
            ));
        }
        let missing = missing_tools();
        if !missing.is_empty() {
            return Err(AppError(format!("Outils manquants : {}", missing.join(", "))));
        }
        let result = self.build_filters(progress)?;
        self.switch(progress.unwrap_or(&noop)).map_err(|e| AppError(format!("Démarrage impossible : {e}\n{}", self.logs())))?;
        log::info!("Simulation démarrée ({})", self.description());
        Ok(result)
    }

    /// Reconstruire + (re)démarrer.
    pub fn apply(&self, progress: Option<Progress>) -> Result<Arc<RenderResult>> {
        self.start(progress)
    }

    /// Démarre l'instance inactive puis arrête l'ancienne, sans coupure.
    ///
    /// La nouvelle instance devient la sortie par défaut (hook) ; WirePlumber y
    /// déplace les flux qui suivent la sortie par défaut ; on attend qu'ils aient
    /// quitté l'ancienne instance avant de l'arrêter. La sortie virtuelle n'est donc
    /// jamais absente et les lecteurs ne se mettent pas en pause.
    pub fn switch(&self, report: Progress) -> std::result::Result<String, SystemError> {
        let active = self.systemd.active_slots();
        let legacy = self.systemd.legacy_active();
        let new = active.first().map_or("a", |s| other_slot(s)).to_string();
        report(0.95, if !active.is_empty() || legacy { "Bascule sans coupure" } else { "Démarrage du service" });
        self.systemd.start(&new)?;
        let node = node_name(&new);
        if self.pw.wait_for_sink(&node, Duration::from_secs(8))?.is_none() {
            let _ = self.systemd.stop(Some(&new));
            return Err(SystemError("la nouvelle instance n'est pas apparue dans PipeWire".into()));
        }
        if self.pw.default_sink(true)? != node {
            self.pw.set_default_sink(&node)?;
        }
        let mut old_nodes: Vec<String> = active.iter().filter(|s| **s != new).map(|s| node_name(s)).collect();
        if legacy {
            old_nodes.push(node_name(""));
        }
        for old in &old_nodes {
            if !self.pw.wait_streams_left(old, Duration::from_secs(3))? {
                log::warn!("Des flux restent liés à {old} (cible explicite) ; arrêt quand même");
            }
        }
        for slot in active.iter().filter(|s| **s != new) {
            self.systemd.stop(Some(slot))?;
        }
        if legacy {
            self.systemd.stop_legacy()?;
            self.systemd.cleanup_legacy()?;
        }
        if self.systemd.active_slots().is_empty() {
            return Err(SystemError("le service s'est arrêté aussitôt".into()));
        }
        Ok(new)
    }

    pub fn stop(&self) -> Result<()> {
        self.systemd.stop(None).map_err(|e| AppError(format!("Arrêt impossible : {e}")))?;
        log::info!("Simulation arrêtée");
        Ok(())
    }

    pub fn set_autostart(&self, enabled: bool) -> Result<()> {
        if !self.supports_live {
            return Err(AppError("Auto-démarrage disponible uniquement sous Linux".into()));
        }
        if enabled && !self.paths.filter_conf().exists() {
            self.build_filters(None).map_err(|e| AppError(format!("Auto-démarrage : {e}")))?;
        }
        let configure = || -> std::result::Result<(), SystemError> {
            if enabled && self.systemd.active_slots().iter().any(|s| s == "a") && !self.systemd.is_linked("a") {
                // L'instance A tourne en transitoire : basculer sur B (sans coupure)
                // libère son nom pour l'unité persistante.
                self.switch(&noop)?;
            }
            self.systemd.set_autostart(enabled)
        };
        configure().map_err(|e| AppError(format!("Auto-démarrage : {e}")))?;
        self.update_settings(&changes(json!({"autostart": enabled})))?;
        Ok(())
    }

    pub fn status(&self) -> Value {
        let rt = self.store.load_runtime();
        let spk = self.speaker(None).map(Speaker::label).unwrap_or_default();
        let slots = if self.supports_live { self.systemd.active_slots() } else { vec![] };
        let default = if self.supports_live { self.pw.default_sink(false).unwrap_or_default() } else { String::new() };
        json!({
            "running": self.is_running(),
            "autostart": self.supports_live && self.systemd.is_enabled(),
            "speaker": spk,
            "headphone": self.headphone().map(|h| h.label()),
            "default_sink": default,
            "instance": slots.first().map(|s| s.to_uppercase()),
            "sim_is_default": is_sim_node(&default),
            "previous_sink": rt.get("previous_sink").cloned().unwrap_or(Value::Null),
            "filters_ready": self.paths.filter_conf().exists(),
        })
    }

    fn logs(&self) -> String {
        self.systemd.recent_logs(15).unwrap_or_default()
    }
}

/// IR générées pour une instance.
pub fn ir_files(paths: &Paths, slot: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = fs::read_dir(paths.slot_dir(slot))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("ir_") && n.ends_with(".wav"))
        })
        .collect();
    out.sort();
    out
}

/// Chemins des IR par paire (`ir_LL.wav`…) d'un dossier.
pub fn ir_paths_in(dir: &Path) -> BTreeMap<String, PathBuf> {
    ae::PAIRS.iter().map(|k| ((*k).to_string(), dir.join(format!("ir_{k}.wav")))).collect()
}
