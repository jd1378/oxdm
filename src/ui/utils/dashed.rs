//! Dashed-border painter for rounded rects.
//!
//! Builds the perimeter as a closed polyline (straight edges + sampled
//! corner arcs), then walks arc-length emitting `dash` / `gap` runs.
//! Mirrors SVG `stroke-dasharray` semantics so corners curve with the
//! dashes instead of getting cut off at the arcs.

use eframe::egui::{self, Pos2, Rect, Stroke};

/// Paint a dashed outline around `rect` with the given corner radius.
///
/// `dash` is the on-segment length, `gap` the off-length (both in
/// points). The polyline is inset by half the stroke width so the
/// dashes sit fully inside the framed area — keeps corners crisp on
/// the pixel grid instead of straddling integer boundaries.
pub fn paint_dashed_rect(
    painter: &egui::Painter,
    rect: Rect,
    radius: f32,
    stroke: Stroke,
    dash: f32,
    gap: f32,
) {
    use std::f32::consts::PI;
    const ARC_STEPS: usize = 24;
    let inset = stroke.width * 0.5;
    let rect = rect.shrink(inset);
    let r = (radius - inset)
        .min(rect.width() * 0.5)
        .min(rect.height() * 0.5)
        .max(0.0);

    let mut path: Vec<Pos2> = Vec::with_capacity(4 + 4 * (ARC_STEPS + 1));
    let arc = |center: Pos2, start: f32, end: f32, out: &mut Vec<Pos2>| {
        for i in 0..=ARC_STEPS {
            let t = i as f32 / ARC_STEPS as f32;
            let a = start + (end - start) * t;
            out.push(Pos2::new(center.x + r * a.cos(), center.y + r * a.sin()));
        }
    };
    path.push(Pos2::new(rect.left() + r, rect.top()));
    path.push(Pos2::new(rect.right() - r, rect.top()));
    arc(
        Pos2::new(rect.right() - r, rect.top() + r),
        -PI * 0.5,
        0.0,
        &mut path,
    );
    path.push(Pos2::new(rect.right(), rect.bottom() - r));
    arc(
        Pos2::new(rect.right() - r, rect.bottom() - r),
        0.0,
        PI * 0.5,
        &mut path,
    );
    path.push(Pos2::new(rect.left() + r, rect.bottom()));
    arc(
        Pos2::new(rect.left() + r, rect.bottom() - r),
        PI * 0.5,
        PI,
        &mut path,
    );
    path.push(Pos2::new(rect.left(), rect.top() + r));
    arc(
        Pos2::new(rect.left() + r, rect.top() + r),
        PI,
        PI * 1.5,
        &mut path,
    );
    if let Some(&first) = path.first() {
        path.push(first);
    }

    // Walk the perimeter accumulating one polyline per dash so each
    // dash renders as a single anti-aliased path (instead of N stacked
    // segments per arc step — which produce blurry overlaps).
    let mut remaining = dash;
    let mut drawing = true;
    let mut buf: Vec<Pos2> = Vec::new();
    if drawing {
        buf.push(path[0]);
    }
    let flush = |buf: &mut Vec<Pos2>| {
        if buf.len() >= 2 {
            painter.add(egui::Shape::line(std::mem::take(buf), stroke));
        } else {
            buf.clear();
        }
    };
    for w in path.windows(2) {
        let mut a = w[0];
        let b = w[1];
        let mut seg_len = (b - a).length();
        if seg_len <= f32::EPSILON {
            continue;
        }
        let dir = (b - a) / seg_len;
        while seg_len > 0.0 {
            let take = seg_len.min(remaining);
            let end = a + dir * take;
            if drawing {
                buf.push(end);
            }
            a = end;
            seg_len -= take;
            remaining -= take;
            if remaining <= f32::EPSILON {
                if drawing {
                    flush(&mut buf);
                }
                drawing = !drawing;
                remaining = if drawing { dash } else { gap };
                if drawing {
                    buf.push(a);
                }
            }
        }
    }
    if drawing {
        flush(&mut buf);
    }
}
