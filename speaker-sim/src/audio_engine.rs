//! Moteur audio : construction des filtres de simulation de haut-parleurs.
//!
//! Chaîne simulée, pour chaque haut-parleur virtuel *s* (gauche/droite) et chaque
//! oreille *e* :
//!
//! ```text
//! IR[s][e] = Casque⁻¹ · ( DF⁻¹ · ( LW · Direct[s][e] + ER · Réflexions[s][e] )
//!                          + SP · Réverbération[s][e] )
//! ```
//!
//! * `Direct` / `Réflexions` : HRIR KEMAR (lecteur SOFA `sofar`) pour la source et ses
//!   sources-images (modèle de pièce parallélépipédique d'Allen & Berkley), avec
//!   retards fractionnaires exacts (appliqués dans le domaine fréquentiel).
//! * `LW`, `ER`, `SP` : courbes CEA-2034 du haut-parleur (Listening Window,
//!   Early Reflections, Sound Power) converties en FIR à phase minimale.
//! * `Réverbération` : bruit décorrélé à décroissance exponentielle (RT60 par
//!   bandes), énergie fixée par la distance critique de la pièce.
//! * `DF⁻¹` : égalisation champ diffus du HRTF (AutoEq vise une cible proche
//!   du champ diffus, on évite ainsi de compter deux fois la résonance du pavillon).
//! * `Casque⁻¹` : correction AutoEq (EQ paramétrique → IIR → réponse impulsionnelle,
//!   phase minimale par construction).
//!
//! Les quatre IR (LL, LR, RL, RR) sont normalisées pour éviter l'écrêtage puis
//! écrites en WAV float32 mono, lues par le convolveur de PipeWire.
//! Ce module ne dépend ni de l'interface ni de PipeWire.

use std::collections::BTreeMap;
use std::f64::consts::PI;
use std::fmt;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::config::read_text_lossy;
use crate::dsp::{
    self, Band, Biquad, BiquadKind, C64, Rng, band_mean, fade_out, interp, interp_all, interp_log, irfft,
    minimum_phase_fir, minimum_phase_spectrum, next_pow2, rfft, rfftfreq, smooth_octave,
};

pub const SPEED_OF_SOUND: f64 = 343.0;
pub const EAR_HEIGHT: f64 = 1.2;
/// Haut-parleur → oreille.
pub const PAIRS: [&str; 4] = ["LL", "LR", "RL", "RR"];
pub const FIR_TAPS: usize = 8192;
pub const HEADROOM_DB: f64 = 1.5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioEngineError(pub String);

impl fmt::Display for AudioEngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AudioEngineError {}

pub type Result<T> = std::result::Result<T, AudioEngineError>;

fn err<T>(msg: impl Into<String>) -> Result<T> {
    Err(AudioEngineError(msg.into()))
}

/// Découpe en lignes comme `str.splitlines()` (`\n`, `\r\n` ou `\r`).
pub fn split_lines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        match rest.find(['\n', '\r']) {
            Some(i) => {
                out.push(&rest[..i]);
                let skip = if rest[i..].starts_with("\r\n") { 2 } else { 1 };
                rest = &rest[i + skip..];
            }
            None => {
                out.push(rest);
                break;
            }
        }
    }
    out
}

/// Nombre comme `float()` de Python (espaces autour tolérés).
fn parse_float(token: &str) -> Option<f64> {
    token.trim().replace('_', "").parse::<f64>().ok()
}

// ---------------------------------------------------------------------------
// Égaliseurs AutoEq
// ---------------------------------------------------------------------------
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HeadphoneEq {
    pub preamp_db: f64,
    pub filters: Vec<Biquad>,
    /// EQ graphique : (fréquences, gains dB).
    pub graphic: Option<(Vec<f64>, Vec<f64>)>,
}

const FILTER_PATTERN: &str = r"Filter\s+\d+\s*:\s*ON\s+(?P<kind>[A-Za-z]+)\s+Fc\s+(?P<fc>[\d.]+)\s*Hz(?:\s+Gain\s+(?P<gain>[-+\d.]+)\s*dB)?(?:\s+Q\s+(?P<q>[\d.]+))?";
static FILTER_AT_START: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!("(?i)^{FILTER_PATTERN}")).expect("motif"));
static FILTER_ANYWHERE: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!("(?i){FILTER_PATTERN}")).expect("motif"));
static PREAMP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^Preamp\s*:\s*([-+\d.]+)\s*dB").expect("motif"));
static PREAMP_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?im)^\s*Preamp\s*:").expect("motif"));
static GRAPHIC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)GraphicEQ\s*:\s*(.+)").expect("motif"));
static GRAPHIC_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)GraphicEQ\s*:").expect("motif"));

/// Analyse un fichier `ParametricEQ.txt` (format Equalizer APO).
pub fn parse_parametric_eq(text: &str) -> HeadphoneEq {
    let mut eq = HeadphoneEq::default();
    for line in split_lines(text) {
        let line = line.trim();
        if let Some(c) = PREAMP.captures(line) {
            if let Some(v) = parse_float(&c[1]) {
                eq.preamp_db = v;
            }
            continue;
        }
        let Some(c) = FILTER_AT_START.captures(line) else { continue };
        let Some(kind) = BiquadKind::from_apo(&c["kind"]) else { continue };
        let Some(fc) = parse_float(&c["fc"]) else { continue };
        let gain = c.name("gain").and_then(|m| parse_float(m.as_str())).unwrap_or(0.0);
        #[allow(clippy::approx_constant)] // Q par défaut des fichiers Equalizer APO (0.7071, et non 1/√2 exact)
        let q = c.name("q").and_then(|m| parse_float(m.as_str())).unwrap_or(0.7071);
        eq.filters.push(Biquad::new(kind, fc, gain, q));
    }
    eq
}

/// Analyse `GraphicEQ: f1 g1; f2 g2; …`.
pub fn parse_graphic_eq(text: &str) -> Result<HeadphoneEq> {
    let Some(c) = GRAPHIC.captures(text) else { return err("Fichier GraphicEQ invalide") };
    let mut f = Vec::new();
    let mut g = Vec::new();
    for item in c[1].split(';') {
        let bits: Vec<&str> = item.split_whitespace().collect();
        if bits.len() >= 2 {
            if let (Some(a), Some(b)) = (parse_float(bits[0]), parse_float(bits[1])) {
                f.push(a);
                g.push(b);
            }
        }
    }
    if f.len() < 2 {
        return err("GraphicEQ : pas assez de points");
    }
    Ok(HeadphoneEq { graphic: Some((f, g)), ..Default::default() })
}

pub fn load_headphone_eq(path: &Path) -> Result<HeadphoneEq> {
    let text = read_text_lossy(path).map_err(|e| AudioEngineError(format!("Lecture impossible : {e}")))?;
    if text.contains("GraphicEQ") {
        return parse_graphic_eq(&text);
    }
    let eq = parse_parametric_eq(&text);
    if eq.filters.is_empty() {
        return err(format!("Aucun filtre reconnu dans {}", path.display()));
    }
    Ok(eq)
}

// ---------------------------------------------------------------------------
// Mesures de casque importées par l'utilisateur
// ---------------------------------------------------------------------------
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeadphoneFileKind {
    /// Equalizer APO / AutoEq.
    Parametric,
    /// GraphicEQ.
    Graphic,
    /// Réponse en fréquence brute.
    Measurement,
}

pub fn detect_headphone_file_kind(text: &str) -> HeadphoneFileKind {
    if GRAPHIC_TAG.is_match(text) {
        HeadphoneFileKind::Graphic
    } else if FILTER_ANYWHERE.is_match(text) || PREAMP_LINE.is_match(text) {
        HeadphoneFileKind::Parametric
    } else {
        HeadphoneFileKind::Measurement
    }
}

static FIELD_SPLIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[,;\t ]+").expect("motif"));
static NUMERIC_START: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[-+]?\d").expect("motif"));

/// Courbe (fréquences croissantes, dB).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Curve {
    pub f: Vec<f64>,
    pub db: Vec<f64>,
}

impl Curve {
    pub fn new(f: Vec<f64>, db: Vec<f64>) -> Self {
        Curve { f, db }
    }
}

/// Réponse en fréquence brute (fréquence, dB) : export REW, Squiglink, CSV AutoEq…
///
/// Lignes de commentaires et en-têtes ignorées ; séparateurs virgule, point-virgule,
/// tabulation ou espaces. Avec un en-tête AutoEq, la colonne `raw` est utilisée.
pub fn parse_measurement(text: &str) -> Result<Curve> {
    let mut column = 1;
    let mut points: Vec<(f64, f64)> = Vec::new();
    for raw in split_lines(text) {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(['*', '#', ';', '/']) {
            continue;
        }
        let fields: Vec<&str> = FIELD_SPLIT.split(line).filter(|f| !f.is_empty()).collect();
        if let Some(first) = fields.first() {
            if !NUMERIC_START.is_match(first) {
                let names: Vec<String> = fields.iter().map(|f| f.trim().trim_matches('"').to_lowercase()).collect();
                if let Some(i) = names.iter().position(|n| n == "raw") {
                    column = i;
                }
                continue;
            }
        }
        let (Some(f), Some(v)) = (
            fields.first().and_then(|t| parse_float(t)),
            fields.get(column).and_then(|t| parse_float(t)),
        ) else {
            continue;
        };
        if f > 0.0 && v.is_finite() {
            points.push((f, v));
        }
    }
    if points.len() < 10 {
        return err("Mesure illisible : il faut au moins 10 lignes « fréquence, dB »");
    }
    points.sort_by(|a, b| a.0.total_cmp(&b.0));
    points.dedup_by(|b, a| b.0 <= a.0);
    let f: Vec<f64> = points.iter().map(|p| p.0).collect();
    let v: Vec<f64> = points.iter().map(|p| p.1).collect();
    if f[0] > 200.0 || f[f.len() - 1] < 5000.0 {
        return err("Mesure incomplète : elle doit couvrir au moins 200 Hz – 5 kHz");
    }
    Ok(Curve::new(f, v))
}

/// Correction (EQ graphique) amenant `measurement` sur la courbe cible `target`.
///
/// Erreur normalisée sur 200 Hz – 2 kHz, lissée au 1/12 d'octave puis au 1/3
/// d'octave au-dessus de 5–10 kHz (zone dépendant du placement), bornée.
pub fn measurement_eq(measurement: &Curve, target: &Curve, max_boost_db: f64, max_cut_db: f64) -> HeadphoneEq {
    let grid = dsp::geomspace(20.0, 20000.0, 480); // ≈ 48 points par octave
    let m = interp_log(&measurement.f, &measurement.db, &grid);
    let t = interp_log(&target.f, &target.db, &grid);
    let mut e: Vec<f64> = m.iter().zip(&t).map(|(a, b)| a - b).collect();
    let offset = band_mean(&grid, &e, 200.0, 2000.0).unwrap_or(0.0);
    e.iter_mut().for_each(|v| *v -= offset);
    let fine = dsp::uniform_filter_nearest(&e, 4); // 1/12 d'octave
    let coarse = dsp::uniform_filter_nearest(&e, 16); // 1/3 d'octave
    let (l5, l10) = (5000f64.ln(), 10000f64.ln());
    let eq = grid
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let w = ((g.ln() - l5) / (l10 - l5)).clamp(0.0, 1.0);
            (-((1.0 - w) * fine[i] + w * coarse[i])).clamp(max_cut_db, max_boost_db)
        })
        .collect();
    HeadphoneEq { graphic: Some((grid, eq)), ..Default::default() }
}

/// Réponse impulsionnelle (phase minimale) de la correction du casque.
///
/// Le préampli AutoEq n'est pas inclus par défaut : la normalisation globale
/// de [`render`] gère déjà la marge anti-écrêtage.
pub fn headphone_eq_fir(eq: &HeadphoneEq, fs: f64, n_taps: usize, include_preamp: bool) -> Vec<f64> {
    if let Some((f, g)) = &eq.graphic {
        return minimum_phase_fir(f, g, fs, n_taps);
    }
    let sos: Vec<dsp::Sos> = eq
        .filters
        .iter()
        .filter_map(|bq| bq.coefficients(fs))
        .map(|(b, a)| [b[0], b[1], b[2], a[0], a[1], a[2]])
        .collect();
    let mut impulse = vec![0.0; n_taps];
    impulse[0] = if include_preamp { 10f64.powf(eq.preamp_db / 20.0) } else { 1.0 };
    let h = if sos.is_empty() { impulse } else { dsp::sosfilt(&sos, &impulse, None) };
    fade_out(&h, 0.2)
}

/// Réponse en amplitude (dB) de l'EQ du casque aux fréquences données.
pub fn eq_response_db(eq: &HeadphoneEq, freqs: &[f64], fs: f64) -> Vec<f64> {
    if let Some((f, g)) = &eq.graphic {
        let lf: Vec<f64> = f.iter().map(|v| v.ln()).collect();
        return freqs.iter().map(|x| interp(x.ln(), &lf, g)).collect();
    }
    let coeffs: Vec<_> = eq.filters.iter().filter_map(|bq| bq.coefficients(fs)).collect();
    freqs.iter().map(|&f| coeffs.iter().map(|(b, a)| dsp::gain_db(b, a, f, fs)).sum()).collect()
}

// ---------------------------------------------------------------------------
// Mesures de haut-parleurs (CEA-2034, format Klippel exporté par Spinorama)
// ---------------------------------------------------------------------------
static THOUSANDS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^-?\d{1,3}(,\d{3})+$").expect("motif"));
static QUOTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""([^"]*)""#).expect("motif"));
static INTEGER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-?\d+").expect("motif"));

/// Nombre Klippel : `1,034.91` (séparateur de milliers) ou `12,5` (virgule décimale).
fn parse_number(token: &str) -> Option<f64> {
    let tok = token.trim().trim_matches('"');
    let tok = if tok.contains(',') {
        if tok.contains('.') || THOUSANDS.is_match(tok) { tok.replace(',', "") } else { tok.replace(',', ".") }
    } else {
        tok.to_string()
    };
    parse_float(&tok)
}

fn split_fields(line: &str) -> Vec<&str> {
    if line.contains('\t') { line.split('\t').collect() } else { line.split_whitespace().collect() }
}

pub type Curves = BTreeMap<String, Curve>;

/// Renvoie `{nom_courbe: (fréquences, dB)}`.
pub fn parse_cea2034(text: &str) -> Result<Curves> {
    let lines: Vec<&str> = split_lines(text).into_iter().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() < 4 {
        return err("Fichier CEA-2034 trop court");
    }
    let names: Vec<String> = QUOTED.captures_iter(lines[1]).map(|c| c[1].to_string()).collect();
    if names.is_empty() {
        return err("En-têtes CEA-2034 introuvables");
    }
    // Colonnes positionnelles (séparées par tabulations) : certaines courbes
    // (DI…) s'arrêtent avant les autres et laissent des champs vides.
    let ncol = 2 * names.len();
    let mut columns: Vec<Vec<f64>> = vec![Vec::new(); ncol];
    for line in &lines[3..] {
        let fields = split_fields(line);
        for (i, col) in columns.iter_mut().enumerate() {
            col.push(fields.get(i).and_then(|t| parse_number(t)).unwrap_or(f64::NAN));
        }
    }
    let mut curves = Curves::new();
    for (i, name) in names.iter().enumerate() {
        let mut pts: Vec<(f64, f64)> = columns[2 * i]
            .iter()
            .zip(&columns[2 * i + 1])
            .filter(|(f, d)| f.is_finite() && d.is_finite() && **f > 0.0)
            .map(|(f, d)| (*f, *d))
            .collect();
        if pts.len() >= 2 {
            pts.sort_by(|a, b| a.0.total_cmp(&b.0));
            curves.insert(name.clone(), Curve::new(pts.iter().map(|p| p.0).collect(), pts.iter().map(|p| p.1).collect()));
        }
    }
    if !curves.contains_key("Listening Window") {
        return err("Courbe « Listening Window » absente de la mesure");
    }
    Ok(curves)
}

/// Pondérations CEA-2034 de la puissance acoustique, par angle (0°…180°, pas de 10°).
const SP_WEIGHTS: [f64; 19] = [
    0.000604486, 0.004730189, 0.008955027, 0.012387354, 0.014989611, 0.016868154, 0.018165946,
    0.019006236, 0.019477078, 0.019629421, 0.019477078, 0.019006236, 0.018165946, 0.016868154,
    0.014989611, 0.012387354, 0.008955027, 0.004730189, 0.000604486,
];

/// Mesures polaires : fréquences et niveaux par angle (ordre du fichier).
#[derive(Clone, Debug, Default)]
pub struct Polar {
    pub freqs: Vec<f64>,
    pub angles: Vec<(i32, Vec<f64>)>,
}

/// `SPL Horizontal.txt` / `SPL Vertical.txt` (Klippel) → (fréquences, {angle: dB}).
pub fn parse_klippel_polar(text: &str) -> Result<Polar> {
    let lines: Vec<&str> = split_lines(text).into_iter().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() < 4 {
        return err("Fichier polaire trop court");
    }
    let mut angles = Vec::new();
    for c in QUOTED.captures_iter(lines[1]) {
        let name = c[1].to_lowercase();
        let angle = if name.contains("on") && name.contains("axis") {
            Some(0)
        } else {
            INTEGER.find(&c[1]).and_then(|m| m.as_str().parse::<i32>().ok())
        };
        match angle {
            Some(a) => angles.push(a),
            None => return err("Angles illisibles dans le fichier polaire"),
        }
    }
    if angles.is_empty() {
        return err("Angles illisibles dans le fichier polaire");
    }
    let width = 2 * angles.len();
    let mut rows: Vec<Vec<f64>> = Vec::new();
    for line in &lines[3..] {
        let fields = split_fields(line);
        let values: Option<Vec<f64>> = fields.iter().take(width).map(|t| parse_number(t)).collect();
        if let Some(values) = values {
            if values.len() == width {
                rows.push(values);
            }
        }
    }
    if rows.len() < 10 {
        return err("Pas assez de points dans le fichier polaire");
    }
    Ok(Polar {
        freqs: rows.iter().map(|r| r[0]).collect(),
        angles: angles.iter().enumerate().map(|(i, &a)| (a, rows.iter().map(|r| r[2 * i + 1]).collect())).collect(),
    })
}

fn allclose(a: &[f64], b: &[f64], rtol: f64) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() <= 1e-8 + rtol * y.abs())
}

/// Courbes CEA-2034 (ANSI/CTA-2034-A) calculées depuis les mesures polaires.
///
/// Moyennes spatiales en pression quadratique : Listening Window (0°, ±10° V,
/// ±10/20/30° H), Early Reflections (sol, plafond, murs avant, latéraux et
/// arrière) et Sound Power (70 points pondérés). Identique au calcul Klippel.
pub fn cea2034_from_polar(horizontal: &Polar, vertical: &Polar) -> Result<Curves> {
    let f = &horizontal.freqs;
    let lf: Vec<f64> = f.iter().map(|v| v.ln()).collect();
    let resample = !allclose(&vertical.freqs, f, 1e-3);
    let lfv: Vec<f64> = vertical.freqs.iter().map(|v| v.ln()).collect();
    let plane = |p: &Polar, other: bool| -> BTreeMap<i32, Vec<f64>> {
        p.angles
            .iter()
            .map(|(a, db)| {
                let db = if other && resample { lf.iter().map(|x| interp(*x, &lfv, db)).collect() } else { db.clone() };
                (if *a == -180 { 180 } else { *a }, db)
            })
            .collect()
    };
    let h = plane(horizontal, false);
    let v = plane(vertical, true);
    let pick = |plane: &BTreeMap<i32, Vec<f64>>, angles: &[i32]| -> Vec<Vec<f64>> {
        angles.iter().filter_map(|a| plane.get(a).cloned()).collect()
    };
    let avg = |curves: Vec<Vec<f64>>| -> Result<Vec<f64>> {
        if curves.is_empty() {
            return err("Angles manquants pour le calcul CEA-2034");
        }
        let n = curves.len() as f64;
        Ok((0..f.len())
            .map(|i| 10.0 * (curves.iter().map(|c| 10f64.powf(c[i] / 10.0)).sum::<f64>() / n).log10())
            .collect())
    };
    let lw = avg([pick(&h, &[0, 10, -10, 20, -20, 30, -30]), pick(&v, &[10, -10])].concat())?;
    // Réflexions sol, plafond, murs avant, latéraux et arrière : moyenne en
    // puissance des 26 directions regroupées (convention du logiciel Klippel,
    // vérifiée à l'identique sur le CEA2034.txt du KEF R3).
    let er = avg([
        pick(&v, &[-20, -30, -40]),
        pick(&v, &[40, 50, 60]),
        pick(&h, &[0, 10, -10, 20, -20, 30, -30]),
        pick(&h, &[40, -40, 50, -50, 60, -60, 70, -70, 80, -80]),
        pick(&h, &[90, -90, 180]),
    ]
    .concat())?;
    let mut num = vec![0.0; f.len()];
    let mut den = 0.0;
    for (plane, skip) in [(&h, &[][..]), (&v, &[0, 180][..])] {
        // 0° et 180° comptés une seule fois
        for (a, db) in plane.iter() {
            if skip.contains(a) {
                continue;
            }
            let w = SP_WEIGHTS[((a.unsigned_abs() / 10) as usize).min(18)];
            for (n, d) in num.iter_mut().zip(db) {
                *n += w * 10f64.powf(d / 10.0);
            }
            den += w;
        }
    }
    let sp: Vec<f64> = num.iter().map(|n| 10.0 * (n / den).log10()).collect();
    let Some(on_axis) = h.get(&0) else { return err("Angles manquants pour le calcul CEA-2034") };
    let mut out = Curves::new();
    out.insert("On Axis".into(), Curve::new(f.clone(), on_axis.clone()));
    out.insert("Listening Window".into(), Curve::new(f.clone(), lw));
    out.insert("Early Reflections".into(), Curve::new(f.clone(), er));
    out.insert("Sound Power".into(), Curve::new(f.clone(), sp));
    Ok(out)
}

/// Fichier `CEA2034.txt`, ou dossier contenant `SPL Horizontal/Vertical.txt`.
pub fn load_speaker(path: &Path) -> Result<SpeakerResponse> {
    let read = |p: PathBuf| read_text_lossy(&p).map_err(|e| AudioEngineError(format!("Lecture impossible : {e}")));
    if path.is_dir() {
        let h = parse_klippel_polar(&read(path.join("SPL Horizontal.txt"))?)?;
        let v = parse_klippel_polar(&read(path.join("SPL Vertical.txt"))?)?;
        return SpeakerResponse::from_curves(&cea2034_from_polar(&h, &v)?);
    }
    SpeakerResponse::from_curves(&parse_cea2034(&read(path.to_path_buf())?)?)
}

/// Courbes CEA-2034 normalisées sur la Listening Window (300 Hz – 3 kHz).
#[derive(Clone, Debug)]
pub struct SpeakerResponse {
    pub freqs: Vec<f64>,
    pub listening_window: Vec<f64>,
    pub early_reflections: Vec<f64>,
    pub sound_power: Vec<f64>,
}

fn resample_curve(curve: Option<&Curve>, f: &[f64], fallback: &[f64]) -> Vec<f64> {
    match curve {
        None => fallback.to_vec(),
        Some(c) => {
            let lc: Vec<f64> = c.f.iter().map(|v| v.ln()).collect();
            f.iter().map(|x| interp(x.ln(), &lc, &c.db)).collect()
        }
    }
}

impl SpeakerResponse {
    pub fn from_curves(curves: &Curves) -> Result<Self> {
        let Some(lw) = curves.get("Listening Window") else {
            return err("Courbe « Listening Window » absente de la mesure");
        };
        let f = lw.f.clone();
        let er = resample_curve(curves.get("Early Reflections"), &f, &lw.db);
        let sp = resample_curve(curves.get("Sound Power"), &f, &er);
        let reference = band_mean(&f, &lw.db, 300.0, 3000.0).unwrap_or_else(|| dsp::median(&lw.db));
        let shift = |v: &[f64]| v.iter().map(|x| x - reference).collect::<Vec<_>>();
        Ok(SpeakerResponse {
            listening_window: shift(&lw.db),
            early_reflections: shift(&er),
            sound_power: shift(&sp),
            freqs: f,
        })
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = read_text_lossy(path).map_err(|e| AudioEngineError(format!("Lecture impossible : {e}")))?;
        Self::from_curves(&parse_cea2034(&text)?)
    }

    pub fn directivity_index(&self, lo: f64, hi: f64) -> f64 {
        let di: Vec<f64> = self.listening_window.iter().zip(&self.sound_power).map(|(a, b)| a - b).collect();
        band_mean(&self.freqs, &di, lo, hi).unwrap_or(0.0)
    }
}

// ---------------------------------------------------------------------------
// HRTF
// ---------------------------------------------------------------------------
/// HRIR (gauche, droite) et retards (s) pour une direction.
#[derive(Clone, Debug, Default)]
pub struct Hrir {
    pub left: Vec<f64>,
    pub right: Vec<f64>,
    pub delay_left: f64,
    pub delay_right: f64,
}

pub trait HrtfProvider {
    fn fs(&self) -> u32;
    /// Direction : x avant, y gauche, z haut.
    fn hrir(&self, direction: [f64; 3]) -> Hrir;
}

/// Lecteur SOFA en pur Rust (`sofar`, portage de libmysofa) : interpolation entre
/// mesures voisines, normalisation et rééchantillonnage identiques à libmysofa.
pub struct SofaHrtf {
    sofa: sofar::reader::Sofar,
    fs: u32,
}

impl SofaHrtf {
    pub fn open(path: &Path, fs: u32) -> Result<Self> {
        let sofa = sofar::reader::OpenOptions::new()
            .sample_rate(fs as f32)
            .open(path)
            .map_err(|e| AudioEngineError(format!("HRTF illisible ({}) : {e}", path.display())))?;
        Ok(SofaHrtf { sofa, fs })
    }

    pub fn length(&self) -> usize {
        self.sofa.filter_len()
    }
}

impl HrtfProvider for SofaHrtf {
    fn fs(&self) -> u32 {
        self.fs
    }

    fn hrir(&self, d: [f64; 3]) -> Hrir {
        let mut filter = sofar::reader::Filter::new(self.sofa.filter_len());
        self.sofa.filter(d[0] as f32, d[1] as f32, d[2] as f32, &mut filter);
        Hrir {
            left: filter.left.iter().map(|&v| f64::from(v)).collect(),
            right: filter.right.iter().map(|&v| f64::from(v)).collect(),
            delay_left: f64::from(filter.ldelay),
            delay_right: f64::from(filter.rdelay),
        }
    }
}

pub fn open_hrtf(path: &Path, fs: u32) -> Result<Box<dyn HrtfProvider>> {
    Ok(Box::new(SofaHrtf::open(path, fs)?))
}

/// Directions ~uniformes sur la sphère (sous l'élévation min. exclue : absente de KEMAR).
pub fn fibonacci_sphere(n: usize, min_elevation_deg: f64) -> Vec<[f64; 3]> {
    let golden = PI * (1.0 + 5f64.sqrt());
    (0..n)
        .map(|k| {
            let i = k as f64 + 0.5;
            let z = 1.0 - 2.0 * i / n as f64;
            let r = (1.0 - z * z).sqrt();
            let phi = golden * i;
            [r * phi.cos(), r * phi.sin(), z]
        })
        .filter(|p| p[2].asin().to_degrees() >= min_elevation_deg)
        .collect()
}

/// Réponse champ diffus (moyenne en puissance des deux oreilles), dB, lissée 1/3 oct.
pub fn diffuse_field_db(hrtf: &dyn HrtfProvider, nfft: usize, n_dirs: usize) -> Vec<f64> {
    let mut power = vec![0.0; nfft / 2 + 1];
    let dirs = fibonacci_sphere(n_dirs, -40.0);
    for d in &dirs {
        let h = hrtf.hrir(*d);
        for (p, (l, r)) in power.iter_mut().zip(rfft(&h.left, nfft).iter().zip(rfft(&h.right, nfft).iter())) {
            *p += l.norm_sqr() + r.norm_sqr();
        }
    }
    let scale = 1.0 / (2 * dirs.len()) as f64;
    power.iter_mut().for_each(|p| *p *= scale);
    let freqs = rfftfreq(nfft, f64::from(hrtf.fs()));
    let db: Vec<f64> = smooth_octave(&freqs, &power, 3.0).iter().map(|p| 10.0 * p.max(1e-12).log10()).collect();
    let reference = band_mean(&freqs, &db, 200.0, 2000.0).unwrap_or(0.0);
    db.iter().map(|v| v - reference).collect()
}

// ---------------------------------------------------------------------------
// Pièce
// ---------------------------------------------------------------------------
pub type Bands = [f64; 7];

/// Bandes d'octave du modèle acoustique.
pub const OCTAVES: Bands = [125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0];

/// Coefficients d'absorption par bande d'octave (valeurs usuelles de la littérature).
pub fn material(name: &str) -> Option<Bands> {
    Some(match name {
        "drywall" => [0.29, 0.10, 0.05, 0.04, 0.07, 0.09, 0.09], // plaque de plâtre sur ossature
        "masonry" => [0.02, 0.02, 0.03, 0.04, 0.05, 0.06, 0.06], // enduit sur brique / béton
        "panel" => [0.30, 0.75, 1.00, 1.00, 1.00, 1.00, 0.95],   // panneau laine minérale 10 cm
        "foam5" => [0.08, 0.25, 0.60, 0.90, 0.95, 0.95, 0.95],   // mousse acoustique 5 cm
        "wool20" => [0.65, 0.95, 1.00, 1.00, 1.00, 1.00, 0.95],  // laine 20 cm, large bande
        "tile" => [0.01, 0.01, 0.015, 0.02, 0.02, 0.02, 0.02],   // carrelage / béton
        "wood" => [0.15, 0.11, 0.10, 0.07, 0.06, 0.07, 0.07],    // parquet sur lambourdes
        "carpet" => [0.08, 0.24, 0.57, 0.69, 0.71, 0.73, 0.73],  // moquette épaisse sur thibaude
        _ => return None,
    })
}

/// Aire d'absorption équivalente du mobilier (m²) pour 25 m² de sol.
fn furniture_area(name: &str) -> Bands {
    match name {
        "furnished" => [2.0, 3.0, 4.0, 5.0, 5.0, 5.0, 5.0],
        "full" => [4.0, 6.0, 9.0, 11.0, 11.0, 11.0, 11.0],
        _ => [0.0; 7],
    }
}
/// Pièges à basses d'angle (jeu complet), aire équivalente (m²).
const BASS_TRAPS_AREA: Bands = [6.0, 4.0, 2.0, 1.0, 1.0, 1.0, 1.0];
/// Absorption de l'air, coefficient m (1/m) à 20 °C, 50 % HR.
const AIR_M: Bands = [0.0001, 0.0003, 0.0006, 0.001, 0.0024, 0.0075, 0.024];

/// Surfaces de la pièce, dans l'ordre utilisé par les sources-images :
/// x=0 mur arrière, x=L mur avant (enceintes), y=0 mur droit, y=W mur gauche, sol, plafond.
pub const SURFACES: [&str; 6] = ["rear", "front", "right", "left", "floor", "ceiling"];

fn map7(f: impl Fn(usize) -> f64) -> Bands {
    std::array::from_fn(f)
}

fn mean2(b: &Bands) -> f64 {
    0.5 * (b[2] + b[3])
}

#[derive(Clone, Debug, PartialEq)]
pub struct RoomSpec {
    pub width: f64,
    pub length: f64,
    pub height: f64,
    pub order: usize,
    pub walls: String,
    /// Fraction des murs couverte de panneaux absorbants.
    pub treatment: f64,
    /// 0 = aucun, 1 = jeu complet de pièges d'angle.
    pub bass_traps: f64,
    pub floor: String,
    pub ceiling: String,
    pub furniture: String,
    /// Diffusion du mur arrière (0–1).
    pub diffusion: f64,
    /// Distance enceintes – mur avant (m).
    pub front_gap: f64,
    pub ear_height: f64,
    /// > 0 : RT60 imposé (moyenne 500 Hz–1 kHz).
    pub rt60_override: f64,
    pub absorber: String,
    pub per_wall: bool,
    pub treatment_front: f64,
    pub treatment_sides: f64,
    pub treatment_rear: f64,
    pub treatment_ceiling: f64,
    /// 0 = à hauteur d'oreille.
    pub speaker_height: f64,
    /// Décalage latéral de l'auditeur (m, + = vers la gauche).
    pub listener_offset: f64,
}

impl Default for RoomSpec {
    fn default() -> Self {
        RoomSpec {
            width: 4.5,
            length: 6.0,
            height: 2.6,
            order: 2,
            walls: "masonry".into(),
            treatment: 0.0,
            bass_traps: 0.0,
            floor: "rug".into(),
            ceiling: "plaster".into(),
            furniture: "furnished".into(),
            diffusion: 0.0,
            front_gap: 0.8,
            ear_height: EAR_HEIGHT,
            rt60_override: 0.0,
            absorber: "wool10".into(),
            per_wall: false,
            treatment_front: 0.0,
            treatment_sides: 0.0,
            treatment_rear: 0.0,
            treatment_ceiling: 0.0,
            speaker_height: 0.0,
            listener_offset: 0.0,
        }
    }
}

/// Taux de traitement par paroi (pour l'affichage).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WallTreatment {
    pub front: f64,
    pub rear: f64,
    pub left: f64,
    pub right: f64,
    pub ceiling: f64,
}

impl RoomSpec {
    pub fn sized(width: f64, length: f64, height: f64) -> Self {
        RoomSpec { width, length, height, ..Default::default() }
    }

    pub fn volume(&self) -> f64 {
        self.width * self.length * self.height
    }

    pub fn areas(&self) -> [f64; 6] {
        let (w, l, h) = (self.width, self.length, self.height);
        [w * h, w * h, l * h, l * h, l * w, l * w]
    }

    pub fn surface(&self) -> f64 {
        self.areas().iter().sum()
    }

    /// Coefficients d'absorption (6 surfaces × 7 bandes).
    pub fn surface_absorption(&self) -> [Bands; 6] {
        let m = |name: &str| material(name).expect("matériau connu");
        let bare = if self.walls == "drywall" { m("drywall") } else { m("masonry") };
        let panel = if self.absorber == "wool10" { m("panel") } else { material(&self.absorber).unwrap_or(m("panel")) };
        let mix = |frac: f64| {
            let c = frac.clamp(0.0, 1.0);
            map7(|b| (1.0 - c) * bare[b] + c * panel[b])
        };
        let wall = mix(self.treatment);
        let floor = match self.floor.as_str() {
            "tile" => m("tile"),
            "carpet" => m("carpet"),
            "rug" => map7(|b| 0.5 * m("wood")[b] + 0.5 * m("carpet")[b]),
            _ => m("wood"),
        };
        let ceiling = if self.ceiling == "cloud" { mix(0.5) } else { bare };
        if self.per_wall {
            let sides = mix(self.treatment_sides);
            return [mix(self.treatment_rear), mix(self.treatment_front), sides, sides, floor, mix(self.treatment_ceiling)];
        }
        [wall, wall, wall, wall, floor, ceiling]
    }

    pub fn wall_treatment(&self) -> WallTreatment {
        if self.per_wall {
            WallTreatment {
                front: self.treatment_front,
                rear: self.treatment_rear,
                left: self.treatment_sides,
                right: self.treatment_sides,
                ceiling: self.treatment_ceiling,
            }
        } else {
            let t = self.treatment;
            WallTreatment { front: t, rear: t, left: t, right: t, ceiling: if self.ceiling == "cloud" { 0.5 } else { 0.0 } }
        }
    }

    fn auto_rt60(&self) -> Bands {
        let alpha = self.surface_absorption();
        let areas = self.areas();
        let furn = furniture_area(&self.furniture);
        let traps = self.bass_traps.clamp(0.0, 1.0);
        let (v, s) = (self.volume(), self.surface());
        map7(|b| {
            let extra = furn[b] * (self.length * self.width / 25.0) + BASS_TRAPS_AREA[b] * traps;
            let a_total: f64 = (0..6).map(|i| areas[i] * alpha[i][b]).sum::<f64>() + extra;
            let mean_alpha = (a_total / s).clamp(0.01, 0.95);
            0.161 * v / (-s * (1.0 - mean_alpha).ln() + 4.0 * AIR_M[b] * v) // Eyring
        })
    }

    /// RT60 (s) par bande d'octave.
    pub fn rt60_bands(&self) -> Bands {
        let rt = self.auto_rt60();
        if self.rt60_override > 0.0 {
            let k = self.rt60_override / mean2(&rt);
            return rt.map(|v| v * k);
        }
        rt
    }

    /// RT60 « moyen » (500 Hz – 1 kHz).
    pub fn rt60(&self) -> f64 {
        mean2(&self.rt60_bands())
    }

    /// Coefficient de réflexion spéculaire β (6 surfaces × 7 bandes).
    pub fn reflection_bands(&self) -> [Bands; 6] {
        let mut alpha = self.surface_absorption();
        if self.rt60_override > 0.0 {
            // RT imposé : absorption uniforme cohérente avec ce RT (Eyring inversé).
            let (v, s) = (self.volume(), self.surface());
            let rt = self.rt60_bands();
            let mean_alpha =
                map7(|b| (1.0 - (-(0.161 * v / rt[b] - 4.0 * AIR_M[b] * v) / s).exp()).clamp(0.01, 0.95));
            alpha = [mean_alpha; 6];
        }
        let mut beta = alpha.map(|row| row.map(|a| (1.0 - a).clamp(0.0, 1.0).sqrt()));
        let diffuse = (1.0 - 0.7 * self.diffusion.clamp(0.0, 1.0)).sqrt(); // mur arrière diffusant
        beta[0].iter_mut().for_each(|b| *b *= diffuse);
        beta
    }

    /// β moyen (rapport, compatibilité).
    pub fn reflection_coefficient(&self) -> f64 {
        let beta = self.reflection_bands();
        beta.iter().map(mean2).sum::<f64>() / 6.0
    }

    pub fn critical_distance_bands(&self) -> Bands {
        let v = self.volume();
        self.rt60_bands().map(|rt| 0.057 * (v / rt).sqrt())
    }

    pub fn critical_distance(&self) -> f64 {
        0.057 * (self.volume() / self.rt60()).sqrt()
    }
}

/// Position de l'auditeur et des enceintes ; repère pièce = repère auditeur
/// (x vers l'avant / le mur avant, y vers la gauche, z vers le haut).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub listener: [f64; 3],
    pub left: [f64; 3],
    pub right: [f64; 3],
}

impl Layout {
    pub fn speaker(&self, side: char) -> [f64; 3] {
        if side == 'L' { self.left } else { self.right }
    }
}

pub fn sub3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn norm3(a: [f64; 3]) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

pub fn stereo_layout(room: &RoomSpec, angle_deg: f64, distance: f64) -> Result<Layout> {
    let a = angle_deg.to_radians();
    let sx = room.length - room.front_gap;
    let lx = sx - distance * a.cos();
    if lx < 0.3 {
        return err("La pièce est trop courte pour cette distance d'écoute");
    }
    let cy = room.width / 2.0;
    let dy = distance * a.sin();
    if cy + dy > room.width - 0.2 {
        return err("La pièce est trop étroite pour cet angle/distance");
    }
    let z = room.ear_height.max(0.3).min(room.height - 0.3);
    let zs = if room.speaker_height <= 0.0 { z } else { room.speaker_height.max(0.2).min(room.height - 0.2) };
    let ly = (cy + room.listener_offset).max(0.3).min(room.width - 0.3);
    Ok(Layout { listener: [lx, ly, z], left: [sx, cy + dy, zs], right: [sx, cy - dy, zs] })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageSource {
    pub position: [f64; 3],
    pub reflections: usize,
    /// Réflexions par surface, ordre [`SURFACES`].
    pub counts: [usize; 6],
}

impl ImageSource {
    /// Atténuation par bande d'octave (produit des β des surfaces touchées).
    pub fn gain_bands(&self, beta: &[Bands; 6]) -> Bands {
        map7(|b| (0..6).map(|s| beta[s][b].powi(self.counts[s] as i32)).product())
    }

    /// Surface d'une réflexion du premier ordre.
    pub fn surface(&self) -> Option<&'static str> {
        (self.reflections == 1).then(|| SURFACES[self.counts.iter().position(|&c| c == 1).unwrap_or(0)])
    }
}

/// Sources-images (Allen & Berkley) jusqu'à `order` réflexions incluses.
///
/// Pour chaque axe d'indice n et de parité p : |n − p| réflexions sur la paroi
/// à 0 et |n| sur la paroi opposée.
pub fn image_sources(source: [f64; 3], room: &RoomSpec, order: usize) -> Vec<ImageSource> {
    let dims = [room.length, room.width, room.height];
    let o = order as i64;
    let mut out = Vec::new();
    for nx in -o..=o {
        for ny in -o..=o {
            for nz in -o..=o {
                let n = [nx, ny, nz];
                for px in 0..2i64 {
                    for py in 0..2i64 {
                        for pz in 0..2i64 {
                            let p = [px, py, pz];
                            let near: [usize; 3] = std::array::from_fn(|i| (n[i] - p[i]).unsigned_abs() as usize);
                            let far: [usize; 3] = std::array::from_fn(|i| n[i].unsigned_abs() as usize);
                            let refl = near.iter().sum::<usize>() + far.iter().sum::<usize>();
                            if refl > order {
                                continue;
                            }
                            let position =
                                std::array::from_fn(|i| (1 - 2 * p[i]) as f64 * source[i] + 2.0 * n[i] as f64 * dims[i]);
                            let counts = [near[0], far[0], near[1], far[1], near[2], far[2]];
                            out.push(ImageSource { position, reflections: refl, counts });
                        }
                    }
                }
            }
        }
    }
    out
}

/// Valeurs par bande d'octave → grille de fréquences (interpolation log, extrémités tenues).
pub fn bands_to_grid(values: &Bands, freqs: &[f64]) -> Vec<f64> {
    BandInterp::new(freqs).apply(values)
}

/// Interpolation précalculée des bandes d'octave vers une grille de fréquences.
struct BandInterp {
    weights: Vec<(usize, f64)>,
}

impl BandInterp {
    fn new(freqs: &[f64]) -> Self {
        let lo: Vec<f64> = OCTAVES.iter().map(|f| f.ln()).collect();
        let weights = freqs
            .iter()
            .map(|f| {
                let x = f.max(1.0).ln();
                if x <= lo[0] {
                    (0, 0.0)
                } else if x >= lo[6] {
                    (5, 1.0)
                } else {
                    let j = lo.partition_point(|&v| v <= x).clamp(1, 6) - 1;
                    (j, (x - lo[j]) / (lo[j + 1] - lo[j]))
                }
            })
            .collect();
        BandInterp { weights }
    }

    fn apply(&self, values: &Bands) -> Vec<f64> {
        self.weights.iter().map(|&(j, t)| values[j] * (1.0 - t) + values[j + 1] * t).collect()
    }
}

// ---------------------------------------------------------------------------
// Rendu
// ---------------------------------------------------------------------------
#[derive(Clone, Debug)]
pub struct RenderParams {
    pub fs: u32,
    pub angle_deg: f64,
    pub distance: f64,
    pub room: RoomSpec,
    pub room_enabled: bool,
    pub room_amount: f64,
    pub diffuse_field_eq: bool,
    pub ir_length_ms: f64,
    pub preamp_db: f64,
    /// + = enceinte droite plus forte.
    pub balance_db: f64,
    /// Plateau grave (105 Hz).
    pub bass_db: f64,
    /// Plateau aigu (10 kHz).
    pub treble_db: f64,
    /// Réverbération tardive.
    pub late_enabled: bool,
    pub seed: u64,
}

impl Default for RenderParams {
    fn default() -> Self {
        RenderParams {
            fs: 48000,
            angle_deg: 30.0,
            distance: 2.0,
            room: RoomSpec::default(),
            room_enabled: true,
            room_amount: 1.0,
            diffuse_field_eq: true,
            ir_length_ms: 400.0,
            preamp_db: 0.0,
            balance_db: 0.0,
            bass_db: 0.0,
            treble_db: 0.0,
            late_enabled: true,
            seed: 20240601,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RenderResult {
    pub fs: u32,
    /// Dans l'ordre [`PAIRS`] : LL, LR, RL, RR.
    pub irs: [Vec<f32>; 4],
    pub gain_db: f64,
    pub plot_freqs: Vec<f64>,
    pub plot_curves: Vec<(String, Vec<f64>)>,
    pub notes: Vec<String>,
    pub rt60_bands: Bands,
}

impl RenderResult {
    pub fn ir(&self, key: &str) -> &[f32] {
        &self.irs[PAIRS.iter().position(|k| *k == key).expect("paire connue")]
    }
}

/// Progression : (fraction 0–1, message).
pub type ProgressFn<'a> = &'a dyn Fn(f64, &str);

/// Queue de réverbération : bruit par bande d'octave, décroissance RT60 propre à
/// chaque bande, énergie `energy[b]` (relative à un son direct d'énergie 1).
fn late_tail(n: usize, fs: u32, rt60: &Bands, energy: &Bands, rng: &mut Rng) -> Vec<f64> {
    let fs = f64::from(fs);
    let noise = rng.normals(n);
    let nyq = fs / 2.0;
    let mut edges = vec![0.0];
    edges.extend(OCTAVES[..6].iter().map(|f| f * 2f64.sqrt()));
    edges.push(nyq);
    let mut out = vec![0.0; n];
    for b in 0..OCTAVES.len() {
        let (lo, hi) = (edges[b], edges[b + 1].min(nyq * 0.98));
        let band = if lo == 0.0 {
            Band::LowPass(hi)
        } else if b == OCTAVES.len() - 1 {
            Band::HighPass(lo)
        } else {
            Band::BandPass(lo, hi)
        };
        let sos = dsp::butter_sos(4, band, fs);
        let mut filtered = dsp::sosfiltfilt(&sos, &noise);
        for (i, v) in filtered.iter_mut().enumerate() {
            *v *= (-6.9078 * (i as f64 / fs) / rt60[b]).exp();
        }
        let e: f64 = filtered.iter().map(|v| v * v).sum();
        if e > 0.0 {
            // Part du spectre d'un son direct blanc tombant dans cette bande.
            let share = (edges[b + 1].min(nyq) - lo) / nyq;
            let k = (energy[b] * share / e).sqrt();
            for (o, v) in out.iter_mut().zip(&filtered) {
                *o += v * k;
            }
        }
    }
    out
}

/// Ajoute `amp · H · e^{−jω(τ)}` à `acc` (rotation de phase récurrente, recalée régulièrement).
fn accumulate_delayed(acc: &mut [C64], h: &[C64], amp: &dyn Fn(usize) -> f64, delay: f64, dw: f64) {
    let step = C64::from_polar(1.0, -dw * delay);
    let mut phasor = C64::new(1.0, 0.0);
    for (k, (a, hv)) in acc.iter_mut().zip(h).enumerate() {
        if k % 4096 == 0 {
            phasor = C64::from_polar(1.0, -dw * k as f64 * delay);
        }
        *a += hv * phasor * amp(k);
        phasor *= step;
    }
}

/// Construit les quatre réponses impulsionnelles binaurales.
pub fn render(
    speaker: &SpeakerResponse,
    hp_fir: Option<&[f64]>,
    hrtf: &dyn HrtfProvider,
    params: &RenderParams,
    progress: Option<ProgressFn>,
) -> Result<RenderResult> {
    let fs = params.fs;
    let fsf = f64::from(fs);
    if hrtf.fs() != fs {
        return err("Le HRTF n'est pas à la fréquence d'échantillonnage du rendu");
    }
    let report = |f: f64, m: &str| {
        if let Some(p) = progress {
            p(f, m)
        }
    };
    let mut notes = Vec::new();
    let room = &params.room;
    let use_room = params.room_enabled && params.room_amount > 0.0;
    let rt_bands = room.rt60_bands();
    let ir_ms = if use_room {
        params.ir_length_ms.max(900.0 * rt_bands[1..5].iter().copied().fold(f64::MIN, f64::max))
    } else {
        params.ir_length_ms
    };
    let ir_len = (ir_ms.min(2500.0) * fsf / 1000.0) as usize;
    let nfft = next_pow2(ir_len + 3 * FIR_TAPS);
    let freqs = rfftfreq(nfft, fsf);
    let nbins = freqs.len();
    let dw = 2.0 * PI * fsf / nfft as f64; // pulsation (rad/s) entre deux raies

    let spectrum_of = |curve: &[f64]| rfft(&minimum_phase_fir(&speaker.freqs, curve, fsf, FIR_TAPS), nfft);

    report(0.05, "Filtres du haut-parleur");
    let h_lw = spectrum_of(&speaker.listening_window);
    let h_er = spectrum_of(&speaker.early_reflections);
    let h_sp = spectrum_of(&speaker.sound_power);
    let mut h_hp = match hp_fir {
        Some(h) => rfft(h, nfft),
        None => vec![C64::new(1.0, 0.0); nbins],
    };
    // Tonalité (plateaux grave / aigu, phase minimale) appliquée avec la correction du casque.
    for bq in [
        Biquad::new(BiquadKind::LowShelf, 105.0, params.bass_db, 0.7),
        Biquad::new(BiquadKind::HighShelf, 10000.0, params.treble_db, 0.7),
    ] {
        if bq.gain_db.abs() <= 1e-6 {
            continue;
        }
        if let Some((b, a)) = bq.coefficients(fsf) {
            for (h, f) in h_hp.iter_mut().zip(&freqs) {
                *h *= dsp::freqz(&b, &a, 2.0 * PI * f / fsf);
            }
        }
    }

    report(0.15, "Champ diffus du HRTF");
    let df_db = diffuse_field_db(hrtf, nfft, 200);
    let ones = vec![C64::new(1.0, 0.0); nbins];
    let (h_df_inv, late_color) = if params.diffuse_field_eq {
        let mag: Vec<f64> = df_db.iter().map(|d| 10f64.powf((-d).clamp(-12.0, 12.0) / 20.0)).collect();
        (minimum_phase_spectrum(&mag, nfft), ones.clone())
    } else {
        let mag: Vec<f64> = df_db.iter().map(|d| 10f64.powf(d / 20.0)).collect();
        (ones.clone(), minimum_phase_spectrum(&mag, nfft))
    };

    let layout = stereo_layout(room, params.angle_deg, params.distance)?;
    let beta = room.reflection_bands();
    let order = if use_room { room.order } else { 0 };
    let pre_delay = 0.0005; // marge avant le son direct (s)

    let mut rng = Rng::new(params.seed);
    let mut late_energy = [0.0; 7];
    let mut t_start = 0.0;
    if use_room {
        let rc = room.critical_distance_bands();
        late_energy = map7(|b| (params.distance / rc[b]).powi(2) * params.room_amount);
        // Sous la fréquence de Schroeder (≈ 2000·√(T/V)), le champ diffus statistique
        // surestime l'énergie (régime modal) : excès borné à +6 dB par rapport aux médiums.
        let cap = 4.0 * mean2(&late_energy);
        late_energy = late_energy.map(|e| e.min(cap));
        if !params.late_enabled {
            late_energy = [0.0; 7];
        }
        t_start = (2e-3 * room.volume().sqrt()).max(0.012);
        notes.push(format!(
            "RT60 {:.2} s (500 Hz–1 kHz), distance critique {:.2} m, {} sources-images/enceinte",
            room.rt60(),
            room.critical_distance(),
            image_sources(layout.left, room, order).len()
        ));
    }

    // Référence temporelle : l'enceinte la plus proche (auditeur décentré possible).
    let d_ref = norm3(sub3(layout.left, layout.listener)).min(norm3(sub3(layout.right, layout.listener)));
    let bands_interp = BandInterp::new(&freqs);
    let mut spectra: [Vec<C64>; 4] = Default::default();
    for (si, side) in ['L', 'R'].into_iter().enumerate() {
        let side_gain = 10f64.powf(if side == 'L' { -params.balance_db } else { params.balance_db } / 40.0);
        let src = layout.speaker(side);
        let mut direct = [vec![C64::default(); nbins], vec![C64::default(); nbins]];
        let mut early = [vec![C64::default(); nbins], vec![C64::default(); nbins]];
        for img in image_sources(src, room, order) {
            let vec = sub3(img.position, layout.listener);
            let dist = norm3(vec);
            let tau = (dist - d_ref) / SPEED_OF_SOUND + pre_delay;
            if tau * fsf >= ir_len as f64 {
                continue;
            }
            let base = params.distance / dist;
            // Réflexion filtrée par les matériaux de chaque paroi touchée.
            let grid = (img.reflections > 0)
                .then(|| bands_interp.apply(&img.gain_bands(&beta)))
                .map(|g| g.into_iter().map(|v| v * base * params.room_amount).collect::<Vec<_>>());
            let amp = |k: usize| grid.as_ref().map_or(base, |g| g[k]);
            let h = hrtf.hrir([vec[0] / dist, vec[1] / dist, vec[2] / dist]);
            let target = if img.reflections == 0 { &mut direct } else { &mut early };
            for (ei, (ir, d)) in [(&h.left, h.delay_left), (&h.right, h.delay_right)].into_iter().enumerate() {
                accumulate_delayed(&mut target[ei], &rfft(ir, nfft), &amp, tau + d, dw);
            }
        }
        report(0.3 + 0.3 * si as f64, &format!("Haut-parleur {side} : sons directs et réflexions"));

        for ei in 0..2 {
            let mut total: Vec<C64> =
                (0..nbins).map(|k| h_df_inv[k] * (h_lw[k] * direct[ei][k] + h_er[k] * early[ei][k])).collect();
            if late_energy.iter().any(|e| *e > 0.0) {
                let mut tail = late_tail(ir_len, fs, &rt_bands, &late_energy, &mut rng);
                for (i, v) in tail.iter_mut().enumerate() {
                    let t = i as f64 / fsf;
                    let onset = ((t - pre_delay - t_start) / 0.01).clamp(0.0, 1.0);
                    *v *= 0.5 - 0.5 * (PI * onset).cos();
                }
                let tail_spec = rfft(&tail, nfft);
                for k in 0..nbins {
                    total[k] += tail_spec[k] * h_sp[k] * late_color[k];
                }
            }
            for (t, h) in total.iter_mut().zip(&h_hp) {
                *t *= h * side_gain;
            }
            spectra[si * 2 + ei] = total;
        }
        report(0.45 + 0.3 * si as f64, &format!("Haut-parleur {side} : réverbération"));
    }

    // Normalisation : pire cas = signal mono identique sur les deux enceintes.
    report(0.85, "Normalisation");
    let mut peak = f64::NEG_INFINITY;
    for ear in 0..2 {
        let p: Vec<f64> = (0..nbins).map(|k| (spectra[ear][k] + spectra[2 + ear][k]).norm_sqr()).collect();
        let smoothed = smooth_octave(&freqs, &p, 6.0);
        for (f, v) in freqs.iter().zip(&smoothed) {
            if (20.0..=20000.0).contains(f) {
                peak = peak.max(10.0 * (v + 1e-20).log10());
            }
        }
    }
    let gain_db = -HEADROOM_DB - peak + params.preamp_db;
    let gain = 10f64.powf(gain_db / 20.0);

    let irs: [Vec<f32>; 4] = std::array::from_fn(|i| {
        let h: Vec<f64> = irfft(&spectra[i], nfft)[..ir_len].iter().map(|v| v * gain).collect();
        fade_out(&h, 0.05).into_iter().map(|v| v as f32).collect()
    });
    if irs.iter().flatten().any(|v| v.abs() > 1.0) {
        notes.push("Attention : crête d'IR > 0 dBFS".into());
    }

    let plot_f = dsp::geomspace(20.0, 20000f64.min(fsf / 2.0 * 0.95), 300);
    let plot_db = |spec: &[C64]| -> Vec<f64> {
        let p: Vec<f64> = spec.iter().map(|c| c.norm_sqr()).collect();
        let smoothed = smooth_octave(&freqs, &p, 6.0);
        interp_all(&plot_f, &freqs, &smoothed).into_iter().map(|v| 10.0 * (v + 1e-20).log10()).collect()
    };
    let ls: Vec<f64> = speaker.freqs.iter().map(|f| f.ln()).collect();
    let mono_left: Vec<C64> = (0..nbins).map(|k| (spectra[0][k] + spectra[2][k]) * gain).collect();
    let curves = vec![
        (
            "Haut-parleur (Listening Window)".to_string(),
            plot_f.iter().map(|f| interp(f.ln(), &ls, &speaker.listening_window)).collect(),
        ),
        ("Correction casque".to_string(), plot_db(&h_hp)),
        ("Oreille gauche (mono, total)".to_string(), plot_db(&mono_left)),
    ];
    report(1.0, "Terminé");
    Ok(RenderResult { fs, irs, gain_db, plot_freqs: plot_f, plot_curves: curves, notes, rt60_bands: rt_bands })
}

fn wav_error(e: impl fmt::Display) -> AudioEngineError {
    AudioEngineError(format!("Écriture WAV impossible : {e}"))
}

fn write_wav(path: &Path, fs: u32, channels: &[&[f32]]) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(wav_error)?;
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = tempfile::Builder::new()
        .prefix(&format!(".{stem}."))
        .suffix(".wav")
        .tempfile_in(dir)
        .map_err(wav_error)?;
    {
        let spec = hound::WavSpec {
            channels: channels.len() as u16,
            sample_rate: fs,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut w = hound::WavWriter::new(BufWriter::new(tmp.as_file()), spec).map_err(wav_error)?;
        let n = channels.iter().map(|c| c.len()).min().unwrap_or(0);
        for i in 0..n {
            for c in channels {
                w.write_sample(c[i]).map_err(wav_error)?;
            }
        }
        w.finalize().map_err(wav_error)?;
    }
    tmp.persist(path).map_err(|e| wav_error(e.error))?;
    Ok(())
}

/// Écrit les IR en WAV float32 mono (écriture atomique).
pub fn write_irs(result: &RenderResult, directory: &Path) -> Result<BTreeMap<String, PathBuf>> {
    let mut paths = BTreeMap::new();
    for (i, key) in PAIRS.iter().enumerate() {
        let dest = directory.join(format!("ir_{key}.wav"));
        write_wav(&dest, result.fs, &[&result.irs[i]])?;
        paths.insert((*key).to_string(), dest);
    }
    Ok(paths)
}

/// WAV 4 canaux « true stereo » (ordre LL, LR, RL, RR) pour convolveurs génériques.
pub fn write_true_stereo(result: &RenderResult, path: &Path) -> Result<PathBuf> {
    let channels: Vec<&[f32]> = result.irs.iter().map(|v| v.as_slice()).collect();
    write_wav(path, result.fs, &channels)?;
    Ok(path.to_path_buf())
}

/// Configuration Equalizer APO (≥ 1.2, canaux virtuels) réalisant la matrice 2×2.
pub fn equalizer_apo_config(ir_paths: &BTreeMap<String, PathBuf>) -> String {
    let q = |k: &str| {
        let p = &ir_paths[k];
        std::path::absolute(p).unwrap_or_else(|_| p.clone()).display().to_string()
    };
    let mut lines = vec![
        "# Speaker Sim (devfrp) – à inclure depuis config.txt : Include: speaker-sim.txt".to_string(),
        "Channel: all".into(),
        "Copy: VLL=L VLR=L VRL=R VRR=R".into(),
    ];
    for k in PAIRS {
        lines.push(format!("Channel: V{k}\nConvolution: {}", q(k)));
    }
    lines.extend(["Channel: all".into(), "Copy: L=VLL+VRL R=VLR+VRR".into(), String::new()]);
    lines.join("\n")
}
