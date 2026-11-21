mod common;

use speaker_sim::audio_engine::{
    self as ae, FIR_TAPS, PAIRS, RenderParams, RenderResult, RoomSpec, SURFACES, SpeakerResponse,
};
use speaker_sim::dsp::{self, Biquad, BiquadKind, linspace};

use common::{FakeHrtf, make_cea_text, make_polar_text, temp_paths};

const PARAMETRIC: &str = "Preamp: -6.1 dB
Filter 1: ON LSC Fc 105 Hz Gain 6.4 dB Q 0.70
Filter 2: ON PK Fc 8800 Hz Gain 5.1 dB Q 1.42
Filter 3: ON HSC Fc 10000 Hz Gain -2.1 dB Q 0.70
Filter 4: OFF PK Fc 1000 Hz Gain 9 dB Q 1
";

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

/// Gain (dB) d'une réponse impulsionnelle finie à la fréquence `f`.
fn fir_gain_db(h: &[f64], f: f64, fs: f64) -> f64 {
    dsp::gain_db(h, &[1.0], f, fs)
}

fn energy(x: &[f32]) -> f64 {
    x.iter().map(|&v| f64::from(v).powi(2)).sum()
}

fn spectrum(x: &[f64], n: usize) -> Vec<f64> {
    let mut padded = x.to_vec();
    padded.resize(n, 0.0);
    dsp::rfft(&padded, n).iter().map(|c| c.norm()).collect()
}

fn as_f64(x: &[f32]) -> Vec<f64> {
    x.iter().map(|&v| f64::from(v)).collect()
}

// --- Égaliseurs ------------------------------------------------------------------

#[test]
fn parse_parametric() {
    let eq = ae::parse_parametric_eq(PARAMETRIC);
    assert!(close(eq.preamp_db, -6.1, 1e-12));
    let kinds: Vec<&str> = eq.filters.iter().map(|f| f.kind.code()).collect();
    assert_eq!(kinds, ["LSC", "PK", "HSC"]); // OFF ignoré
    assert!(close(eq.filters[1].fc, 8800.0, 1e-12));
    assert!(close(eq.filters[1].q, 1.42, 1e-12));
}

#[test]
fn peaking_gain_at_center() {
    for gain in [6.0, -4.5] {
        let (b, a) = Biquad::new(BiquadKind::Peaking, 1000.0, gain, 1.0).coefficients(48000.0).unwrap();
        assert!(close(dsp::gain_db(&b, &a, 1000.0, 48000.0), gain, 1e-3));
    }
}

#[test]
fn shelves_asymptotes() {
    let fs = 48000.0;
    let (lb, la) = Biquad::new(BiquadKind::LowShelf, 200.0, 6.0, 0.707).coefficients(fs).unwrap();
    let (hb, ha) = Biquad::new(BiquadKind::HighShelf, 2000.0, -6.0, 0.707).coefficients(fs).unwrap();
    assert!(close(dsp::gain_db(&lb, &la, 10.0, fs), 6.0, 0.1));
    assert!(close(dsp::gain_db(&lb, &la, 20000.0, fs), 0.0, 0.1));
    assert!(close(dsp::gain_db(&hb, &ha, 20000.0, fs), -6.0, 0.2));
}

#[test]
fn filter_above_nyquist_skipped() {
    assert!(Biquad::new(BiquadKind::Peaking, 30000.0, 3.0, 1.0).coefficients(48000.0).is_none());
}

#[test]
fn fir_matches_response() {
    let fs = 48000.0;
    let eq = ae::parse_parametric_eq(PARAMETRIC);
    let h = ae::headphone_eq_fir(&eq, fs, FIR_TAPS, false);
    let freqs = [50.0, 105.0, 1000.0, 8800.0, 15000.0];
    let expected = ae::eq_response_db(&eq, &freqs, fs);
    for (f, e) in freqs.iter().zip(expected) {
        let got = fir_gain_db(&h, *f, fs);
        assert!(close(got, e, 0.2), "{f} Hz : {got} au lieu de {e}");
    }
}

#[test]
fn graphic_eq() {
    let eq = ae::parse_graphic_eq("GraphicEQ: 20 -3; 1000 0; 20000 4").unwrap();
    assert!(eq.graphic.is_some());
    let h = ae::headphone_eq_fir(&eq, 48000.0, FIR_TAPS, false);
    assert!(close(fir_gain_db(&h, 1000.0, 48000.0), 0.0, 0.3));
}

// --- Phase minimale --------------------------------------------------------------

#[test]
fn minimum_phase_magnitude_and_energy_front_loaded() {
    let fs = 48000.0;
    let f = [20.0, 100.0, 1000.0, 5000.0, 20000.0];
    let db = [-10.0, -3.0, 0.0, 3.0, -6.0];
    let h = dsp::minimum_phase_fir(&f, &db, fs, 4096);
    for (probe, want) in [(100.0, -3.0), (1000.0, 0.0), (5000.0, 3.0)] {
        let got = fir_gain_db(&h, probe, fs);
        assert!(close(got, want, 0.5), "{probe} Hz : {got}");
    }
    let total: f64 = h.iter().map(|v| v * v).sum();
    let head: f64 = h[..=256].iter().map(|v| v * v).sum();
    assert!(head / total > 0.95); // phase minimale : énergie au début
}

#[test]
fn octave_smoothing_flat() {
    let freqs = linspace(0.0, 24000.0, 1025);
    let out = dsp::smooth_octave(&freqs, &vec![1.0; 1025], 3.0);
    assert!(out.iter().all(|v| close(*v, 1.0, 1e-12)));
}

// --- CEA-2034 et mesures polaires -----------------------------------------------

#[test]
fn cea_parse_with_thousands_and_normalisation() {
    let curves = ae::parse_cea2034(&make_cea_text(60)).unwrap();
    assert!(curves.contains_key("Sound Power"));
    let lw = &curves["Listening Window"];
    assert!(close(*lw.f.last().unwrap(), 20000.0, 1.0)); // « 20,000.00 » bien lu
    let spk = SpeakerResponse::from_curves(&curves).unwrap();
    assert!(close(dsp::mean(&spk.listening_window), 0.0, 1e-6));
    assert!(close(spk.directivity_index(1000.0, 4000.0), 6.0, 1e-6));
}

#[test]
fn cea_missing_listening_window() {
    let bad = "\"CEA2034\"\n\"On Axis\"\n\"f\"\t\"spl\"\n100\t80\n200\t81\n";
    assert!(ae::parse_cea2034(bad).is_err());
}

#[test]
fn polar_omni_gives_identical_curves() {
    let h = ae::parse_klippel_polar(&make_polar_text("Horizontal", 0.0)).unwrap();
    let v = ae::parse_klippel_polar(&make_polar_text("Vertical", 0.0)).unwrap();
    assert_eq!(h.angles.len(), 36);
    let curves = ae::cea2034_from_polar(&h, &v).unwrap();
    for name in ["Listening Window", "Early Reflections", "Sound Power"] {
        assert!(curves[name].db.iter().all(|x| close(*x, 86.0, 1e-9)), "{name}");
    }
}

#[test]
fn polar_directional_speaker_has_positive_di() {
    let h = ae::parse_klippel_polar(&make_polar_text("Horizontal", 1.5)).unwrap();
    let v = ae::parse_klippel_polar(&make_polar_text("Vertical", 1.5)).unwrap();
    let spk = SpeakerResponse::from_curves(&ae::cea2034_from_polar(&h, &v).unwrap()).unwrap();
    assert!(spk.directivity_index(1000.0, 4000.0) > 3.0);
    assert!(spk.early_reflections.iter().zip(&spk.listening_window).all(|(er, lw)| er < lw));
}

#[test]
fn load_speaker_from_folder() {
    let (_tmp, paths) = temp_paths();
    let folder = paths.cache_dir.join("spk");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("SPL Horizontal.txt"), make_polar_text("Horizontal", 1.0)).unwrap();
    std::fs::write(folder.join("SPL Vertical.txt"), make_polar_text("Vertical", 1.0)).unwrap();
    assert!(ae::load_speaker(&folder).unwrap().directivity_index(1000.0, 4000.0) > 1.0);
}

// --- Pièce -----------------------------------------------------------------------

fn surface(name: &str) -> usize {
    SURFACES.iter().position(|s| *s == name).unwrap()
}

#[test]
fn image_source_counts() {
    let room = RoomSpec::sized(4.0, 5.0, 2.5);
    let src = [1.0, 1.5, 1.2];
    let counts: Vec<usize> = [0, 1, 2].iter().map(|&k| ae::image_sources(src, &room, k).len()).collect();
    assert_eq!(counts, [1, 7, 25]);
}

#[test]
fn first_order_images_hit_one_named_surface() {
    let room = RoomSpec::sized(4.0, 5.0, 2.5);
    let imgs: Vec<_> =
        ae::image_sources([1.0, 1.5, 1.2], &room, 1).into_iter().filter(|i| i.reflections == 1).collect();
    let by_surface: std::collections::HashMap<&str, [f64; 3]> =
        imgs.iter().map(|i| (i.surface().unwrap(), i.position)).collect();
    assert_eq!(by_surface.len(), SURFACES.len());
    assert!(close(by_surface["floor"][2], -1.2, 1e-12)); // sol
    assert!(close(by_surface["ceiling"][2], 2.0 * 2.5 - 1.2, 1e-12)); // plafond
    assert!(close(by_surface["front"][0], 2.0 * 5.0 - 1.0, 1e-12)); // mur avant (x = L)
    assert!(close(by_surface["rear"][0], -1.0, 1e-12));
}

#[test]
fn treatment_shortens_rt60_mostly_above_bass() {
    let bare = RoomSpec { treatment: 0.0, ..RoomSpec::default() }.rt60_bands();
    let treated = RoomSpec { treatment: 0.6, ..RoomSpec::default() }.rt60_bands();
    assert!(treated.iter().zip(&bare).all(|(t, b)| t < b));
    let ratio: Vec<f64> = treated.iter().zip(&bare).map(|(t, b)| t / b).collect();
    assert!(ratio[0] > ratio[3]); // les panneaux agissent peu à 125 Hz
}

#[test]
fn bass_traps_act_on_low_bands() {
    let base = RoomSpec::default().rt60_bands();
    let traps = RoomSpec { bass_traps: 1.0, ..RoomSpec::default() }.rt60_bands();
    assert!(traps[0] < base[0] * 0.8);
    assert!(traps[4] > base[4] * 0.9);
}

#[test]
fn materials_and_furniture() {
    let tile = RoomSpec { floor: "tile".into(), furniture: "empty".into(), ..RoomSpec::default() }.rt60();
    let carpet = RoomSpec { floor: "carpet".into(), furniture: "full".into(), ..RoomSpec::default() }.rt60();
    assert!(tile > 2.0 * carpet);
    assert!(
        RoomSpec { walls: "drywall".into(), ..RoomSpec::default() }.rt60_bands()[0]
            < RoomSpec { walls: "masonry".into(), ..RoomSpec::default() }.rt60_bands()[0]
    );
}

#[test]
fn rt60_override() {
    let room = RoomSpec { rt60_override: 0.5, ..RoomSpec::default() };
    assert!(close(room.rt60(), 0.5, 1e-6));
    // L'absorption uniforme déduite reproduit ce RT : β cohérent avec Sabine/Eyring.
    assert!(room.reflection_bands().iter().flatten().all(|b| *b > 0.0 && *b < 1.0));
}

#[test]
fn reflection_filtered_by_treated_walls() {
    let room = RoomSpec { treatment: 1.0, ..RoomSpec::default() };
    let beta = room.reflection_bands();
    let img = ae::image_sources([5.0, 2.0, 1.2], &room, 1)
        .into_iter()
        .find(|i| i.counts[surface("left")] == 1)
        .unwrap();
    let g = img.gain_bands(&beta);
    assert!(g[0] > 3.0 * g[4]); // panneau : grave réfléchi, aigu absorbé
}

#[test]
fn diffusion_reduces_rear_specular() {
    assert!(
        RoomSpec { diffusion: 1.0, ..RoomSpec::default() }.reflection_bands()[0][3]
            < RoomSpec { diffusion: 0.0, ..RoomSpec::default() }.reflection_bands()[0][3]
    );
}

#[test]
fn order_4_image_count() {
    let room = RoomSpec::sized(5.0, 6.0, 3.0);
    assert_eq!(ae::image_sources([4.0, 2.0, 1.2], &room, 4).len(), 129);
}

#[test]
fn per_wall_treatment_only_changes_that_wall() {
    let base = RoomSpec { per_wall: true, ..RoomSpec::default() }.reflection_bands();
    let front = RoomSpec { per_wall: true, treatment_front: 1.0, ..RoomSpec::default() }.reflection_bands();
    let i_front = surface("front");
    assert!(front[i_front][3] < base[i_front][3] * 0.5);
    for i in (0..6).filter(|&i| i != i_front) {
        for b in 0..7 {
            assert!(close(front[i][b], base[i][b], 1e-12), "surface {i}, bande {b}");
        }
    }
}

#[test]
fn absorber_types() {
    let lf = |a: &str| {
        RoomSpec { treatment: 0.6, absorber: a.into(), ..RoomSpec::default() }.rt60_bands()[1]
    };
    assert!(lf("foam5") > lf("wool10"));
    assert!(lf("wool10") > lf("wool20"));
}

#[test]
fn speaker_height_and_listener_offset() {
    let room = RoomSpec { speaker_height: 1.0, listener_offset: 0.5, ..RoomSpec::sized(4.5, 6.0, 2.6) };
    let lay = ae::stereo_layout(&room, 30.0, 2.0).unwrap();
    assert!(close(lay.left[2], 1.0, 1e-12));
    assert!(close(lay.listener[2], 1.2, 1e-12));
    assert!(close(lay.listener[1], 4.5 / 2.0 + 0.5, 1e-12));
    let dl = ae::norm3(ae::sub3(lay.left, lay.listener));
    let dr = ae::norm3(ae::sub3(lay.right, lay.listener));
    assert!(dl < dr); // décalé vers la gauche : enceinte gauche plus proche
}

#[test]
fn layout_symmetry_front_gap_and_errors() {
    let room = RoomSpec { front_gap: 1.0, ear_height: 1.1, ..RoomSpec::sized(4.5, 6.0, 2.6) };
    let layout = ae::stereo_layout(&room, 30.0, 2.0).unwrap();
    assert!(close(ae::norm3(ae::sub3(layout.left, layout.listener)), 2.0, 1e-12));
    assert!(layout.left[1] > layout.right[1]); // y vers la gauche
    assert!(close(layout.left[0], 5.0, 1e-12)); // 1 m du mur avant
    assert!(close(layout.listener[2], 1.1, 1e-12));
    assert!(ae::stereo_layout(&RoomSpec::sized(2.0, 6.0, 2.6), 60.0, 3.0).is_err());
}

// --- Rendu -----------------------------------------------------------------------

fn speaker() -> SpeakerResponse {
    SpeakerResponse::from_curves(&ae::parse_cea2034(&make_cea_text(60)).unwrap()).unwrap()
}

fn params() -> RenderParams {
    RenderParams {
        fs: 48000,
        ir_length_ms: 200.0,
        room: RoomSpec { rt60_override: 0.2, ..RoomSpec::default() },
        ..RenderParams::default()
    }
}

fn render(p: RenderParams) -> RenderResult {
    ae::render(&speaker(), None, &FakeHrtf::new(48000), &p, None).unwrap()
}

fn dry() -> RenderParams {
    RenderParams { room_enabled: false, ..params() }
}

#[test]
fn render_shapes_and_headroom() {
    let res = render(params());
    let lengths: std::collections::HashSet<usize> = res.irs.iter().map(Vec::len).collect();
    assert_eq!(lengths.len(), 1);
    for ear in ['L', 'R'] {
        let mono: Vec<f64> = as_f64(res.ir(&format!("L{ear}")))
            .iter()
            .zip(as_f64(res.ir(&format!("R{ear}"))))
            .map(|(a, b)| a + b)
            .collect();
        let peak = spectrum(&mono, 1 << 16).into_iter().fold(0.0, f64::max);
        assert!(20.0 * peak.log10() < 3.0);
    }
}

#[test]
fn ipsilateral_louder_and_symmetric() {
    let res = render(dry());
    let e = |k: &str| energy(res.ir(k));
    assert!(e("LL") > e("LR"));
    assert!(e("RR") > e("RL"));
    assert!(close(e("LL"), e("RR"), e("LL") * 0.02));
}

#[test]
fn room_adds_tail() {
    let dry = render(dry());
    let wet = render(RenderParams { room_enabled: true, ..params() });
    let tail = (0.05 * 48000.0) as usize;
    assert!(energy(&wet.ir("LL")[tail..]) > 10.0 * energy(&dry.ir("LL")[tail..]) + 1e-12);
}

#[test]
fn balance_and_offset() {
    let right = render(RenderParams { balance_db: 6.0, ..dry() });
    assert!(energy(right.ir("RR")) > 3.0 * energy(right.ir("LL")));
    let mut seat = dry();
    seat.room.listener_offset = 0.6;
    let res = render(seat);
    assert!(energy(res.ir("LL")) > energy(res.ir("RR")));
    for v in &res.irs {
        // aucun repliement circulaire : fin silencieuse
        let tail_max = v[v.len() - 200..].iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(tail_max < 1e-3);
    }
}

#[test]
fn tone_controls() {
    let flat = render(dry());
    let bass = render(RenderParams { bass_db: 6.0, ..dry() });
    let n = 1 << 15;
    let f = dsp::rfftfreq(n, 48000.0);
    let (sf, sb) = (spectrum(&as_f64(flat.ir("LL")), n), spectrum(&as_f64(bass.ir("LL")), n));
    let ratio: Vec<f64> = sb.iter().zip(&sf).map(|(b, a)| 20.0 * (b / a).log10()).collect();
    let band = |lo: f64, hi: f64| {
        let v: Vec<f64> = f.iter().zip(&ratio).filter(|(x, _)| **x > lo && **x < hi).map(|(_, r)| *r).collect();
        dsp::mean(&v)
    };
    assert!(band(30.0, 60.0) - band(2000.0, 4000.0) > 4.0);
}

#[test]
fn rate_mismatch() {
    let p = RenderParams { fs: 48000, ..RenderParams::default() };
    assert!(ae::render(&speaker(), None, &FakeHrtf::new(44100), &p, None).is_err());
}

#[test]
fn write_irs() {
    let (_tmp, paths) = temp_paths();
    let res = render(dry());
    let out = ae::write_irs(&res, &paths.filters_dir()).unwrap();
    let mut keys: Vec<&str> = out.keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(keys, PAIRS);
    let reader = hound::WavReader::open(&out["LL"]).unwrap();
    let spec = reader.spec();
    assert_eq!(spec.sample_rate, 48000);
    assert_eq!(spec.sample_format, hound::SampleFormat::Float);
    assert_eq!(spec.bits_per_sample, 32);
}

#[test]
fn equalizer_apo_config_routes_four_channels() {
    let (_tmp, paths) = temp_paths();
    let out = ae::write_irs(&render(dry()), &paths.filters_dir()).unwrap();
    let conf = ae::equalizer_apo_config(&out);
    for k in PAIRS {
        assert!(conf.contains(&format!("Channel: V{k}\nConvolution: ")), "{k}");
    }
    assert!(conf.contains("Copy: L=VLL+VRL R=VLR+VRR"));
}

// --- Lecteur SOFA (KEMAR du système, s'il est installé) ---------------------------

#[test]
fn sofa_kemar_left_source_reaches_left_ear_first() {
    let Some(path) = speaker_sim::download::SYSTEM_SOFA_PATHS.iter().map(std::path::Path::new).find(|p| p.is_file())
    else {
        eprintln!("KEMAR système absent : test ignoré");
        return;
    };
    for fs in [48000, 44100] {
        let hrtf = ae::SofaHrtf::open(path, fs).unwrap();
        use ae::HrtfProvider;
        assert_eq!(hrtf.fs(), fs);
        let h = hrtf.hrir([0.0, 1.0, 0.0]); // azimut 90° : à gauche
        let e = |x: &[f64]| x.iter().map(|v| v * v).sum::<f64>();
        assert!(e(&h.left) > 2.0 * e(&h.right), "{fs} Hz");
        assert!(h.delay_left <= h.delay_right, "{fs} Hz");
        let front = hrtf.hrir([1.0, 0.0, 0.0]);
        assert!((e(&front.left) / e(&front.right)).log10().abs() < 0.15, "{fs} Hz : face non symétrique");
    }
}
