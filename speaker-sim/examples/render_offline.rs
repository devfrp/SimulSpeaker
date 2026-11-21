//! Exemple : générer les IR binaurales sans PipeWire ni interface graphique.
//!
//! ```text
//! cargo run --release --example render_offline -- "Genelec 8030C" "oratory1990/over-ear/Sennheiser HD 650" /tmp/irs
//! ```
//!
//! Télécharge (une fois) les mesures dans le cache de Speaker Sim, puis écrit
//! ir_LL.wav, ir_LR.wav, ir_RL.wav, ir_RR.wav dans le dossier indiqué : utilisables
//! dans n'importe quel convolveur (EasyEffects, Equalizer APO, REAPER…).

use std::path::Path;
use std::process::ExitCode;

use speaker_sim::audio_engine::{self as ae, RenderParams, RoomSpec, SpeakerResponse};
use speaker_sim::config::{Paths, load_speaker_catalog};
use speaker_sim::download::Downloader;

fn run(speaker_id: &str, headphone_path: &str, out_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let fs = 48000;
    let dl = Downloader::new(Paths::default_paths().cache_dir);
    let speaker = load_speaker_catalog()
        .into_iter()
        .find(|s| s.id == speaker_id)
        .ok_or_else(|| format!("haut-parleur inconnu : {speaker_id}"))?;
    let entry = dl
        .load_headphones(None)?
        .into_iter()
        .find(|e| e.path == headphone_path)
        .ok_or_else(|| format!("casque absent de l'index AutoEq : {headphone_path}"))?;

    let response = SpeakerResponse::load(&dl.ensure_speaker(&speaker, None)?)?;
    let eq = ae::load_headphone_eq(&dl.ensure_headphone_eq(&entry, None)?)?;
    let hp_fir = ae::headphone_eq_fir(&eq, f64::from(fs), ae::FIR_TAPS, false);
    let hrtf = ae::open_hrtf(&dl.ensure_hrtf("", None)?, fs)?;

    let params = RenderParams {
        fs,
        angle_deg: 30.0,
        distance: 2.0,
        room: RoomSpec { rt60_override: 0.35, ..RoomSpec::sized(4.5, 6.0, 2.6) },
        ..RenderParams::default()
    };
    let progress = |f: f64, m: &str| println!("{:5.1} %  {m}", f * 100.0);
    let result = ae::render(&response, Some(&hp_fir), hrtf.as_ref(), &params, Some(&progress))?;
    std::fs::create_dir_all(out_dir)?;
    for (key, path) in ae::write_irs(&result, out_dir)? {
        println!("{key} {}", path.display());
    }
    println!("Gain de normalisation : {:+.1} dB", result.gain_db);
    for note in &result.notes {
        println!("{note}");
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [speaker, headphone, out] = args.as_slice() else {
        eprintln!("usage : render_offline <haut-parleur> <chemin AutoEq du casque> <dossier>");
        return ExitCode::from(2);
    };
    match run(speaker, headphone, Path::new(out)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("erreur : {e}");
            ExitCode::FAILURE
        }
    }
}
