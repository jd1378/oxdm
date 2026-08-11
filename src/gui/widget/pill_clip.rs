//! Clipping a shape to a pill, by hand.
//!
//! tiny-skia's geometry backend ignores `with_clip`, so a shape stops
//! where its own geometry stops and nowhere else. Everything a progress
//! bar draws inside its track (the fill, the moving stripes, the
//! strike-through) therefore has to arrive already cut to the track's
//! rounded outline, which is what this does.

use iced::widget::canvas;
use iced::{Point, Size};

/// A pill's outline, as a convex polygon fine enough to clip against.
///
/// Sixteen segments per cap put the approximation within `r * 0.005`
/// of the true arc, which at a 4px radius is a fiftieth of a pixel.
pub(crate) fn pill_polygon(size: Size, radius: f32) -> Vec<Point> {
    const SEG: usize = 16;
    let r = radius.min(size.width / 2.0).max(0.0);
    let mut pts = Vec::with_capacity(2 * (SEG + 1));
    let arc = |c: Point, from: f32, to: f32, out: &mut Vec<Point>| {
        for i in 0..=SEG {
            let a = from + (to - from) * (i as f32 / SEG as f32);
            out.push(Point::new(c.x + r * a.cos(), c.y + r * a.sin()));
        }
    };
    let (top, bottom) = (r, size.height - r);
    // Right cap, then left: one closed ring, in one direction.
    arc(
        Point::new(size.width - r, top),
        -std::f32::consts::FRAC_PI_2,
        std::f32::consts::FRAC_PI_2,
        &mut pts,
    );
    if bottom > top {
        pts.push(Point::new(size.width - r, bottom));
    }
    arc(
        Point::new(r, bottom),
        std::f32::consts::FRAC_PI_2,
        std::f32::consts::PI * 1.5,
        &mut pts,
    );
    pts
}

/// Sutherland-Hodgman clip of `poly` against the convex `clip`
/// polygon. Both must be convex; the bands and the pill are.
pub(crate) fn clip_poly_convex(poly: &[Point], clip: &[Point]) -> Vec<Point> {
    // Which side of an edge counts as inside depends on which way the
    // clip polygon is wound, so ask its own area rather than assuming.
    let area: f32 = (0..clip.len())
        .map(|i| {
            let (a, b) = (clip[i], clip[(i + 1) % clip.len()]);
            a.x * b.y - b.x * a.y
        })
        .sum();
    let orient = if area >= 0.0 { 1.0 } else { -1.0 };
    let mut pts = poly.to_vec();
    for i in 0..clip.len() {
        if pts.len() < 3 {
            return Vec::new();
        }
        let (a, b) = (clip[i], clip[(i + 1) % clip.len()]);
        let side = |p: &Point| orient * ((b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x));
        let mut out = Vec::with_capacity(pts.len() + 2);
        for j in 0..pts.len() {
            let cur = pts[j];
            let next = pts[(j + 1) % pts.len()];
            let (dc, dn) = (side(&cur), side(&next));
            if dc >= 0.0 {
                out.push(cur);
            }
            if (dc >= 0.0) != (dn >= 0.0) {
                let t = dc / (dc - dn);
                out.push(Point::new(
                    cur.x + t * (next.x - cur.x),
                    cur.y + t * (next.y - cur.y),
                ));
            }
        }
        pts = out;
    }
    pts
}

pub(crate) fn poly_path(pts: &[Point]) -> Option<canvas::Path> {
    if pts.len() < 3 {
        return None;
    }
    let mut b = canvas::path::Builder::new();
    b.move_to(pts[0]);
    for p in &pts[1..] {
        b.line_to(*p);
    }
    b.close();
    Some(b.build())
}

/// A band clipped to `x0..x1` *and* to the bar's rounded outline.
pub(crate) fn band_path(
    poly: &[Point],
    x0: f32,
    x1: f32,
    outline: &[Point],
) -> Option<canvas::Path> {
    let pts = clip_poly_x_pts(poly, x0, x1);
    if pts.len() < 3 {
        return None;
    }
    poly_path(&clip_poly_convex(&pts, outline))
}

/// The same x-range clip, handing back the points so a second clip can
/// run on them.
pub(crate) fn clip_poly_x_pts(poly: &[Point], x0: f32, x1: f32) -> Vec<Point> {
    let clip_half = |pts: &[Point], edge: f32, sign: f32| -> Vec<Point> {
        let inside = |p: &Point| (p.x - edge) * sign <= 0.0;
        let mut out = Vec::with_capacity(pts.len() + 2);
        for i in 0..pts.len() {
            let cur = pts[i];
            let next = pts[(i + 1) % pts.len()];
            let (cur_in, next_in) = (inside(&cur), inside(&next));
            if cur_in {
                out.push(cur);
            }
            if cur_in != next_in {
                let t = (edge - cur.x) / (next.x - cur.x);
                out.push(Point::new(edge, cur.y + t * (next.y - cur.y)));
            }
        }
        out
    };
    let pts = clip_half(poly, x1, 1.0);
    if pts.is_empty() {
        return Vec::new();
    }
    clip_half(&pts, x0, -1.0)
}

/// A progress fill: a plain rectangle `x0..x1`, cut to the track's
/// outline.
///
/// Cutting rather than rounding the fill itself is what makes the ends
/// behave. A fill drawn as its own pill has to invent a minimum width
/// (or it is a sliver with two caps and no middle) and has to decide
/// when to round its right edge, which is a jump. A rectangle behind a
/// pill-shaped window just grows: it appears as a hairline of the left
/// cap's curve and it swallows the right cap a column at a time.
pub(crate) fn clipped_fill(rect: iced::Rectangle, outline: &[Point]) -> Option<canvas::Path> {
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return None;
    }
    let corners = [
        Point::new(rect.x, rect.y),
        Point::new(rect.x + rect.width, rect.y),
        Point::new(rect.x + rect.width, rect.y + rect.height),
        Point::new(rect.x, rect.y + rect.height),
    ];
    poly_path(&clip_poly_convex(&corners, outline))
}

/// `pill_polygon`, moved to `origin`.
pub(crate) fn pill_polygon_at(origin: Point, size: Size, radius: f32) -> Vec<Point> {
    pill_polygon(size, radius)
        .into_iter()
        .map(|p| Point::new(p.x + origin.x, p.y + origin.y))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::{Point, Size};

    /// Every point of a clipped band has to be inside the pill, or the
    /// band is what draws the square corner the pill was hiding.
    #[test]
    fn a_band_never_reaches_outside_the_pill() {
        let size = Size::new(200.0, 8.0);
        let r = size.height / 2.0;
        let outline = pill_polygon(size, r);
        // A band lying right across the left cap, the case that leaked.
        let poly = [
            Point::new(-4.0, size.height),
            Point::new(2.0, size.height),
            Point::new(10.0, 0.0),
            Point::new(4.0, 0.0),
        ];
        let pts = clip_poly_convex(&clip_poly_x_pts(&poly, 0.0, 100.0), &outline);
        assert!(pts.len() >= 3, "the band survives the clip");
        for p in &pts {
            // Distance to the pill's spine, which is `r` at most
            // anywhere inside it. A hair of slack for the polygon
            // approximation of the arc.
            let cx = p.x.clamp(r, size.width - r);
            let d = ((p.x - cx).powi(2) + (p.y - r).powi(2)).sqrt();
            assert!(d <= r + 0.05, "{p:?} sits {d} from the spine, r = {r}");
        }
    }

    /// Clipping must not eat the middle of the bar, where there is no
    /// curve to respect.
    #[test]
    fn a_band_in_the_straight_middle_is_left_alone() {
        let size = Size::new(200.0, 8.0);
        let outline = pill_polygon(size, size.height / 2.0);
        let poly = [
            Point::new(100.0, size.height),
            Point::new(106.0, size.height),
            Point::new(114.0, 0.0),
            Point::new(108.0, 0.0),
        ];
        let pts = clip_poly_convex(&poly, &outline);
        assert_eq!(pts.len(), 4, "an untouched band keeps its four corners");
    }

    /// A fill just starting shows the curve of the cap, not a stub of
    /// some minimum width: the old shape drew a whole rounded end at
    /// any fraction above zero.
    #[test]
    fn a_barely_started_fill_is_barely_wide() {
        let size = Size::new(200.0, 8.0);
        let outline = pill_polygon(size, size.height / 2.0);
        let path = clipped_fill(
            iced::Rectangle::new(Point::ORIGIN, Size::new(1.0, size.height)),
            &outline,
        );
        let pts = clip_poly_convex(
            &[
                Point::new(0.0, 0.0),
                Point::new(1.0, 0.0),
                Point::new(1.0, size.height),
                Point::new(0.0, size.height),
            ],
            &outline,
        );
        assert!(path.is_some(), "1px of fill still draws");
        assert!(
            pts.iter().all(|p| p.x <= 1.0 + 0.001),
            "and it stays within its own column"
        );
    }

    /// The last stretch has to arrive column by column rather than
    /// snapping onto the right cap.
    #[test]
    fn the_fill_grows_into_the_right_cap_a_column_at_a_time() {
        let size = Size::new(200.0, 8.0);
        let r = size.height / 2.0;
        let outline = pill_polygon(size, r);
        let width_at = |fw: f32| -> f32 {
            let pts = clip_poly_convex(
                &[
                    Point::new(0.0, 0.0),
                    Point::new(fw, 0.0),
                    Point::new(fw, size.height),
                    Point::new(0.0, size.height),
                ],
                &outline,
            );
            pts.iter().fold(0.0_f32, |m, p| m.max(p.x))
        };
        // Inside the cap the fill keeps reaching further right, in
        // steps its own size, with no jump to the end.
        let a = width_at(size.width - r);
        let b = width_at(size.width - r / 2.0);
        let c = width_at(size.width);
        assert!(a < b && b < c, "{a} < {b} < {c}");
        assert!(c <= size.width + 0.001);
    }
}
