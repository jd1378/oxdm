//! Dashed rounded-rectangle outline.
//!
//! `iced::Border` strokes solid only, so the design's "nothing here
//! yet, drop something in" outline is drawn on a canvas: a rounded
//! rect stroked with a dash pattern, sized to whatever it is laid over.

use iced::widget::canvas;
use iced::{Color, Element, Length, Point, Rectangle, Size};

/// An outline that fills its parent, for stacking behind a card's
/// content in place of a solid border.
///
/// `dash` is the nominal length of one dash in pixels; the gap follows
/// it in proportion, and both are stretched so a whole number of them
/// goes round (see `draw`). Shorter dashes read as finer stippling —
/// a hint rather than a fence.
pub fn dashed_frame<'a, M: 'a>(color: Color, radius: f32, dash: f32) -> Element<'a, M> {
    canvas(Dashed {
        color,
        radius,
        dash,
    })
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

struct Dashed {
    color: Color,
    radius: f32,
    dash: f32,
}

impl<M> canvas::Program<M> for Dashed {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        const WIDTH: f32 = 1.0;
        // The gap keeps this ratio to the dash whatever length the
        // caller asks for, so the outline reads the same at any scale.
        const GAP_RATIO: f32 = 0.8;
        let dash = self.dash.max(1.0);
        let gap = dash * GAP_RATIO;

        let mut frame = canvas::Frame::new(renderer, bounds.size());
        // Inset by half the stroke so the line lands inside the bounds
        // instead of straddling them and losing half its width.
        let inset = WIDTH / 2.0;
        let size = Size::new(
            (bounds.width - WIDTH).max(0.0),
            (bounds.height - WIDTH).max(0.0),
        );
        let path =
            canvas::Path::rounded_rectangle(Point::new(inset, inset), size, self.radius.into());

        // Stretch the pattern so a whole number of dash+gap periods
        // fits the outline exactly. Left at its nominal length, the
        // last period is cut wherever the path happens to end — one
        // gap wider or narrower than every other, and always in the
        // same corner, which is exactly where the eye goes.
        let r = self.radius.min(size.width / 2.0).min(size.height / 2.0);
        let perimeter = 2.0 * (size.width - 2.0 * r).max(0.0)
            + 2.0 * (size.height - 2.0 * r).max(0.0)
            + 2.0 * std::f32::consts::PI * r;
        let period = dash + gap;
        let periods = (perimeter / period).round().max(1.0);
        let scale = perimeter / (periods * period);
        let segments = [dash * scale, gap * scale];

        frame.stroke(
            &path,
            canvas::Stroke {
                style: canvas::Style::Solid(self.color),
                width: WIDTH,
                line_dash: canvas::LineDash {
                    segments: &segments,
                    offset: 0,
                },
                ..Default::default()
            },
        );
        vec![frame.into_geometry()]
    }
}

/// A 1px dashed horizontal rule, full width.
///
/// Separates rows that sit directly on the page instead of inside a
/// bordered card: with no surface to divide, a solid hairline reads as
/// the edge of something, while a dashed one reads as a break between
/// two things on the same surface.
pub fn dashed_rule<'a, M: 'a>(color: Color) -> Element<'a, M> {
    canvas(Rule { color })
        .width(Length::Fill)
        .height(Length::Fixed(1.0))
        .into()
}

struct Rule {
    color: Color,
}

impl<M> canvas::Program<M> for Rule {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        // The same stipple the empty-card outline uses, so the two read
        // as one idea at two sizes.
        const DASH: f32 = 2.0;
        const GAP: f32 = 3.0;
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let mut path = canvas::path::Builder::new();
        // Half a pixel down: a 1px line drawn on the boundary straddles
        // two rows of pixels and renders as a 2px smudge.
        path.move_to(Point::new(0.0, 0.5));
        path.line_to(Point::new(bounds.width, 0.5));
        frame.stroke(
            &path.build(),
            canvas::Stroke {
                style: canvas::Style::Solid(self.color),
                width: 1.0,
                line_dash: canvas::LineDash {
                    segments: &[DASH, GAP],
                    offset: 0,
                },
                ..Default::default()
            },
        );
        vec![frame.into_geometry()]
    }
}
