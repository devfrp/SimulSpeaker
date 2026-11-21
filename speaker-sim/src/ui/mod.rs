//! Interface graphique (egui) — « console de salle ».
//!
//! Disposition :
//!
//! * barre supérieure : chaîne casque → enceinte → salle, état, comparaison A/B, langue,
//!   bouton d'alimentation ;
//! * gauche : 01 Casque (recherche AutoEq + imports), 02 Enceintes (filtre studio / hi-fi) ;
//! * centre : vue en plan interactive de la pièce (traitement, matériaux, rayons des
//!   premières réflexions ; l'auditeur et les enceintes se déplacent à la souris) ;
//! * droite : réponse simulée ou RT60 par octave ;
//! * bas : réglages en onglets, chacun avec son interrupteur.
//!
//! Toutes les opérations longues passent par [`GuiApp::run_task`] (fil de travail).
//! En mode « Auto », chaque changement est appliqué après une courte pause ; la
//! bascule A/B du moteur évite toute coupure.

mod panels;
mod plan;
mod plots;
mod style;

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use eframe::egui;
use serde_json::{Value, json};

use crate::app::{AppError, HeadphoneSummary, SpeakerSimApp, changes};
use crate::audio_engine::{HeadphoneFileKind, RenderResult};
use crate::config::{APP_ID, APP_NAME, HeadphoneEntry, Json, ROOM_KEYS, Settings};
use crate::download::{AUTOEQ_TARGETS, search_text};
use crate::i18n::{num, set_language, tr, tr_message, trf};
use crate::logging::setup_logging;
use crate::system::Sink;

const AUTO_DELAY: Duration = Duration::from_millis(1500);
const STATE_PERIOD: Duration = Duration::from_secs(3);
const SLIDER_DEBOUNCE: Duration = Duration::from_millis(350);

type DoneFn = Box<dyn FnOnce(&mut GuiApp, Box<dyn Any + Send>)>;
type ErrorFn = Box<dyn FnOnce(&mut GuiApp, String)>;

enum TaskMsg {
    Progress(u64, f64, String),
    Done(u64, Box<dyn Any + Send>),
    Failed(u64, String),
}

struct Handlers {
    quiet: bool,
    silent: bool,
    on_done: DoneFn,
    on_error: Option<ErrorFn>,
}

/// Options d'une tâche de fond.
#[derive(Default)]
struct TaskOpts {
    /// Message d'état affiché au départ.
    message: &'static str,
    /// Tâche discrète : ne bloque pas les boutons.
    quiet: bool,
    on_error: Option<ErrorFn>,
}

/// État de la simulation (lu en arrière-plan : systemctl, runtime.json).
struct StateSnapshot {
    running: bool,
    slots: Vec<String>,
    previous_sink: Option<String>,
}

enum Dialog {
    Error(String),
    Import { path: PathBuf, kind: HeadphoneFileKind, name: String, target: usize },
}

pub struct GuiApp {
    app: Arc<SpeakerSimApp>,
    /// Copie des réglages (et leur forme JSON), rafraîchie après chaque modification.
    s: Settings,
    sj: Json,
    // casques
    hp_entries: Vec<HeadphoneEntry>,
    hp_search: Vec<String>,
    hp_filtered: Vec<usize>,
    hp_query: String,
    hp_loaded: bool,
    hp_error: Option<String>,
    hp_reveal: bool,
    hp_summaries: HashMap<String, Option<HeadphoneSummary>>,
    hp_pending: HashSet<String>,
    alt_cache: Option<(String, Vec<HeadphoneEntry>)>,
    // enceintes
    spk_query: String,
    spk_category: String,
    spk_reveal: bool,
    // affichage
    source_tab: usize,
    analysis_tab: usize,
    show_settings: bool,
    settings_tab: String,
    last_order: i64,
    // état
    busy: bool,
    dirty: bool,
    pending_apply: bool,
    auto_deadline: Option<Instant>,
    running: bool,
    slots: Vec<String>,
    previous_sink: Option<String>,
    state_rx: Option<Receiver<StateSnapshot>>,
    next_state_poll: Instant,
    sinks: Vec<Sink>,
    status: String,
    progress: f32,
    result: Option<Arc<RenderResult>>,
    // tâches
    tx: Sender<TaskMsg>,
    rx: Receiver<TaskMsg>,
    next_task: u64,
    handlers: HashMap<u64, Handlers>,
    ctx: egui::Context,
    // interactions
    plan: plan::PlanState,
    slider_preview: Option<(&'static str, f64)>,
    slider_pending: Option<(&'static str, f64, Instant)>,
    dialog: Option<Dialog>,
}

impl GuiApp {
    fn new(cc: &eframe::CreationContext<'_>, app: SpeakerSimApp) -> Self {
        style::install_fonts(&cc.egui_ctx);
        style::install_style(&cc.egui_ctx);
        let (tx, rx) = channel();
        let s = app.settings();
        let extra = |k: &str| s.extra.get(k).cloned();
        let analysis_tab = extra("analysis_tab").and_then(|v| v.as_u64()).filter(|t| *t <= 1).unwrap_or(1) as usize;
        let mut gui = GuiApp {
            sj: s.to_json(),
            hp_entries: vec![],
            hp_search: vec![],
            hp_filtered: vec![],
            hp_query: String::new(),
            hp_loaded: false,
            hp_error: None,
            hp_reveal: false,
            hp_summaries: HashMap::new(),
            hp_pending: HashSet::new(),
            alt_cache: None,
            spk_query: String::new(),
            spk_category: String::new(),
            spk_reveal: true,
            source_tab: 0,
            analysis_tab,
            show_settings: extra("show_settings").and_then(|v| v.as_bool()).unwrap_or(true),
            settings_tab: extra("settings_tab").and_then(|v| v.as_str().map(str::to_string)).unwrap_or("room".into()),
            last_order: if s.reflection_order > 0 { s.reflection_order } else { 2 },
            busy: false,
            dirty: false,
            pending_apply: false,
            auto_deadline: None,
            running: false,
            slots: vec![],
            previous_sink: None,
            state_rx: None,
            next_state_poll: Instant::now(),
            sinks: vec![],
            status: tr("PRÊT"),
            progress: 0.0,
            result: None,
            tx,
            rx,
            next_task: 0,
            handlers: HashMap::new(),
            ctx: cc.egui_ctx.clone(),
            plan: plan::PlanState::default(),
            slider_preview: None,
            slider_pending: None,
            dialog: None,
            s,
            app: Arc::new(app),
        };
        gui.set_imports_only();
        gui.refresh_sinks();
        gui.run_task(
            |app, progress| app.headphones(Some(progress)).map(|h| h.as_ref().clone()),
            |gui, entries: Vec<HeadphoneEntry>| gui.headphones_loaded(entries),
            TaskOpts {
                message: "Index AutoEq…",
                quiet: true,
                on_error: Some(Box::new(|gui, msg| gui.hp_error = Some(msg))),
            },
        );
        gui
    }

    fn auto_apply_enabled(&self) -> bool {
        self.s.extra.get("auto_apply").and_then(Value::as_bool).unwrap_or(true)
    }

    // -- état -----------------------------------------------------------------
    fn reload_settings(&mut self) {
        self.s = self.app.settings();
        self.sj = self.s.to_json();
        if self.s.reflection_order > 0 {
            self.last_order = self.s.reflection_order;
        }
    }

    fn field_f64(&self, key: &str) -> f64 {
        self.sj.get(key).and_then(Value::as_f64).unwrap_or(0.0)
    }

    fn field(&self, key: &str) -> Value {
        self.sj.get(key).cloned().unwrap_or(Value::Null)
    }

    /// Réglages affichés (aperçu d'un curseur en cours de déplacement compris).
    fn view_settings(&self) -> Settings {
        match self.slider_preview {
            Some((k, v)) => self.s.with(&changes(json!({ k: v }))),
            None => self.s.clone(),
        }
    }

    fn poll_state(&mut self) {
        if self.state_rx.is_some() {
            return;
        }
        self.next_state_poll = Instant::now() + STATE_PERIOD;
        if !self.app.supports_live {
            return;
        }
        let (tx, rx) = channel();
        let app = self.app.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let slots = app.systemd.active_slots();
            let running = !slots.is_empty() || app.systemd.legacy_active();
            let previous_sink = app.store.load_runtime().get("previous_sink").and_then(Value::as_str).map(str::to_string);
            let _ = tx.send(StateSnapshot { running, slots, previous_sink });
            ctx.request_repaint();
        });
        self.state_rx = Some(rx);
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
        self.plan.preview = None;
        self.poll_state();
        if self.auto_apply_enabled() && self.app.supports_live && self.running {
            self.auto_deadline = Some(Instant::now() + AUTO_DELAY);
        }
    }

    fn error(&mut self, msg: impl Into<String>) {
        self.dialog = Some(Dialog::Error(msg.into()));
    }

    fn try_update(&mut self, change: Json) -> bool {
        match self.app.update_settings(&change) {
            Ok(_) => {
                self.reload_settings();
                true
            }
            Err(e) => {
                self.error(tr_message(&e.0));
                false
            }
        }
    }

    /// Préférence d'affichage seule : enregistrée, sans régénérer les filtres.
    fn on_ui_state(&mut self, key: &str, value: Value) {
        if let Err(e) = self.app.set_extra(key, value) {
            log::warn!("Préférence d'affichage non enregistrée : {e}");
        }
        self.reload_settings();
    }

    // -- tâches ---------------------------------------------------------------
    fn run_task<T, J, D>(&mut self, job: J, on_done: D, opts: TaskOpts)
    where
        T: Send + 'static,
        J: FnOnce(&SpeakerSimApp, &dyn Fn(f64, &str)) -> Result<T, AppError> + Send + 'static,
        D: FnOnce(&mut GuiApp, T) + 'static,
    {
        let quiet = opts.quiet;
        // Une tâche discrète sans message (résumé…) ne touche ni à l'état ni à la progression.
        let silent = quiet && opts.message.is_empty();
        if !quiet {
            self.busy = true;
        }
        if !silent {
            self.status = tr_message(opts.message).to_uppercase();
            self.progress = 0.0;
        }
        let id = self.next_task;
        self.next_task += 1;
        self.handlers.insert(
            id,
            Handlers {
                quiet,
                silent,
                on_done: Box::new(move |gui, value| {
                    if let Ok(v) = value.downcast::<T>() {
                        on_done(gui, *v);
                    }
                }),
                on_error: opts.on_error,
            },
        );
        let tx = self.tx.clone();
        let app = self.app.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let progress_tx = tx.clone();
            let progress_ctx = ctx.clone();
            let report = move |f: f64, m: &str| {
                let _ = progress_tx.send(TaskMsg::Progress(id, f, m.to_string()));
                progress_ctx.request_repaint();
            };
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&app, &report)));
            let msg = match outcome {
                Ok(Ok(v)) => TaskMsg::Done(id, Box::new(v)),
                Ok(Err(e)) => TaskMsg::Failed(id, e.0),
                Err(panic) => {
                    let text = panic
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                        .unwrap_or_default();
                    log::error!("Erreur inattendue : {text}");
                    TaskMsg::Failed(id, format!("Erreur inattendue : {text}"))
                }
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    fn finish_task(&mut self, h: &Handlers) {
        if !h.quiet {
            self.busy = false;
        }
        if !h.silent {
            self.progress = 0.0;
        }
        self.state_rx = None;
        self.poll_state();
        if self.pending_apply && !self.busy {
            self.pending_apply = false;
            self.auto_deadline = Some(Instant::now() + AUTO_DELAY);
        }
    }

    /// Messages des fils de travail, minuteries.
    fn pump(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                TaskMsg::Progress(id, f, m) => {
                    if self.handlers.get(&id).is_some_and(|h| !h.silent) {
                        self.progress = f as f32;
                        self.status = tr_message(&m).to_uppercase();
                    }
                }
                TaskMsg::Done(id, value) => {
                    if let Some(h) = self.handlers.remove(&id) {
                        self.finish_task(&h);
                        (h.on_done)(self, value);
                    }
                }
                TaskMsg::Failed(id, msg) => {
                    if let Some(h) = self.handlers.remove(&id) {
                        self.finish_task(&h);
                        if !h.silent {
                            self.status = tr("ERREUR");
                        }
                        let msg = tr_message(&msg);
                        match h.on_error {
                            Some(f) => f(self, msg),
                            None => self.error(msg),
                        }
                    }
                }
            }
        }
        if let Some(rx) = &self.state_rx {
            if let Ok(snap) = rx.try_recv() {
                self.running = snap.running;
                self.slots = snap.slots;
                self.previous_sink = snap.previous_sink;
                self.state_rx = None;
            }
        }
        let now = Instant::now();
        if now >= self.next_state_poll {
            self.poll_state();
        }
        if let Some((key, v, at)) = self.slider_pending {
            if now >= at {
                self.slider_pending = None;
                self.slider_preview = None;
                self.on_setting(changes(json!({ key: v })));
            }
        }
        if self.auto_deadline.is_some_and(|t| now >= t) {
            self.auto_deadline = None;
            if self.dirty {
                if self.busy {
                    self.pending_apply = true;
                } else {
                    self.on_apply(true);
                }
            }
        }
        let mut wake = self.next_state_poll;
        for t in [self.auto_deadline, self.slider_pending.map(|p| p.2)].into_iter().flatten() {
            wake = wake.min(t);
        }
        self.ctx.request_repaint_after(wake.saturating_duration_since(now));
    }

    // -- casques --------------------------------------------------------------
    fn set_entries(&mut self, entries: Vec<HeadphoneEntry>) {
        self.hp_search = entries.iter().map(search_text).collect();
        self.hp_entries = entries;
        self.apply_hp_filter();
    }

    fn set_imports_only(&mut self) {
        let imports = self.app.imported_headphones();
        self.set_entries(imports);
    }

    fn reload_headphones(&mut self) {
        let mut all = self.app.imported_headphones();
        if let Some(index) = self.app.cached_headphones() {
            all.extend(index.iter().cloned());
        }
        self.set_entries(all);
        self.alt_cache = None;
    }

    fn apply_hp_filter(&mut self) {
        let q = crate::download::Query::new(&self.hp_query);
        self.hp_filtered = (0..self.hp_entries.len()).filter(|&i| q.matches_lowercase(&self.hp_search[i])).collect();
    }

    fn headphones_loaded(&mut self, entries: Vec<HeadphoneEntry>) {
        let n = entries.len();
        let mut all = self.app.imported_headphones();
        all.extend(entries);
        self.set_entries(all);
        self.hp_loaded = true;
        self.hp_reveal = true;
        self.alt_cache = None;
        self.status = trf("INDEX AUTOEQ CHARGÉ · {n} MESURES", &[("n", &n.to_string())]);
    }

    fn alternatives(&mut self) -> Vec<HeadphoneEntry> {
        let hp = self.s.headphone.clone();
        let key = hp.as_ref().map(|h| h.path.clone()).unwrap_or_default();
        if let Some((k, v)) = &self.alt_cache {
            if *k == key {
                return v.clone();
            }
        }
        let alts = self.app.alternatives(hp.as_ref());
        if self.app.cached_headphones().is_some() {
            self.alt_cache = Some((key, alts.clone()));
        }
        alts
    }

    /// Résumé de l'EQ du casque courant (calculé en arrière-plan, une fois par casque).
    fn headphone_summary_text(&mut self) -> String {
        let Some(hp) = self.s.headphone.clone() else { return String::new() };
        let key = format!("{}|{}", hp.path, hp.target);
        if let Some(summary) = self.hp_summaries.get(&key) {
            return summary.map(summary_text).unwrap_or_default();
        }
        if self.hp_pending.insert(key.clone()) {
            let (k1, k2) = (key.clone(), key);
            self.run_task(
                |app, _| app.headphone_summary(),
                move |gui, summary: Option<HeadphoneSummary>| {
                    gui.hp_pending.remove(&k1);
                    gui.hp_summaries.insert(k1, summary);
                },
                TaskOpts {
                    quiet: true,
                    on_error: Some(Box::new(move |gui, _| {
                        gui.hp_pending.remove(&k2);
                        gui.hp_summaries.insert(k2, None);
                    })),
                    ..Default::default()
                },
            );
        }
        String::new()
    }

    // -- actions --------------------------------------------------------------
    fn on_headphone(&mut self, entry: &HeadphoneEntry) {
        if let Err(e) = self.app.select_headphone(Some(entry)) {
            return self.error(tr_message(&e.0));
        }
        self.reload_settings();
        self.mark_dirty();
    }

    fn on_speaker(&mut self, id: &str) {
        if let Err(e) = self.app.select_speaker(id) {
            return self.error(tr_message(&e.0));
        }
        self.reload_settings();
        self.mark_dirty();
    }

    fn on_setting(&mut self, mut change: Json) {
        if change.keys().any(|k| ROOM_KEYS.contains(&k.as_str())) {
            change.insert("room_preset".into(), json!(""));
        }
        let placement = ["speaker_angle", "speaker_distance", "front_gap", "listener_offset"];
        if change.keys().any(|k| placement.contains(&k.as_str())) && !self.s.placement_custom {
            change.insert("placement_custom".into(), json!(true)); // glisser sur le plan réactive
        }
        if self.try_update(change) {
            self.mark_dirty();
        }
    }

    fn on_preset(&mut self, key: &str) {
        if let Err(e) = self.app.apply_room_preset(key) {
            return self.error(tr_message(&e.0));
        }
        self.reload_settings();
        self.mark_dirty();
    }

    /// Bascule entre les configurations A et B ; l'autre est gardée telle quelle.
    fn on_ab(&mut self, slot: &str) {
        if slot == self.app.ab_slot() {
            return;
        }
        if let Err(e) = self.app.ab_switch(slot) {
            return self.error(tr_message(&e.0));
        }
        self.reload_settings();
        self.spk_reveal = true;
        self.mark_dirty();
        self.status = trf("CONFIGURATION {slot}", &[("slot", slot)]);
    }

    fn on_language(&mut self, code: &str) {
        if self.try_update(changes(json!({"language": code}))) {
            set_language(code);
            self.status = tr("PRÊT");
        }
    }

    fn on_toggle(&mut self) {
        if self.running {
            self.run_task(
                |app, _| app.stop(),
                |gui, ()| gui.status = tr("SIMULATION ARRÊTÉE"),
                TaskOpts { message: "Arrêt…", ..Default::default() },
            );
        } else {
            self.run_task(
                |app, p| app.start(Some(p)),
                GuiApp::done_apply,
                TaskOpts { message: "Démarrage…", ..Default::default() },
            );
        }
    }

    fn on_apply(&mut self, auto: bool) {
        let message = if auto { "Application automatique…" } else { "Génération des filtres…" };
        self.run_task(
            |app, p| if app.is_running() { app.apply(Some(p)) } else { app.build_filters(Some(p)) },
            GuiApp::done_apply,
            TaskOpts { message, ..Default::default() },
        );
    }

    fn done_apply(&mut self, result: Arc<RenderResult>) {
        self.dirty = false;
        self.show_result(Some(result));
    }

    fn show_result(&mut self, result: Option<Arc<RenderResult>>) {
        if let Some(r) = result {
            self.result = Some(r);
        }
        // État réel lu juste après la tâche ; l'instance démarrée apparaît aussitôt.
        let live = self.app.supports_live && self.app.is_running();
        self.running = live || self.running;
        self.status = tr(if live { "FILTRES APPLIQUÉS" } else { "FILTRES GÉNÉRÉS" });
    }

    fn on_import(&mut self) {
        let start = self.app.paths.state_dir.parent().map(PathBuf::from).unwrap_or_default();
        let picked = rfd::FileDialog::new()
            .set_title(tr("Importer une mesure ou un EQ de casque"))
            .set_directory(start)
            .add_filter(tr("Mesures et EQ"), &["txt", "csv", "frd", "tsv"])
            .add_filter(tr("Tous les fichiers"), &["*"])
            .pick_file();
        let Some(path) = picked else { return };
        let kind = match crate::config::read_text_lossy(&path) {
            Ok(text) => crate::audio_engine::detect_headphone_file_kind(&text),
            Err(e) => return self.error(trf("Lecture impossible : {e}", &[("e", &e.to_string())])),
        };
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        self.dialog = Some(Dialog::Import { path, kind, name, target: 0 });
    }

    fn finish_import(&mut self, path: PathBuf, kind: HeadphoneFileKind, name: String, target: usize) {
        let target = if kind == HeadphoneFileKind::Measurement { AUTOEQ_TARGETS[target] } else { "" };
        match self.app.import_headphone(&path, &name, target) {
            Ok(entry) => {
                self.reload_settings();
                self.reload_headphones();
                self.hp_reveal = true;
                self.status = trf("CASQUE IMPORTÉ : {x}", &[("x", &entry.label())]).to_uppercase();
                self.mark_dirty();
            }
            Err(e) => self.error(tr_message(&e.0)),
        }
    }

    fn on_remove_import(&mut self, entry: &HeadphoneEntry) {
        if let Err(e) = self.app.remove_import(entry) {
            return self.error(tr_message(&e.0));
        }
        self.reload_settings();
        self.reload_headphones();
        self.mark_dirty();
    }

    fn on_import_target(&mut self, target: &str) {
        let Some(hp) = self.s.headphone.clone() else { return };
        if !hp.is_import() || target == hp.target {
            return;
        }
        if let Err(e) = self.app.set_import_target(&hp, target) {
            return self.error(tr_message(&e.0));
        }
        self.reload_settings();
        self.reload_headphones();
        self.hp_reveal = true;
        self.mark_dirty();
    }

    fn on_export(&mut self) {
        let start = self.app.paths.state_dir.parent().map(PathBuf::from).unwrap_or_default();
        let Some(folder) =
            rfd::FileDialog::new().set_title(tr("Dossier d'export des filtres")).set_directory(start).pick_folder()
        else {
            return;
        };
        let shown = folder.display().to_string().to_uppercase();
        let dest = folder.clone();
        self.run_task(
            move |app, p| app.export(&dest, Some(p)),
            move |gui, paths: Vec<PathBuf>| {
                gui.show_result(gui.app.last_result());
                gui.status = trf("{n} FICHIERS EXPORTÉS VERS {folder}", &[("n", &paths.len().to_string()), ("folder", &shown)]);
            },
            TaskOpts { message: "Export des filtres…", ..Default::default() },
        );
    }

    fn on_autostart(&mut self, enabled: bool) {
        self.run_task(
            move |app, _| app.set_autostart(enabled),
            move |gui, ()| {
                gui.reload_settings();
                gui.status = tr(if enabled { "AUTO-DÉMARRAGE ACTIVÉ" } else { "AUTO-DÉMARRAGE DÉSACTIVÉ" });
            },
            TaskOpts {
                message: "Configuration de l'auto-démarrage…",
                on_error: Some(Box::new(|gui, msg| {
                    gui.reload_settings();
                    gui.error(msg);
                })),
                ..Default::default()
            },
        );
    }

    fn refresh_sinks(&mut self) {
        if !self.app.supports_live {
            return;
        }
        self.run_task(
            |app, _| app.pw.sinks(false).map_err(AppError::from),
            |gui, sinks: Vec<Sink>| gui.sinks = sinks,
            TaskOpts {
                quiet: true,
                on_error: Some(Box::new(|gui, e| gui.status = trf("PIPEWIRE INDISPONIBLE : {e}", &[("e", &e)]))),
                ..Default::default()
            },
        );
    }
}

fn summary_text(s: HeadphoneSummary) -> String {
    let sign = if s.preamp_db >= 0.0 { "+" } else { "−" };
    let pre = trf("préampli {v} dB", &[("v", &format!("{sign}{}", num(s.preamp_db.abs(), 1)))]);
    if s.points > 0 {
        format!("{} · {pre}", trf("EQ graphique · {n} points", &[("n", &s.points.to_string())]))
    } else {
        format!("{} · {pre}", trf("{n} filtres", &[("n", &s.filters.to_string())]))
    }
}

impl eframe::App for GuiApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.pump();
        self.draw(ui);
    }
}

pub fn run() -> ExitCode {
    let app = match SpeakerSimApp::new() {
        Ok(app) => app,
        Err(e) => {
            set_language("");
            eprintln!("{}", trf("Initialisation impossible : {e}", &[("e", &e.0)]));
            return ExitCode::FAILURE;
        }
    };
    setup_logging(&app.paths, false);
    set_language(&app.settings().language);
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(APP_NAME)
        .with_app_id(APP_ID)
        .with_inner_size([1440.0, 900.0])
        .with_min_inner_size([960.0, 640.0]);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../../packaging/icon.png")) {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    let result = eframe::run_native(APP_NAME, options, Box::new(|cc| Ok(Box::new(GuiApp::new(cc, app)))));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{APP_NAME} : {e}");
            ExitCode::FAILURE
        }
    }
}

