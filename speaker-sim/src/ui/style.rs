//! Identité visuelle (thème Cobalt du site, clair ou sombre ; polices) et petits
//! composants : cartes, légendes, interrupteurs, touches, curseurs, bouton d'alimentation.
//!
//! Habillage : papier, encre, filets, un seul signal cobalt. Les vues de mesure (plan,
//! courbes, RT60) restent sur graphite, comme le code sur le site.

use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame, Margin, Pos2, Rect,
    Response, Sense, Stroke, StrokeKind, Ui, Vec2,
    epaint::text::{FontTweak, VariationCoords},
};

use crate::i18n::tr;

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// Couleurs d'un mode (clair ou sombre, re-dérivé et non inversé).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    // Habillage
    pub paper: Color32,
    pub paper2: Color32,
    pub paper3: Color32,
    pub ink: Color32,
    pub ink2: Color32,
    pub ink_muted: Color32,
    pub ink_faint: Color32,
    pub rule: Color32,
    pub rule2: Color32,
    pub accent: Color32,
    pub accent_deep: Color32,
    pub accent_edge: Color32,
    pub accent_wash: Color32,
    pub accent_ink: Color32,
    pub key_face: Color32,
    pub key_edge: Color32,
    // Vues de mesure (graphite)
    pub bg: Color32,
    pub panel: Color32,
    pub panel2: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub signal: Color32,
    pub signal_2: Color32,
    pub red: Color32,
}

pub const LIGHT: Palette = Palette {
    paper: hex(0xf8fbfe),
    paper2: hex(0xeff3f8),
    paper3: hex(0xe3e8ef),
    ink: hex(0x111825),
    ink2: hex(0x363e49),
    ink_muted: hex(0x5d646e),
    ink_faint: hex(0x9aa1ab),
    rule: hex(0xdadee4),
    rule2: hex(0xc2c8cf),
    accent: hex(0x0462d3),
    accent_deep: hex(0x0040bb),
    accent_edge: hex(0x003d98),
    accent_wash: hex(0xe3f0ff),
    accent_ink: hex(0xf8fbfe),
    key_face: hex(0xfcfdff),
    key_edge: hex(0xb7bec8),
    bg: hex(0x121823),
    panel: hex(0x1e2532),
    panel2: hex(0x262f3e),
    border: hex(0x2d3645),
    border_strong: hex(0x3a4b66),
    text: hex(0xe3e8ee),
    muted: hex(0x939ca9),
    signal: hex(0x75b4ff),
    signal_2: hex(0xe3e8ee),
    red: hex(0xf2555a),
};

pub const DARK: Palette = Palette {
    paper: hex(0x171e29),
    paper2: hex(0x10161f),
    paper3: hex(0x212935),
    ink: hex(0xe6ecf2),
    ink2: hex(0xbec5cd),
    ink_muted: hex(0x9199a4),
    ink_faint: hex(0x626a75),
    rule: hex(0x29313d),
    rule2: hex(0x3a4350),
    accent: hex(0x498beb),
    accent_deep: hex(0x85bbfd),
    accent_edge: hex(0x1f53aa),
    accent_wash: hex(0x172a45),
    accent_ink: hex(0x070b12),
    key_face: hex(0x1d2531),
    key_edge: hex(0x0a0e14),
    bg: hex(0x0a0e15),
    panel: hex(0x121823),
    panel2: hex(0x1a2130),
    border: hex(0x232b39),
    border_strong: hex(0x314058),
    text: hex(0xe3e8ee),
    muted: hex(0x939ca9),
    signal: hex(0x75b4ff),
    signal_2: hex(0xe3e8ee),
    red: hex(0xf2555a),
};

/// Enceinte en cours de déplacement sur le plan.
pub const SIGNAL_HOT: Color32 = hex(0xa9d0ff);

static DARK_MODE: AtomicBool = AtomicBool::new(false);

/// Palette du mode actif.
pub fn pal() -> &'static Palette {
    if DARK_MODE.load(Ordering::Relaxed) { &DARK } else { &LIGHT }
}

/// Mode effectif d'un réglage « auto », « light » ou « dark » (auto : celui du système).
pub fn resolve_mode(theme: &str, system_dark: bool) -> bool {
    match theme {
        "dark" => true,
        "light" => false,
        _ => system_dark,
    }
}

/// Active le mode clair ou sombre ; vrai s'il a changé.
pub fn set_dark(dark: bool) -> bool {
    DARK_MODE.swap(dark, Ordering::Relaxed) != dark
}

pub fn is_dark() -> bool {
    DARK_MODE.load(Ordering::Relaxed)
}

/// Couleur avec opacité (0–255).
pub fn alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

static PLEX: &[u8] = include_bytes!("../../packaging/fonts/IBMPlexSans.ttf");
static FAMILJEN: &[u8] = include_bytes!("../../packaging/fonts/FamiljenGrotesk.ttf");
static MONO: &[u8] = include_bytes!("../../packaging/fonts/JetBrainsMono.ttf");

fn weighted(bytes: &'static [u8], weight: f32) -> FontData {
    FontData::from_static(bytes)
        .tweak(FontTweak { coords: VariationCoords::new([("wght", weight)]), ..Default::default() })
}

/// Titres (Familjen Grotesk, demi-gras).
pub fn display() -> FontFamily {
    FontFamily::Name("display".into())
}

/// Touches, légendes, pastilles (Familjen Grotesk, medium).
pub fn display_medium() -> FontFamily {
    FontFamily::Name("display-medium".into())
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

pub fn label_font(size: f32) -> FontId {
    FontId::new(size, display_medium())
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    for (name, bytes, weight) in [
        ("IBM Plex Sans", PLEX, 400.0),
        ("IBM Plex Sans SemiBold", PLEX, 600.0),
        ("Familjen Grotesk", FAMILJEN, 600.0),
        ("Familjen Grotesk Medium", FAMILJEN, 500.0),
        ("JetBrains Mono", MONO, 400.0),
        ("JetBrains Mono Bold", MONO, 700.0),
    ] {
        fonts.font_data.insert(name.into(), weighted(bytes, weight).into());
    }
    let fallback: Vec<String> = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    // JetBrains Mono en secours : symboles absents des polices de texte (○ ● → ★).
    let with_fallback = |first: &str| -> Vec<String> {
        [first, "JetBrains Mono"].iter().map(|f| (*f).to_string()).chain(fallback.iter().cloned()).collect()
    };
    fonts.families.insert(FontFamily::Proportional, with_fallback("IBM Plex Sans"));
    fonts.families.insert(FontFamily::Monospace, with_fallback("JetBrains Mono"));
    fonts.families.insert(display(), with_fallback("Familjen Grotesk"));
    fonts.families.insert(display_medium(), with_fallback("Familjen Grotesk Medium"));
    fonts.families.insert(mono_bold(), with_fallback("JetBrains Mono Bold"));
    fonts.families.insert(body_semibold(), with_fallback("IBM Plex Sans SemiBold"));
    ctx.set_fonts(fonts);
}

/// Style egui du mode actif (à rappeler après [`set_dark`]).
pub fn install_style(ctx: &egui::Context) {
    let p = *pal();
    let dark = is_dark();
    ctx.set_theme(if dark { egui::ThemePreference::Dark } else { egui::ThemePreference::Light });
    ctx.all_styles_mut(|style| {
        style.text_styles.insert(egui::TextStyle::Body, body(13.0));
        style.text_styles.insert(egui::TextStyle::Button, label_font(13.0));
        style.text_styles.insert(egui::TextStyle::Small, mono(11.0));
        style.text_styles.insert(egui::TextStyle::Monospace, mono(12.0));
        style.text_styles.insert(egui::TextStyle::Heading, FontId::new(16.0, display()));
        style.spacing.item_spacing = Vec2::new(8.0, 8.0);
        style.spacing.button_padding = Vec2::new(14.0, 7.0);
        style.spacing.slider_rail_height = 4.0;
        style.spacing.interact_size.y = 28.0;
        style.interaction.tooltip_delay = 0.4;
        let v = &mut style.visuals;
        v.dark_mode = dark;
        v.override_text_color = Some(p.ink);
        v.panel_fill = p.paper2;
        v.window_fill = p.paper;
        v.window_stroke = Stroke::new(1.0, p.rule2);
        v.window_corner_radius = CornerRadius::same(8);
        v.menu_corner_radius = CornerRadius::same(6);
        v.extreme_bg_color = p.key_face;
        v.text_edit_bg_color = Some(p.key_face);
        v.faint_bg_color = p.paper2;
        v.selection.bg_fill = p.accent;
        v.selection.stroke = Stroke::new(1.0, p.accent_ink);
        v.hyperlink_color = p.accent_deep;
        v.slider_trailing_fill = true;
        v.handle_shape = egui::style::HandleShape::Circle;
        let w = &mut v.widgets;
        for (state, fill, stroke) in [
            (&mut w.noninteractive, p.paper, p.rule),
            (&mut w.inactive, p.rule2, p.ink_muted),
            (&mut w.hovered, p.key_face, p.accent),
            (&mut w.active, p.paper3, p.accent),
            (&mut w.open, p.key_face, p.accent),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::new(1.0, stroke);
            state.fg_stroke = Stroke::new(1.0, p.ink);
            state.corner_radius = CornerRadius::same(6);
            state.expansion = 0.0;
        }
        // Listes déroulantes et boutons egui natifs : face de touche, texte cobalt au survol.
        w.inactive.weak_bg_fill = p.key_face;
        w.hovered.weak_bg_fill = p.accent_wash;
        w.hovered.fg_stroke = Stroke::new(1.0, p.accent_deep);
        w.active.fg_stroke = Stroke::new(1.0, p.accent_deep);
    });
}

// ---------------------------------------------------------------------------
// Composants
// ---------------------------------------------------------------------------
pub fn card_frame() -> Frame {
    let p = pal();
    Frame::new()
        .fill(p.paper)
        .stroke(Stroke::new(1.0, p.rule))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(14, 12))
}

/// Première lettre en capitale (« bande claire… » → « Bande claire… »).
pub fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

/// Pastille « 01 », « PLAN »…
pub fn tag(ui: &mut Ui, text: &str) -> Response {
    let p = pal();
    let galley = ui.painter().layout_no_wrap(tr(text), mono(11.0), p.accent_deep);
    let size = galley.size() + Vec2::new(12.0, 4.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(size.x, size.y.max(18.0)), Sense::hover());
    ui.painter().rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0, p.rule2), StrokeKind::Inside);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, p.accent_deep);
    resp
}

pub fn section_title(ui: &mut Ui, text: &str) -> Response {
    ui.label(egui::RichText::new(tr(text)).font(FontId::new(16.0, display())).color(pal().ink))
}

/// Légende en casse normale (première lettre en capitale), grisée.
pub fn caption(ui: &mut Ui, text: &str) -> Response {
    caption_raw(ui, &sentence(&tr(text)))
}

pub fn caption_raw(ui: &mut Ui, text: &str) -> Response {
    ui.label(egui::RichText::new(text).font(label_font(12.0)).color(pal().ink_muted))
}

pub fn muted(ui: &mut Ui, text: &str) -> Response {
    ui.label(egui::RichText::new(tr(text)).color(pal().ink_muted))
}

pub fn muted_wrap(ui: &mut Ui, text: &str) -> Response {
    ui.add(egui::Label::new(egui::RichText::new(tr(text)).color(pal().ink_muted)).wrap())
}

pub fn big_name(ui: &mut Ui, text: &str) -> Response {
    ui.add(egui::Label::new(egui::RichText::new(text).font(FontId::new(15.0, display())).color(pal().ink)).wrap())
}

/// Interrupteur seul ; renvoie la réponse (`changed()` lors d'un clic).
pub fn switch(ui: &mut Ui, on: &mut bool, size: Vec2) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let p = pal();
        let t = ui.ctx().animate_bool_responsive(resp.id, *on);
        let enabled = ui.is_enabled();
        let track = if *on { p.accent } else { p.rule2 };
        let track = if enabled { track } else { alpha(track, 120) };
        let r = rect.height() / 2.0;
        ui.painter().rect_filled(rect, CornerRadius::same(r as u8), track);
        let x = egui::lerp((rect.left() + r)..=(rect.right() - r), t);
        let knob = if enabled { p.key_face } else { p.paper2 };
        ui.painter().circle(Pos2::new(x, rect.center().y), r - 2.5, knob, Stroke::new(0.5, alpha(p.key_edge, 160)));
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Interrupteur suivi de son libellé (cliquable aussi).
pub fn toggle(ui: &mut Ui, on: &mut bool, text: &str) -> Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 9.0;
        let mut resp = switch(ui, on, Vec2::new(30.0, 16.0));
        if !text.is_empty() {
            let color = if ui.is_enabled() { pal().ink } else { pal().ink_faint };
            let label = ui.add(egui::Label::new(egui::RichText::new(tr(text)).color(color)).sense(Sense::click()));
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

/// Couleurs d'une touche.
#[derive(Clone, Copy)]
struct KeyLook {
    face: Color32,
    rim: Color32,
    edge: Color32,
    text: Color32,
    down: bool,
}

/// Peint une touche : arête basse de 3 px (1 px enfoncée), face cerclée d'un filet.
/// Renvoie le rectangle de la face, où centrer le libellé.
fn paint_key(painter: &egui::Painter, rect: Rect, radius: CornerRadius, look: KeyLook) -> Rect {
    let lift = if look.down { 1.0 } else { 3.0 };
    let face = Rect::from_min_max(Pos2::new(rect.left(), rect.top() + (3.0 - lift)), rect.max);
    painter.rect_filled(face, radius, look.edge);
    let top = Rect::from_min_max(face.min, Pos2::new(face.right(), face.bottom() - lift));
    painter.rect(top, radius, look.face, Stroke::new(1.0, look.rim), StrokeKind::Inside);
    top
}

fn key_look(hovered: bool, enabled: bool) -> KeyLook {
    let p = pal();
    if !enabled {
        return KeyLook { face: p.paper2, rim: p.rule, edge: p.rule, text: p.ink_faint, down: false };
    }
    KeyLook {
        face: p.key_face,
        rim: if hovered { p.key_edge } else { p.rule2 },
        edge: p.key_edge,
        text: if hovered { p.accent_deep } else { p.ink },
        down: false,
    }
}

/// Choix exclusif sous forme de touches accolées, de largeur égale.
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
    let p = pal();
    let n = options.len().max(1);
    let height = 30.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let seg_w = rect.width() / n as f32;
    let enabled = ui.is_enabled();
    let mut chosen = None;
    for (i, (value, text)) in options.iter().enumerate() {
        let r = Rect::from_min_size(Pos2::new(rect.left() + i as f32 * seg_w, rect.top()), Vec2::new(seg_w, height));
        let resp = ui.interact(r, ui.scope_id().with(("seg", i, text)), Sense::click());
        let selected = current == Some(value);
        let radius = CornerRadius {
            nw: if i == 0 { 6 } else { 0 },
            sw: if i == 0 { 6 } else { 0 },
            ne: if i == n - 1 { 6 } else { 0 },
            se: if i == n - 1 { 6 } else { 0 },
        };
        let mut look = key_look(resp.hovered(), enabled);
        if enabled {
            look.text = if selected || resp.hovered() { p.accent_deep } else { p.ink2 };
            look.rim = p.rule2;
        }
        if selected {
            look.face = p.paper3;
            look.down = true;
        }
        let face = paint_key(ui.painter(), r, radius, look);
        let font = if selected { FontId::new(12.0, display()) } else { label_font(12.0) };
        ui.painter().text(face.center(), Align2::CENTER_CENTER, text, font, look.text);
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !selected && enabled {
            chosen = Some(value.clone());
        }
    }
    chosen
}

/// Touche « puce » (préréglages, Réglages) : sélectionnée en cobalt.
pub fn chip(ui: &mut Ui, text: &str, selected: bool, width: f32) -> Response {
    let p = pal();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 30.0), Sense::click());
    let mut look = key_look(resp.hovered(), ui.is_enabled());
    if selected {
        look = KeyLook { face: p.accent_wash, rim: p.accent, edge: p.accent, text: p.accent_deep, down: true };
    }
    let face = paint_key(ui.painter(), rect, CornerRadius::same(6), look);
    ui.painter().text(face.center(), Align2::CENTER_CENTER, text, label_font(12.0), look.text);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Touche standard ; `primary` : cobalt plein.
pub fn button(ui: &mut Ui, text: &str, primary: bool) -> Response {
    let p = pal();
    let font = if primary { FontId::new(13.0, display()) } else { label_font(13.0) };
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, p.ink);
    let size = Vec2::new(galley.size().x + 28.0, 34.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let enabled = ui.is_enabled();
    let hovered = resp.hovered();
    let down = resp.is_pointer_button_down_on();
    let mut look = key_look(hovered, enabled);
    if enabled && primary {
        let face = if down {
            p.accent_edge
        } else if hovered {
            p.accent_deep
        } else {
            p.accent
        };
        look = KeyLook { face, rim: p.accent_edge, edge: p.accent_edge, text: p.accent_ink, down };
    } else if enabled && down {
        look.face = p.paper3;
        look.down = true;
    }
    let face = paint_key(ui.painter(), rect, CornerRadius::same(6), look);
    ui.painter().galley(face.center() - galley.size() / 2.0, galley, look.text);
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
    _accent: bool,
) -> SliderEvent {
    let p = pal();
    let mut ev = SliderEvent::default();
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tr(text)).color(p.ink_muted));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(shown).font(mono(12.0)).color(p.accent_deep));
            });
        });
        ui.scope(|ui| {
            // Rail en filet (fond inactif) ; poignée cerclée comme une touche.
            ui.visuals_mut().widgets.inactive.fg_stroke = Stroke::new(1.0, p.key_edge);
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

/// Bouton d'alimentation en touche : face claire à l'arrêt, cobalt enfoncé en marche.
pub fn power_button(ui: &mut Ui, on: bool) -> Response {
    let p = pal();
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(46.0), Sense::click());
    let down = on || resp.is_pointer_button_down_on();
    let (mut face, mut edge, mut rim) = if on {
        (p.accent, p.accent_edge, p.accent_edge)
    } else if down {
        (p.paper3, p.key_edge, p.rule2)
    } else {
        (p.key_face, p.key_edge, p.rule2)
    };
    let mut glyph = if on {
        p.accent_ink
    } else if resp.hovered() {
        p.accent_deep
    } else {
        p.ink
    };
    if !ui.is_enabled() {
        (face, edge, rim, glyph) = (p.paper2, p.rule, p.rule, p.ink_faint);
    }
    let key = Rect::from_min_size(rect.min + Vec2::new(4.0, 4.0), Vec2::new(38.0, 39.0));
    let top = paint_key(ui.painter(), key, CornerRadius::same(7), KeyLook { face, rim, edge, text: glyph, down });
    let c = top.center();
    let stroke = Stroke::new(2.2, glyph);
    // Arc de 300° ouvert en haut, puis la barre verticale.
    let points: Vec<Pos2> = (0..=40)
        .map(|i| {
            let a = (120.0 - 300.0 * i as f32 / 40.0).to_radians();
            Pos2::new(c.x + 8.0 * a.cos(), c.y + 1.0 - 8.0 * a.sin())
        })
        .collect();
    ui.painter().add(egui::Shape::line(points, stroke));
    ui.painter().line_segment([Pos2::new(c.x, c.y - 9.0), Pos2::new(c.x, c.y + 1.0)], stroke);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Pastille d'état (« ● LIVE · A », « ○ VEILLE ») ; le rond est dessiné (glyphe absent
/// des polices embarquées), plein en marche, vide en veille.
pub fn status_pill(ui: &mut Ui, text: &str, live: bool) -> Response {
    let p = pal();
    let (fill, color) = if live { (p.accent_wash, p.accent_deep) } else { (p.paper2, p.ink_muted) };
    let label = text.trim_start_matches(['●', '○']).trim_start();
    let dot = label.len() != text.len();
    let galley = ui.painter().layout_no_wrap(label.to_string(), label_font(12.0), color);
    let lead = if dot { 14.0 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(galley.size() + Vec2::new(20.0 + lead, 8.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(4), fill);
    let text_pos = Pos2::new(rect.left() + 10.0 + lead, rect.center().y - galley.size().y / 2.0);
    if dot {
        let c = Pos2::new(rect.left() + 14.0, rect.center().y);
        if live {
            ui.painter().circle_filled(c, 3.5, color);
        } else {
            ui.painter().circle_stroke(c, 3.0, Stroke::new(1.2, color));
        }
    }
    ui.painter().galley(text_pos, galley, color);
    resp
}
