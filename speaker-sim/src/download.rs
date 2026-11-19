//! Téléchargement et cache des mesures (AutoEq, Spinorama, HRTF KEMAR).
//!
//! Toutes les sources sont figées sur un commit Git : un fichier présent dans le
//! cache est donc immuable et n'est jamais retéléchargé. Le cache vit dans
//! `~/.cache/devfrp-speaker-sim/` avec la disposition suivante :
//!
//! ```text
//! autoeq/<commit12>/INDEX.md
//! autoeq/<commit12>/<source>/<rig>/<modèle>/<modèle> ParametricEQ.txt
//! spinorama/<commit12>/<haut-parleur>/CEA2034.txt
//! hrtf/mit_kemar_normal_pinna.sofa          (seulement s'il n'est pas déjà sur le système)
//! ```
//!
//! Ce module ne dépend pas de l'interface : les rappels de progression sont de simples
//! fonctions `(octets_reçus, octets_totaux_ou_None)`.

use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};

pub use crate::config::{HeadphoneEntry, IMPORT_PREFIX};
use crate::config::{APP_VERSION, Speaker};

pub const AUTOEQ_COMMIT: &str = "7ae0f56d53074872b028649617a22bbb4232feb7";
pub const AUTOEQ_RAW: &str = "https://raw.githubusercontent.com/jaakkopasanen/AutoEq";
pub const SPINORAMA_COMMIT: &str = "acc757bb98d63327092ee537bde25d9c227811f3";
pub const SPINORAMA_RAW: &str = "https://raw.githubusercontent.com/pierreaubert/spinorama";
pub const KEMAR_URL: &str = "https://sofacoustics.org/data/database/mit/mit_kemar_normal_pinna.sofa";

/// Emplacements où libmysofa installe généralement le HRTF MIT KEMAR.
pub const SYSTEM_SOFA_PATHS: &[&str] = &[
    "/usr/share/libmysofa/MIT_KEMAR_normal_pinna.sofa",
    "/usr/share/libmysofa/default.sofa",
    "/usr/local/share/libmysofa/MIT_KEMAR_normal_pinna.sofa",
];

pub fn user_agent() -> String {
    format!("devfrp-speaker-sim/{APP_VERSION} (+https://github.com/devfrp)")
}

/// Courbes cibles AutoEq proposées pour corriger une mesure brute importée.
pub const AUTOEQ_TARGETS: &[&str] = &[
    "Harman over-ear 2018",
    "Harman in-ear 2019",
    "Diffuse field ISO 11904-1",
    "Diffuse field GRAS KEMAR",
];

/// Progression : (octets reçus, total s'il est connu).
pub type ProgressCallback<'a> = &'a dyn Fn(u64, Option<u64>);

/// Échec réseau ou fichier introuvable à la source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadError(pub String);

impl fmt::Display for DownloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DownloadError {}

/// Analyse `INDEX.md` d'AutoEq.
///
/// Format d'une ligne : `- [NOM](./CHEMIN%20ENCODÉ) by SOURCE[ on BANC]`.
/// Le nom et le chemin peuvent contenir des parenthèses, d'où l'analyse par
/// séparateurs plutôt que par expression régulière gourmande.
pub fn parse_autoeq_index(text: &str) -> Vec<HeadphoneEntry> {
    let mut entries = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if !line.starts_with("- [") || !line.contains("](./") {
            continue;
        }
        let Some((name, rest)) = line[3..].split_once("](./") else { continue };
        let Some((path_enc, by_part)) = rest.rsplit_once(") by ") else { continue };
        let (source, rig) = by_part.split_once(" on ").unwrap_or((by_part, ""));
        let path = percent_decode_str(path_enc).decode_utf8_lossy().into_owned();
        // Le « rig » de l'index est le banc de mesure ; on garde plutôt le dossier
        // (ex. « 711 in-ear ») qui distingue aussi intra/circum-auriculaire.
        let parts: Vec<&str> = path.split('/').collect();
        let rig_dir = if parts.len() >= 3 { parts[1] } else { rig };
        entries.push(HeadphoneEntry::new(name, source.trim(), rig_dir.trim(), &path));
    }
    entries
}

/// Un motif par mot, ancré en début de mot : « 990 » ne trouve pas « 1990 »,
/// mais « hd 65 » trouve « HD 650 ».
#[derive(Clone, Debug, Default)]
pub struct Query {
    words: Vec<String>,
}

impl Query {
    pub fn new(query: &str) -> Self {
        Query { words: query.to_lowercase().split_whitespace().map(str::to_string).collect() }
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// Vrai si chaque mot commence un mot de `haystack` (insensible à la casse).
    pub fn matches(&self, haystack: &str) -> bool {
        if self.words.is_empty() {
            return true;
        }
        let hay = haystack.to_lowercase();
        self.matches_lowercase(&hay)
    }

    /// Comme [`Query::matches`], `hay` étant déjà en minuscules.
    pub fn matches_lowercase(&self, hay: &str) -> bool {
        self.words.iter().all(|w| {
            hay.match_indices(w.as_str()).any(|(i, _)| {
                hay[..i].chars().next_back().is_none_or(|c| !(c.is_ascii_digit() || c.is_ascii_lowercase()))
            })
        })
    }
}

/// Texte de recherche d'une entrée (nom, source, banc), en minuscules.
pub fn search_text(e: &HeadphoneEntry) -> String {
    format!("{} {} {}", e.name, e.source, e.rig).to_lowercase()
}

/// Recherche insensible à la casse : chaque mot doit commencer un mot de l'entrée.
pub fn search_headphones<'a>(
    entries: impl IntoIterator<Item = &'a HeadphoneEntry>,
    query: &str,
    limit: Option<usize>,
) -> Vec<HeadphoneEntry> {
    let q = Query::new(query);
    let mut out = Vec::new();
    for e in entries {
        if q.matches_lowercase(&search_text(e)) {
            out.push(e.clone());
            if limit.is_some_and(|l| out.len() >= l) {
                break;
            }
        }
    }
    out
}

/// Caractères laissés tels quels dans un chemin d'URL (comme `quote(path, safe="/()")`).
const PATH_SAFE: &AsciiSet =
    &NON_ALPHANUMERIC.remove(b'_').remove(b'.').remove(b'-').remove(b'~').remove(b'/').remove(b'(').remove(b')');

pub fn quote_path(path: &str) -> String {
    utf8_percent_encode(path, PATH_SAFE).to_string()
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------
pub struct HttpResponse {
    pub status: u16,
    pub content_length: Option<u64>,
    pub body: Box<dyn Read + Send>,
}

/// Client HTTP minimal (remplaçable dans les tests).
pub trait HttpClient: Send + Sync {
    /// Erreur = problème réseau (nouvelle tentative) ; un statut HTTP n'est pas une erreur.
    fn get(&self, url: &str, timeout: Duration) -> Result<HttpResponse, String>;
}

pub struct UreqClient;

impl HttpClient for UreqClient {
    fn get(&self, url: &str, timeout: Duration) -> Result<HttpResponse, String> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .user_agent(user_agent())
            .build()
            .into();
        let resp = agent.get(url).call().map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let content_length = resp
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse().ok());
        Ok(HttpResponse { status, content_length, body: Box::new(resp.into_body().into_reader()) })
    }
}

/// Téléchargements atomiques avec cache et nouvelles tentatives.
#[derive(Clone)]
pub struct Downloader {
    pub cache_dir: PathBuf,
    pub client: Arc<dyn HttpClient>,
    pub retries: usize,
    pub timeout: Duration,
    /// Fichiers SOFA du système essayés avant de télécharger KEMAR.
    pub system_sofa_paths: Vec<PathBuf>,
    /// Pause entre deux tentatives (base, doublée à chaque échec).
    pub retry_delay: Duration,
}

impl Downloader {
    pub fn new(cache_dir: PathBuf) -> Self {
        Self::with_client(cache_dir, Arc::new(UreqClient))
    }

    pub fn with_client(cache_dir: PathBuf, client: Arc<dyn HttpClient>) -> Self {
        Downloader {
            cache_dir,
            client,
            retries: 3,
            timeout: Duration::from_secs(30),
            system_sofa_paths: SYSTEM_SOFA_PATHS.iter().map(PathBuf::from).collect(),
            retry_delay: Duration::from_millis(500),
        }
    }

    pub fn autoeq_dir(&self) -> PathBuf {
        self.cache_dir.join("autoeq").join(&AUTOEQ_COMMIT[..12])
    }

    pub fn spinorama_dir(&self) -> PathBuf {
        self.cache_dir.join("spinorama").join(&SPINORAMA_COMMIT[..12])
    }

    /// Télécharge `url` vers `dest` sauf si le fichier existe déjà.
    pub fn fetch(&self, url: &str, dest: &Path, progress: Option<ProgressCallback>) -> Result<PathBuf, DownloadError> {
        if fs::metadata(dest).is_ok_and(|m| m.is_file() && m.len() > 0) {
            return Ok(dest.to_path_buf());
        }
        let dir = dest.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(dir).map_err(|e| DownloadError(format!("{}: {e}", dir.display())))?;
        let tries = self.retries.max(1);
        let mut last = String::new();
        for attempt in 0..tries {
            match self.fetch_once(url, dest, progress) {
                Ok(()) => return Ok(dest.to_path_buf()),
                Err(FetchError::Final(e)) => return Err(e), // 404 & co : inutile de réessayer
                Err(FetchError::Transient(e)) => {
                    last = e;
                    if attempt + 1 < tries {
                        std::thread::sleep(self.retry_delay * (1u32 << attempt.min(3)).min(5));
                    }
                }
            }
        }
        Err(DownloadError(format!("Échec du téléchargement de {url} : {last}")))
    }

    fn fetch_once(&self, url: &str, dest: &Path, progress: Option<ProgressCallback>) -> Result<(), FetchError> {
        let mut resp = self.client.get(url, self.timeout).map_err(FetchError::Transient)?;
        if resp.status == 404 {
            return Err(FetchError::Final(DownloadError(format!("Introuvable (404) : {url}"))));
        }
        if resp.status >= 400 {
            return Err(FetchError::Final(DownloadError(format!("HTTP {} : {url}", resp.status))));
        }
        let dir = dest.parent().unwrap_or(Path::new("."));
        let mut tmp = tempfile::Builder::new()
            .prefix(".part-")
            .tempfile_in(dir)
            .map_err(|e| FetchError::Transient(e.to_string()))?;
        let mut received = 0u64;
        let mut buf = vec![0u8; 65536];
        loop {
            let n = resp.body.read(&mut buf).map_err(|e| FetchError::Transient(e.to_string()))?;
            if n == 0 {
                break;
            }
            tmp.write_all(&buf[..n]).map_err(|e| FetchError::Transient(e.to_string()))?;
            received += n as u64;
            if let Some(p) = progress {
                p(received, resp.content_length);
            }
        }
        if received == 0 {
            return Err(FetchError::Final(DownloadError(format!("Réponse vide : {url}"))));
        }
        tmp.persist(dest).map_err(|e| FetchError::Transient(e.error.to_string()))?;
        Ok(())
    }

    // -- AutoEq ---------------------------------------------------------
    pub fn autoeq_url(&self, rel_path: &str) -> String {
        format!("{AUTOEQ_RAW}/{AUTOEQ_COMMIT}/results/{}", quote_path(rel_path))
    }

    pub fn ensure_autoeq_index(&self, progress: Option<ProgressCallback>) -> Result<PathBuf, DownloadError> {
        self.fetch(&self.autoeq_url("INDEX.md"), &self.autoeq_dir().join("INDEX.md"), progress)
    }

    pub fn load_headphones(&self, progress: Option<ProgressCallback>) -> Result<Vec<HeadphoneEntry>, DownloadError> {
        let path = self.ensure_autoeq_index(progress)?;
        let text = fs::read_to_string(&path).map_err(|e| DownloadError(format!("{}: {e}", path.display())))?;
        Ok(parse_autoeq_index(&text))
    }

    /// Renvoie le fichier d'EQ du casque (ParametricEQ, sinon GraphicEQ).
    pub fn ensure_headphone_eq(
        &self,
        entry: &HeadphoneEntry,
        progress: Option<ProgressCallback>,
    ) -> Result<PathBuf, DownloadError> {
        let leaf = entry.path.trim_end_matches('/').rsplit('/').next().unwrap_or_default();
        let mut errors = Vec::new();
        for suffix in ["ParametricEQ.txt", "GraphicEQ.txt"] {
            let rel = format!("{}/{leaf} {suffix}", entry.path);
            match self.fetch(&self.autoeq_url(&rel), &self.autoeq_dir().join(&rel), progress) {
                Ok(p) => return Ok(p),
                Err(e) => errors.push(e.0),
            }
        }
        Err(DownloadError(format!("Aucune correction AutoEq pour « {} » :\n{}", entry.name, errors.join("\n"))))
    }

    /// Courbe cible AutoEq (`targets/<nom>.csv`, commit figé).
    pub fn ensure_autoeq_target(&self, name: &str, progress: Option<ProgressCallback>) -> Result<PathBuf, DownloadError> {
        let rel = format!("targets/{name}.csv");
        let url = format!("{AUTOEQ_RAW}/{AUTOEQ_COMMIT}/{}", quote_path(&rel));
        self.fetch(&url, &self.autoeq_dir().join(&rel), progress)
    }

    // -- Spinorama ------------------------------------------------------
    pub fn spinorama_url(&self, rel_path: &str) -> String {
        format!("{SPINORAMA_RAW}/{SPINORAMA_COMMIT}/{}", quote_path(rel_path))
    }

    /// Télécharge la mesure d'un haut-parleur du catalogue (format Klippel).
    ///
    /// `CEA2034.txt` → chemin du fichier ; mesures polaires seules
    /// (`format: polar`) → dossier contenant `SPL Horizontal/Vertical.txt`,
    /// dont le CEA-2034 est calculé localement.
    pub fn ensure_speaker(&self, speaker: &Speaker, progress: Option<ProgressCallback>) -> Result<PathBuf, DownloadError> {
        let folder = self.spinorama_dir().join(&speaker.id);
        if speaker.is_polar() {
            for name in ["SPL Horizontal.txt", "SPL Vertical.txt"] {
                self.fetch(&self.spinorama_url(&format!("{}/{name}", speaker.path)), &folder.join(name), progress)?;
            }
            return Ok(folder);
        }
        self.fetch(&self.spinorama_url(&speaker.path), &folder.join("CEA2034.txt"), progress)
    }

    // -- HRTF -----------------------------------------------------------
    /// Chemin d'un fichier SOFA KEMAR : celui choisi, celui du système, ou téléchargé.
    pub fn ensure_hrtf(&self, preferred: &str, progress: Option<ProgressCallback>) -> Result<PathBuf, DownloadError> {
        if !preferred.is_empty() {
            let p = expand_user(preferred);
            if p.is_file() {
                return Ok(p);
            }
        }
        if let Some(p) = self.system_sofa_paths.iter().find(|p| p.is_file()) {
            return Ok(p.clone());
        }
        self.fetch(KEMAR_URL, &self.cache_dir.join("hrtf").join("mit_kemar_normal_pinna.sofa"), progress)
    }

    pub fn cache_size(&self) -> u64 {
        fn walk(p: &Path) -> u64 {
            let Ok(entries) = fs::read_dir(p) else { return 0 };
            entries
                .flatten()
                .map(|e| match e.file_type() {
                    Ok(t) if t.is_dir() => walk(&e.path()),
                    Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
                    _ => 0,
                })
                .sum()
        }
        walk(&self.cache_dir)
    }
}

enum FetchError {
    Final(DownloadError),
    Transient(String),
}

/// `~/…` → dossier personnel.
pub fn expand_user(path: &str) -> PathBuf {
    match path.strip_prefix("~") {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', '\\']) => {
            crate::config::home_dir().join(rest.trim_start_matches(['/', '\\']))
        }
        _ => PathBuf::from(path),
    }
}
