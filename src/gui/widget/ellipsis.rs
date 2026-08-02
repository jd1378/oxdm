//! Single-line text that ends in an ellipsis when it does not fit.
//!
//! iced's `text` has no `text-overflow`: with `Wrapping::None` inside a
//! clipping container the glyphs are simply sliced mid-stroke, which is
//! what the download table's Name column used to do. Truncation needs
//! the shaped width of a candidate string, and only `layout` has the
//! renderer to measure with — hence a widget rather than a helper that
//! trims the `String` up front.

use iced::advanced::text::{Paragraph as _, Renderer as TextRenderer};
use iced::advanced::{Layout, Widget, layout, mouse, renderer, widget};
use iced::{Color, Element, Font, Length, Pixels, Rectangle, Size};

const ELLIPSIS: &str = "\u{2026}";

/// Text that truncates to `…` at the width its parent allows.
pub fn ellipsized<'a, M: 'a>(
    content: impl Into<String>,
    font: Font,
    size: f32,
    color: Color,
) -> Element<'a, M> {
    Element::new(Ellipsized {
        content: content.into(),
        font,
        size,
        color,
    })
}

struct Ellipsized {
    content: String,
    font: Font,
    size: f32,
    color: Color,
}

/// Laid-out paragraph plus the width it was fitted to, so a re-layout at
/// an unchanged width can reuse it (column drags re-lay out every row on
/// every frame).
struct State<P> {
    paragraph: P,
    fitted_to: f32,
    /// The *source* string the paragraph was built from — the cache key
    /// alongside the width, not the truncated result.
    source: String,
}

impl<P: Default> Default for State<P> {
    fn default() -> Self {
        Self {
            paragraph: P::default(),
            fitted_to: f32::NAN,
            source: String::new(),
        }
    }
}

impl Ellipsized {
    fn text<'a>(&self, content: &'a str) -> iced::advanced::text::Text<&'a str, Font> {
        iced::advanced::text::Text {
            content,
            bounds: Size::INFINITE,
            size: Pixels(self.size),
            line_height: iced::advanced::text::LineHeight::default(),
            font: self.font,
            align_x: iced::advanced::text::Alignment::Left,
            align_y: iced::alignment::Vertical::Top,
            shaping: iced::advanced::text::Shaping::Advanced,
            wrapping: iced::advanced::text::Wrapping::None,
        }
    }

    /// Longest prefix of `content` that fits `max` once the ellipsis is
    /// appended. Binary search over character counts — byte slicing
    /// would split multi-byte characters.
    fn truncate<P: iced::advanced::text::Paragraph<Font = Font>>(&self, max: f32) -> P {
        let full = P::with_text(self.text(&self.content));
        if full.min_bounds().width <= max {
            return full;
        }

        let chars: Vec<usize> = self
            .content
            .char_indices()
            .map(|(i, _)| i)
            .chain(std::iter::once(self.content.len()))
            .collect();

        // `lo` always fits, `hi` never does; converge on the boundary.
        let (mut lo, mut hi) = (0usize, chars.len() - 1);
        let mut best = P::with_text(self.text(ELLIPSIS));
        while lo < hi {
            let mid = lo + (hi - lo).div_ceil(2);
            let candidate = format!("{}{ELLIPSIS}", &self.content[..chars[mid]]);
            let paragraph = P::with_text(self.text(&candidate));
            if paragraph.min_bounds().width <= max {
                best = paragraph;
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        best
    }
}

impl<M, R> Widget<M, iced::Theme, R> for Ellipsized
where
    R: TextRenderer<Font = Font>,
{
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<State<R::Paragraph>>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(State::<R::Paragraph>::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        _renderer: &R,
        limits: &layout::Limits,
    ) -> layout::Node {
        let state = tree.state.downcast_mut::<State<R::Paragraph>>();
        let max = limits.max().width;
        if state.fitted_to != max || state.source != self.content {
            state.paragraph = self.truncate::<R::Paragraph>(max);
            state.fitted_to = max;
            state.source.clone_from(&self.content);
        }
        let bounds = state.paragraph.min_bounds();
        layout::Node::new(limits.resolve(
            Length::Shrink,
            Length::Shrink,
            Size::new(bounds.width, bounds.height),
        ))
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut R,
        _theme: &iced::Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State<R::Paragraph>>();
        let bounds = layout.bounds();
        renderer.fill_paragraph(
            &state.paragraph,
            bounds.position(),
            self.color,
            bounds.intersection(viewport).unwrap_or(bounds),
        );
    }
}
