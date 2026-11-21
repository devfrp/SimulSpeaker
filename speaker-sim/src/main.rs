//! Point d'entrée : interface graphique par défaut, ou commandes en ligne.
//!
//! ```text
//! speaker-sim                                  # interface graphique
//! speaker-sim --search-headphones "hd 650"     # chercher un casque
//! speaker-sim --set-headphone "oratory1990/over-ear/Sennheiser HD 650"
//! speaker-sim --set-speaker "KEF R3" --start   # choisir et démarrer
//! speaker-sim --status
//! speaker-sim --stop
//! speaker-sim --autostart on
//! speaker-sim --export ~/irs                   # Windows/macOS : filtres portables
//! speaker-sim --import-headphone mesure.txt --name "Mon casque" --target "Harman in-ear 2019"
//! ```
//!
//! `--hook-started` / `--hook-stopped` sont appelés par l'unité systemd.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, ValueEnum};

use speaker_sim::app::{AppError, SpeakerSimApp};
use speaker_sim::config::{APP_NAME, APP_VERSION, ConfigStore, Paths, py_float};
use speaker_sim::logging::setup_logging;
use speaker_sim::system::{PipeWire, default_runner, hook_started, hook_stopped};

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum OnOff {
    On,
    Off,
}

#[derive(Parser, Debug, Default)]
#[command(name = "speaker-sim", about = "Simulation de haut-parleurs au casque (PipeWire)", disable_version_flag = true)]
struct Args {
    /// générer les filtres et démarrer
    #[arg(long)]
    start: bool,
    /// synonyme de --start
    #[arg(long)]
    apply: bool,
    /// arrêter et restaurer la sortie
    #[arg(long)]
    stop: bool,
    /// afficher l'état (JSON)
    #[arg(long)]
    status: bool,
    /// générer les filtres sans démarrer
    #[arg(long)]
    build: bool,
    /// exporter les filtres (WAV, true stereo, Equalizer APO)
    #[arg(long, value_name = "DOSSIER")]
    export: Option<PathBuf>,
    /// afficher la version
    #[arg(long)]
    version: bool,
    /// lister les haut-parleurs du catalogue
    #[arg(long)]
    list_speakers: bool,
    /// chercher dans l'index AutoEq
    #[arg(long, value_name = "TEXTE")]
    search_headphones: Option<String>,
    /// casque, par chemin AutoEq (source/banc/modèle)
    #[arg(long, value_name = "CHEMIN")]
    set_headphone: Option<String>,
    /// haut-parleur, par identifiant
    #[arg(long, value_name = "ID")]
    set_speaker: Option<String>,
    /// auto-démarrage à la connexion
    #[arg(long, value_enum)]
    autostart: Option<OnOff>,
    #[arg(short, long)]
    verbose: bool,
    /// importer une mesure (fréquence, dB) ou un EQ AutoEq / Equalizer APO
    #[arg(long, value_name = "FICHIER")]
    import_headphone: Option<PathBuf>,
    /// nom du casque importé
    #[arg(long, value_name = "NOM")]
    name: Option<String>,
    /// cible pour une mesure brute (défaut : Harman over-ear 2018)
    #[arg(long, value_name = "CIBLE")]
    target: Option<String>,
    // Hooks systemd : argument facultatif = instance (a/b) ; absent = unité v1.0.
    #[arg(long, num_args = 0..=1, default_missing_value = "", hide = true)]
    hook_started: Option<String>,
    #[arg(long, num_args = 0..=1, default_missing_value = "", hide = true)]
    hook_stopped: Option<String>,
}

impl Args {
    fn is_cli(&self) -> bool {
        self.start
            || self.apply
            || self.stop
            || self.status
            || self.build
            || self.list_speakers
            || self.search_headphones.is_some()
            || self.set_headphone.is_some()
            || self.set_speaker.is_some()
            || self.autostart.is_some()
            || self.export.is_some()
            || self.import_headphone.is_some()
    }
}

fn run_hooks(args: &Args) -> ExitCode {
    let store = ConfigStore::new(Paths::default_paths());
    let pw = PipeWire::new(default_runner());
    let log = |m: &str| eprintln!("{m}");
    // Un hook ne doit jamais faire échouer l'unité.
    let res = match (&args.hook_started, &args.hook_stopped) {
        (Some(slot), _) => hook_started(&store, &pw, slot, &log),
        (_, Some(slot)) => hook_stopped(&store, &pw, slot, &log),
        _ => Ok(()),
    };
    if let Err(e) = res {
        eprintln!("hook: {e}");
    }
    ExitCode::SUCCESS
}

fn run_cli(args: &Args) -> Result<(), AppError> {
    let app = SpeakerSimApp::new()?;
    setup_logging(&app.paths, args.verbose);
    let progress = |fraction: f64, message: &str| eprintln!("[{:3.0} %] {message}", fraction * 100.0);

    if args.list_speakers {
        for spk in &app.speakers {
            let score = spk.pref_score_wsub.map_or("—".to_string(), py_float);
            println!("{:<40} {:<7} score {score:>4}  {}", spk.id, spk.category, spk.shape);
        }
    }
    if let Some(query) = &args.search_headphones {
        for e in app.search_headphones(query, Some(50))? {
            println!("{}\t({}, {})", e.path, e.source, e.rig);
        }
    }
    if let Some(wanted) = &args.set_headphone {
        let entry = match app.headphones(None)?.iter().find(|e| &e.path == wanted) {
            Some(e) => e.clone(),
            None => {
                let mut matches = app.search_headphones(wanted, Some(2))?;
                if matches.len() != 1 {
                    return Err(AppError(
                        "Casque ambigu ou introuvable ; utilisez le chemin affiché par --search-headphones".into(),
                    ));
                }
                matches.remove(0)
            }
        };
        app.select_headphone(Some(&entry))?;
        println!("Casque : {}", entry.label());
    }
    if let Some(file) = &args.import_headphone {
        let entry = app.import_headphone(file, args.name.as_deref().unwrap_or(""), args.target.as_deref().unwrap_or(""))?;
        println!("Casque importé et sélectionné : {}", entry.label());
    }
    if let Some(id) = &args.set_speaker {
        app.select_speaker(id)?;
        println!("Haut-parleur : {}", app.description());
    }
    if args.build {
        for note in &app.build_filters(Some(&progress))?.notes {
            println!("{note}");
        }
    }
    if let Some(dest) = &args.export {
        for path in app.export(dest, Some(&progress))? {
            println!("{}", path.display());
        }
    }
    if args.start || args.apply {
        for note in &app.start(Some(&progress))?.notes {
            println!("{note}");
        }
    }
    if args.stop {
        app.stop()?;
    }
    if let Some(state) = args.autostart {
        app.set_autostart(state == OnOff::On)?;
    }
    if args.status {
        println!("{}", serde_json::to_string_pretty(&app.status()).unwrap_or_default());
    }
    Ok(())
}

#[cfg(feature = "gui")]
fn run_gui() -> ExitCode {
    speaker_sim::ui::run()
}

#[cfg(not(feature = "gui"))]
fn run_gui() -> ExitCode {
    eprintln!("{APP_NAME} a été compilé sans interface graphique ; voir --help.");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args = Args::parse();
    if args.version {
        println!("{APP_NAME} {APP_VERSION}");
        return ExitCode::SUCCESS;
    }
    if args.hook_started.is_some() || args.hook_stopped.is_some() {
        return run_hooks(&args);
    }
    if !args.is_cli() {
        return run_gui();
    }
    match run_cli(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Erreur : {e}");
            ExitCode::FAILURE
        }
    }
}
