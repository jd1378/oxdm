//! Horizontal rounded-rect progress bar.

use eframe::egui::{Color32, CornerRadius, Rect, Response, Sense, Ui, Vec2};

/// `frac` clamped to [0, 1]. Caller chooses `track`/`fill` (use
/// `Tokens::progress_track` / `fill` or status colours).
pub fn pill_progress(
    ui: &mut Ui,
    frac: f32,
    width: f32,
    height: f32,
    track: Color32,
    fill: Color32,
) -> Response {
    let frac = frac.clamp(0.0, 1.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter().clone();
    let rounding: CornerRadius = (height * 0.5).into();
    painter.rect_filled(rect, rounding, track);
    if frac > 0.0 {
        // Paint the fill as if it spanned the full track (so its rounded
        // corners follow the track curve), and clip visibility to a
        // vertical strip [left, left+fw]. Geometrically intersects the
        // pill shape with the strip — no bleed past the track at small
        // frac.
        let fw = (rect.width() * frac).min(rect.width());
        let strip = Rect::from_min_size(rect.min, Vec2::new(fw, rect.height()));
        ui.painter_at(strip).rect_filled(rect, rounding, fill);
    }
    resp
}
