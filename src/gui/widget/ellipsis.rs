//! Text that ends in an ellipsis when it does not fit.
//!
//! iced's `text` has no `text-overflow`: with `Wrapping::None` inside a
//! clipping container the glyphs are simply sliced mid-stroke, which is
//! what the download table's Name column used to do. Truncation needs
//! the shaped width of a candidate string, and only `layout` has the
//! renderer to measure with — hence a widget rather than a helper that
//! trims the `String` up front.
//!
//! Two shapes, one measurement: [`ellipsized`] fits one line to a width,
//! and [`ellipsized_lines`] fills a fixed number of lines, breaking
//! between glyphs, before ellipsising the last one.

use iced::advanced::text::{Paragraph as _, Renderer as TextRenderer};
use iced::advanced::{Layout, Widget, layout, mouse, renderer, widget};
use iced::{Color, Element, Font, Length, Pixels, Rectangle, Size};

const ELLIPSIS: &str = "\u{2026}";

/// A chip's text is centred over the full width it was fitted to; a
/// single line reports its own width and is placed by its parent.
const CHIP_LINE_HEIGHT: f32 = 1.1;

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
        max_lines: 1,
    })
}

/// Text that fills up to `max_lines` lines at the width its parent
/// allows and ellipsises whatever is left over.
///
/// Breaks between glyphs rather than between words: this is for labels
/// with no spaces to break at (a file extension), where wrapping at word
/// boundaries means not wrapping at all and the glyphs run out past the
/// box they were meant to sit in. Lines are centred on each other, so
/// the parent only has to centre the block.
pub fn ellipsized_lines<'a, M: 'a>(
    content: impl Into<String>,
    font: Font,
    size: f32,
    color: Color,
    max_lines: u16,
) -> Element<'a, M> {
    Element::new(Ellipsized {
        content: content.into(),
        font,
        size,
        color,
        max_lines: max_lines.max(1),
    })
}

struct Ellipsized {
    content: String,
    font: Font,
    size: f32,
    color: Color,
    /// `1` keeps the original single-line behaviour: fit by width, and
    /// take only as much width as the result needs. Above that the fit
    /// is by height instead, and the widget claims the whole width so
    /// the centred lines land where the parent expects them.
    max_lines: u16,
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
    fn wraps(&self) -> bool {
        self.max_lines > 1
    }

    fn text<'a>(&self, content: &'a str, width: f32) -> iced::advanced::text::Text<&'a str, Font> {
        iced::advanced::text::Text {
            content,
            // A wrapping paragraph has to be measured inside the width
            // it will wrap at; a single line is measured unbounded and
            // compared against that width afterwards.
            bounds: if self.wraps() {
                Size::new(width, f32::INFINITY)
            } else {
                Size::INFINITE
            },
            size: Pixels(self.size),
            line_height: if self.wraps() {
                iced::advanced::text::LineHeight::Relative(CHIP_LINE_HEIGHT)
            } else {
                iced::advanced::text::LineHeight::default()
            },
            font: self.font,
            align_x: if self.wraps() {
                iced::advanced::text::Alignment::Center
            } else {
                iced::advanced::text::Alignment::Left
            },
            align_y: iced::alignment::Vertical::Top,
            shaping: iced::advanced::text::Shaping::Advanced,
            wrapping: if self.wraps() {
                iced::advanced::text::Wrapping::Glyph
            } else {
                iced::advanced::text::Wrapping::None
            },
        }
    }

    /// Does this paragraph sit inside the space it was given? Width for
    /// one line, height for several: a wrapped paragraph is always as
    /// wide as it was allowed to be, so only the line count says whether
    /// it fits.
    fn fits<P: iced::advanced::text::Paragraph<Font = Font>>(&self, p: &P, max: f32) -> bool {
        if self.wraps() {
            // Half a pixel of slack: the measured height of N lines is
            // a rounded accumulation, not exactly N × line height.
            p.min_bounds().height <= f32::from(self.max_lines) * self.size * CHIP_LINE_HEIGHT + 0.5
        } else {
            p.min_bounds().width <= max
        }
    }

    /// Longest prefix of `content` that fits `max` once the ellipsis is
    /// appended. Binary search over character counts — byte slicing
    /// would split multi-byte characters.
    fn truncate<P: iced::advanced::text::Paragraph<Font = Font>>(&self, max: f32) -> P {
        let full = P::with_text(self.text(&self.content, max));
        if self.fits(&full, max) {
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
        let mut best = P::with_text(self.text(ELLIPSIS, max));
        while lo < hi {
            let mid = lo + (hi - lo).div_ceil(2);
            let candidate = format!("{}{ELLIPSIS}", &self.content[..chars[mid]]);
            let paragraph = P::with_text(self.text(&candidate, max));
            if self.fits(&paragraph, max) {
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
        // The wrapped paragraph centres its lines inside the width it
        // was laid out at, so the node has to be that width for the
        // glyphs to land where the centring put them. A single line is
        // placed by whatever centres it, and claims only what it needs.
        layout::Node::new(limits.resolve(
            if self.wraps() {
                Length::Fill
            } else {
                Length::Shrink
            },
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
        // A paragraph is drawn from the origin it is handed; its own
        // `align_x` only arranges its lines against each other, so
        // centring the block inside the node is this widget's job. The
        // single-line node is exactly as wide as its text, which makes
        // this a no-op there.
        let mut origin = bounds.position();
        if self.wraps() {
            origin.x += ((bounds.width - state.paragraph.min_bounds().width) / 2.0).max(0.0);
        }
        renderer.fill_paragraph(
            &state.paragraph,
            origin,
            self.color,
            bounds.intersection(viewport).unwrap_or(bounds),
        );
    }
}
