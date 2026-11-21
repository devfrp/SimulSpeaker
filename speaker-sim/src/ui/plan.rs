//! Vue de dessus de la pièce : murs (traités ou non), matériaux, mobilier, enceintes,
//! auditeur, son direct et premières réflexions. L'auditeur et les enceintes se déplacent
//! à la souris ; le relâchement valide le nouveau placement.

use std::f32::consts::PI;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
    epaint::{RectShape, TextShape},
};
use serde_json::json;

use super::style::{AMBER, AMBER_HOVER, BG, BORDER_STRONG, MUTED, PANEL, PANEL2, RED, TEAL, TEXT, alpha, mono};
use crate::app::changes;
use crate::audio_engine::{self as ae, Layout, RoomSpec};
use crate::config::{Json, Settings};
use crate::i18n::{num, tr, trf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Listener,
    Left,
    Right,
}

/// Placement pendant un glissement (aperçu, pas encore enregistré).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub speaker_angle: f64,
    pub speaker_distance: f64,
    pub front_gap: f64,
    pub listener_offset: f64,
}

impl Placement {
    fn of(s: &Settings) -> Self {
        Placement {
            speaker_angle: s.speaker_angle,
            speaker_distance: s.speaker_distance,
            front_gap: s.front_gap,
            listener_offset: s.listener_offset,
        }
    }
}

#[derive(Default)]
pub struct PlanState {
    drag: Option<Target>,
    pub preview: Option<Placement>,
}

struct Geometry {
    scale: f32,
    origin: Pos2,
    width: f64,
    length: f64,
}

impl Geometry {
    fn fit(rect: Rect, room: &RoomSpec) -> Self {
        let m = 38.0;
        let w = rect.width() - 2.0 * m;
        let h = rect.height() - 2.0 * m - 18.0;
        let scale = (w / room.width as f32).min(h / room.length as f32).max(10.0);
        let (rw, rh) = (room.width as f32 * scale, room.length as f32 * scale);
        let origin = Pos2::new(rect.left() + (rect.width() - rw) / 2.0, rect.top() + m + (h - rh) / 2.0);
        Geometry { scale, origin, width: room.width, length: room.length }
    }

    /// Mur avant (x = L) en haut, gauche de l'auditeur (+y) à gauche de l'écran.
    fn to_screen(&self, x: f64, y: f64) -> Pos2 {
        Pos2::new(
            self.origin.x + (self.width - y) as f32 * self.scale,
            self.origin.y + (self.length - x) as f32 * self.scale,
        )
    }

    fn to_room(&self, p: Pos2) -> (f64, f64) {
        let y = self.width - f64::from((p.x - self.origin.x) / self.scale);
        let x = self.length - f64::from((p.y - self.origin.y) / self.scale);
        (x, y)
    }
}

fn layout_for(room: &RoomSpec, pl: &Placement) -> (RoomSpec, Option<Layout>) {
    let room = RoomSpec { front_gap: pl.front_gap, listener_offset: pl.listener_offset, ..room.clone() };
    let layout = ae::stereo_layout(&room, pl.speaker_angle, pl.speaker_distance).ok();
    (room, layout)
}

fn rotate(v: Vec2, angle: f32) -> Vec2 {
    let (s, c) = angle.sin_cos();
    Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
}

fn ellipse(p: &egui::Painter, c: Pos2, rx: f32, ry: f32, fill: Color32, stroke: Stroke) {
    p.add(Shape::ellipse_filled(c, Vec2::new(rx, ry), fill));
    if stroke.width > 0.0 {
        p.add(Shape::ellipse_stroke(c, Vec2::new(rx, ry), stroke));
    }
}

/// Dessine le plan ; renvoie le placement validé à la fin d'un glissement.
/// `s` : réglages effectifs, `room` : pièce correspondante.
pub fn room_plan(ui: &mut Ui, state: &mut PlanState, s: &Settings, room: &RoomSpec) -> Option<Json> {
    let size = Vec2::new(ui.available_width(), ui.available_height().max(220.0));
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, BG);
    let geo = Geometry::fit(rect, room);
    let placement = state.preview.unwrap_or_else(|| Placement::of(s));
    let (room_view, layout) = layout_for(room, &placement);

    // -- interaction -----------------------------------------------------------
    let hit = |pos: Pos2| -> Option<Target> {
        let lay = layout?;
        [(Target::Listener, lay.listener), (Target::Left, lay.left), (Target::Right, lay.right)]
            .into_iter()
            .find(|(_, pt)| geo.to_screen(pt[0], pt[1]).distance(pos) < (0.25 * geo.scale).max(16.0))
            .map(|(t, _)| t)
    };
    let mut committed = None;
    if resp.drag_started() {
        state.drag = resp.interact_pointer_pos().and_then(hit);
    }
    if let (Some(target), Some(pos)) = (state.drag, resp.interact_pointer_pos()) {
        if resp.dragged() {
            state.preview = Some(drag_to(&geo, &room_view, &placement, target, pos));
        }
    }
    if resp.drag_stopped() && state.drag.take().is_some() {
        if let Some(pl) = state.preview {
            committed = Some(changes(json!({
                "speaker_distance": pl.speaker_distance, "speaker_angle": pl.speaker_angle,
                "front_gap": pl.front_gap, "listener_offset": pl.listener_offset,
            })));
        }
    }
    if state.drag.is_some() {
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    } else if resp.hover_pos().and_then(hit).is_some() {
        ui.ctx().set_cursor_icon(CursorIcon::Grab);
    }
    let drag = state.drag;
    // Aperçu affiché pendant le glissement ; après validation les réglages prennent le relais.
    let (room_view, layout) = match state.preview {
        Some(pl) => layout_for(room, &pl),
        None => (room_view, layout),
    };
    paint(&painter, rect, &geo, s, &room_view, layout.as_ref(), state.preview.unwrap_or(placement), drag);
    committed
}

fn drag_to(geo: &Geometry, room: &RoomSpec, pl: &Placement, target: Target, pos: Pos2) -> Placement {
    let (x, y) = geo.to_room(pos);
    let a = pl.speaker_angle.to_radians();
    let mut sx = room.length - pl.front_gap;
    let mut dy = pl.speaker_distance * a.sin();
    let mut lx = sx - pl.speaker_distance * a.cos();
    let mut offset = pl.listener_offset;
    if target == Target::Listener {
        lx = x.max(0.3).min(sx - 0.3);
        let half = room.width / 2.0 - 0.3;
        offset = (y - room.width / 2.0).max(-half).min(half);
        if offset.abs() < 0.08 {
            offset = 0.0; // aimantation au centre
        }
    } else {
        sx = x.max(lx + 0.3).min(room.length - 0.2);
        dy = (y - room.width / 2.0).abs().max(0.15).min(room.width / 2.0 - 0.25);
    }
    let dx = sx - lx;
    let dist = dx.hypot(dy).clamp(0.5, 6.0);
    let angle = dy.atan2(dx).to_degrees().clamp(10.0, 60.0);
    let r2 = |v: f64| (v * 100.0).round() / 100.0;
    Placement {
        speaker_distance: r2(dist),
        speaker_angle: angle.round(),
        front_gap: r2(room.length - sx),
        listener_offset: r2(offset),
    }
}

#[allow(clippy::too_many_arguments)]
fn paint(
    p: &egui::Painter,
    area: Rect,
    geo: &Geometry,
    s: &Settings,
    room: &RoomSpec,
    layout: Option<&Layout>,
    pl: Placement,
    drag: Option<Target>,
) {
    let sc = geo.scale;
    let (w, l) = (room.width, room.length);
    let rect = Rect::from_two_pos(geo.to_screen(l, w), geo.to_screen(0.0, 0.0));

    // Sol
    p.rect_filled(rect, 0.0, PANEL);
    let floor = p.with_clip_rect(rect.intersect(area));
    match room.floor.as_str() {
        "tile" => {
            let step = 0.5 * sc;
            let stroke = Stroke::new(1.0, alpha(TEXT, 14));
            let mut x = rect.left();
            while x < rect.right() {
                floor.line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], stroke);
                x += step;
            }
            let mut y = rect.top();
            while y < rect.bottom() {
                floor.line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], stroke);
                y += step;
            }
        }
        "wood" | "rug" => {
            let stroke = Stroke::new(1.0, alpha(AMBER, 16));
            let mut y = rect.top();
            while y < rect.bottom() {
                floor.line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], stroke);
                y += 0.18 * sc;
            }
        }
        "carpet" => {
            floor.rect_filled(rect, 0.0, alpha(TEAL, 18));
        }
        _ => {}
    }
    let lis = layout.map(|l| l.listener);
    if let (Some(lis), "rug") = (lis, room.floor.as_str()) {
        let c = geo.to_screen(lis[0] + 0.6, w / 2.0);
        let rug = Rect::from_min_size(Pos2::new(c.x - 1.3 * sc, c.y - 1.0 * sc), Vec2::new(2.6 * sc, 2.0 * sc));
        floor.rect(rug, CornerRadius::same(8), alpha(TEAL, 22), Stroke::new(1.0, alpha(TEAL, 60)), StrokeKind::Inside);
    }

    // Mobilier
    let furn_stroke = Stroke::new(1.0, BORDER_STRONG);
    if let Some(lis) = lis.filter(|_| matches!(room.furniture.as_str(), "furnished" | "full")) {
        let c = geo.to_screen((lis[0] - 0.55).max(0.45), w / 2.0);
        let sofa = Rect::from_min_size(Pos2::new(c.x - 1.0 * sc, c.y - 0.05 * sc), Vec2::new(2.0 * sc, 0.75 * sc));
        floor.rect(sofa, CornerRadius::same(6), PANEL2, furn_stroke, StrokeKind::Inside);
    }
    if room.furniture == "full" {
        let a = geo.to_screen(l * 0.55, 0.38);
        floor.rect(
            Rect::from_min_size(a, Vec2::new(0.36 * sc, (2.2f64.min(l * 0.4)) as f32 * sc)),
            CornerRadius::ZERO,
            PANEL2,
            furn_stroke,
            StrokeKind::Inside,
        );
        let b = geo.to_screen(l * 0.35, w);
        floor.rect(Rect::from_min_size(b, Vec2::splat(0.9 * sc)), CornerRadius::same(5), PANEL2, furn_stroke, StrokeKind::Inside);
    }

    // Nuage acoustique (au plafond, au-dessus de la zone d'écoute)
    let wt = room.wall_treatment();
    if let (Some(lay), true) = (layout, wt.ceiling > 0.0) {
        let half = 1.2 * (wt.ceiling * 2.0).clamp(0.4, 1.0);
        let a = geo.to_screen(lay.left[0] - 0.1, w / 2.0 + half);
        let b = geo.to_screen(lay.listener[0] - 0.2, w / 2.0 - half);
        let cloud = Rect::from_two_pos(a, b);
        floor.rect_filled(cloud, CornerRadius::same(10), alpha(TEAL, (8.0 + 30.0 * wt.ceiling) as u8));
        dashed_rect(&floor, cloud, Stroke::new(1.2, alpha(TEAL, (60.0 + 120.0 * wt.ceiling) as u8)));
        floor.text(cloud.min + Vec2::new(6.0, 4.0), Align2::LEFT_TOP, tr("nuage"), mono(10.0), alpha(TEAL, 170));
    }

    // Murs
    p.rect_stroke(rect, CornerRadius::ZERO, Stroke::new(6.0, BORDER_STRONG), StrokeKind::Middle);

    // Panneaux absorbants répartis sur chaque mur
    let thick: f32 = match room.absorber.as_str() {
        "foam5" => 3.0,
        "wool20" => 8.0,
        _ => 5.0,
    };
    let walls = [
        (rect.left_top(), rect.right_top(), w, wt.front),
        (rect.left_bottom(), rect.right_bottom(), w, wt.rear),
        (rect.left_top(), rect.left_bottom(), l, wt.left),
        (rect.right_top(), rect.right_bottom(), l, wt.right),
    ];
    for (a, b, length, t) in walls {
        if t <= 0.0 {
            continue;
        }
        let n = ((t * length / 0.6).round() as usize).max(1);
        let width = 0.6f64.min(t * length / n as f64) as f32;
        let horizontal = (a.y - b.y).abs() < 1.0;
        for i in 0..n {
            let centre = (i as f32 + 0.5) / n as f32;
            let c = a + (b - a) * centre;
            let half = width * sc / 2.0;
            let r = if horizontal {
                Rect::from_center_size(c, Vec2::new(2.0 * half, 2.0 * thick))
            } else {
                Rect::from_center_size(c, Vec2::new(2.0 * thick, 2.0 * half))
            };
            p.rect_filled(r, CornerRadius::same(2), alpha(TEAL, 220));
        }
    }

    // Pièges à basses d'angle
    if room.bass_traps > 0.0 {
        let size = (0.25 + 0.25 * room.bass_traps) as f32 * sc;
        for (corner, dx, dy) in [
            (rect.left_top(), 1.0, 1.0),
            (rect.right_top(), -1.0, 1.0),
            (rect.left_bottom(), 1.0, -1.0),
            (rect.right_bottom(), -1.0, -1.0),
        ] {
            p.add(Shape::convex_polygon(
                vec![corner, corner + Vec2::new(dx * size, 0.0), corner + Vec2::new(0.0, dy * size)],
                alpha(TEAL, 150),
                Stroke::NONE,
            ));
        }
    }

    // Diffuseur sur le mur arrière
    if room.diffusion > 0.0 {
        let span = room.diffusion as f32 * rect.width() * 0.8;
        let x0 = rect.center().x - span / 2.0;
        let y = rect.bottom() - 4.0;
        let teeth = ((span / 9.0) as usize).max(3);
        let mut pts = vec![Pos2::new(x0, y)];
        for i in 0..teeth {
            let xa = x0 + span * (i as f32 + 0.5) / teeth as f32;
            let xb = x0 + span * (i as f32 + 1.0) / teeth as f32;
            pts.push(Pos2::new(xa, y - 7.0 - 5.0 * ((i * 7) % 3) as f32));
            pts.push(Pos2::new(xb, y));
        }
        p.add(Shape::line(pts, Stroke::new(1.4, alpha(AMBER, 200))));
    }

    let Some(lay) = layout else {
        p.text(rect.center(), Align2::CENTER_CENTER, tr("Placement impossible dans cette pièce"), mono(12.0), RED);
        labels(p, area, rect, room);
        return;
    };
    let lis = lay.listener;

    // Rayons : premières réflexions (murs) — seulement si elles sont simulées — puis son direct
    let beta = room.reflection_bands();
    let reflections_on = s.room_enabled && s.room_amount > 0.0 && s.reflection_order >= 1;
    if !reflections_on {
        let text = if !s.room_enabled || s.room_amount <= 0.0 {
            tr("pièce désactivée")
        } else {
            tr("réflexions désactivées (ordre : aucune)")
        };
        p.text(Pos2::new(rect.center().x, rect.center().y + 0.9 * sc + 9.0), Align2::CENTER_CENTER, text, mono(11.0), alpha(RED, 220));
    }
    for side in if reflections_on { &['L', 'R'][..] } else { &[][..] } {
        let src = lay.speaker(*side);
        for img in ae::image_sources(src, room, 1) {
            let Some(surf) = img.surface() else { continue };
            if !matches!(surf, "rear" | "front" | "right" | "left") {
                continue;
            }
            let gain = img.gain_bands(&beta)[3] as f32; // 1 kHz
            let (lx, ly) = (lis[0], lis[1]);
            let (ix, iy) = (img.position[0], img.position[1]);
            let hit = if matches!(surf, "rear" | "front") {
                let wx = if surf == "rear" { 0.0 } else { l };
                let k = if ix != lx { (wx - lx) / (ix - lx) } else { 0.0 };
                (wx, ly + k * (iy - ly))
            } else {
                let wy = if surf == "right" { 0.0 } else { w };
                let k = if iy != ly { (wy - ly) / (iy - ly) } else { 0.0 };
                (lx + k * (ix - lx), wy)
            };
            let stroke = Stroke::new(1.2, alpha(AMBER, (30.0 + 170.0 * gain * gain) as u8));
            let pts = [geo.to_screen(src[0], src[1]), geo.to_screen(hit.0, hit.1), geo.to_screen(lx, ly)];
            for seg in pts.windows(2) {
                p.extend(Shape::dashed_line(seg, stroke, 5.0, 3.0));
            }
            p.circle_filled(geo.to_screen(hit.0, hit.1), 3.0, alpha(AMBER, (60.0 + 160.0 * gain) as u8));
        }
    }
    for side in ['L', 'R'] {
        let sp = lay.speaker(side);
        p.line_segment([geo.to_screen(sp[0], sp[1]), geo.to_screen(lis[0], lis[1])], Stroke::new(2.0, AMBER));
    }

    // Enceintes orientées vers l'auditeur
    let head = geo.to_screen(lis[0], lis[1]);
    for (side, target) in [('L', Target::Left), ('R', Target::Right)] {
        let sp = lay.speaker(side);
        let c = geo.to_screen(sp[0], sp[1]);
        let angle = (head.y - c.y).atan2(head.x - c.x) - PI / 2.0;
        let hot = drag == Some(target);
        let body = Rect::from_center_size(c, Vec2::new(0.26 * sc, 0.24 * sc));
        p.add(
            RectShape::new(body, CornerRadius::same(4), if hot { AMBER_HOVER } else { AMBER }, Stroke::new(1.5, BG), StrokeKind::Middle)
                .with_angle(angle),
        );
        p.circle_filled(c + rotate(Vec2::new(0.0, 0.035 * sc), angle), 0.07 * sc, BG);
        let off = if side == 'L' { -0.32 * sc } else { 0.32 * sc };
        p.text(Pos2::new(c.x + off, c.y), Align2::CENTER_CENTER, side, egui::FontId::new(11.0, super::style::mono_bold()), AMBER);
    }

    // Auditeur : tête, nez vers l'avant
    let r = 0.16 * sc;
    if drag == Some(Target::Listener) {
        for (k, a) in [(2.4, 14u8), (1.9, 22), (1.4, 34)] {
            p.circle_filled(head, r * k, alpha(TEAL, a));
        }
    }
    p.add(Shape::convex_polygon(
        vec![head + Vec2::new(-r * 0.35, -r * 0.85), head + Vec2::new(r * 0.35, -r * 0.85), head + Vec2::new(0.0, -r * 1.35)],
        TEAL,
        Stroke::NONE,
    ));
    ellipse(p, head + Vec2::new(-r, 0.0), r * 0.25, r * 0.4, TEAL, Stroke::NONE);
    ellipse(p, head + Vec2::new(r, 0.0), r * 0.25, r * 0.4, TEAL, Stroke::NONE);
    p.circle(head, r, BG, Stroke::new(2.0, TEAL));

    // Cote du triangle stéréo
    let left = lay.left;
    let mid = (geo.to_screen(left[0], left[1]) + head.to_vec2()) / 2.0;
    p.text(
        Pos2::new(mid.x - 10.0, mid.y),
        Align2::RIGHT_CENTER,
        format!("{} m · {:.0}°", num(pl.speaker_distance, 2), pl.speaker_angle),
        mono(11.0),
        alpha(TEXT, 210),
    );
    labels(p, area, rect, room);
}

fn dashed_rect(p: &egui::Painter, r: Rect, stroke: Stroke) {
    let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    p.extend(Shape::dashed_line(&pts, stroke, 5.0, 3.0));
}

fn labels(p: &egui::Painter, area: Rect, rect: Rect, room: &RoomSpec) {
    let font = mono(11.0);
    p.text(
        Pos2::new(rect.center().x, rect.top() - 18.0),
        Align2::CENTER_CENTER,
        trf("mur avant · {w} m", &[("w", &num(room.width, 1))]),
        font.clone(),
        MUTED,
    );
    let mut dims = trf(
        "{l} m × {w} m × h {h} m  ·  {v} m³  ·  oreilles {e} m",
        &[
            ("l", &num(room.length, 1)),
            ("w", &num(room.width, 1)),
            ("h", &num(room.height, 1)),
            ("v", &num(room.volume(), 0)),
            ("e", &num(room.ear_height, 2)),
        ],
    );
    if room.speaker_height > 0.0 {
        dims += &trf(" · tweeters {t} m", &[("t", &num(room.speaker_height, 2))]);
    }
    p.text(Pos2::new(rect.center().x, rect.bottom() + 16.0), Align2::CENTER_CENTER, dims, font.clone(), MUTED);
    let galley = p.layout_no_wrap(format!("{} m", num(room.length, 1)), font.clone(), MUTED);
    let (gw, gh) = (galley.size().x, galley.size().y);
    let pos = Pos2::new(rect.left() - 22.0 - gh / 2.0, rect.center().y + gw / 2.0);
    p.add(TextShape::new(pos, galley, MUTED).with_angle(-PI / 2.0));
    p.text(
        Pos2::new(area.right() - 8.0, area.bottom() - 4.0),
        Align2::RIGHT_BOTTOM,
        tr("glissez l'auditeur ou les enceintes  ·  — direct  ┄ réflexions"),
        font,
        alpha(MUTED, 160),
    );
}
