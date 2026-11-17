//! Speaker Sim : simulation de haut-parleurs mesurés dans une pièce, au casque.
//!
//! Moteur (`audio_engine`, `dsp`), données (`config`, `download`), intégration Linux
//! (`system` : PipeWire + systemd), orchestration (`app`) et interface (`ui`, option `gui`).

pub mod config;
pub mod dsp;
pub mod i18n;
pub mod logging;
