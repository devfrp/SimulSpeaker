//! Outils de traitement du signal : FFT réelle, interpolation, phase minimale,
//! biquads RBJ, filtres de Butterworth (SOS), filtrage aller-retour, lissages.
//!
//! Les conventions suivent celles de NumPy/SciPy utilisées par la première version :
//! `irfft` normalise par 1/n, `interp` maintient les valeurs aux extrémités, etc.

use std::cell::RefCell;
use std::f64::consts::PI;

pub use realfft::num_complex::Complex64 as C64;
use realfft::RealFftPlanner;

thread_local! {
    static PLANNER: RefCell<RealFftPlanner<f64>> = RefCell::new(RealFftPlanner::new());
}

/// FFT réelle sur `n` points (signal tronqué ou complété de zéros) : `n/2 + 1` raies.
pub fn rfft(x: &[f64], n: usize) -> Vec<C64> {
    let mut input = vec![0.0; n];
    let m = x.len().min(n);
    input[..m].copy_from_slice(&x[..m]);
    let plan = PLANNER.with(|p| p.borrow_mut().plan_fft_forward(n));
    let mut out = plan.make_output_vec();
    plan.process(&mut input, &mut out).expect("FFT directe");
    out
}

/// FFT réelle inverse sur `n` points, normalisée par 1/n (comme `numpy.fft.irfft`).
pub fn irfft(spec: &[C64], n: usize) -> Vec<f64> {
    let bins = n / 2 + 1;
    let mut input = vec![C64::new(0.0, 0.0); bins];
    let m = spec.len().min(bins);
    input[..m].copy_from_slice(&spec[..m]);
    input[0].im = 0.0;
    if n % 2 == 0 {
        input[bins - 1].im = 0.0;
    }
    let plan = PLANNER.with(|p| p.borrow_mut().plan_fft_inverse(n));
    let mut out = plan.make_output_vec();
    plan.process(&mut input, &mut out).expect("FFT inverse");
    let scale = 1.0 / n as f64;
    out.iter_mut().for_each(|v| *v *= scale);
    out
}

/// Fréquences des raies de `rfft` (`numpy.fft.rfftfreq(n, 1/fs)`).
pub fn rfftfreq(n: usize, fs: f64) -> Vec<f64> {
    (0..=n / 2).map(|k| k as f64 * fs / n as f64).collect()
}

pub fn next_pow2(n: usize) -> usize {
    n.max(1).next_power_of_two()
}

pub fn linspace(a: f64, b: f64, n: usize) -> Vec<f64> {
    match n {
        0 => vec![],
        1 => vec![a],
        _ => (0..n).map(|i| if i == n - 1 { b } else { a + (b - a) * i as f64 / (n - 1) as f64 }).collect(),
    }
}

pub fn geomspace(a: f64, b: f64, n: usize) -> Vec<f64> {
    let mut out: Vec<f64> = linspace(a.ln(), b.ln(), n).into_iter().map(f64::exp).collect();
    if let Some(first) = out.first_mut() {
        *first = a;
    }
    if n > 1 {
        out[n - 1] = b;
    }
    out
}

/// Interpolation linéaire, valeurs maintenues aux extrémités (`numpy.interp`).
pub fn interp(x: f64, xp: &[f64], fp: &[f64]) -> f64 {
    let n = xp.len();
    if n == 0 {
        return f64::NAN;
    }
    if x <= xp[0] {
        return fp[0];
    }
    if x >= xp[n - 1] {
        return fp[n - 1];
    }
    let j = xp.partition_point(|&v| v <= x).clamp(1, n - 1);
    let (x0, x1) = (xp[j - 1], xp[j]);
    if x1 == x0 {
        return fp[j];
    }
    fp[j - 1] + (fp[j] - fp[j - 1]) * (x - x0) / (x1 - x0)
}

pub fn interp_all(xs: &[f64], xp: &[f64], fp: &[f64]) -> Vec<f64> {
    xs.iter().map(|&x| interp(x, xp, fp)).collect()
}

/// Interpolation en log-fréquence, valeurs maintenues aux extrémités.
pub fn interp_log(freqs: &[f64], mag_db: &[f64], grid: &[f64]) -> Vec<f64> {
    let lf: Vec<f64> = freqs.iter().map(|f| f.ln()).collect();
    let floor = freqs[0] * 1e-3;
    grid.iter().map(|&g| interp(g.max(floor).ln(), &lf, mag_db)).collect()
}

pub fn mean(values: &[f64]) -> f64 {
    if values.is_empty() { f64::NAN } else { values.iter().sum::<f64>() / values.len() as f64 }
}

pub fn median(values: &[f64]) -> f64 {
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    match v.len() {
        0 => f64::NAN,
        n if n % 2 == 1 => v[n / 2],
        n => 0.5 * (v[n / 2 - 1] + v[n / 2]),
    }
}

/// Moyenne des valeurs dont la fréquence est dans `[lo, hi]`.
pub fn band_mean(freqs: &[f64], values: &[f64], lo: f64, hi: f64) -> Option<f64> {
    let band: Vec<f64> =
        freqs.iter().zip(values).filter(|(f, _)| **f >= lo && **f <= hi).map(|(_, v)| *v).collect();
    (!band.is_empty()).then(|| mean(&band))
}

/// Fondu de sortie en demi-cosinus sur la fraction finale `fraction` du signal.
pub fn fade_out(h: &[f64], fraction: f64) -> Vec<f64> {
    let n = h.len();
    let k = ((n as f64 * fraction) as usize).max(2).min(n);
    let mut out = h.to_vec();
    for (i, w) in linspace(0.0, PI, k).into_iter().enumerate() {
        out[n - k + i] *= 0.5 * (1.0 + w.cos());
    }
    out
}

/// Spectre à phase minimale (méthode du cepstre réel) d'une amplitude sur `rfftfreq(nfft)`.
pub fn minimum_phase_spectrum(mag: &[f64], nfft: usize) -> Vec<C64> {
    let log_mag: Vec<C64> = mag.iter().map(|m| C64::new(m.max(1e-6).ln(), 0.0)).collect();
    let mut cep = irfft(&log_mag, nfft);
    for (i, c) in cep.iter_mut().enumerate() {
        let fold = if i == 0 || i == nfft / 2 {
            1.0
        } else if i < nfft / 2 {
            2.0
        } else {
            0.0
        };
        *c *= fold;
    }
    rfft(&cep, nfft).into_iter().map(C64::exp).collect()
}

/// FIR à phase minimale suivant la courbe `mag_db` (dB) aux fréquences `freqs`.
pub fn minimum_phase_fir(freqs: &[f64], mag_db: &[f64], fs: f64, n_taps: usize) -> Vec<f64> {
    let nfft = next_pow2(4 * n_taps);
    let grid = rfftfreq(nfft, fs);
    let mag: Vec<f64> = interp_log(freqs, mag_db, &grid).into_iter().map(|db| 10f64.powf(db / 20.0)).collect();
    let h = irfft(&minimum_phase_spectrum(&mag, nfft), nfft);
    fade_out(&h[..n_taps], 0.1)
}

/// Lissage 1/`fraction` d'octave d'un spectre de puissance (grille linéaire).
pub fn smooth_octave(freqs: &[f64], power: &[f64], fraction: f64) -> Vec<f64> {
    let n = power.len();
    let mut csum = Vec::with_capacity(n + 1);
    csum.push(0.0);
    let mut acc = 0.0;
    for p in power {
        acc += p;
        csum.push(acc);
    }
    let df = freqs[1] - freqs[0];
    let half = 2f64.powf(1.0 / (2.0 * fraction));
    let last = (n - 1) as f64;
    freqs
        .iter()
        .map(|&f| {
            let lo = (f / half / df).floor().clamp(0.0, last) as usize;
            let hi = ((f * half / df).ceil().clamp(0.0, last) as usize).max(lo);
            (csum[hi + 1] - csum[lo]) / (hi - lo + 1) as f64
        })
        .collect()
}

/// Moyenne glissante (`scipy.ndimage.uniform_filter1d`, mode « nearest »).
pub fn uniform_filter_nearest(values: &[f64], size: usize) -> Vec<f64> {
    let size = size.max(1);
    let n = values.len() as isize;
    let center = (size / 2) as isize;
    (0..n)
        .map(|i| {
            let s: f64 = (0..size as isize).map(|k| values[(i - center + k).clamp(0, n - 1) as usize]).sum();
            s / size as f64
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Biquads (Audio EQ Cookbook)
// ---------------------------------------------------------------------------
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BiquadKind {
    Peaking,
    LowShelf,
    HighShelf,
    LowPass,
    HighPass,
}

impl BiquadKind {
    /// Types reconnus dans un fichier Equalizer APO / AutoEq.
    pub fn from_apo(code: &str) -> Option<Self> {
        Some(match code.to_uppercase().as_str() {
            "PK" | "PEQ" => BiquadKind::Peaking,
            "LSC" | "LS" | "LSQ" => BiquadKind::LowShelf,
            "HSC" | "HS" | "HSQ" => BiquadKind::HighShelf,
            "LP" | "LPQ" => BiquadKind::LowPass,
            "HP" | "HPQ" => BiquadKind::HighPass,
            _ => return None,
        })
    }

    pub fn code(self) -> &'static str {
        match self {
            BiquadKind::Peaking => "PK",
            BiquadKind::LowShelf => "LSC",
            BiquadKind::HighShelf => "HSC",
            BiquadKind::LowPass => "LP",
            BiquadKind::HighPass => "HP",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Biquad {
    pub kind: BiquadKind,
    pub fc: f64,
    pub gain_db: f64,
    pub q: f64,
}

impl Biquad {
    pub fn new(kind: BiquadKind, fc: f64, gain_db: f64, q: f64) -> Self {
        Biquad { kind, fc, gain_db, q }
    }

    /// Coefficients normalisés (`b`, `a`, a0 = 1). `None` si fc ≥ Nyquist.
    pub fn coefficients(&self, fs: f64) -> Option<([f64; 3], [f64; 3])> {
        if self.fc <= 0.0 || self.fc >= fs / 2.0 * 0.999 {
            return None;
        }
        let a_lin = 10f64.powf(self.gain_db / 40.0);
        let w0 = 2.0 * PI * self.fc / fs;
        let (cw, sw) = (w0.cos(), w0.sin());
        let alpha = sw / (2.0 * self.q.max(1e-3));
        let sq = 2.0 * a_lin.sqrt() * alpha;
        let (b, a) = match self.kind {
            BiquadKind::Peaking => (
                [1.0 + alpha * a_lin, -2.0 * cw, 1.0 - alpha * a_lin],
                [1.0 + alpha / a_lin, -2.0 * cw, 1.0 - alpha / a_lin],
            ),
            BiquadKind::LowShelf => (
                [
                    a_lin * ((a_lin + 1.0) - (a_lin - 1.0) * cw + sq),
                    2.0 * a_lin * ((a_lin - 1.0) - (a_lin + 1.0) * cw),
                    a_lin * ((a_lin + 1.0) - (a_lin - 1.0) * cw - sq),
                ],
                [
                    (a_lin + 1.0) + (a_lin - 1.0) * cw + sq,
                    -2.0 * ((a_lin - 1.0) + (a_lin + 1.0) * cw),
                    (a_lin + 1.0) + (a_lin - 1.0) * cw - sq,
                ],
            ),
            BiquadKind::HighShelf => (
                [
                    a_lin * ((a_lin + 1.0) + (a_lin - 1.0) * cw + sq),
                    -2.0 * a_lin * ((a_lin - 1.0) + (a_lin + 1.0) * cw),
                    a_lin * ((a_lin + 1.0) + (a_lin - 1.0) * cw - sq),
                ],
                [
                    (a_lin + 1.0) - (a_lin - 1.0) * cw + sq,
                    2.0 * ((a_lin - 1.0) - (a_lin + 1.0) * cw),
                    (a_lin + 1.0) - (a_lin - 1.0) * cw - sq,
                ],
            ),
            BiquadKind::LowPass => (
                [(1.0 - cw) / 2.0, 1.0 - cw, (1.0 - cw) / 2.0],
                [1.0 + alpha, -2.0 * cw, 1.0 - alpha],
            ),
            BiquadKind::HighPass => (
                [(1.0 + cw) / 2.0, -(1.0 + cw), (1.0 + cw) / 2.0],
                [1.0 + alpha, -2.0 * cw, 1.0 - alpha],
            ),
        };
        let a0 = a[0];
        Some((b.map(|v| v / a0), a.map(|v| v / a0)))
    }
}

/// Réponse complexe d'un filtre `b/a` à la pulsation normalisée `w` (rad/échantillon).
pub fn freqz(b: &[f64], a: &[f64], w: f64) -> C64 {
    let eval = |c: &[f64]| -> C64 {
        c.iter().enumerate().map(|(k, &v)| C64::from_polar(v, -w * k as f64)).sum()
    };
    eval(b) / eval(a)
}

/// Gain (dB) d'un filtre `b/a` à la fréquence `f`.
pub fn gain_db(b: &[f64], a: &[f64], f: f64, fs: f64) -> f64 {
    20.0 * freqz(b, a, 2.0 * PI * f / fs).norm().max(1e-12).log10()
}

/// Section du second ordre : `[b0, b1, b2, a0, a1, a2]` (a0 = 1).
pub type Sos = [f64; 6];

/// Filtrage en cascade (forme directe II transposée). `zi` : états initiaux par section.
pub fn sosfilt(sos: &[Sos], x: &[f64], zi: Option<&[[f64; 2]]>) -> Vec<f64> {
    let mut state: Vec<[f64; 2]> = zi.map(|z| z.to_vec()).unwrap_or_else(|| vec![[0.0; 2]; sos.len()]);
    let mut y = x.to_vec();
    for (s, z) in sos.iter().zip(state.iter_mut()) {
        let [b0, b1, b2, _, a1, a2] = *s;
        for v in y.iter_mut() {
            let input = *v;
            let out = b0 * input + z[0];
            z[0] = b1 * input - a1 * out + z[1];
            z[1] = b2 * input - a2 * out;
            *v = out;
        }
    }
    y
}

/// États initiaux en régime établi pour une entrée unité (`scipy.signal.sosfilt_zi`).
pub fn sosfilt_zi(sos: &[Sos]) -> Vec<[f64; 2]> {
    let mut scale = 1.0;
    sos.iter()
        .map(|s| {
            let [b0, b1, b2, _, a1, a2] = *s;
            let g = (b0 + b1 + b2) / (1.0 + a1 + a2);
            let zi = [(g - b0) * scale, (b2 - a2 * g) * scale];
            scale *= g;
            zi
        })
        .collect()
}

/// Filtrage aller-retour à phase nulle (`scipy.signal.sosfiltfilt`, extension impaire).
pub fn sosfiltfilt(sos: &[Sos], x: &[f64]) -> Vec<f64> {
    let zeros_b = sos.iter().filter(|s| s[2] == 0.0).count();
    let zeros_a = sos.iter().filter(|s| s[5] == 0.0).count();
    let ntaps = 2 * sos.len() + 1 - zeros_b.min(zeros_a);
    let n = x.len();
    let padlen = (3 * ntaps).min(n.saturating_sub(1));
    let mut ext = Vec::with_capacity(n + 2 * padlen);
    for i in (1..=padlen).rev() {
        ext.push(2.0 * x[0] - x[i]);
    }
    ext.extend_from_slice(x);
    for i in 1..=padlen {
        ext.push(2.0 * x[n - 1] - x[n - 1 - i]);
    }
    let zi = sosfilt_zi(sos);
    let scaled = |k: f64| zi.iter().map(|z| [z[0] * k, z[1] * k]).collect::<Vec<_>>();
    let y = sosfilt(sos, &ext, Some(&scaled(ext[0])));
    let mut rev: Vec<f64> = y.into_iter().rev().collect();
    let y0 = rev[0];
    rev = sosfilt(sos, &rev, Some(&scaled(y0)));
    rev.reverse();
    rev[padlen..padlen + n].to_vec()
}

#[derive(Clone, Copy, Debug)]
pub enum Band {
    LowPass(f64),
    HighPass(f64),
    BandPass(f64, f64),
}

/// Filtre de Butterworth numérique d'ordre `order` en sections du second ordre
/// (`scipy.signal.butter(..., output="sos")` : prototype, transformation, bilinéaire).
pub fn butter_sos(order: usize, band: Band, fs: f64) -> Vec<Sos> {
    // Prototype analogique : pôles sur le demi-cercle gauche.
    let n = order as i64;
    let proto: Vec<C64> = (0..n)
        .map(|i| {
            let m = (-n + 1 + 2 * i) as f64;
            -C64::from_polar(1.0, PI * m / (2.0 * order as f64))
        })
        .collect();
    // Fréquences pré-déformées (fs normalisée à 2, comme SciPy).
    let warp = |f: f64| 4.0 * (PI * (f / (fs / 2.0)) / 2.0).tan();
    let (zeros, poles, k): (Vec<C64>, Vec<C64>, f64) = match band {
        Band::LowPass(f) => {
            let wo = warp(f);
            (vec![], proto.iter().map(|p| p * wo).collect(), wo.powi(order as i32))
        }
        Band::HighPass(f) => {
            let wo = warp(f);
            let prod: C64 = proto.iter().map(|p| -p).product();
            (vec![C64::new(0.0, 0.0); order], proto.iter().map(|p| wo / p).collect(), (C64::new(1.0, 0.0) / prod).re)
        }
        Band::BandPass(lo, hi) => {
            let (w1, w2) = (warp(lo), warp(hi));
            let (bw, wo) = (w2 - w1, (w1 * w2).sqrt());
            let mut poles = Vec::with_capacity(2 * order);
            let mut second = Vec::with_capacity(order);
            for p in &proto {
                let pl = p * bw / 2.0;
                let root = (pl * pl - wo * wo).sqrt();
                poles.push(pl + root);
                second.push(pl - root);
            }
            poles.extend(second);
            (vec![C64::new(0.0, 0.0); order], poles, bw.powi(order as i32))
        }
    };
    // Transformation bilinéaire (fs = 2).
    let fs2 = 4.0;
    let mut zd: Vec<C64> = zeros.iter().map(|z| (fs2 + z) / (fs2 - z)).collect();
    let pd: Vec<C64> = poles.iter().map(|p| (fs2 + p) / (fs2 - p)).collect();
    zd.extend(std::iter::repeat_n(C64::new(-1.0, 0.0), poles.len() - zeros.len()));
    let num: C64 = zeros.iter().map(|z| fs2 - z).product();
    let den: C64 = poles.iter().map(|p| fs2 - p).product();
    let kd = k * (num / den).re;
    zpk_to_sos(&zd, &pd, kd)
}

fn zpk_to_sos(zeros: &[C64], poles: &[C64], k: f64) -> Vec<Sos> {
    let mut pole_pairs: Vec<(C64, C64)> = poles.iter().filter(|p| p.im > 1e-12).map(|p| (*p, p.conj())).collect();
    let mut real: Vec<f64> = poles.iter().filter(|p| p.im.abs() <= 1e-12).map(|p| p.re).collect();
    real.sort_by(f64::total_cmp);
    for pair in real.chunks(2) {
        let b = pair.get(1).copied().unwrap_or(0.0);
        pole_pairs.push((C64::new(pair[0], 0.0), C64::new(b, 0.0)));
    }
    // Zéros réels (±1, 0) : le plus petit avec le plus grand, puis vers le centre.
    let mut zr: Vec<C64> = zeros.to_vec();
    zr.sort_by(|a, b| a.re.total_cmp(&b.re));
    let zero_pairs = zr.len() / 2;
    pole_pairs
        .iter()
        .enumerate()
        .map(|(i, (p1, p2))| {
            let (b1, b2) = if i < zero_pairs {
                let (z1, z2) = (zr[i], zr[zr.len() - 1 - i]);
                (-(z1 + z2).re, (z1 * z2).re)
            } else {
                (0.0, 0.0)
            };
            let g = if i == 0 { k } else { 1.0 };
            [g, b1 * g, b2 * g, 1.0, -(p1 + p2).re, (p1 * p2).re]
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Bruit pseudo-aléatoire reproductible
// ---------------------------------------------------------------------------
/// Générateur xoshiro256** (graine développée par SplitMix64), loi normale par Box-Muller.
pub struct Rng {
    s: [u64; 4],
    spare: Option<f64>,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut x = seed;
        let mut next = || {
            x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = x;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        };
        Rng { s: [next(), next(), next(), next()], spare: None }
    }

    fn next_u64(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// Uniforme dans ]0, 1[.
    fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    pub fn standard_normal(&mut self) -> f64 {
        if let Some(v) = self.spare.take() {
            return v;
        }
        let (u1, u2) = (self.uniform(), self.uniform());
        let r = (-2.0 * u1.ln()).sqrt();
        let (s, c) = (2.0 * PI * u2).sin_cos();
        self.spare = Some(r * s);
        r * c
    }

    pub fn normals(&mut self, n: usize) -> Vec<f64> {
        (0..n).map(|_| self.standard_normal()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_roundtrip() {
        let x: Vec<f64> = (0..64).map(|i| (i as f64 * 0.3).sin()).collect();
        let y = irfft(&rfft(&x, 64), 64);
        for (a, b) in x.iter().zip(&y) {
            assert!((a - b).abs() < 1e-12);
        }
    }

    #[test]
    fn butterworth_matches_scipy() {
        // scipy.signal.butter(4, 1000, "lowpass", fs=48000, output="sos") : gain DC = 1,
        // −3 dB à la coupure, pente de 24 dB/octave.
        let fs = 48000.0;
        let sos = butter_sos(4, Band::LowPass(1000.0), fs);
        let total = |f: f64| -> f64 {
            sos.iter().map(|s| gain_db(&s[..3], &s[3..], f, fs)).sum()
        };
        assert!(total(1.0).abs() < 1e-6);
        assert!((total(1000.0) + 3.0103).abs() < 0.01);
        // Réponse exacte après transformation bilinéaire : |H|² = 1 / (1 + (tan(πf/fs) / tan(πfc/fs))^2n).
        // La pente dépasse 24 dB/octave en approchant de Nyquist.
        for f in [2000.0, 4000.0, 8000.0] {
            let r = (PI * f / fs).tan() / (PI * 1000.0 / fs).tan();
            let exact = -10.0 * (1.0 + r.powi(8)).log10();
            assert!((total(f) - exact).abs() < 0.01, "{f} Hz : {} au lieu de {exact}", total(f));
        }
        let bp = butter_sos(4, Band::BandPass(500.0, 1000.0), fs);
        assert_eq!(bp.len(), 4);
        let g = |f: f64| -> f64 { bp.iter().map(|s| gain_db(&s[..3], &s[3..], f, fs)).sum() };
        assert!(g(707.0).abs() < 0.1);
        assert!((g(500.0) + 3.0103).abs() < 0.05);
        let hp = butter_sos(4, Band::HighPass(2000.0), fs);
        let g = |f: f64| -> f64 { hp.iter().map(|s| gain_db(&s[..3], &s[3..], f, fs)).sum() };
        assert!(g(20000.0).abs() < 0.01);
        assert!((g(2000.0) + 3.0103).abs() < 0.01);
    }

    #[test]
    fn filtfilt_keeps_constant_and_phase() {
        let sos = butter_sos(4, Band::LowPass(2000.0), 48000.0);
        let y = sosfiltfilt(&sos, &vec![1.0; 500]);
        assert!(y.iter().all(|v| (v - 1.0).abs() < 1e-9));
        let x: Vec<f64> = (0..2000).map(|i| (2.0 * PI * 100.0 * i as f64 / 48000.0).sin()).collect();
        let y = sosfiltfilt(&sos, &x);
        for i in 500..1500 {
            assert!((x[i] - y[i]).abs() < 1e-3);
        }
    }

    #[test]
    fn uniform_filter_matches_scipy() {
        // Fenêtre de SciPy pour une taille paire : [i − 2, i + 1] (size1 = size / 2).
        let out = uniform_filter_nearest(&[1.0, 2.0, 3.0, 4.0, 5.0], 4);
        for (a, b) in out.iter().zip([1.25, 1.75, 2.5, 3.5, 4.25]) {
            assert!((a - b).abs() < 1e-12, "{out:?}");
        }
    }

    #[test]
    fn normal_noise_statistics() {
        let v = Rng::new(1).normals(100_000);
        let m = mean(&v);
        let var = v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / v.len() as f64;
        assert!(m.abs() < 0.02 && (var - 1.0).abs() < 0.02);
    }
}
