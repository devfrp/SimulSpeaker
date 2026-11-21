//! Identité visuelle (palette devfrp, polices) et petits composants : cartes, légendes,
//! interrupteurs, boutons segmentés, curseurs, bouton d'alimentation.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame, Margin, Pos2, Rect,
    Response, Sense, Stroke, StrokeKind, Ui, Vec2,
    epaint::text::{FontTweak, VariationCoords},
};

use crate::i18n::tr;

pub const BG: Color32 = Color32::from_rgb(0x14, 0x15, 0x1a);
pub const PANEL: Color32 = Color32::from_rgb(0x1b, 0x1d, 0x24);
pub const PANEL2: Color32 = Color32::from_rgb(0x21, 0x23, 0x2b);
pub const BORDER: Color32 = Color32::from_rgb(0x26, 0x28, 0x2f);
pub const BORDER_STRONG: Color32 = Color32::from_rgb(0x34, 0x36, 0x3e);
pub const TEXT: Color32 = Color32::from_rgb(0xed, 0xed, 0xef);
pub const MUTED: Color32 = Color32::from_rgb(0x8b, 0x8d, 0x98);
pub const DIM: Color32 = Color32::from_rgb(0x5a, 0x5c, 0x66);
pub const AMBER: Color32 = Color32::from_rgb(0xe8, 0xa3, 0x3d);
pub const AMBER_HOVER: Color32 = Color32::from_rgb(0xf0, 0xb2, 0x5a);
pub const TEAL: Color32 = Color32::from_rgb(0x49, 0xb9, 0xae);
pub const RED: Color32 = Color32::from_rgb(0xe5, 0x48, 0x4d);

/// Couleur avec opacité (0–255).
pub fn alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

static INTER: &[u8] = include_bytes!("../../packaging/fonts/Inter.ttf");
static GROTESK: &[u8] = include_bytes!("../../packaging/fonts/SpaceGrotesk.ttf");
static MONO: &[u8] = include_bytes!("../../packaging/fonts/JetBrainsMono.ttf");

fn weighted(bytes: &'static [u8], weight: f32) -> FontData {
    FontData::from_static(bytes)
        .tweak(FontTweak { coords: VariationCoords::new([("wght", weight)]), ..Default::default() })
}

pub fn display() -> FontFamily {
    FontFamily::Name("display".into())
}

pub fn mono_bold() -> FontFamily {
    FontFamily::Name("mono-bold".into())
}

pub fn body_semibold() -> FontFamily {
    FontFamily::Name("body-semibold".into())
}

pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

pub fn body(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    for (name, bytes, weight) in [
        ("Inter", INTER, 400.0),
        ("Inter SemiBold", INTER, 600.0),
        ("Space Grotesk", GROTESK, 600.0),
        ("JetBrains Mono", MONO, 400.0),
        ("JetBrains Mono Bold", MONO, 700.0),
    ] {
        fonts.font_data.insert(name.into(), weighted(bytes, weight).into());
    }
    let fallback: Vec<String> = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let with_fallback = |first: &str| -> Vec<String> {
        std::iter::once(first.to_string()).chain(fallback.iter().cloned()).collect()
    };
    fonts.families.insert(FontFamily::Proportional, with_fallback("Inter"));
    fonts.families.insert(FontFamily::Monospace, with_fallback("JetBrains Mono"));
    fonts.families.insert(display(), with_fallback("Space Grotesk"));
    fonts.families.insert(mono_bold(), with_fallback("JetBrains Mono Bold"));
    fonts.families.insert(body_semibold(), with_fallback("Inter SemiBold"));
    ctx.set_fonts(fonts);
}

pub fn install_style(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.all_styles_mut(|style| {
        style.text_styles.insert(egui::TextStyle::Body, body(13.0));
        style.text_styles.insert(egui::TextStyle::Button, mono(12.0));
        style.text_styles.insert(egui::TextStyle::Small, mono(11.0));
        style.text_styles.insert(egui::TextStyle::Monospace, mono(12.0));
        style.text_styles.insert(egui::TextStyle::Heading, FontId::new(16.0, display()));
        style.spacing.item_spacing = Vec2::new(8.0, 8.0);
        style.spacing.button_padding = Vec2::new(14.0, 7.0);
        style.spacing.slider_rail_height = 4.0;
        style.spacing.interact_size.y = 28.0;
        style.interaction.tooltip_delay = 0.4;
        let v = &mut style.visuals;
        v.dark_mode = true;
        v.override_text_color = Some(TEXT);
        v.panel_fill = BG;
        v.window_fill = PANEL2;
        v.window_stroke = Stroke::new(1.0, BORDER_STRONG);
        v.window_corner_radius = CornerRadius::same(12);
        v.extreme_bg_color = PANEL2;
        v.faint_bg_color = PANEL;
        v.selection.bg_fill = AMBER;
        v.selection.stroke = Stroke::new(1.0, BG);
        v.hyperlink_color = TEAL;
        v.slider_trailing_fill = true;
        v.handle_shape = egui::style::HandleShape::Circle;
        let w = &mut v.widgets;
        for (state, fill, stroke) in [
            (&mut w.noninteractive, PANEL, BORDER),
            (&mut w.inactive, PANEL2, BORDER_STRONG),
            (&mut w.hovered, PANEL2, TEAL),
            (&mut w.active, BORDER_STRONG, TEAL),
            (&mut w.open, PANEL2, TEAL),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::new(1.0, stroke);
            state.fg_stroke = Stroke::new(1.0, TEXT);
            state.corner_radius = CornerRadius::same(8);
            state.expansion = 0.0;
        }
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        w.inactive.bg_fill = BORDER_STRONG; // rail des curseurs
    });
}

// ---------------------------------------------------------------------------
// Composants
// ---------------------------------------------------------------------------
pub fn card_frame() -> Frame {
    Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::symmetric(14, 12))
}

/// Pastille « 01 », « PLAN »…
pub fn tag(ui: &mut Ui, text: &str) -> Response {
    let galley = ui.painter().layout_no_wrap(tr(text), mono(11.0), AMBER);
    let size = galley.size() + Vec2::new(14.0, 4.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(size.x, size.y.max(18.0)), Sense::hover());
    ui.painter().rect_stroke(rect, CornerRadius::same(9), Stroke::new(1.0, BORDER_STRONG), StrokeKind::Inside);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, AMBER);
    resp
}

pub fn section_title(ui: &mut Ui, text: &str) -> Response {
    ui.label(egui::RichText::new(tr(text)).font(FontId::new(16.0, display())).color(TEXT))
}

/// Légende en capitales (police à chasse fixe, grisée).
pub fn caption(ui: &mut Ui, text: &str) -> Response {
    ui.label(egui::RichText::new(tr(text).to_uppercase()).font(mono(11.0)).color(MUTED))
}

pub fn caption_raw(ui: &mut Ui, text: &str) -> Response {
    ui.label(egui::RichText::new(text).font(mono(11.0)).color(MUTED))
}

pub fn muted(ui: &mut Ui, text: &str) -> Response {
    ui.label(egui::RichText::new(tr(text)).color(MUTED))
}

pub fn muted_wrap(ui: &mut Ui, text: &str) -> Response {
    ui.add(egui::Label::new(egui::RichText::new(tr(text)).color(MUTED)).wrap())
}

pub fn big_name(ui: &mut Ui, text: &str) -> Response {
    ui.add(egui::Label::new(egui::RichText::new(text).font(FontId::new(15.0, display())).color(TEXT)).wrap())
}

/// Interrupteur seul ; renvoie la réponse (`changed()` lors d'un clic).
pub fn switch(ui: &mut Ui, on: &mut bool, size: Vec2) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(resp.id, *on);
        let enabled = ui.is_enabled();
        let track = if *on { TEAL } else { BORDER_STRONG };
        let track = if enabled { track } else { alpha(track, 120) };
        let r = rect.height() / 2.0;
        ui.painter().rect_filled(rect, CornerRadius::same(r as u8), track);
        let x = egui::lerp((rect.left() + r)..=(rect.right() - r), t);
        let knob = if enabled { TEXT } else { MUTED };
        ui.painter().circle_filled(Pos2::new(x, rect.center().y), r - 2.5, knob);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Interrupteur suivi de son libellé (cliquable aussi).
pub fn toggle(ui: &mut Ui, on: &mut bool, text: &str) -> Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 9.0;
        let mut resp = switch(ui, on, Vec2::new(30.0, 16.0));
        if !text.is_empty() {
            let label = ui.add(egui::Label::new(tr(text)).sense(Sense::click()));
            if label.clicked() {
                *on = !*on;
                resp.mark_changed();
            }
            resp = resp.union(label);
        }
        resp
    })
    .inner
}

/// Choix exclusif sous forme de boutons accolés, de largeur égale.
pub fn segmented<T: PartialEq + Clone>(ui: &mut Ui, options: &[(T, String)], current: Option<&T>) -> Option<T> {
    let width = ui.available_width();
    segmented_width(ui, options, current, width)
}

pub fn segmented_width<T: PartialEq + Clone>(
    ui: &mut Ui,
    options: &[(T, String)],
    current: Option<&T>,
    width: f32,
) -> Option<T> {
    let n = options.len().max(1);
    let height = 28.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let seg_w = rect.width() / n as f32;
    let mut chosen = None;
    for (i, (value, text)) in options.iter().enumerate() {
        let r = Rect::from_min_size(Pos2::new(rect.left() + i as f32 * seg_w, rect.top()), Vec2::new(seg_w, height));
        let resp = ui.interact(r, ui.scope_id().with(("seg", i, text)), Sense::click());
        let selected = current == Some(value);
        let radius = CornerRadius {
            nw: if i == 0 { 7 } else { 0 },
            sw: if i == 0 { 7 } else { 0 },
            ne: if i == n - 1 { 7 } else { 0 },
            se: if i == n - 1 { 7 } else { 0 },
        };
        let (fill, stroke, color) = if selected {
            (AMBER, AMBER, BG)
        } else if resp.hovered() {
            (PANEL2, BORDER_STRONG, TEXT)
        } else {
            (PANEL2, BORDER_STRONG, MUTED)
        };
        let painter = ui.painter();
        painter.rect(r, radius, fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
        let font = if selected { FontId::new(11.0, mono_bold()) } else { mono(11.0) };
        painter.text(r.center(), Align2::CENTER_CENTER, text, font, color);
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !selected {
            chosen = Some(value.clone());
        }
    }
    chosen
}

/// Bouton « puce » (préréglages, Réglages) : sélectionné en sarcelle.
pub fn chip(ui: &mut Ui, text: &str, selected: bool, width: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 28.0), Sense::click());
    let (fill, stroke, color) = if selected {
        (alpha(TEAL, 28), TEAL, TEAL)
    } else if resp.hovered() {
        (PANEL2, TEAL, TEXT)
    } else {
        (PANEL2, BORDER_STRONG, TEXT)
    };
    ui.painter().rect(rect, CornerRadius::same(13), fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, mono(11.0), color);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Bouton standard ; `primary` : ambre plein.
pub fn button(ui: &mut Ui, text: &str, primary: bool) -> Response {
    let font = if primary { FontId::new(12.0, mono_bold()) } else { mono(12.0) };
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, TEXT);
    let size = Vec2::new(galley.size().x + 28.0, 32.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let enabled = ui.is_enabled();
    let (fill, stroke, color) = match (primary, enabled, resp.hovered()) {
        (_, false, _) => (PANEL, BORDER, DIM),
        (true, true, h) => (if h { AMBER_HOVER } else { AMBER }, AMBER, BG),
        (false, true, true) => (PANEL2, TEAL, TEXT),
        (false, true, false) => (PANEL, BORDER_STRONG, TEXT),
    };
    ui.painter().rect(rect, CornerRadius::same(8), fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
    if enabled { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp }
}

/// Événement d'un curseur : valeur en cours (aperçu) et valeur validée.
#[derive(Default)]
pub struct SliderEvent {
    pub moved: Option<f64>,
    pub released: bool,
    pub keyboard: bool,
}

/// Curseur avec libellé et valeur affichée.
#[allow(clippy::too_many_arguments)]
pub fn value_slider(
    ui: &mut Ui,
    text: &str,
    value: &mut f64,
    lo: f64,
    hi: f64,
    step: f64,
    shown: &str,
    teal: bool,
) -> SliderEvent {
    let mut ev = SliderEvent::default();
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tr(text)).color(MUTED));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(shown).font(mono(12.0)).color(AMBER));
            });
        });
        ui.scope(|ui| {
            if teal {
                ui.visuals_mut().selection.bg_fill = TEAL;
            }
            ui.spacing_mut().slider_width = ui.available_width();
            let before = *value;
            let resp = ui.add(egui::Slider::new(value, lo..=hi).step_by(step).show_value(false));
            let snapped = lo + ((*value - lo) / step).round() * step;
            *value = (snapped * 1e6).round() / 1e6;
            if (*value - before).abs() > 1e-12 {
                ev.moved = Some(*value);
                ev.keyboard = !resp.dragged() && !resp.drag_stopped();
            }
            if resp.drag_stopped() {
                ev.released = true;
            }
        });
    });
    ev
}

/// Bouton d'alimentation rond : ambre à l'arrêt, sarcelle lumineux en marche.
pub fn power_button(ui: &mut Ui, on: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(46.0), Sense::click());
    let p = ui.painter();
    let c = rect.center();
    let color = if !ui.is_enabled() {
        MUTED
    } else if on {
        TEAL
    } else {
        AMBER
    };
    if on {
        for (r, a) in [(23.0, 18u8), (21.0, 26), (19.5, 40)] {
            p.circle_filled(c, r, alpha(TEAL, a));
        }
    }
    p.circle(c, 18.0, if resp.hovered() { BORDER_STRONG } else { PANEL2 }, Stroke::new(2.0, color));
    // Arc de 300° ouvert en haut, puis la barre verticale.
    let points: Vec<Pos2> = (0..=40)
        .map(|i| {
            let a = (120.0 - 300.0 * i as f32 / 40.0).to_radians();
            Pos2::new(c.x + 8.0 * a.cos(), c.y - 8.0 * a.sin())
        })
        .collect();
    p.add(egui::Shape::line(points, Stroke::new(2.4, color)));
    p.line_segment([Pos2::new(c.x, c.y - 11.0), Pos2::new(c.x, c.y - 1.0)], Stroke::new(2.4, color));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Pastille d'état (« ● LIVE · A », « ○ VEILLE »).
pub fn status_pill(ui: &mut Ui, text: &str, live: bool) -> Response {
    let (fill, color) = if live { (alpha(TEAL, 30), TEAL) } else { (PANEL2, MUTED) };
    let galley = ui.painter().layout_no_wrap(text.to_string(), mono(11.0), color);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + Vec2::new(20.0, 8.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(10), fill);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
    resp
}
