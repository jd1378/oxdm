//! Bordered progress bar with a centered "{label} · {pct}%" caption.
//! Used by the main download table's status column.

use eframe::egui::{self, Color32, Painter, Pos2, Rect, Stroke, Vec2};

use crate::ui::color::clay;
use crate::ui::theme::{self, radius};

/// Paint a status-column progress bar into `rect`.
///
/// Track: `bg_sunken` with a 1px black-@10% inside stroke and `SM`
/// corner radius. Fill: clay-300 with alpha 150 when `selected`, 100
/// otherwise — inset 1px so its rounded corners follow the track
/// curve at 100% without bleeding past the border. Caption is drawn
/// centered with `fg_1`.
pub fn inline_progress(
    painter: &Painter,
    rect: Rect,
    t: &theme::Tokens,
    frac: f32,
    label: &str,
    selected: bool,
) {
    let frac = frac.clamp(0.0, 1.0);
    let fill_color = {
        let c = clay::C300;
        let a = if selected { 150 } else { 100 };
        Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
    };
    let stroke_color = Color32::from_black_alpha(26);
    painter.rect(
        rect,
        radius::SM as f32,
        t.bg_sunken,
        Stroke::new(1.0, stroke_color),
        egui::StrokeKind::Inside,
    );
    if frac > 0.0 {
        // To avoid the fill leaking past the track's rounded corners at
        // small percentages, paint the fill as a full-width rounded rect
        // matching the track curve and clip its visible portion to a
        // rectangular strip — geometrically: intersection of the track
        // shape with [left, left+fill_w]. This way the top/bottom edges
        // always sit inside the track's curve regardless of frac.
        let inner = rect.shrink(1.0);
        let r = (radius::SM as f32 - 1.0).max(0.0);
        let fill_w = (inner.width() * frac).min(inner.width());
        let strip = Rect::from_min_size(inner.min, Vec2::new(fill_w, inner.height()));
        painter
            .with_clip_rect(strip)
            .rect_filled(inner, r, fill_color);
    }
    let pct = (frac * 100.0).round() as i32;
    let text = format!("{label} · {pct}%");
    let g = painter.layout_no_wrap(text, theme::body_bold(11.0), t.fg_1);
    painter.galley(
        Pos2::new(
            rect.center().x - g.size().x / 2.0,
            rect.center().y - g.size().y / 2.0,
        ),
        g,
        t.fg_1,
    );
}
