//! Animated striped progress bar (download window hero bar) and the
//! transfer-rate chart canvas.

use iced::widget::canvas;
use iced::{Color, Element, Length, Point, Rectangle, Size};

use super::pill_clip::{band_path, clipped_fill, pill_polygon};

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

/// A full-height column of the bar, `x0` to `x1`.
fn rect(x0: f32, x1: f32, size: Size) -> iced::Rectangle {
    iced::Rectangle::new(Point::new(x0, 0.0), Size::new(x1 - x0, size.height))
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

        // Every shape inside the track arrives pre-cut to the
        // track's outline: tiny-skia ignores `with_clip`.
        let outline = pill_polygon(size, radius);
        match self.fill_gradient {
            None => {
                if let Some(path) = clipped_fill(rect(0.0, fw, size), &outline) {
                    frame.fill(&path, self.fill);
                }
            }
            Some((left, right)) => {
                let n = ((fw / 8.0) as usize).clamp(2, 24);
                let slice_w = fw / n as f32;
                if slice_w < 2.0 {
                    if let Some(path) = clipped_fill(rect(0.0, fw, size), &outline) {
                        frame.fill(&path, mix(left, right, 0.5));
                    }
                } else {
                    // Slices of flat colour, each cut to the outline,
                    // so the caps need no special case: the first and
                    // last simply come out round.
                    for i in 0..n {
                        let t = i as f32 / (n - 1) as f32;
                        let x0 = i as f32 * slice_w;
                        let x1 = (x0 + slice_w + 0.5).min(fw);
                        if let Some(path) = clipped_fill(rect(x0, x1, size), &outline) {
                            frame.fill(&path, mix(left, right, t));
                        }
                    }
                }
            }
        }

        // Animated stripes: 45° bands intersected with the filled part
        // of the bar *and* with the bar's own pill outline, so a band
        // stops at the rounded end instead of squaring it off (manual
        // clip — see note above).
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
