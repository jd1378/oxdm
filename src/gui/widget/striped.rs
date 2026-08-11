//! Animated striped progress bar (download window hero bar) and the
//! transfer-rate chart canvas.

use iced::widget::canvas;
use iced::{Color, Element, Length, Point, Rectangle, Size};

use crate::gui::color::{mix, with_alpha};

/// Pill progress with optional horizontal gradient fill and animated
/// 45° stripes while `animate` (offset driven by `time_s`).
#[allow(clippy::too_many_arguments)]
pub fn striped_progress<'a, M: 'a>(
    frac: f32,
    width: Length,
    height: f32,
    track: Color,
    fill: Color,
    fill_gradient: Option<(Color, Color)>,
    animate: bool,
    time_s: f32,
) -> Element<'a, M> {
    striped_progress_hatched(
        frac,
        width,
        height,
        track,
        fill,
        fill_gradient,
        animate,
        time_s,
        None,
    )
}

/// Same bar, struck through with static diagonal bands across its whole
/// width (design `.big-progress.is-will-restart .bp-strike`). Says the
/// progress underneath is not going to be used — the download has to
/// start over.
#[allow(clippy::too_many_arguments)]
pub fn striped_progress_hatched<'a, M: 'a>(
    frac: f32,
    width: Length,
    height: f32,
    track: Color,
    fill: Color,
    fill_gradient: Option<(Color, Color)>,
    animate: bool,
    time_s: f32,
    hatch: Option<Color>,
) -> Element<'a, M> {
    canvas(Striped {
        frac: frac.clamp(0.0, 1.0),
        track,
        fill,
        fill_gradient,
        animate,
        time_s,
        hatch,
    })
    .width(width)
    .height(Length::Fixed(height))
    .into()
}

struct Striped {
    frac: f32,
    track: Color,
    fill: Color,
    fill_gradient: Option<(Color, Color)>,
    animate: bool,
    time_s: f32,
    /// Static strike-through bands over the entire bar, fill included.
    hatch: Option<Color>,
}

impl<M> canvas::Program<M> for Striped {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let size = bounds.size();
        let radius = size.height / 2.0;

        let track_path = canvas::Path::rounded_rectangle(Point::ORIGIN, size, radius.into());
        frame.fill(&track_path, self.track);

        if self.frac <= 0.0 {
            if let Some(color) = self.hatch {
                hatch_bands(&mut frame, size, color);
            }
            return vec![frame.into_geometry()];
        }
        let fw = size.width * self.frac;

        // tiny-skia's geometry backend ignores `with_clip`, so all
        // shapes are constructed pre-clipped.
        match self.fill_gradient {
            None => {
                let path = super::pills::fill_path(size, self.frac, 0.0);
                frame.fill(&path, self.fill);
            }
            Some((left, right)) => {
                let n = ((fw / 8.0) as usize).clamp(2, 24);
                let slice_w = fw / n as f32;
                if slice_w < 2.0 {
                    let path = super::pills::fill_path(size, self.frac, 0.0);
                    frame.fill(&path, mix(left, right, 0.5));
                } else {
                    // Rectangular slices, then both end caps are
                    // re-fixed with single-color rounded slices.
                    let radius = size.height / 2.0;
                    // Where the square slices have to stop. On the left
                    // that is the cap; on the right it is the cap only
                    // once the fill has reached it, since a fill that
                    // ends mid-track ends square by design. Running a
                    // slice under the cap leaves its corners outside the
                    // pill: the cap paints its own rounded shape and
                    // cannot erase what is drawn beyond it.
                    let right_limit = if fw >= size.width - radius {
                        size.width - radius
                    } else {
                        fw
                    };
                    for i in 0..n {
                        let t = i as f32 / (n - 1) as f32;
                        let x = i as f32 * slice_w;
                        let (x0, x1) = (x.max(radius), (x + slice_w + 0.5).min(right_limit));
                        if x1 <= x0 {
                            continue;
                        }
                        let path = canvas::Path::rectangle(
                            Point::new(x0, 0.0),
                            Size::new(x1 - x0, size.height),
                        );
                        frame.fill(&path, mix(left, right, t));
                    }
                    // Left cap.
                    let cap = canvas::Path::rounded_rectangle(
                        Point::ORIGIN,
                        Size::new(2.0 * radius, size.height),
                        iced::border::Radius {
                            top_left: radius,
                            bottom_left: radius,
                            top_right: 0.0,
                            bottom_right: 0.0,
                        },
                    );
                    frame.fill(&cap, left);
                    // Right cap when the bar reaches the track end.
                    if fw >= size.width - radius {
                        let cap = canvas::Path::rounded_rectangle(
                            Point::new(size.width - 2.0 * radius, 0.0),
                            Size::new(2.0 * radius, size.height),
                            iced::border::Radius {
                                top_left: 0.0,
                                bottom_left: 0.0,
                                top_right: radius,
                                bottom_right: radius,
                            },
                        );
                        frame.fill(&cap, right);
                    }
                }
            }
        }

        // Animated stripes: 45° bands intersected with the filled part
        // of the bar *and* with the bar's own pill outline, so a band
        // stops at the rounded end instead of squaring it off (manual
        // clip — see note above).
        let outline = pill_polygon(size, radius);
        if self.animate {
            let angle = 45.0_f32.to_radians();
            let perp_period = 14.0;
            let h_period = perp_period / angle.cos();
            let band_w = 6.0 / angle.cos();
            let offset = (self.time_s * 25.0) % h_period;
            let stripe = with_alpha(Color::WHITE, 46.0 / 255.0);
            let h = size.height;
            let mut x = -h - h_period + offset;
            while x < fw + h {
                // Parallelogram: bottom edge [x, x+band_w], top edge
                // shifted right by h (45° going up-right).
                let poly = [
                    Point::new(x, h),
                    Point::new(x + band_w, h),
                    Point::new(x + band_w + h, 0.0),
                    Point::new(x + h, 0.0),
                ];
                if let Some(path) = band_path(&poly, 0.0, fw, &outline) {
                    frame.fill(&path, stripe);
                }
                x += h_period;
            }
        }

        // Strike-through last so it reads over the fill (design
        // `.bp-strike` sits above `.fill`), and across the whole track:
        // the part still to download is being discarded too.
        if let Some(color) = self.hatch {
            hatch_bands(&mut frame, size, color);
        }

        vec![frame.into_geometry()]
    }
}

/// Design `.bp-strike`: 1px bands every 11px at -45°, drawn over the
/// full width of the bar.
fn hatch_bands(frame: &mut canvas::Frame, size: Size, color: Color) {
    let outline = pill_polygon(size, size.height / 2.0);
    const PERIOD: f32 = 11.0;
    const BAND: f32 = 1.5;
    let h = size.height;
    let angle = 45.0_f32.to_radians();
    let h_period = PERIOD / angle.cos();
    let band_w = BAND / angle.cos();
    let mut x = -h;
    while x < size.width + h {
        // Mirror of the animated stripes, sloping the other way: top
        // edge shifted *left* by h instead of right.
        let poly = [
            Point::new(x, h),
            Point::new(x + band_w, h),
            Point::new(x + band_w - h, 0.0),
            Point::new(x - h, 0.0),
        ];
        if let Some(path) = band_path(&poly, 0.0, size.width, &outline) {
            frame.fill(&path, color);
        }
        x += h_period;
    }
}

/// The bar's own outline, as a convex polygon fine enough to clip
/// against.
///
/// The stripes are diagonal bands over a pill, and tiny-skia's geometry
/// backend ignores `with_clip`, so the only way a band stops at the
/// rounded end is for its *shape* to stop there. Eight segments per cap
/// put the approximation within `r * 0.02` of the true arc, which at a
/// 4px radius is under a tenth of a pixel.
fn pill_polygon(size: Size, radius: f32) -> Vec<Point> {
    const SEG: usize = 8;
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
fn clip_poly_convex(poly: &[Point], clip: &[Point]) -> Vec<Point> {
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

fn poly_path(pts: &[Point]) -> Option<canvas::Path> {
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
fn band_path(poly: &[Point], x0: f32, x1: f32, outline: &[Point]) -> Option<canvas::Path> {
    let pts = clip_poly_x_pts(poly, x0, x1);
    if pts.len() < 3 {
        return None;
    }
    poly_path(&clip_poly_convex(&pts, outline))
}

/// The same x-range clip, handing back the points so a second clip can
/// run on them.
fn clip_poly_x_pts(poly: &[Point], x0: f32, x1: f32) -> Vec<Point> {
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

/// Transfer-rate chart: dotted gridlines, avg dashed line, polyline +
/// translucent area fill.
pub struct RateChart {
    pub samples: Vec<f32>,
    pub max: f32,
    pub avg: f32,
    pub accent: Color,
    pub grid: Color,
    pub label_color: Color,
}

pub fn rate_chart<'a, M: 'a>(chart: RateChart, height: f32) -> Element<'a, M> {
    canvas(chart)
        .width(Length::Fill)
        .height(Length::Fixed(height))
        .into()
}

impl<M> canvas::Program<M> for RateChart {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let size = bounds.size();
        let max = self.max.max(1.0);
        // Inset the plot's top so the top gridline's label fits inside
        // the canvas (egui padded the plot rect the same way).
        let top_inset = 12.0;
        let plot_y = |fraq: f32| top_inset + (size.height - top_inset) * (1.0 - fraq);

        // Dotted gridlines at 0 / 33 / 67 / 100% with speed labels
        // above each line (egui parity: mono 10, fg_3; "0 B/s" pinned
        // at the bottom, the rest only once data sets the scale).
        let has_data = !self.samples.is_empty();
        for fraq in [0.0_f32, 0.33, 0.67, 1.0] {
            let y = plot_y(fraq);
            let mut x = 0.0;
            while x < size.width {
                let path = canvas::Path::rectangle(Point::new(x, y), Size::new(1.8, 1.2));
                frame.fill(&path, self.grid);
                x += 1.8 + 4.5;
            }
            if fraq == 0.0 || has_data {
                let label = if fraq == 0.0 {
                    "0 B/s".to_owned()
                } else {
                    crate::gui::format::format_speed((max * fraq) as f64)
                };
                frame.fill_text(canvas::Text {
                    content: label,
                    position: Point::new(0.0, y - 2.0),
                    color: self.label_color,
                    size: 10.0.into(),
                    font: crate::gui::theme::MONO,
                    align_y: iced::alignment::Vertical::Bottom,
                    ..canvas::Text::default()
                });
            }
        }

        // Average dashed line.
        if self.avg > 0.0 {
            let y = plot_y((self.avg / max).clamp(0.0, 1.0));
            let mut x = 0.0;
            while x < size.width {
                let path = canvas::Path::rectangle(Point::new(x, y), Size::new(6.0, 1.2));
                frame.fill(&path, with_alpha(self.accent, 0.65));
                x += 6.0 + 12.0;
            }
        }

        if self.samples.len() >= 2 {
            let step = size.width / (self.samples.len() - 1) as f32;
            let pt = |i: usize| {
                Point::new(
                    i as f32 * step,
                    plot_y((self.samples[i] / max).clamp(0.0, 1.0)),
                )
            };
            // Area fill.
            let mut area = canvas::path::Builder::new();
            area.move_to(Point::new(0.0, size.height));
            for i in 0..self.samples.len() {
                area.line_to(pt(i));
            }
            area.line_to(Point::new(size.width, size.height));
            area.close();
            frame.fill(&area.build(), with_alpha(self.accent, 36.0 / 255.0));
            // Polyline.
            let mut line = canvas::path::Builder::new();
            line.move_to(pt(0));
            for i in 1..self.samples.len() {
                line.line_to(pt(i));
            }
            frame.stroke(
                &line.build(),
                canvas::Stroke::default()
                    .with_color(self.accent)
                    .with_width(2.0),
            );
        }

        vec![frame.into_geometry()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
