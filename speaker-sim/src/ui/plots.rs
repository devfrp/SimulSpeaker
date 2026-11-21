//! Courbes : réponse en fréquence simulée et RT60 par octave (vu-mètres).

use eframe::egui::{self, Align2, Color32, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use super::style::{AMBER, BORDER, BORDER_STRONG, MUTED, RED, TEAL, TEXT, alpha, mono};
use crate::i18n::{num, tr};

const COLORS: [Color32; 3] = [AMBER, TEAL, TEXT];
const NAMES: [&str; 3] = ["enceinte", "casque", "oreille"];

/// Réponse simulée : enceinte, correction du casque, oreille (recalées sur 200 Hz – 2 kHz).
pub fn response_plot(ui: &mut Ui, freqs: Option<&[f64]>, curves: &[(String, Vec<f64>)]) {
    let size = Vec2::new(ui.available_width(), ui.available_height().max(90.0));
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter_at(rect);
    let Some(freqs) = freqs.filter(|_| !curves.is_empty()) else {
        p.text(rect.center(), Align2::CENTER_CENTER, tr("« Appliquer » pour calculer la réponse simulée"), mono(10.0), MUTED);
        return;
    };
    let r = Rect::from_min_max(rect.min + Vec2::new(36.0, 22.0), rect.max - Vec2::new(8.0, 20.0));
    let (lo_db, hi_db) = (-24.0, 12.0);
    let x_of = |f: f64| r.left() + r.width() * ((f.log10() - 20f64.log10()) / 3.0) as f32;
    let y_of = |db: f64| r.bottom() - r.height() * ((db.clamp(lo_db, hi_db) - lo_db) / (hi_db - lo_db)) as f32;
    for (f, txt) in [(20.0, "20"), (100.0, "100"), (1000.0, "1k"), (10000.0, "10k"), (20000.0, "20k")] {
        let x = x_of(f);
        p.line_segment([Pos2::new(x, r.top()), Pos2::new(x, r.bottom())], Stroke::new(1.0, BORDER));
        p.text(Pos2::new(x, r.bottom() + 4.0), Align2::CENTER_TOP, txt, mono(10.0), MUTED);
    }
    for db in [-24.0, -12.0, 0.0, 12.0] {
        let y = y_of(db);
        let c = if db == 0.0 { BORDER_STRONG } else { BORDER };
        p.line_segment([Pos2::new(r.left(), y), Pos2::new(r.right(), y)], Stroke::new(1.0, c));
        p.text(Pos2::new(rect.left() + 30.0, y), Align2::RIGHT_CENTER, format!("{db:+}"), mono(10.0), MUTED);
    }
    for (i, (_, values)) in curves.iter().enumerate() {
        let band: Vec<f64> =
            freqs.iter().zip(values).filter(|(f, _)| (200.0..=2000.0).contains(*f)).map(|(_, v)| *v).collect();
        let offset = if band.is_empty() { 0.0 } else { band.iter().sum::<f64>() / band.len() as f64 };
        let pts: Vec<Pos2> = freqs.iter().zip(values).map(|(f, v)| Pos2::new(x_of(*f), y_of(v - offset))).collect();
        p.add(egui::Shape::line(pts, Stroke::new(1.8, alpha(COLORS[i % 3], 230))));
    }
    let mut x = r.left();
    for (i, (name, _)) in curves.iter().enumerate() {
        let short = if i < NAMES.len() { tr(NAMES[i]) } else { name.clone() };
        p.rect_filled(Rect::from_min_size(Pos2::new(x, rect.top() + 8.0), Vec2::new(12.0, 2.0)), 0.0, COLORS[i % 3]);
        let g = p.layout_no_wrap(short, mono(10.0), alpha(TEXT, 210));
        let w = g.size().x;
        p.galley(Pos2::new(x + 16.0, rect.top() + 2.0), g, TEXT);
        x += 16.0 + w + 16.0;
    }
}

const BANDS: [&str; 7] = ["125", "250", "500", "1k", "2k", "4k", "8k"];

/// RT60 par bande d'octave, façon vu-mètre ; zone conseillée en sarcelle.
pub fn rt_meter(ui: &mut Ui, values: &[f64; 7]) {
    let size = Vec2::new(ui.available_width(), (ui.available_height() - 24.0).max(90.0));
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter_at(rect);
    let r = Rect::from_min_max(rect.min + Vec2::new(8.0, 18.0), rect.max - Vec2::new(8.0, 20.0));
    let top = (values.iter().copied().fold(0.0, f64::max) * 1.15).max(1.0);
    let y_of = |t: f64| r.bottom() - r.height() * (t.min(top) / top) as f32;
    // Zone conseillée pour l'écoute critique (0,2–0,4 s)
    p.rect_filled(Rect::from_x_y_ranges(r.x_range(), y_of(0.4)..=y_of(0.2)), 0.0, alpha(TEAL, 26));
    p.add(egui::Shape::dashed_line(
        &[Pos2::new(r.left(), y_of(0.4)), Pos2::new(r.right(), y_of(0.4))],
        Stroke::new(1.0, alpha(TEAL, 90)),
        4.0,
        3.0,
    ));
    let n = values.len();
    let slot = r.width() / n as f32;
    let segs = 14;
    for (i, &v) in values.iter().enumerate() {
        let x = r.left() + i as f32 * slot + slot * 0.22;
        let w = slot * 0.56;
        let lit = (segs as f64 * v.min(top) / top).round() as usize;
        let seg_h = r.height() / segs as f32;
        for k in 0..segs {
            let y = r.bottom() - (k + 1) as f32 * seg_h + 1.5;
            let level = (k + 1) as f64 / segs as f64 * top;
            let color = if k < lit {
                if level <= 0.45 {
                    TEAL
                } else if level <= 0.8 {
                    AMBER
                } else {
                    RED
                }
            } else {
                BORDER
            };
            p.rect_filled(Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, (seg_h - 3.0).max(1.0))), 0.0, color);
        }
        let cx = x + w / 2.0;
        p.text(Pos2::new(cx, r.top() - 4.0), Align2::CENTER_BOTTOM, num(v, 2), mono(10.0), alpha(TEXT, 220));
        p.text(Pos2::new(cx, r.bottom() + 4.0), Align2::CENTER_TOP, BANDS[i], mono(10.0), MUTED);
    }
}
