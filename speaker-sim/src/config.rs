//! Paramètres, état d'exécution et chemins de l'application.
//!
//! Fichiers gérés (aucun autre emplacement n'est écrit) :
//!
//! ```text
//! ~/.local/state/devfrp-speaker-sim/config.json     paramètres utilisateur
//! ~/.local/state/devfrp-speaker-sim/runtime.json    état d'exécution (sortie précédente…)
//! ~/.local/state/devfrp-speaker-sim/filters/        IR générées + configuration filter-chain
//! ~/.local/state/devfrp-speaker-sim/<unité>.service unité systemd (auto-démarrage)
//! ~/.cache/devfrp-speaker-sim/                      mesures téléchargées
//! ```
//!
//! Les variables `XDG_STATE_HOME` / `XDG_CACHE_HOME` sont respectées.
//! Les noms de fichiers évitent volontairement `settings.json` et `headphones/`
//! déjà utilisés par le greffon Omarchy du même nom.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const APP_ID: &str = "devfrp-speaker-sim";
pub const APP_NAME: &str = "Speaker Sim";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const IS_LINUX: bool = cfg!(target_os = "linux");

/// Réglages par défaut et catalogue, embarqués dans l'exécutable.
pub const DEFAULT_CONFIG_JSON: &str = include_str!("../data/default_config.json");
pub const SPEAKER_CATALOG_JSON: &str = include_str!("../data/speakers.json");

/// Préfixe du chemin d'un casque importé par l'utilisateur.
pub const IMPORT_PREFIX: &str = "import:";

pub type Json = Map<String, Value>;

/// Dossier personnel (`HOME`, ou `USERPROFILE` sous Windows).
pub fn home_dir() -> PathBuf {
    #[allow(deprecated)]
    std::env::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    pub state_dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl Paths {
    /// Linux : XDG. Windows : %LOCALAPPDATA%. macOS : ~/Library.
    pub fn default_paths() -> Self {
        Self::for_platform(std::env::consts::OS, |k| std::env::var(k).ok(), &home_dir())
    }

    /// Chemins pour un système donné (`linux`, `windows`, `macos`) ; testable.
    pub fn for_platform(os: &str, env: impl Fn(&str) -> Option<String>, home: &Path) -> Self {
        match os {
            "windows" => {
                let base = env("LOCALAPPDATA")
                    .filter(|v| !v.is_empty())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join("AppData").join("Local"))
                    .join(APP_ID);
                Paths { state_dir: base.join("State"), cache_dir: base.join("Cache") }
            }
            "macos" => {
                let lib = home.join("Library");
                Paths {
                    state_dir: lib.join("Application Support").join(APP_ID),
                    cache_dir: lib.join("Caches").join(APP_ID),
                }
            }
            _ => {
                let xdg = |var: &str, fallback: &str| match env(var) {
                    Some(v) if !v.is_empty() && Path::new(&v).is_absolute() => PathBuf::from(v),
                    _ => home.join(fallback),
                };
                Paths {
                    state_dir: xdg("XDG_STATE_HOME", ".local/state").join(APP_ID),
                    cache_dir: xdg("XDG_CACHE_HOME", ".cache").join(APP_ID),
                }
            }
        }
    }

    pub fn config_file(&self) -> PathBuf {
        self.state_dir.join("config.json")
    }
    pub fn runtime_file(&self) -> PathBuf {
        self.state_dir.join("runtime.json")
    }
    pub fn filters_dir(&self) -> PathBuf {
        self.state_dir.join("filters")
    }
    /// IR et configuration d'une instance (A/B) ; chaque instance a ses fichiers.
    pub fn slot_dir(&self, slot: &str) -> PathBuf {
        self.filters_dir().join(slot)
    }
    pub fn slot_conf(&self, slot: &str) -> PathBuf {
        self.slot_dir(slot).join("filter-chain.conf")
    }
    pub fn filter_conf(&self) -> PathBuf {
        self.slot_conf("a")
    }
    /// Mesures et EQ de casques importés par l'utilisateur.
    pub fn imports_dir(&self) -> PathBuf {
        self.state_dir.join("imports")
    }
    pub fn log_file(&self) -> PathBuf {
        self.state_dir.join("speaker-sim.log")
    }

    pub fn ensure(&self) -> std::io::Result<()> {
        fs::create_dir_all(&self.state_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.state_dir, fs::Permissions::from_mode(0o700))?;
        }
        fs::create_dir_all(self.filters_dir())?;
        fs::create_dir_all(&self.cache_dir)?;
        Ok(())
    }
}

/// Une mesure de casque : ligne de l'index AutoEq (`results/INDEX.md`) ou import.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HeadphoneEntry {
    pub name: String,
    pub source: String,
    pub rig: String,
    /// Chemin relatif à results/, décodé (ex. « oratory1990/over-ear/Sennheiser HD 650 »).
    pub path: String,
    /// Import d'une mesure brute : courbe cible AutoEq.
    #[serde(default)]
    pub target: String,
}

impl HeadphoneEntry {
    pub fn new(name: &str, source: &str, rig: &str, path: &str) -> Self {
        HeadphoneEntry {
            name: name.into(),
            source: source.into(),
            rig: rig.into(),
            path: path.into(),
            target: String::new(),
        }
    }

    pub fn is_import(&self) -> bool {
        self.path.starts_with(IMPORT_PREFIX)
    }

    pub fn label(&self) -> String {
        let rig = if self.rig.is_empty() { String::new() } else { format!(" · {}", self.rig) };
        format!("{} — {}{}", self.name, self.source, rig)
    }

    pub fn to_json(&self) -> Value {
        serde_json::json!({"name": self.name, "source": self.source, "rig": self.rig,
                           "path": self.path, "target": self.target})
    }

    /// Lecture tolérante : `name` et `path` obligatoires, le reste converti en texte.
    pub fn from_json(data: &Value) -> Option<Self> {
        let obj = data.as_object()?;
        let text = |k: &str| obj.get(k).map(value_text).unwrap_or_default();
        if !obj.contains_key("name") || !obj.contains_key("path") {
            return None;
        }
        Some(HeadphoneEntry {
            name: text("name"),
            source: text("source"),
            rig: text("rig"),
            path: text("path"),
            target: text("target"),
        })
    }
}

fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Paramètres persistants (`config.json`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Settings {
    pub headphone: Option<HeadphoneEntry>,
    pub speaker_id: String,
    /// Nom du nœud PipeWire ; "" = sortie par défaut au démarrage.
    pub output_sink: String,
    /// 0 = fréquence de l'horloge PipeWire.
    pub sample_rate: i64,
    /// Demi-angle du triangle stéréo (degrés).
    pub speaker_angle: f64,
    pub speaker_distance: f64,
    pub room_enabled: bool,
    pub room_width: f64,
    pub room_length: f64,
    pub room_height: f64,
    /// Secondes, utilisé si `rt60_auto` est faux.
    pub rt60: f64,
    /// RT60 calculé depuis les matériaux (par octave).
    pub rt60_auto: bool,
    /// "" = réglages personnalisés.
    pub room_preset: String,
    pub room_walls: String,
    pub room_treatment: f64,
    pub room_bass_traps: f64,
    pub room_floor: String,
    pub room_ceiling: String,
    pub room_furniture: String,
    pub room_diffusion: f64,
    pub front_gap: f64,
    pub ear_height: f64,
    /// Hauteur des tweeters (m), 0 = à hauteur d'oreille.
    pub speaker_height: f64,
    /// Décalage latéral de l'auditeur (m, + = gauche).
    pub listener_offset: f64,
    pub room_absorber: String,
    pub treatment_per_wall: bool,
    pub treatment_front: f64,
    pub treatment_sides: f64,
    pub treatment_rear: f64,
    pub treatment_ceiling: f64,
    /// + = enceinte droite plus forte.
    pub balance_db: f64,
    /// Tonalité : plateau grave 105 Hz.
    pub bass_db: f64,
    /// Tonalité : plateau aigu 10 kHz.
    pub treble_db: f64,
    pub tone_enabled: bool,
    pub treatment_enabled: bool,
    pub late_enabled: bool,
    pub placement_custom: bool,
    pub advanced_enabled: bool,
    /// "fr", "en" ; "" = langue du système.
    pub language: String,
    /// "auto" (système), "light", "dark".
    pub theme: String,
    pub room_amount: f64,
    pub reflection_order: i64,
    pub diffuse_field_eq: bool,
    pub headphone_eq_enabled: bool,
    pub preamp_db: f64,
    pub ir_length_ms: i64,
    /// "" = KEMAR du système ou téléchargé.
    pub sofa_file: String,
    pub autostart: bool,
    pub extra: Json,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            headphone: None,
            speaker_id: "KEF R3".into(),
            output_sink: String::new(),
            sample_rate: 0,
            speaker_angle: 30.0,
            speaker_distance: 2.0,
            room_enabled: true,
            room_width: 4.5,
            room_length: 6.0,
            room_height: 2.6,
            rt60: 0.35,
            rt60_auto: true,
            room_preset: "salon".into(),
            room_walls: "masonry".into(),
            room_treatment: 0.0,
            room_bass_traps: 0.0,
            room_floor: "rug".into(),
            room_ceiling: "plaster".into(),
            room_furniture: "furnished".into(),
            room_diffusion: 0.1,
            front_gap: 0.8,
            ear_height: 1.2,
            speaker_height: 0.0,
            listener_offset: 0.0,
            room_absorber: "wool10".into(),
            treatment_per_wall: false,
            treatment_front: 0.0,
            treatment_sides: 0.0,
            treatment_rear: 0.0,
            treatment_ceiling: 0.0,
            balance_db: 0.0,
            bass_db: 0.0,
            treble_db: 0.0,
            tone_enabled: true,
            treatment_enabled: true,
            late_enabled: true,
            placement_custom: true,
            advanced_enabled: true,
            language: String::new(),
            theme: "auto".into(),
            room_amount: 1.0,
            reflection_order: 2,
            diffuse_field_eq: true,
            headphone_eq_enabled: true,
            preamp_db: 0.0,
            ir_length_ms: 400,
            sofa_file: String::new(),
            autostart: false,
            extra: Json::new(),
        }
    }
}

// Conversions tolérantes d'une valeur JSON (anciennes configurations, saisie à la main).
fn coerce_bool(v: &Value) -> Option<bool> {
    Some(match v {
        Value::Bool(b) => *b,
        Value::String(s) => matches!(s.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on" | "oui"),
        Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
        Value::Null => false,
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    })
}

fn coerce_f64(v: &Value) -> Option<f64> {
    let x = match v {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        Value::Bool(b) => f64::from(u8::from(*b)),
        _ => return None,
    };
    if x.is_nan() { None } else { Some(x) }
}

fn coerce_int(v: &Value) -> Option<i64> {
    let x = coerce_f64(v)?;
    x.is_finite().then(|| x.round_ties_even() as i64)
}

fn coerce_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

macro_rules! settings_fields {
    ($m:ident; $( $kind:ident $name:ident ),* $(,)?) => {
        impl Settings {
            /// Champs connus (pour filtrer les clés inconnues).
            pub const FIELDS: &'static [&'static str] = &["headphone", "extra", $( stringify!($name) ),*];

            fn apply_json(&mut self, $m: &Json) {
                $(
                    if let Some(v) = $m.get(stringify!($name)) {
                        if let Some(x) = settings_fields!(@coerce $kind v) {
                            self.$name = x;
                        }
                    }
                )*
            }
        }
    };
    (@coerce bool $v:ident) => { coerce_bool($v) };
    (@coerce f64 $v:ident) => { coerce_f64($v) };
    (@coerce int $v:ident) => { coerce_int($v) };
    (@coerce str $v:ident) => { coerce_string($v) };
}

settings_fields!(m;
    str speaker_id, str output_sink, int sample_rate, f64 speaker_angle, f64 speaker_distance,
    bool room_enabled, f64 room_width, f64 room_length, f64 room_height, f64 rt60,
    bool rt60_auto, str room_preset, str room_walls, f64 room_treatment, f64 room_bass_traps,
    str room_floor, str room_ceiling, str room_furniture, f64 room_diffusion, f64 front_gap,
    f64 ear_height, f64 speaker_height, f64 listener_offset, str room_absorber,
    bool treatment_per_wall, f64 treatment_front, f64 treatment_sides, f64 treatment_rear,
    f64 treatment_ceiling, f64 balance_db, f64 bass_db, f64 treble_db, bool tone_enabled,
    bool treatment_enabled, bool late_enabled, bool placement_custom, bool advanced_enabled,
    str language, str theme, f64 room_amount, int reflection_order, bool diffuse_field_eq,
    bool headphone_eq_enabled, f64 preamp_db, int ir_length_ms, str sofa_file, bool autostart,
);

/// Bornes de validation : (min, max). Les valeurs hors bornes sont ramenées dedans.
pub const LIMITS: &[(&str, f64, f64)] = &[
    ("sample_rate", 0.0, 192000.0),
    ("speaker_angle", 10.0, 80.0),
    ("speaker_distance", 0.5, 10.0),
    ("room_width", 2.0, 20.0),
    ("room_length", 2.0, 30.0),
    ("room_height", 2.0, 8.0),
    ("rt60", 0.1, 4.0),
    ("room_treatment", 0.0, 1.0),
    ("room_bass_traps", 0.0, 1.0),
    ("room_diffusion", 0.0, 1.0),
    ("front_gap", 0.2, 5.0),
    ("ear_height", 0.6, 2.0),
    ("room_amount", 0.05, 2.0),
    ("reflection_order", 0.0, 4.0),
    ("preamp_db", -30.0, 20.0),
    ("ir_length_ms", 50.0, 2500.0),
    ("treatment_front", 0.0, 1.0),
    ("treatment_sides", 0.0, 1.0),
    ("treatment_rear", 0.0, 1.0),
    ("treatment_ceiling", 0.0, 1.0),
    ("speaker_height", 0.0, 3.0),
    ("listener_offset", -3.0, 3.0),
    ("balance_db", -6.0, 6.0),
    ("bass_db", -12.0, 12.0),
    ("treble_db", -12.0, 12.0),
];

fn limit(name: &str) -> (f64, f64) {
    LIMITS.iter().find(|(n, _, _)| *n == name).map(|&(_, lo, hi)| (lo, hi)).expect("borne connue")
}

/// Valeurs autorisées des réglages à choix.
pub const CHOICES: &[(&str, &[&str])] = &[
    ("room_walls", &["masonry", "drywall"]),
    ("room_floor", &["tile", "wood", "rug", "carpet"]),
    ("room_ceiling", &["plaster", "cloud"]),
    ("room_furniture", &["empty", "furnished", "full"]),
    ("room_absorber", &["foam5", "wool10", "wool20"]),
    ("language", &["", "fr", "en"]),
    ("theme", &["auto", "light", "dark"]),
];

impl Settings {
    /// Réglages depuis un dictionnaire JSON : clés inconnues ignorées, valeurs converties,
    /// valeur par défaut si la conversion échoue.
    pub fn from_json(data: &Json) -> Self {
        let mut s = Settings::default();
        s.apply_json(data);
        if let Some(v) = data.get("headphone") {
            s.headphone = HeadphoneEntry::from_json(v);
        }
        if let Some(Value::Object(extra)) = data.get("extra") {
            s.extra = extra.clone();
        }
        s
    }

    pub fn to_json(&self) -> Json {
        match serde_json::to_value(self) {
            Ok(Value::Object(m)) => m,
            _ => Json::new(),
        }
    }

    /// Copie avec des modifications (clés inconnues ignorées), non validée.
    pub fn with(&self, changes: &Json) -> Self {
        let mut data = self.to_json();
        for (k, v) in changes {
            if Self::FIELDS.contains(&k.as_str()) {
                data.insert(k.clone(), v.clone());
            }
        }
        Settings::from_json(&data)
    }

    fn choice_mut(&mut self, name: &str) -> &mut String {
        match name {
            "room_walls" => &mut self.room_walls,
            "room_floor" => &mut self.room_floor,
            "room_ceiling" => &mut self.room_ceiling,
            "room_furniture" => &mut self.room_furniture,
            "room_absorber" => &mut self.room_absorber,
            "language" => &mut self.language,
            "theme" => &mut self.theme,
            _ => unreachable!("choix inconnu : {name}"),
        }
    }

    /// Copie avec valeurs ramenées dans les bornes et cohérence de la pièce.
    pub fn validated(&self) -> Settings {
        let mut s = self.clone();
        let d = Settings::default();
        // Une seule façon de couper la pièce : l'interrupteur (anciennes configs : quantité 0).
        if s.room_amount <= 0.0 {
            s.room_enabled = false;
            s.room_amount = 1.0;
        }
        let clamp = |v: f64, name: &str| {
            let (lo, hi) = limit(name);
            v.max(lo).min(hi)
        };
        macro_rules! clamp_f {
            ($($f:ident),*) => { $( s.$f = clamp(s.$f, stringify!($f)); )* };
        }
        macro_rules! clamp_i {
            ($($f:ident),*) => { $( s.$f = clamp(s.$f as f64, stringify!($f)) as i64; )* };
        }
        clamp_f!(speaker_angle, speaker_distance, room_width, room_length, room_height, rt60,
                 room_treatment, room_bass_traps, room_diffusion, front_gap, ear_height,
                 room_amount, preamp_db, treatment_front, treatment_sides, treatment_rear,
                 treatment_ceiling, speaker_height, listener_offset, balance_db, bass_db,
                 treble_db);
        clamp_i!(sample_rate, reflection_order, ir_length_ms);
        if s.sample_rate != 0 && s.sample_rate < 8000 {
            s.sample_rate = 0;
        }
        for (name, allowed) in CHOICES {
            let default = d.clone().choice_mut(name).clone();
            let v = s.choice_mut(name);
            if !allowed.contains(&v.as_str()) {
                *v = default;
            }
        }
        if room_preset(&s.room_preset).is_none() {
            s.room_preset.clear();
        }
        s.ear_height = s.ear_height.min(s.room_height - 0.3);
        if s.speaker_height != 0.0 {
            s.speaker_height = s.speaker_height.max(0.2).min(s.room_height - 0.2);
        }
        let half = s.room_width / 2.0 - 0.3;
        s.listener_offset = (-half).max(s.listener_offset.min(half));
        // La pièce doit contenir le triangle stéréo (0,5 m de marge).
        let a = s.speaker_angle.to_radians();
        s.room_width = s.room_width.max(2.0 * s.speaker_distance * a.sin() + 1.0);
        s.room_length = s.room_length.max(s.front_gap + s.speaker_distance * a.cos() + 0.5);
        s
    }
}

/// Préréglage de salle : chaque entrée remplace les réglages de pièce correspondants.
#[derive(Clone, Copy, Debug)]
pub struct RoomPreset {
    pub key: &'static str,
    pub label: &'static str,
    pub room_width: f64,
    pub room_length: f64,
    pub room_height: f64,
    pub room_walls: &'static str,
    pub room_treatment: f64,
    pub room_bass_traps: f64,
    pub room_floor: &'static str,
    pub room_ceiling: &'static str,
    pub room_furniture: &'static str,
    pub room_diffusion: f64,
    pub front_gap: f64,
}

macro_rules! preset {
    ($key:literal, $label:literal, $w:expr, $l:expr, $h:expr, $walls:literal, $t:expr, $bt:expr,
     $floor:literal, $ceil:literal, $furn:literal, $diff:expr, $gap:expr) => {
        RoomPreset { key: $key, label: $label, room_width: $w, room_length: $l, room_height: $h,
                     room_walls: $walls, room_treatment: $t, room_bass_traps: $bt,
                     room_floor: $floor, room_ceiling: $ceil, room_furniture: $furn,
                     room_diffusion: $diff, front_gap: $gap }
    };
}

pub const ROOM_PRESETS: &[RoomPreset] = &[
    preset!("salon", "Salon", 4.5, 6.0, 2.6, "masonry", 0.0, 0.0, "rug", "plaster", "furnished", 0.1, 0.8),
    preset!("studio", "Studio traité", 4.2, 5.5, 2.7, "masonry", 0.25, 1.0, "rug", "plaster", "furnished", 0.6, 0.6),
    preset!("regie", "Régie mate", 3.8, 4.6, 2.6, "drywall", 0.35, 1.0, "rug", "cloud", "furnished", 0.3, 0.5),
    preset!("home_cinema", "Home-cinéma", 4.0, 6.0, 2.5, "drywall", 0.3, 1.0, "carpet", "plaster", "furnished", 0.5, 0.7),
    preset!("chambre", "Chambre", 3.5, 4.0, 2.5, "masonry", 0.0, 0.0, "carpet", "plaster", "full", 0.0, 0.4),
    preset!("nue", "Pièce nue", 4.0, 5.0, 2.5, "masonry", 0.0, 0.0, "tile", "plaster", "empty", 0.0, 0.8),
];

pub const ROOM_KEYS: &[&str] = &[
    "room_width", "room_length", "room_height", "room_walls", "room_treatment",
    "room_bass_traps", "room_floor", "room_ceiling", "room_furniture", "room_diffusion",
    "front_gap", "room_absorber", "treatment_per_wall", "treatment_front", "treatment_sides",
    "treatment_rear", "treatment_ceiling",
];

pub fn room_preset(key: &str) -> Option<&'static RoomPreset> {
    ROOM_PRESETS.iter().find(|p| p.key == key)
}

impl RoomPreset {
    /// Réglages de pièce du préréglage (sans le libellé).
    pub fn values(&self) -> Json {
        let v = serde_json::json!({
            "room_width": self.room_width, "room_length": self.room_length,
            "room_height": self.room_height, "room_walls": self.room_walls,
            "room_treatment": self.room_treatment, "room_bass_traps": self.room_bass_traps,
            "room_floor": self.room_floor, "room_ceiling": self.room_ceiling,
            "room_furniture": self.room_furniture, "room_diffusion": self.room_diffusion,
            "front_gap": self.front_gap,
        });
        v.as_object().cloned().unwrap_or_default()
    }
}

/// Réglages à appliquer pour un préréglage (RT60 recalculé automatiquement).
pub fn preset_settings(key: &str) -> Json {
    let mut out = room_preset(key).map(RoomPreset::values).unwrap_or_default();
    for (k, v) in [
        ("room_preset", Value::from(key)),
        ("rt60_auto", Value::from(true)),
        ("room_enabled", Value::from(true)),
        ("room_absorber", Value::from("wool10")),
        ("treatment_per_wall", Value::from(false)),
    ] {
        out.insert(k.into(), v);
    }
    out
}

/// Écriture atomique (fichier temporaire du même dossier puis renommage).
pub fn atomic_write(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut tmp = tempfile::Builder::new().prefix(&format!(".{name}.")).tempfile_in(dir)?;
    tmp.write_all(data)?;
    tmp.as_file().sync_all().ok();
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

pub fn atomic_write_json(path: &Path, data: &Value) -> std::io::Result<()> {
    let mut text = serde_json::to_string_pretty(data).map_err(std::io::Error::other)?;
    text.push('\n');
    atomic_write(path, text.as_bytes())
}

/// Lecture d'un fichier texte comme le ferait Python : UTF-8 tolérant, fins de ligne unifiées.
pub fn read_text_lossy(path: &Path) -> std::io::Result<String> {
    let bytes = fs::read(path)?;
    let text = String::from_utf8_lossy(&bytes);
    Ok(text.replace("\r\n", "\n").replace('\r', "\n"))
}

fn read_json(path: &Path) -> Option<Json> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(_) => {
            set_aside(path);
            return None;
        }
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(m)) => Some(m),
        Ok(_) => None,
        Err(_) => {
            // Fichier corrompu : on le met de côté plutôt que de l'écraser silencieusement.
            set_aside(path);
            None
        }
    }
}

fn set_aside(path: &Path) {
    let mut name = path.as_os_str().to_owned();
    name.push(".corrupt");
    let _ = fs::rename(path, PathBuf::from(name));
}

/// Lecture/écriture de `config.json` et `runtime.json`.
#[derive(Clone, Debug)]
pub struct ConfigStore {
    pub paths: Paths,
}

impl ConfigStore {
    pub fn new(paths: Paths) -> Self {
        ConfigStore { paths }
    }

    pub fn load(&self) -> Settings {
        let mut data: Json = serde_json::from_str(DEFAULT_CONFIG_JSON).unwrap_or_default();
        if let Some(user) = read_json(&self.paths.config_file()) {
            data.extend(user);
        }
        Settings::from_json(&data).validated()
    }

    pub fn save(&self, settings: &Settings) -> std::io::Result<()> {
        atomic_write_json(&self.paths.config_file(), &Value::Object(settings.validated().to_json()))
    }

    /// État d'exécution : sortie précédente, instance active, horodatage…
    pub fn load_runtime(&self) -> Json {
        read_json(&self.paths.runtime_file()).unwrap_or_default()
    }

    pub fn save_runtime(&self, runtime: &Json) -> std::io::Result<()> {
        atomic_write_json(&self.paths.runtime_file(), &Value::Object(runtime.clone()))
    }

    /// Modifie l'état d'exécution ; `None` retire la clé.
    pub fn update_runtime(&self, changes: &[(&str, Option<Value>)]) -> std::io::Result<Json> {
        let mut rt = self.load_runtime();
        for (k, v) in changes {
            match v {
                Some(v) => {
                    rt.insert((*k).into(), v.clone());
                }
                None => {
                    rt.remove(*k);
                }
            }
        }
        self.save_runtime(&rt)?;
        Ok(rt)
    }
}

/// Un haut-parleur du catalogue (`data/speakers.json`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Speaker {
    pub id: String,
    pub brand: String,
    pub model: String,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub shape: String,
    #[serde(default)]
    pub price: String,
    pub pref_score: Option<f64>,
    pub pref_score_wsub: Option<f64>,
    pub lfx_hz: Option<f64>,
    pub path: String,
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default = "default_category")]
    pub category: String,
    #[serde(default = "default_origin")]
    pub origin: String,
}

fn default_format() -> String {
    "cea2034".into()
}
fn default_category() -> String {
    "hifi".into()
}
fn default_origin() -> String {
    "ASR".into()
}

impl Speaker {
    pub fn label(&self) -> String {
        format!("{} {}", self.brand, self.model)
    }
    pub fn is_studio(&self) -> bool {
        self.category == "studio"
    }
    pub fn is_polar(&self) -> bool {
        self.format == "polar"
    }
}

#[derive(Deserialize)]
struct Catalog {
    speakers: Vec<Speaker>,
}

pub fn load_speaker_catalog() -> Vec<Speaker> {
    serde_json::from_str::<Catalog>(SPEAKER_CATALOG_JSON).map(|c| c.speakers).unwrap_or_default()
}

/// Nombre affiché comme `str(float)` de Python (« 6.0 », « 6.33 »).
pub fn py_float(x: f64) -> String {
    if x.is_finite() && x.fract() == 0.0 && x.abs() < 1e16 { format!("{x:.1}") } else { format!("{x}") }
}
