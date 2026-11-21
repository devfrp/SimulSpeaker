//! Rendu des panneaux : barre supérieure, sources (casque, enceintes), plan, analyse,
//! réglages en onglets (chacun avec son interrupteur), pied de page et dialogues.

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Frame, Id, Layout, Margin, Pos2, Rect, RichText, Sense,
    Stroke, StrokeKind, TextEdit, Ui, UiBuilder, Vec2,
    text::{LayoutJob, TextFormat},
};
use serde_json::{Value, json};

use super::style::{
    self, AMBER, BG, BORDER, BORDER_STRONG, DIM, MUTED, PANEL, PANEL2, TEAL, TEXT, alpha, body, caption, chip,
    mono, muted, muted_wrap, section_title, segmented, segmented_width, switch, tag, toggle,
};
use super::{Dialog, GuiApp, plan, plots};
use crate::app::changes;
use crate::audio_engine::HeadphoneFileKind;
use crate::config::{HeadphoneEntry, ROOM_PRESETS, room_preset};
use crate::download::{AUTOEQ_TARGETS, Query};
use crate::i18n::{self, LANGUAGES, num, tr, trf};

/// Onglet → réglage booléen piloté par son interrupteur.
const SWITCHES: &[(&str, &str)] = &[
    ("room", "room_enabled"),
    ("treatment", "treatment_enabled"),
    ("reverb", "late_enabled"),
    ("placement", "placement_custom"),
    ("headphone", "headphone_eq_enabled"),
    ("listening", "tone_enabled"),
    ("advanced", "advanced_enabled"),
];
const ROOM_DEPENDENT: &[&str] = &["treatment", "reflections", "reverb"];
const INERT_TEXT: &str = "Sans effet : la pièce est désactivée (onglet Salle).";

fn off_text(key: &str) -> &'static str {
    match key {
        "room" => "Pièce désactivée : écoute en champ libre, sans réflexions ni réverbération.",
        "treatment" => "Traitement désactivé : parois nues, sans panneaux, pièges ni nuage.",
        "reflections" => "Réflexions précoces désactivées : seule la réverbération tardive reste.",
        "reverb" => "Réverbération tardive désactivée : seules les réflexions précoces restent.",
        "placement" => "Placement standard : triangle stéréo à 30°, 2 m, auditeur centré.",
        "headphone" => "Correction du casque désactivée : réponse brute du casque.",
        "listening" => "Tonalité et balance désactivées (réponse neutre).",
        "advanced" => "Valeurs par défaut : égalisation champ diffus, fréquence automatique.",
        "output" => "Simulation arrêtée : le son va directement au casque.",
        _ => "",
    }
}

fn pct(v: f64) -> String {
    format!("{:.0} %", v * 100.0)
}
fn meters(v: f64) -> String {
    format!("{} m", num(v, 1))
}
fn meters2(v: f64) -> String {
    format!("{} m", num(v, 2))
}
fn db(v: f64) -> String {
    format!("{v:+.1} dB")
}
fn degrees(v: f64) -> String {
    format!("{v:.0}°")
}
fn seconds(v: f64) -> String {
    format!("{} s", num(v, 2))
}
fn offset(v: f64) -> String {
    if v.abs() < 1e-9 {
        tr("centré")
    } else {
        trf("{d} m {side}", &[("d", &num(v.abs(), 2)), ("side", &tr(if v > 0.0 { "gauche" } else { "droite" }))])
    }
}
fn tweeter(v: f64) -> String {
    if v <= 0.0 { tr("aux oreilles") } else { meters2(v) }
}
fn balance(v: f64) -> String {
    if v.abs() < 1e-9 {
        tr("centre")
    } else {
        trf("{v} dB {side}", &[("v", &num(v.abs(), 1)), ("side", &tr(if v > 0.0 { "droite" } else { "gauche" }))])
    }
}

/// Peint une carte dans `rect` et renvoie l'interface intérieure.
fn card_in(ui: &mut Ui, rect: Rect, margin: Margin) -> Ui {
    ui.painter().rect(rect, CornerRadius::same(14), PANEL, Stroke::new(1.0, BORDER), StrokeKind::Inside);
    let inner = Rect::from_min_max(
        rect.min + Vec2::new(f32::from(margin.left), f32::from(margin.top)),
        rect.max - Vec2::new(f32::from(margin.right), f32::from(margin.bottom)),
    );
    ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Min)))
}

/// Rangée de groupes de réglages : largeur proportionnelle, largeur minimale respectée.
fn groups(ui: &mut Ui, specs: &[(&str, f32, f32)], mut content: impl FnMut(usize, &mut Ui)) {
    let gap = 24.0;
    let avail = ui.available_width() - gap * (specs.len().saturating_sub(1)) as f32;
    let total: f32 = specs.iter().map(|s| s.1).sum();
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for (i, (title, stretch, min)) in specs.iter().enumerate() {
            let w = (avail * stretch / total).max(*min);
            ui.allocate_ui_with_layout(Vec2::new(w, 0.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(w);
                ui.spacing_mut().item_spacing.y = 8.0;
                if !title.is_empty() {
                    caption(ui, title);
                }
                content(i, ui);
            });
        }
    });
}

fn hint(text: String) -> RichText {
    RichText::new(text).color(MUTED)
}

fn search_field(ui: &mut Ui, query: &mut String, placeholder: String, enabled: bool) -> bool {
    let mut changed = false;
    ui.add_enabled_ui(enabled, |ui| {
        ui.horizontal(|ui| {
            let clear = !query.is_empty();
            let width = ui.available_width() - if clear { 32.0 } else { 0.0 };
            let resp = ui.add_sized([width, 30.0], TextEdit::singleline(query).hint_text(hint(placeholder)).margin(Margin::symmetric(10, 6)));
            changed |= resp.changed();
            if clear && ui.add_sized([24.0, 30.0], egui::Button::new("×").frame(false)).clicked() {
                query.clear();
                changed = true;
            }
        });
    });
    changed
}

/// Ligne de liste sélectionnable (survol, sélection ambre).
fn list_row(ui: &mut Ui, text: &str, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::click());
    if selected {
        ui.painter().rect_filled(rect, CornerRadius::same(6), alpha(AMBER, 40));
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(6), PANEL2);
    }
    let color = if selected { AMBER } else { TEXT };
    let galley = ui.painter().layout(text.to_string(), body(13.0), color, rect.width() - 16.0);
    let galley = if galley.rows.len() > 1 {
        let mut job = LayoutJob::simple_singleline(text.to_string(), body(13.0), color);
        job.wrap.max_width = rect.width() - 16.0;
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        ui.painter().layout_job(job)
    } else {
        galley
    };
    ui.painter().galley(Pos2::new(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0), galley, color);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

impl GuiApp {
    pub(super) fn draw(&mut self, root: &mut Ui) {
        let live = self.app.supports_live;
        egui::Panel::top("topbar")
            .frame(
                Frame::new().fill(Color32::from_rgba_unmultiplied(20, 21, 26, 235)).inner_margin(Margin {
                    left: 18,
                    right: 14,
                    top: 6,
                    bottom: 6,
                }),
            )
            .show(root, |ui| self.topbar(ui, live));
        egui::Panel::top("progress").exact_size(2.0).frame(Frame::new().fill(BG)).show_separator_line(false).show(root, |ui| {
            let r = ui.max_rect();
            if self.progress > 0.0 {
                let w = r.width() * self.progress.clamp(0.0, 1.0);
                ui.painter().rect_filled(Rect::from_min_size(r.min, Vec2::new(w, 2.0)), 0.0, AMBER);
            }
        });
        egui::Panel::bottom("footer")
            .frame(Frame::new().fill(BG).inner_margin(Margin { left: 18, right: 18, top: 3, bottom: 3 }))
            .show(root, |ui| self.footer(ui));
        egui::Panel::bottom("settings")
            .frame(Frame::new().fill(BG).inner_margin(Margin { left: 14, right: 14, top: 0, bottom: 8 }))
            .show_separator_line(false)
            .resizable(false)
            .show(root, |ui| self.settings_band(ui, live));
        egui::CentralPanel::default()
            .frame(Frame::new().fill(BG).inner_margin(Margin { left: 14, right: 14, top: 10, bottom: 8 }))
            .show(root, |ui| self.upper(ui));
        self.dialogs(root.ctx());
    }

    // -- barre supérieure -------------------------------------------------------
    fn topbar(&mut self, ui: &mut Ui, live: bool) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            let mut job = LayoutJob::default();
            job.append("● ", 0.0, TextFormat::simple(FontId::new(15.0, style::mono_bold()), TEAL));
            job.append("speaker-sim", 0.0, TextFormat::simple(FontId::new(15.0, style::mono_bold()), TEXT));
            ui.label(job);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                if live {
                    ui.add_enabled_ui(!self.busy, |ui| {
                        let resp = style::power_button(ui, self.running)
                            .on_hover_text(tr(if self.running { "Arrêter la simulation" } else { "Démarrer la simulation" }));
                        if resp.clicked() {
                            self.on_toggle();
                        }
                    });
                }
                ui.add_enabled_ui(!self.busy, |ui| {
                    if style::button(ui, &tr(if self.dirty { "Appliquer •" } else { "Appliquer" }), false).clicked() {
                        self.on_apply(false);
                    }
                    let export = style::button(ui, &tr("Exporter"), !live)
                        .on_hover_text(tr("Enregistrer les filtres (WAV, true stereo, Equalizer APO)"));
                    if export.clicked() {
                        self.on_export();
                    }
                });
                let label = tr("Réglages");
                let w = ui.painter().layout_no_wrap(label.clone(), mono(11.0), TEXT).size().x + 24.0;
                let resp = chip(ui, &label, self.show_settings, w)
                    .on_hover_text(tr("Déplier / replier les réglages (les onglets restent visibles)"));
                if resp.clicked() {
                    self.show_settings = !self.show_settings;
                    self.on_ui_state("show_settings", json!(self.show_settings));
                }
                let langs: Vec<(String, String)> =
                    LANGUAGES.iter().map(|(c, _)| ((*c).to_string(), c.to_uppercase())).collect();
                ui.allocate_ui(Vec2::new(84.0, 28.0), |ui| {
                    let current = i18n::language().to_string();
                    if let Some(code) = segmented_width(ui, &langs, Some(&current), 84.0) {
                        self.on_language(&code);
                    }
                })
                .response
                .on_hover_text(tr("Langue de l'interface"));
                let ab = vec![("A".to_string(), "A".to_string()), ("B".to_string(), "B".to_string())];
                ui.allocate_ui(Vec2::new(64.0, 28.0), |ui| {
                    if let Some(slot) = segmented_width(ui, &ab, Some(&self.app.ab_slot()), 64.0) {
                        self.on_ab(&slot);
                    }
                })
                .response
                .on_hover_text(tr(
                    "Comparaison A/B : deux configurations complètes (enceintes, pièce,\nplacement, écoute). Basculer applique l'autre sans coupure.",
                ));
                if live {
                    let mut auto = self.auto_apply_enabled();
                    let resp = toggle(ui, &mut auto, "Auto").on_hover_text(tr(
                        "Appliquer automatiquement chaque changement pendant la simulation\n(bascule sans coupure)",
                    ));
                    if resp.changed() {
                        self.on_ui_state("auto_apply", json!(auto));
                    }
                    let text = if self.running {
                        format!("● LIVE · {}", if self.slots.is_empty() { "V1".into() } else { self.slots.join("/").to_uppercase() })
                    } else {
                        tr("○ VEILLE")
                    };
                    style::status_pill(ui, &text, self.running);
                }
                self.chain(ui);
            });
        });
    }

    fn preset_label(&self) -> String {
        tr(room_preset(&self.s.room_preset).map_or("Pièce personnalisée", |p| p.label))
    }

    /// Chaîne casque → enceinte → salle (coupée plutôt que d'élargir la fenêtre).
    fn chain(&self, ui: &mut Ui) {
        let hp = self.s.headphone.as_ref().map_or(tr("sans casque"), |h| h.name.clone());
        let spk = self.app.speaker(None).map(|s| s.label()).unwrap_or_default();
        let parts = [hp, spk, self.preset_label()];
        let mut job = LayoutJob::default();
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                job.append(" → ", 0.0, TextFormat::simple(mono(12.0), AMBER));
            }
            job.append(part, 0.0, TextFormat::simple(mono(12.0), MUTED));
        }
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        let width = ui.available_width();
        ui.allocate_ui_with_layout(Vec2::new(width, 28.0), Layout::left_to_right(Align::Center), |ui| {
            job.wrap.max_width = width;
            ui.add(egui::Label::new(job).truncate()).on_hover_text(parts.join(" → "));
        });
    }

    fn footer(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&self.status).font(mono(11.0)).color(MUTED));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(r) = &self.result {
                    let notes: Vec<String> = r.notes.iter().map(|n| crate::i18n::tr_message(n)).collect();
                    ui.label(RichText::new(format!("GAIN {:+.1} DB · {} HZ", r.gain_db, r.fs)).font(mono(11.0)).color(MUTED))
                        .on_hover_text(notes.join("\n"));
                }
            });
        });
    }

    // -- zone haute : sources · plan · analyse ------------------------------------
    fn upper(&mut self, ui: &mut Ui) {
        let full = ui.available_rect_before_wrap();
        ui.allocate_rect(full, Sense::hover());
        let gap = 14.0;
        let left_w = 330.0;
        let rest = (full.width() - left_w - 2.0 * gap).max(400.0);
        let right_w = (rest / 4.0).clamp(300.0, 400.0);
        let plan_w = rest - right_w;
        let left = Rect::from_min_size(full.min, Vec2::new(left_w, full.height()));
        let plan_r = Rect::from_min_size(Pos2::new(left.right() + gap, full.top()), Vec2::new(plan_w, full.height()));
        let right = Rect::from_min_size(Pos2::new(plan_r.right() + gap, full.top()), Vec2::new(right_w, full.height()));

        // Sources : casque et enceintes en onglets
        let mut col = ui.new_child(UiBuilder::new().max_rect(left).layout(Layout::top_down(Align::Min)));
        let tabs = vec![(0usize, tr("01 Casque")), (1usize, tr("02 Enceintes"))];
        if let Some(t) = segmented(&mut col, &tabs, Some(&self.source_tab)) {
            self.source_tab = t;
        }
        let card_rect = Rect::from_min_max(Pos2::new(left.left(), left.top() + 36.0), left.max);
        let mut inner = card_in(ui, card_rect, Margin::symmetric(14, 12));
        if self.source_tab == 0 {
            self.headphone_panel(&mut inner);
        } else {
            self.speaker_panel(&mut inner);
        }

        // Plan
        let mut pl = card_in(ui, plan_r, Margin { left: 14, right: 14, top: 12, bottom: 8 });
        self.plan_card(&mut pl);

        // Analyse : réponse simulée ou RT60, au choix
        let mut an = card_in(ui, right, Margin { left: 14, right: 14, top: 12, bottom: 10 });
        let tabs = vec![(0usize, tr("Réponse simulée")), (1usize, tr("Réverbération"))];
        if let Some(t) = segmented(&mut an, &tabs, Some(&self.analysis_tab)) {
            self.analysis_tab = t;
            self.on_ui_state("analysis_tab", json!(t));
        }
        if self.analysis_tab == 0 {
            let r = self.result.clone();
            plots::response_plot(&mut an, r.as_ref().map(|r| r.plot_freqs.as_slice()), r.as_ref().map_or(&[][..], |r| &r.plot_curves));
        } else {
            let view = self.view_settings();
            let rt = self.app.room_spec(Some(&view)).rt60_bands();
            plots::rt_meter(&mut an, &rt);
            caption(&mut an, "zone sarcelle : 0,2–0,4 s, écoute critique");
        }
    }

    fn headphone_panel(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            tag(ui, "01");
            section_title(ui, "Casque");
        });
        let placeholder = tr("Rechercher parmi 8 850 mesures…");
        if search_field(ui, &mut self.hp_query, placeholder, self.hp_loaded || !self.hp_entries.is_empty()) {
            self.apply_hp_filter();
        }
        let current = self.s.headphone.as_ref().map(|h| h.path.clone());
        let list_h = (ui.available_height() - 80.0).max(90.0);
        let row_h = 28.0;
        let mut clicked: Option<HeadphoneEntry> = None;
        let mut remove: Option<HeadphoneEntry> = None;
        let mut area = egui::ScrollArea::vertical().id_salt("hp_list").max_height(list_h).auto_shrink([false, false]);
        if self.hp_reveal {
            self.hp_reveal = false;
            if let Some(pos) = self.hp_filtered.iter().position(|&i| Some(&self.hp_entries[i].path) == current.as_ref()) {
                area = area.vertical_scroll_offset((pos as f32 * (row_h + ui.spacing().item_spacing.y) - list_h / 2.0).max(0.0));
            }
        }
        Frame::new().fill(BG).stroke(Stroke::new(1.0, BORDER)).corner_radius(CornerRadius::same(10)).inner_margin(Margin::same(4)).show(ui, |ui| {
            ui.set_height(list_h);
            area.show_rows(ui, row_h, self.hp_filtered.len(), |ui, range| {
                for &i in &self.hp_filtered[range] {
                    let e = &self.hp_entries[i];
                    let text = format!("{}{}", if e.is_import() { "★ " } else { "" }, e.name);
                    let resp = list_row(ui, &text, Some(&e.path) == current.as_ref()).on_hover_text(e.label());
                    if resp.clicked() {
                        clicked = Some(e.clone());
                    }
                    if e.is_import() {
                        resp.context_menu(|ui| {
                            if ui.button(tr("Supprimer cet import")).clicked() {
                                remove = Some(e.clone());
                                ui.close();
                            }
                        });
                    }
                }
            });
        });
        let count = match (&self.hp_error, self.hp_loaded) {
            (Some(msg), _) => trf("INDEX INDISPONIBLE : {msg}", &[("msg", msg)]),
            (None, false) => tr("chargement de l'index autoeq…").to_uppercase(),
            (None, true) => trf(
                "{shown} / {total} mesures",
                &[("shown", &self.hp_filtered.len().to_string()), ("total", &self.hp_entries.len().to_string())],
            )
            .to_uppercase(),
        };
        ui.add(egui::Label::new(RichText::new(count).font(mono(11.0)).color(MUTED)).truncate());
        ui.horizontal(|ui| {
            let mut on = self.s.headphone_eq_enabled;
            if toggle(ui, &mut on, "Correction").on_hover_text(tr("Appliquer la correction du casque (AutoEq ou import)")).changed() {
                self.on_setting(changes(json!({"headphone_eq_enabled": on})));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let resp = style::button(ui, &tr("Importer…"), false).on_hover_text(tr(
                    "Votre mesure (fréquence, dB : REW, Squiglink, CSV AutoEq)\nou un EQ AutoEq / Equalizer APO (ParametricEQ, GraphicEQ)",
                ));
                if resp.clicked() {
                    self.on_import();
                }
            });
        });
        if let Some(e) = clicked {
            self.on_headphone(&e);
        }
        if let Some(e) = remove {
            self.on_remove_import(&e);
        }
    }

    fn speaker_panel(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            tag(ui, "02");
            section_title(ui, "Enceintes");
        });
        let n = self.app.speakers.len();
        let n_studio = self.app.speakers.iter().filter(|s| s.is_studio()).count();
        search_field(ui, &mut self.spk_query, trf("Filtrer {n} modèles…", &[("n", &n.to_string())]), true);
        let cats = vec![
            (String::new(), tr("Toutes")),
            ("studio".to_string(), format!("Studio · {n_studio}")),
            ("hifi".to_string(), format!("Hi-Fi · {}", n - n_studio)),
        ];
        if let Some(c) = segmented(ui, &cats, Some(&self.spk_category)) {
            self.spk_category = c;
        }
        let q = Query::new(&self.spk_query);
        let current = self.s.speaker_id.clone();
        let mut chosen = None;
        let reveal = std::mem::take(&mut self.spk_reveal);
        Frame::new().fill(BG).stroke(Stroke::new(1.0, BORDER)).corner_radius(CornerRadius::same(10)).inner_margin(Margin::same(4)).show(ui, |ui| {
            let h = ui.available_height().max(90.0);
            ui.set_height(h);
            egui::ScrollArea::vertical().id_salt("spk_list").auto_shrink([false, false]).show(ui, |ui| {
                for spk in &self.app.speakers {
                    let label = spk.label();
                    if !q.matches(&label) || (!self.spk_category.is_empty() && spk.category != self.spk_category) {
                        continue;
                    }
                    let kind = match spk.shape.as_str() {
                        "bookshelves" => "bibliothèque",
                        "floorstanders" => "colonne",
                        "center" => "centrale",
                        other => other,
                    };
                    let score = |v: Option<f64>| v.map_or("—".to_string(), crate::config::py_float);
                    let tip = trf(
                        "{cat} · {kind} · {type}\nscore Olive {score} ({wsub} avec caisson) · LFX {lfx} Hz\nmesure : {origin}, Klippel NFS{computed}",
                        &[
                            ("cat", &tr(if spk.is_studio() { "moniteur de studio" } else { "hi-fi" })),
                            ("kind", &tr(kind)),
                            ("type", &spk.kind),
                            ("score", &score(spk.pref_score)),
                            ("wsub", &score(spk.pref_score_wsub)),
                            ("lfx", &spk.lfx_hz.map_or("—".to_string(), |v| format!("{v:.0}"))),
                            ("origin", &spk.origin),
                            ("computed", &if spk.is_polar() { tr(" (CEA-2034 calculé depuis les mesures polaires)") } else { String::new() }),
                        ],
                    );
                    let selected = spk.id == current;
                    let resp = list_row(ui, &label, selected).on_hover_text(tip);
                    if selected && reveal {
                        resp.scroll_to_me(Some(Align::Center));
                    }
                    if resp.clicked() && !selected {
                        chosen = Some(spk.id.clone());
                    }
                }
            });
        });
        if let Some(id) = chosen {
            self.on_speaker(&id);
        }
    }

    fn plan_card(&mut self, ui: &mut Ui) {
        let view = self.view_settings();
        let eff = self.app.effective(Some(&view));
        let room = self.app.room_spec(Some(&view));
        ui.horizontal(|ui| {
            tag(ui, "PLAN");
            section_title(ui, &self.preset_label());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let treat = if room.per_wall {
                    tr("traitement par paroi")
                } else {
                    trf("traitement {p} %", &[("p", &format!("{:.0}", room.treatment * 100.0))])
                };
                let hud = format!("RT60 {} s  ·  Dc {} m  ·  {treat}", num(room.rt60(), 2), num(room.critical_distance(), 2));
                ui.add(egui::Label::new(RichText::new(hud.to_uppercase()).font(mono(11.0)).color(MUTED)).truncate());
            });
        });
        if let Some(change) = plan::room_plan(ui, &mut self.plan, &eff, &room) {
            self.on_setting(change);
        }
    }

    // -- réglages -------------------------------------------------------------------
    fn tabs(&self, live: bool) -> Vec<(&'static str, &'static str)> {
        let mut t = vec![
            ("room", "Salle"),
            ("treatment", "Traitement"),
            ("reflections", "Réflexions"),
            ("reverb", "Réverbération"),
            ("placement", "Placement"),
            ("headphone", "Casque"),
            ("listening", "Écoute"),
            ("advanced", "Avancé"),
        ];
        if live {
            t.push(("output", "Sortie"));
        }
        t
    }

    fn tab_on(&self, key: &str) -> bool {
        match key {
            "reflections" => self.s.reflection_order > 0,
            "output" => self.running,
            _ => SWITCHES.iter().find(|(k, _)| *k == key).is_some_and(|(_, f)| self.field(f).as_bool().unwrap_or(true)),
        }
    }

    fn select_tab(&mut self, key: &str) {
        if self.settings_tab != key {
            self.settings_tab = key.to_string();
            self.on_ui_state("settings_tab", json!(key));
        }
        if !self.show_settings {
            // un clic sur un onglet déplie les réglages
            self.show_settings = true;
            self.on_ui_state("show_settings", json!(true));
        }
    }

    fn switched(&mut self, key: &str, on: bool) {
        self.select_tab(key);
        match key {
            "reflections" => self.on_setting(changes(json!({"reflection_order": if on { self.last_order } else { 0 }}))),
            "output" => {
                if on != self.running {
                    self.on_toggle();
                }
            }
            _ => {
                if let Some((_, field)) = SWITCHES.iter().find(|(k, _)| *k == key) {
                    self.on_setting(changes(json!({ *field: on })));
                }
            }
        }
    }

    fn settings_band(&mut self, ui: &mut Ui, live: bool) {
        let tabs = self.tabs(live);
        if !tabs.iter().any(|(k, _)| *k == self.settings_tab) {
            self.settings_tab = "room".into();
        }
        // Barre d'onglets
        let gap = 6.0;
        let w = (ui.available_width() - gap * (tabs.len() - 1) as f32) / tabs.len() as f32;
        let mut action: Option<(&str, Option<bool>)> = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (key, title) in &tabs {
                let on = self.tab_on(key);
                let inert = ROOM_DEPENDENT.contains(key) && !self.s.room_enabled;
                let current = self.settings_tab == *key;
                let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 32.0), Sense::hover());
                let (fill, stroke) = if current { (PANEL2, AMBER) } else { (PANEL, BORDER) };
                ui.painter().rect(rect, CornerRadius::same(9), fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
                let sw_rect = Rect::from_center_size(Pos2::new(rect.right() - 8.0 - 12.0, rect.center().y), Vec2::new(24.0, 13.0));
                let label_rect = Rect::from_min_max(rect.min, Pos2::new(sw_rect.left() - 4.0, rect.max.y));
                let label = ui.interact(label_rect, Id::unique(("tab", *key)), Sense::click());
                let color = if label.hovered() {
                    AMBER
                } else if inert {
                    DIM
                } else if !on {
                    MUTED
                } else {
                    TEXT
                };
                let mut job = LayoutJob::simple_singleline(tr(title), body(12.0), color);
                job.wrap.max_width = label_rect.width() - 12.0;
                job.wrap.max_rows = 1;
                job.wrap.break_anywhere = true;
                let galley = ui.painter().layout_job(job);
                ui.painter().galley(Pos2::new(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0), galley, color);
                if label.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    action = Some((key, None));
                }
                let off = off_text(key);
                let mut tip = trf("Activer / désactiver « {tab} ».", &[("tab", &tr(title))]);
                if !off.is_empty() {
                    tip += &format!("\n{}", trf("Coupé : {what}", &[("what", &tr(off))]));
                }
                let mut state = on;
                let resp = ui.put(sw_rect, |ui: &mut Ui| switch(ui, &mut state, Vec2::new(24.0, 13.0))).on_hover_text(tip);
                if resp.changed() {
                    action = Some((key, Some(state)));
                }
            }
        });
        match action {
            Some((key, None)) => self.select_tab(key),
            Some((key, Some(on))) => self.switched(key, on),
            None => {}
        }
        if !self.show_settings {
            return;
        }
        ui.add_space(6.0);
        let key = self.settings_tab.clone();
        let on = self.tab_on(&key);
        let inert = ROOM_DEPENDENT.contains(&key.as_str()) && !self.s.room_enabled;
        style::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            if !on || inert {
                muted_wrap(ui, if inert && on { INERT_TEXT } else { off_text(&key) });
            }
            if on && !inert {
                egui::ScrollArea::horizontal().id_salt("page").show(ui, |ui| match key.as_str() {
                    "room" => self.page_room(ui),
                    "treatment" => self.page_treatment(ui),
                    "reflections" => self.page_reflections(ui),
                    "reverb" => self.page_reverb(ui),
                    "placement" => self.page_placement(ui),
                    "headphone" => self.page_headphone(ui),
                    "listening" => self.page_listening(ui),
                    "advanced" => self.page_advanced(ui),
                    "output" => self.page_output(ui),
                    _ => {}
                });
            }
        });
    }

    /// Curseur lié à un réglage : aperçu pendant le déplacement, validation au relâchement.
    #[allow(clippy::too_many_arguments)]
    fn slider(&mut self, ui: &mut Ui, key: &'static str, text: &str, lo: f64, hi: f64, step: f64, fmt: fn(f64) -> String, teal: bool) {
        let mut v = match self.slider_preview {
            Some((k, v)) if k == key => v,
            _ => match self.slider_pending {
                Some((k, v, _)) if k == key => v,
                _ => self.field_f64(key),
            },
        };
        let shown = fmt(v);
        let ev = style::value_slider(ui, text, &mut v, lo, hi, step, &shown, teal);
        if let Some(moved) = ev.moved {
            self.slider_preview = Some((key, moved));
            if ev.keyboard {
                // clavier / molette : validation après une pause
                self.slider_pending = Some((key, moved, std::time::Instant::now() + super::SLIDER_DEBOUNCE));
            }
        }
        if ev.released {
            self.slider_pending = None;
            self.slider_preview = None;
            self.on_setting(changes(json!({ key: v })));
        }
    }

    fn seg(&mut self, ui: &mut Ui, key: &str, text: &str, options: &[(Value, &str)]) -> egui::Response {
        if !text.is_empty() {
            muted(ui, text);
        }
        let opts: Vec<(Value, String)> = options.iter().map(|(v, t)| (v.clone(), tr(t))).collect();
        let current = self.field(key);
        let current = opts.iter().find(|(v, _)| match (v.as_f64(), current.as_f64()) {
            (Some(a), Some(b)) => (a - b).abs() < 1e-9,
            _ => *v == current,
        });
        let current = current.map(|(v, _)| v.clone());
        let inner = ui.scope(|ui| segmented(ui, &opts, current.as_ref()));
        if let Some(v) = inner.inner {
            self.on_setting(changes(json!({ key: v })));
        }
        inner.response
    }

    fn page_room(&mut self, ui: &mut Ui) {
        groups(ui, &[("préréglages", 3.0, 250.0), ("dimensions", 2.0, 180.0), ("matériaux", 3.0, 270.0), ("intensité", 2.0, 180.0)], |i, ui| match i {
            0 => {
                let w = (ui.available_width() - 6.0) / 2.0;
                let mut chosen = None;
                for row in ROOM_PRESETS.chunks(2) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        for p in row {
                            if chip(ui, &tr(p.label), self.s.room_preset == p.key, w).clicked() {
                                chosen = Some(p.key);
                            }
                        }
                    });
                }
                if room_preset(&self.s.room_preset).is_none() {
                    caption(ui, "RÉGLAGES PERSONNALISÉS");
                }
                if let Some(k) = chosen {
                    self.on_preset(k);
                }
            }
            1 => {
                self.slider(ui, "room_length", "Longueur", 2.0, 30.0, 0.1, meters, false);
                self.slider(ui, "room_width", "Largeur", 2.0, 20.0, 0.1, meters, false);
                self.slider(ui, "room_height", "Hauteur sous plafond", 2.0, 8.0, 0.1, meters, false);
            }
            2 => {
                self.seg(ui, "room_walls", "", &[(json!("masonry"), "Maçonnerie"), (json!("drywall"), "Placo")]);
                self.seg(ui, "room_floor", "", &[(json!("tile"), "Carrelage"), (json!("wood"), "Parquet"), (json!("rug"), "Tapis"), (json!("carpet"), "Moquette")]);
                self.seg(ui, "room_furniture", "", &[(json!("empty"), "Vide"), (json!("furnished"), "Meublé"), (json!("full"), "Très")])
                    .on_hover_text(tr("Vide · meublé · très meublé"));
            }
            _ => self.slider(ui, "room_amount", "Quantité de pièce", 0.05, 2.0, 0.05, pct, false),
        });
    }

    fn page_treatment(&mut self, ui: &mut Ui) {
        groups(ui, &[("absorbeurs", 2.0, 240.0), ("couverture", 4.0, 380.0), ("grave", 2.0, 240.0)], |i, ui| match i {
            0 => {
                self.seg(ui, "room_absorber", "", &[(json!("foam5"), "Mousse"), (json!("wool10"), "Laine 10"), (json!("wool20"), "Laine 20")])
                    .on_hover_text(tr("Mousse 5 cm · laine minérale 10 cm · laine 20 cm (large bande, absorbe aussi le grave)"));
                let mut per_wall = self.s.treatment_per_wall;
                if toggle(ui, &mut per_wall, "Paroi par paroi").changed() {
                    self.on_setting(changes(json!({"treatment_per_wall": per_wall})));
                }
            }
            1 => {
                let half = (ui.available_width() - 18.0) / 2.0;
                if self.s.treatment_per_wall {
                    let walls = [
                        ("treatment_front", "Mur avant"),
                        ("treatment_sides", "Murs latéraux"),
                        ("treatment_rear", "Mur arrière"),
                        ("treatment_ceiling", "Plafond"),
                    ];
                    for row in walls.chunks(2) {
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 18.0;
                            for (key, text) in row {
                                ui.allocate_ui_with_layout(Vec2::new(half, 0.0), Layout::top_down(Align::Min), |ui| {
                                    ui.set_width(half);
                                    self.slider(ui, key, text, 0.0, 1.0, 0.05, pct, true);
                                });
                            }
                        });
                    }
                } else {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 18.0;
                        ui.allocate_ui_with_layout(Vec2::new(half, 0.0), Layout::top_down(Align::Min), |ui| {
                            ui.set_width(half);
                            self.slider(ui, "room_treatment", "Murs traités", 0.0, 1.0, 0.05, pct, true);
                        });
                        ui.allocate_ui_with_layout(Vec2::new(half, 0.0), Layout::top_down(Align::Min), |ui| {
                            ui.set_width(half);
                            self.seg(ui, "room_ceiling", "Plafond", &[(json!("plaster"), "Plâtre"), (json!("cloud"), "Nuage")]);
                        });
                    });
                }
            }
            _ => {
                self.seg(ui, "room_bass_traps", "Pièges à basses (angles)", &[(json!(0.0), "Aucun"), (json!(0.5), "Quelques"), (json!(1.0), "Complet")]);
            }
        });
    }

    fn page_reflections(&mut self, ui: &mut Ui) {
        groups(ui, &[("ordre", 1.0, 260.0), ("diffusion", 1.0, 240.0), ("", 1.0, 0.0)], |i, ui| match i {
            0 => {
                self.seg(ui, "reflection_order", "Ordre (rebonds simulés)", &[(json!(1), "1"), (json!(2), "2"), (json!(3), "3"), (json!(4), "4")]);
            }
            1 => self.slider(ui, "room_diffusion", "Diffusion du mur arrière", 0.0, 1.0, 0.05, pct, false),
            _ => {}
        });
    }

    fn page_reverb(&mut self, ui: &mut Ui) {
        groups(ui, &[("temps de réverbération", 2.0, 300.0), ("", 3.0, 300.0)], |i, ui| match i {
            0 => {
                let mut auto = self.s.rt60_auto;
                if toggle(ui, &mut auto, "RT60 calculé (matériaux)").changed() {
                    self.on_setting(changes(json!({"rt60_auto": auto})));
                }
                let enabled = !self.s.rt60_auto;
                ui.add_enabled_ui(enabled, |ui| {
                    self.slider(ui, "rt60", "RT60 imposé (500 Hz–1 kHz)", 0.1, 4.0, 0.05, seconds, false);
                });
            }
            _ => {
                muted_wrap(ui, "La réverbération tardive est un champ diffus dont la durée suit le RT60 de chaque octave (vu-mètres « Réverbération »).");
            }
        });
    }

    fn page_placement(&mut self, ui: &mut Ui) {
        groups(ui, &[("", 1.0, 220.0), ("", 1.0, 220.0), ("", 1.0, 220.0)], |i, ui| match i {
            0 => {
                self.slider(ui, "speaker_angle", "Angle des enceintes", 10.0, 80.0, 1.0, degrees, false);
                self.slider(ui, "speaker_distance", "Distance d'écoute", 0.5, 10.0, 0.05, meters2, false);
            }
            1 => {
                self.slider(ui, "front_gap", "Enceintes – mur avant", 0.2, 5.0, 0.05, meters2, false);
                self.slider(ui, "listener_offset", "Décalage latéral", -3.0, 3.0, 0.05, offset, false);
            }
            _ => {
                self.slider(ui, "ear_height", "Hauteur des oreilles", 0.6, 2.0, 0.05, meters2, false);
                self.slider(ui, "speaker_height", "Hauteur des tweeters", 0.0, 3.0, 0.05, tweeter, false);
            }
        });
    }

    fn page_headphone(&mut self, ui: &mut Ui) {
        let entry = self.s.headphone.clone();
        let summary = self.headphone_summary_text();
        let alternatives = self.alternatives();
        groups(ui, &[("correction", 3.0, 280.0), ("mesure", 3.0, 280.0), ("", 3.0, 260.0)], |i, ui| match i {
            0 => {
                match &entry {
                    None => {
                        style::big_name(ui, &tr("Aucun casque"));
                        caption(ui, "SANS CORRECTION");
                    }
                    Some(e) => {
                        style::big_name(ui, &e.name);
                        style::caption_raw(ui, &format!("{} · {}", e.source, e.rig).to_uppercase());
                    }
                }
                if !summary.is_empty() {
                    ui.label(RichText::new(&summary).font(mono(11.0)).color(MUTED));
                }
            }
            1 => {
                let raw_import = entry.as_ref().is_some_and(|e| e.is_import() && !e.target.is_empty());
                let many = alternatives.len() > 1;
                if many {
                    muted(ui, "Autres mesures de ce modèle");
                    let current = entry.as_ref().map(|e| e.path.clone()).unwrap_or_default();
                    let shown = alternatives
                        .iter()
                        .find(|e| e.path == current)
                        .map(|e| if e.rig.is_empty() { e.source.clone() } else { format!("{} · {}", e.source, e.rig) })
                        .unwrap_or_default();
                    let mut chosen = None;
                    egui::ComboBox::from_id_salt("hp_alt").width(ui.available_width()).selected_text(shown).show_ui(ui, |ui| {
                        for e in &alternatives {
                            let text = if e.rig.is_empty() { e.source.clone() } else { format!("{} · {}", e.source, e.rig) };
                            if ui.selectable_label(e.path == current, text).clicked() {
                                chosen = Some(e.clone());
                            }
                        }
                    })
                    .response
                    .on_hover_text(tr("Même casque mesuré par une autre source ou sur un autre banc"));
                    if let Some(e) = chosen.filter(|e| Some(e) != entry.as_ref()) {
                        self.on_headphone(&e);
                        self.hp_reveal = true;
                    }
                }
                if raw_import {
                    muted(ui, "Cible de la correction");
                    let current = entry.as_ref().map(|e| e.target.clone()).unwrap_or_default();
                    let mut chosen = None;
                    egui::ComboBox::from_id_salt("hp_target").width(ui.available_width()).selected_text(current.clone()).show_ui(ui, |ui| {
                        for t in AUTOEQ_TARGETS {
                            if ui.selectable_label(*t == current, *t).clicked() {
                                chosen = Some(*t);
                            }
                        }
                    })
                    .response
                    .on_hover_text(tr("Courbe vers laquelle votre mesure brute est corrigée"));
                    if let Some(t) = chosen {
                        self.on_import_target(t);
                    }
                }
                if entry.is_some() && !many && !raw_import {
                    muted_wrap(ui, "Une seule mesure disponible pour ce modèle.");
                }
            }
            _ => {
                muted_wrap(ui, "Correction AutoEq de votre modèle (ou de votre propre mesure importée), appliquée avant la simulation.");
                if style::button(ui, &tr("Importer une mesure…"), false).clicked() {
                    self.on_import();
                }
            }
        });
    }

    fn page_listening(&mut self, ui: &mut Ui) {
        groups(ui, &[("", 1.0, 200.0), ("", 1.0, 200.0), ("", 1.0, 200.0), ("", 1.0, 200.0)], |i, ui| match i {
            0 => self.slider(ui, "bass_db", "Basses (105 Hz)", -12.0, 12.0, 0.5, db, false),
            1 => self.slider(ui, "treble_db", "Aigus (10 kHz)", -12.0, 12.0, 0.5, db, false),
            2 => self.slider(ui, "balance_db", "Balance", -6.0, 6.0, 0.5, balance, false),
            _ => self.slider(ui, "preamp_db", "Préampli", -30.0, 20.0, 0.5, db, false),
        });
    }

    fn page_advanced(&mut self, ui: &mut Ui) {
        groups(ui, &[("hrtf", 1.0, 260.0), ("fréquence d'échantillonnage", 1.0, 260.0), ("", 1.0, 0.0)], |i, ui| match i {
            0 => {
                let mut df = self.s.diffuse_field_eq;
                if toggle(ui, &mut df, "Égalisation champ diffus").changed() {
                    self.on_setting(changes(json!({"diffuse_field_eq": df})));
                }
            }
            1 => {
                let rates = [("Auto (PipeWire / 48 kHz)", 0i64), ("44,1 kHz", 44100), ("48 kHz", 48000), ("96 kHz", 96000)];
                let current = self.s.sample_rate;
                let shown = rates.iter().find(|r| r.1 == current).map_or(tr(rates[0].0), |r| tr(r.0));
                let mut chosen = None;
                egui::ComboBox::from_id_salt("rate").width(ui.available_width()).selected_text(shown).show_ui(ui, |ui| {
                    for (text, value) in rates {
                        if ui.selectable_label(value == current, tr(text)).clicked() {
                            chosen = Some(value);
                        }
                    }
                });
                if let Some(v) = chosen.filter(|v| *v != current) {
                    self.on_setting(changes(json!({"sample_rate": v})));
                }
            }
            _ => {}
        });
    }

    fn page_output(&mut self, ui: &mut Ui) {
        groups(ui, &[("", 1.0, 320.0), ("", 1.0, 0.0)], |i, ui| {
            if i != 0 {
                return;
            }
            muted(ui, "Casque (sortie physique)");
            ui.horizontal(|ui| {
                let current = self.s.output_sink.clone();
                let auto = tr("Auto (sortie par défaut)");
                let shown = self.sinks.iter().find(|s| s.name == current).map_or(auto.clone(), |s| s.label().to_string());
                let mut chosen = None;
                egui::ComboBox::from_id_salt("sink").width((ui.available_width() - 48.0).max(260.0)).selected_text(shown).show_ui(ui, |ui| {
                    if ui.selectable_label(current.is_empty(), auto.clone()).clicked() {
                        chosen = Some(String::new());
                    }
                    for s in &self.sinks {
                        if ui.selectable_label(s.name == current, s.label()).clicked() {
                            chosen = Some(s.name.clone());
                        }
                    }
                });
                if style::button(ui, "↻", false).on_hover_text(tr("Actualiser la liste des sorties")).clicked() {
                    self.refresh_sinks();
                }
                if let Some(name) = chosen.filter(|n| *n != current) {
                    self.on_setting(changes(json!({"output_sink": name})));
                }
            });
            let mut autostart = self.s.autostart;
            if toggle(ui, &mut autostart, "Démarrer à la connexion").changed() {
                self.on_autostart(autostart);
            }
            if let (true, Some(prev)) = (self.running, &self.previous_sink) {
                style::caption_raw(ui, &trf("À L'ARRÊT : RETOUR SUR {sink}", &[("sink", prev)]));
            }
        });
    }

    // -- dialogues ------------------------------------------------------------------
    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.dialog.take() else { return };
        let frame = Frame::new().fill(PANEL2).stroke(Stroke::new(1.0, BORDER_STRONG)).corner_radius(CornerRadius::same(12)).inner_margin(Margin::same(18));
        match dialog {
            Dialog::Error(msg) => {
                let mut close = false;
                let resp = egui::Modal::new(Id::unique("error")).frame(frame).show(ctx, |ui| {
                    ui.set_max_width(460.0);
                    ui.label(RichText::new(crate::config::APP_NAME).font(FontId::new(16.0, style::display())));
                    ui.add_space(6.0);
                    ui.add(egui::Label::new(&msg).wrap());
                    ui.add_space(10.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        close = style::button(ui, &tr("Fermer"), true).clicked();
                    });
                });
                if !(close || resp.should_close()) {
                    self.dialog = Some(Dialog::Error(msg));
                }
            }
            Dialog::Import { path, kind, mut name, mut target } => {
                let (mut ok, mut cancel) = (false, false);
                let resp = egui::Modal::new(Id::unique("import")).frame(frame).show(ctx, |ui| {
                    ui.set_width(440.0);
                    ui.label(RichText::new(tr("Nom du casque")).font(FontId::new(16.0, style::display())));
                    ui.add_space(4.0);
                    muted(ui, "Nom affiché :");
                    let edit = ui.add_sized([ui.available_width(), 30.0], TextEdit::singleline(&mut name).margin(Margin::symmetric(10, 6)));
                    if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        ok = true;
                    }
                    if kind == HeadphoneFileKind::Measurement {
                        ui.add_space(8.0);
                        ui.label(RichText::new(tr("Courbe cible")).font(FontId::new(14.0, style::display())));
                        muted_wrap(ui, "Mesure brute détectée : vers quelle cible corriger ?\n(Harman over-ear = cible des corrections AutoEq de casques circum-auriculaires)");
                        egui::ComboBox::from_id_salt("import_target").width(ui.available_width()).selected_text(AUTOEQ_TARGETS[target]).show_ui(ui, |ui| {
                            for (i, t) in AUTOEQ_TARGETS.iter().enumerate() {
                                ui.selectable_value(&mut target, i, *t);
                            }
                        });
                    }
                    ui.add_space(12.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ok |= style::button(ui, &tr("Importer"), true).clicked();
                        cancel = style::button(ui, &tr("Annuler"), false).clicked();
                    });
                });
                if ok {
                    self.finish_import(path, kind, name, target);
                } else if !(cancel || resp.should_close()) {
                    self.dialog = Some(Dialog::Import { path, kind, name, target });
                }
            }
        }
    }
}

