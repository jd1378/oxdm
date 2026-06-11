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
    canvas(Striped {
        frac: frac.clamp(0.0, 1.0),
        track,
        fill,
        fill_gradient,
        animate,
        time_s,
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
                    for i in 0..n {
                        let t = i as f32 / (n - 1) as f32;
                        let x = i as f32 * slice_w;
                        let (x0, x1) = (x.max(radius), (x + slice_w + 0.5).min(fw - 0.0));
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

        // Animated stripes: 45° bands intersected with the fill rect
        // (manual clip — see note above).
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
                if let Some(path) = clip_poly_x(&poly, 0.0, fw) {
                    frame.fill(&path, stripe);
                }
                x += h_period;
            }
        }

        vec![frame.into_geometry()]
    }
}

/// Sutherland-Hodgman clip of a convex polygon against
/// `x0 <= x <= x1`. Returns `None` when fully outside.
fn clip_poly_x(poly: &[Point], x0: f32, x1: f32) -> Option<canvas::Path> {
    let clip_half = |pts: &[Point], keep_left_of: f32, sign: f32| -> Vec<Point> {
        let inside = |p: &Point| (p.x - keep_left_of) * sign <= 0.0;
        let mut out = Vec::with_capacity(pts.len() + 2);
        for i in 0..pts.len() {
            let cur = pts[i];
            let next = pts[(i + 1) % pts.len()];
            let cur_in = inside(&cur);
            let next_in = inside(&next);
            if cur_in {
                out.push(cur);
            }
            if cur_in != next_in {
                let t = (keep_left_of - cur.x) / (next.x - cur.x);
                out.push(Point::new(keep_left_of, cur.y + t * (next.y - cur.y)));
            }
        }
        out
    };
    let pts = clip_half(poly, x1, 1.0);
    if pts.is_empty() {
        return None;
    }
    let pts = clip_half(&pts, x0, -1.0);
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

/// Transfer-rate chart: dotted gridlines, avg dashed line, polyline +
/// translucent area fill.
pub struct RateChart {
    pub samples: Vec<f32>,
    pub max: f32,
    pub avg: f32,
    pub accent: Color,
    pub grid: Color,
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

        // Dotted gridlines at 0 / 33 / 67 / 100%.
        for fraq in [0.0_f32, 0.33, 0.67, 1.0] {
            let y = size.height * (1.0 - fraq);
            let mut x = 0.0;
            while x < size.width {
                let path = canvas::Path::rectangle(Point::new(x, y), Size::new(1.8, 1.2));
                frame.fill(&path, self.grid);
                x += 1.8 + 4.5;
            }
        }

        // Average dashed line.
        if self.avg > 0.0 {
            let y = size.height * (1.0 - (self.avg / max).clamp(0.0, 1.0));
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
                    size.height * (1.0 - (self.samples[i] / max).clamp(0.0, 1.0)),
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
