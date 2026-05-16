//! Big pill-shaped progress bar with optional diagonal stripes scrolling
//! across the filled portion. Mirrors the indeterminate-feel of the
//! download window's hero progress while still showing real progress.

use std::time::Duration;

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

/// Paint a pill-shaped progress bar. `fill_gradient` is `Some((c_left,
/// c_right))` for a horizontal gradient (active downloads), `None` for
/// a solid `fill`. When `animate` is true, scrolls 45° stripes across
/// the bar and requests a follow-up repaint. `bg` is the colour behind
/// the bar — used to mask any stripe pixels that leak past the rounded
/// curve when stripes are drawn through the rounded caps.
#[allow(clippy::too_many_arguments)]
pub fn striped_progress(
    ui: &mut egui::Ui,
    frac: f32,
    width: f32,
    height: f32,
    track: Color32,
    fill: Color32,
    fill_gradient: Option<(Color32, Color32)>,
    animate: bool,
    bg: Color32,
) {
    let frac = frac.clamp(0.0, 1.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter().clone();
    let r = height / 2.0;
    painter.rect_filled(rect, r, track);
    if frac > 0.0 {
        // Paint the fill as if it spanned the whole track (so its rounded
        // corners follow the track curve), then clip visibility to the
        // vertical strip [left, left+fw]. Intersection-with-strip avoids
        // the small-frac bleed past the track's rounded top/bottom edges.
        let fw = (rect.width() * frac).min(rect.width());
        let strip = Rect::from_min_size(rect.min, Vec2::new(fw, rect.height()));
        let clip = ui.painter_at(strip);
        if let Some((c0, c1)) = fill_gradient {
            paint_gradient_pill(&clip, rect, r, c0, c1);
        } else {
            clip.rect_filled(rect, r, fill);
        }
    }

    if animate && frac > 0.0 {
        // Match the CSS reference (design/styles.css `.big-progress .stripe`):
        // 45° stripes, 6px white + 8px gap (perpendicular period 14px),
        // ~18% white, scrolling at ~25 px/s along x. Stripes are
        // rectangularly clipped to the filled strip; any pixels that
        // leak past the rounded caps get masked out below with `bg`.
        let fw = (rect.width() * frac).min(rect.width());
        let strip = Rect::from_min_size(rect.min, Vec2::new(fw, rect.height()));
        let clip = ui.painter_at(strip);
        let angle_rad: f32 = 45.0_f32.to_radians();
        let perp_period = 14.0_f32;
        let stripe_w = 6.0_f32;
        let h = rect.height();
        let cap_extend = 4.0_f32;
        let v_span = h + 2.0 * cap_extend;
        let slope = angle_rad.tan();
        let dx = v_span / slope;
        let h_period = perp_period / angle_rad.cos();
        let offset = (ui.input(|i| i.time) as f32 * 25.0) % h_period;
        // rgba(255,255,255,0.18) per the design CSS.
        let stripe_color = Color32::from_white_alpha(46);
        let mut x = rect.left() - dx + offset;
        while x < rect.right() {
            let p1 = Pos2::new(x, rect.top() - cap_extend);
            let p2 = Pos2::new(x + dx, rect.bottom() + cap_extend);
            clip.line_segment([p1, p2], Stroke::new(stripe_w, stripe_color));
            x += h_period;
        }
        // Mask any stripe pixels that leaked past the rounded caps by
        // painting an *outside* stroke of `bg` around the bar — fills
        // the band between the bar's rounded curve and its bbox in
        // parent-bg colour, blending invisibly with the surrounding ui.
        painter.rect_stroke(rect, r, Stroke::new(r + 1.0, bg), StrokeKind::Outside);
        let minimized = ui.input(|i| i.viewport().minimized.unwrap_or(false));
        if !minimized {
            ui.ctx().request_repaint_after(Duration::from_millis(33));
        }
    }
}

/// Paint a pill (rounded-rect with rounding = half height) filled with
/// a horizontal linear gradient from `c_left` to `c_right`. Built as a
/// row of solid-color vertical slices using egui's `rect_filled` so
/// each slice gets proper AA — only the leftmost slice rounds its left
/// edge, only the rightmost rounds its right edge.
fn paint_gradient_pill(
    painter: &egui::Painter,
    rect: Rect,
    r: f32,
    c_left: Color32,
    c_right: Color32,
) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let r_used = r.min(rect.width() * 0.5).max(0.0).round() as u8;
    // Each slice must be >= 2*r wide so end-cap rounding isn't clamped
    // flat by egui. If the rect itself is too narrow for even two such
    // slices, fall back to a single rounded pill with the midpoint colour.
    let min_slice_w = (r * 2.0).max(1.0);
    let max_n = (rect.width() / min_slice_w).floor() as usize;
    if max_n < 2 {
        let mix =
            |a: u8, b: u8| -> u8 { ((a as f32 + b as f32) * 0.5).round().clamp(0.0, 255.0) as u8 };
        let color = Color32::from_rgba_unmultiplied(
            mix(c_left.r(), c_right.r()),
            mix(c_left.g(), c_right.g()),
            mix(c_left.b(), c_right.b()),
            mix(c_left.a(), c_right.a()),
        );
        painter.rect_filled(rect, r_used, color);
        return;
    }
    let n = max_n.min(24);
    let total_w = rect.width();
    for i in 0..n {
        let t0 = i as f32 / n as f32;
        let t1 = (i + 1) as f32 / n as f32;
        let t_mid = (t0 + t1) * 0.5;
        let mix = |a: u8, b: u8| -> u8 {
            (a as f32 + (b as f32 - a as f32) * t_mid)
                .round()
                .clamp(0.0, 255.0) as u8
        };
        let color = Color32::from_rgba_unmultiplied(
            mix(c_left.r(), c_right.r()),
            mix(c_left.g(), c_right.g()),
            mix(c_left.b(), c_right.b()),
            mix(c_left.a(), c_right.a()),
        );
        // Adjacent slices overlap by 0.5 px to suppress hairline seams
        // from AA pixel rounding between solid rects.
        let x_a = rect.left() + total_w * t0;
        let x_b = rect.left() + total_w * t1;
        let slice_left = if i == 0 { x_a } else { x_a - 0.5 };
        let slice_right = if i + 1 == n { x_b } else { x_b + 0.5 };
        let slice = Rect::from_min_max(
            Pos2::new(slice_left, rect.top()),
            Pos2::new(slice_right, rect.bottom()),
        );
        let rounding = egui::CornerRadius {
            nw: if i == 0 { r_used } else { 0 },
            sw: if i == 0 { r_used } else { 0 },
            ne: if i + 1 == n { r_used } else { 0 },
            se: if i + 1 == n { r_used } else { 0 },
        };
        painter.rect_filled(slice, rounding, color);
    }
}
