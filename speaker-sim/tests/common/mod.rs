//! Outils communs aux tests : chemins temporaires, HRTF factice, exécuteur et client HTTP simulés.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use speaker_sim::audio_engine::{Hrir, HrtfProvider};
use speaker_sim::config::Paths;
use speaker_sim::download::{HttpClient, HttpResponse};
use speaker_sim::dsp::geomspace;
use speaker_sim::system::{CmdOutput, Runner, SystemError};

pub const CEA_SAMPLE: &str = "\"CEA2034\"\n\
\"On Axis\"\t\t\"Listening Window\"\t\t\"Early Reflections\"\t\t\"Sound Power\"\t\t\"Sound Power DI\"\n\
\"Frequency [Hz]\"\t\"SPL\"\t\"Frequency [Hz]\"\t\"SPL\"\t\"Frequency [Hz]\"\t\"SPL\"\t\"Frequency [Hz]\"\t\"SPL\"\t\"Frequency [Hz]\"\t\"SPL\"\n";

/// Nombre avec séparateur de milliers « , » et deux décimales (« 20,000.00 »).
fn thousands(f: f64) -> String {
    let s = format!("{f:.2}");
    let (int, frac) = s.split_once('.').unwrap();
    let mut out = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    format!("{out}.{frac}")
}

/// Mesure synthétique : LW plate à 86 dB, ER −2 dB, SP −6 dB, avec milliers « 1,000 ».
pub fn make_cea_text(n: usize) -> String {
    let rows: Vec<String> = geomspace(20.0, 20000.0, n)
        .into_iter()
        .map(|f| {
            let fs = thousands(f);
            [fs.as_str(), "86.5", &fs, "86.0", &fs, "84.0", &fs, "80.0", &fs, "6.0"].join("\t")
        })
        .collect();
    format!("{CEA_SAMPLE}{}\n", rows.join("\n"))
}

/// Fichier polaire Klippel synthétique : niveau qui baisse avec l'angle (« beam » dB/10°).
pub fn make_polar_text(plane: &str, beam: f64) -> String {
    let mut angles = vec![0i32];
    for k in (10..180).step_by(10) {
        angles.push(k);
        angles.push(-k);
    }
    angles.push(180);
    let names: Vec<String> =
        angles.iter().map(|&a| if a == 0 { "\"On-Axis\"".into() } else { format!("\"{a}°\"") }).collect();
    let rows: Vec<String> = geomspace(20.0, 20000.0, 40)
        .into_iter()
        .map(|f| {
            let mut cells = Vec::new();
            for &a in &angles {
                cells.push(format!("{f:.4}"));
                cells.push(format!("{:.3}", 86.0 - beam * f64::from(a.abs()) / 10.0));
            }
            cells.join("\t")
        })
        .collect();
    format!("\"SPL {plane}\"\n{}\n\"Frequency\"\t\"SPL\"\n{}\n", names.join("\t\t"), rows.join("\n"))
}

/// Dossier temporaire et chemins d'état / de cache à l'intérieur.
pub fn temp_paths() -> (tempfile::TempDir, Paths) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let paths = Paths {
        state_dir: root.join("state").join("devfrp-speaker-sim"),
        cache_dir: root.join("cache").join("devfrp-speaker-sim"),
    };
    paths.ensure().unwrap();
    (tmp, paths)
}

/// HRTF minimal : impulsion retardée selon l'azimut, oreille opposée atténuée.
pub struct FakeHrtf {
    pub fs: u32,
    pub length: usize,
}

impl FakeHrtf {
    pub fn new(fs: u32) -> Self {
        FakeHrtf { fs, length: 64 }
    }
}

impl HrtfProvider for FakeHrtf {
    fn fs(&self) -> u32 {
        self.fs
    }

    fn hrir(&self, d: [f64; 3]) -> Hrir {
        let az = d[1].atan2(d[0]);
        let itd = 0.0003 * az.sin(); // > 0 : source à gauche
        let fs = f64::from(self.fs);
        let mut left = vec![0.0; self.length];
        let mut right = vec![0.0; self.length];
        left[8 + ((-itd * fs).round().max(0.0) as usize)] = if d[1] >= 0.0 { 1.0 } else { 0.5 };
        right[8 + ((itd * fs).round().max(0.0) as usize)] = if d[1] <= 0.0 { 1.0 } else { 0.5 };
        Hrir { left, right, delay_left: 0.0, delay_right: 0.0 }
    }
}

/// Enregistre les commandes et renvoie des réponses programmées (par préfixe).
#[derive(Clone, Default)]
pub struct FakeRunner {
    pub calls: Arc<Mutex<Vec<Vec<String>>>>,
    pub responses: Arc<Mutex<Vec<(String, i32, String)>>>,
}

impl FakeRunner {
    pub fn new(responses: &[(&str, i32, &str)]) -> Self {
        let r = FakeRunner::default();
        for (p, c, o) in responses {
            r.respond(p, *c, o);
        }
        r
    }

    /// Ajoute (ou remplace) la réponse à un préfixe de commande.
    pub fn respond(&self, prefix: &str, code: i32, out: &str) {
        let mut rs = self.responses.lock().unwrap();
        rs.retain(|(p, _, _)| p != prefix);
        rs.push((prefix.into(), code, out.into()));
    }

    pub fn runner(&self) -> Runner {
        let me = self.clone();
        Arc::new(move |cmd: &[String]| -> Result<CmdOutput, SystemError> {
            me.calls.lock().unwrap().push(cmd.to_vec());
            let key = cmd.join(" ");
            for (prefix, code, out) in me.responses.lock().unwrap().iter() {
                if key.starts_with(prefix.as_str()) {
                    return Ok(CmdOutput { code: *code, stdout: out.clone(), stderr: String::new() });
                }
            }
            Ok(CmdOutput { code: 0, stdout: String::new(), stderr: String::new() })
        })
    }

    pub fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().unwrap().clone()
    }

    pub fn joined(&self) -> Vec<String> {
        self.calls().iter().map(|c| c.join(" ")).collect()
    }
}

/// Réponse d'une route du client HTTP simulé.
#[derive(Clone)]
pub enum Route {
    Body(Vec<u8>),
    NetworkError(String),
}

/// Client HTTP simulé : route par suffixe d'URL, 404 sinon.
#[derive(Clone, Default)]
pub struct FakeHttp {
    pub routes: Arc<Mutex<BTreeMap<String, Route>>>,
    pub urls: Arc<Mutex<Vec<String>>>,
}

impl FakeHttp {
    pub fn new(routes: &[(&str, &[u8])]) -> Self {
        let h = FakeHttp::default();
        for (s, b) in routes {
            h.route(s, b);
        }
        h
    }

    pub fn route(&self, suffix: &str, body: &[u8]) {
        self.routes.lock().unwrap().insert(suffix.into(), Route::Body(body.to_vec()));
    }

    pub fn fail(&self, suffix: &str, message: &str) {
        self.routes.lock().unwrap().insert(suffix.into(), Route::NetworkError(message.into()));
    }

    pub fn urls(&self) -> Vec<String> {
        self.urls.lock().unwrap().clone()
    }
}

impl HttpClient for FakeHttp {
    fn get(&self, url: &str, _timeout: Duration) -> Result<HttpResponse, String> {
        self.urls.lock().unwrap().push(url.into());
        for (suffix, route) in self.routes.lock().unwrap().iter() {
            if url.ends_with(suffix.as_str()) {
                return match route {
                    Route::NetworkError(e) => Err(e.clone()),
                    Route::Body(b) => Ok(HttpResponse {
                        status: 200,
                        content_length: Some(b.len() as u64),
                        body: Box::new(Cursor::new(b.clone())),
                    }),
                };
            }
        }
        Ok(HttpResponse { status: 404, content_length: Some(0), body: Box::new(Cursor::new(Vec::new())) })
    }
}
