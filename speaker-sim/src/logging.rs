//! Journal de l'application : `speaker-sim.log` dans le dossier d'état, tournant à 500 ko
//! (une sauvegarde `.1`), plus la sortie d'erreur en mode verbeux.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use log::{Level, LevelFilter, Log, Metadata, Record};

use crate::config::Paths;

const MAX_BYTES: u64 = 512_000;

struct FileLogger {
    path: PathBuf,
    file: Mutex<Option<File>>,
    stderr: bool,
    level: Level,
}

impl FileLogger {
    fn open(&self) -> Option<File> {
        OpenOptions::new().create(true).append(true).open(&self.path).ok()
    }

    fn rotate_if_needed(&self, slot: &mut Option<File>) {
        if fs::metadata(&self.path).is_ok_and(|m| m.len() >= MAX_BYTES) {
            *slot = None;
            let mut backup = self.path.clone().into_os_string();
            backup.push(".1");
            let _ = fs::rename(&self.path, PathBuf::from(backup));
        }
        if slot.is_none() {
            *slot = self.open();
        }
    }
}

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level && metadata.target().starts_with("speaker_sim")
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S,%3f");
        let line = format!("{stamp} {} {}\n", record.level(), record.args());
        if let Ok(mut slot) = self.file.lock() {
            self.rotate_if_needed(&mut slot);
            if let Some(f) = slot.as_mut() {
                let _ = f.write_all(line.as_bytes());
            }
        }
        if self.stderr {
            eprint!("{}", record.args());
            eprintln!();
        }
    }

    fn flush(&self) {
        if let Ok(mut slot) = self.file.lock() {
            if let Some(f) = slot.as_mut() {
                let _ = f.flush();
            }
        }
    }
}

static LOGGER: OnceLock<FileLogger> = OnceLock::new();

/// Installe le journal (une seule fois par processus).
pub fn setup_logging(paths: &Paths, verbose: bool) {
    if LOGGER.get().is_some() {
        return;
    }
    let _ = paths.ensure();
    let level = if verbose { Level::Debug } else { Level::Info };
    let logger = LOGGER.get_or_init(|| FileLogger {
        path: paths.log_file(),
        file: Mutex::new(None),
        stderr: verbose,
        level,
    });
    if log::set_logger(logger).is_ok() {
        log::set_max_level(if verbose { LevelFilter::Debug } else { LevelFilter::Info });
    }
}
