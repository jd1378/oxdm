//! Small status/label primitives: count pill, status dot, pill-shaped
//! progress, inline (table cell) progress, eyebrow + field labels.

use iced::widget::{canvas, container, row, text};
use iced::{Alignment, Border, Color, Element, Length, Point, Rectangle, Size};

use crate::gui::color::with_alpha;
use crate::gui::theme::{self, Tokens};

/// Label chip (design `.prop-cs-algochip`): mono label on a sunken,
/// 1px-bordered rounded rect. Used for read-only enumerations such as
/// the supported-checksum list.
pub fn chip<'a, M: 'a>(t: &Tokens, label: impl Into<String>) -> Element<'a, M> {
    let t = *t;
    container(
        text(label.into())
            .font(theme::MONO)
            .size(CHIP_TEXT)
            // Pin the line box to the font size. iced's default line
            // height reserves descender room that an all-caps label
            // never uses, so the glyphs sit above the chip's centre.
            .line_height(text::LineHeight::Absolute(CHIP_TEXT.into()))
            .color(t.fg_3),
    )
    .padding([CHIP_PAD_Y, CHIP_PAD_X])
    .style(move |_| container::Style {
        background: Some(t.bg_sunken.into()),
        border: Border {
            color: t.border_subtle,
            width: 1.0,
            radius: theme::radius::XS.into(),
        },
        snap: true,
        ..Default::default()
    })
    .into()
}

const CHIP_TEXT: f32 = 10.0;
const CHIP_PAD_Y: f32 = 2.0;
const CHIP_PAD_X: f32 = 6.0;

/// Count badge: 16px pill, mono(10), 6px x-pad, min width 16.
pub fn pill_count<'a, M: 'a>(n: u64, fg: Color, bg: Color) -> Element<'a, M> {
    container(text(n.to_string()).font(theme::MONO).size(10.0).color(fg))
        .height(Length::Fixed(16.0))
        .padding([0.0, 6.0])
        .align_y(Alignment::Center)
        .align_x(Alignment::Center)
        .style(move |_| container::Style {
            background: Some(bg.into()),
            border: Border {
                radius: 8.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

/// 8px colored dot + bold label in the same color. 6px gap.
pub fn status_dot<'a, M: 'a>(
    color: Color,
    label: impl Into<String>,
    font_size: f32,
) -> Element<'a, M> {
    row![
        dot(8.0, color),
        text(label.into())
            .font(theme::BODY_BOLD)
            .size(font_size)
            .color(color),
    ]
    .spacing(6.0)
    .align_y(Alignment::Center)
    .into()
}

/// Plain filled circle of `size` px.
pub fn dot<'a, M: 'a>(size: f32, color: Color) -> Element<'a, M> {
    container(iced::widget::Space::new())
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .style(move |_| container::Style {
            background: Some(color.into()),
            border: Border {
                radius: (size / 2.0).into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

/// Small rounded square swatch (sidebar queue color), 8px radius 2.
pub fn swatch<'a, M: 'a>(size: f32, radius: f32, color: Color) -> Element<'a, M> {
    container(iced::widget::Space::new())
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .style(move |_| container::Style {
            background: Some(color.into()),
            border: Border {
                radius: radius.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

/// Pill-shaped progress bar (canvas; clipped fill avoids corner bleed).
pub fn pill_progress<'a, M: 'a>(
    frac: f32,
    width: Length,
    height: f32,
    track: Color,
    fill: Color,
) -> Element<'a, M> {
    canvas(PillProgress {
        frac: frac.clamp(0.0, 1.0),
        track,
        fill,
        label: None,
        label_color: Color::TRANSPARENT,
        border: None,
    })
    .width(width)
    .height(Length::Fixed(height))
    .into()
}

/// Fill treatment of an [`inline_progress`] bar (design
/// `.pfill` / `.pfill.paused` / `.pfill.failed`): a stalled transfer
/// still shows how far it got, but must not read as live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressTone {
    /// Live transfer — clay fill.
    Active,
    /// Stopped with bytes on disk — muted grey fill.
    Paused,
    /// Stopped by an error — rust fill.
    Failed,
}

/// Table-cell progress: sunken track + translucent fill (per `tone`) +
/// centered `"{label} · {pct}%"` body_bold(11) caption.
pub fn inline_progress<'a, M: 'a>(
    t: &Tokens,
    frac: f32,
    label: String,
    selected: bool,
    tone: ProgressTone,
    width: Length,
    height: f32,
) -> Element<'a, M> {
    // Selection wins over tone: on a selected row the design's
    // `tr.selected .pfill` rule overrides the paused/failed variants
    // (equal specificity, declared later), so every fill goes solid
    // clay against the selected background.
    let fill = if selected {
        with_alpha(crate::gui::color::clay::C300, 150.0 / 255.0)
    } else {
        match tone {
            ProgressTone::Active => with_alpha(crate::gui::color::clay::C300, 100.0 / 255.0),
            ProgressTone::Paused => with_alpha(t.fg_3, 0.35),
            ProgressTone::Failed => with_alpha(crate::gui::color::rust::R300, 0.5),
        }
    };
    canvas(PillProgress {
        frac: frac.clamp(0.0, 1.0),
        track: t.bg_sunken,
        fill,
        label: Some(format!("{label} · {}%", (frac * 100.0).round() as u32)),
        label_color: t.fg_1,
        border: Some(Color {
            a: 26.0 / 255.0,
            ..Color::BLACK
        }),
    })
    .width(width)
    .height(Length::Fixed(height))
    .into()
}

struct PillProgress {
    frac: f32,
    track: Color,
    fill: Color,
    label: Option<String>,
    label_color: Color,
    border: Option<Color>,
}

impl<M> canvas::Program<M> for PillProgress {
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
        let rounded = |frame: &mut canvas::Frame, w: f32, color: Color| {
            let path = canvas::Path::rounded_rectangle(
                Point::ORIGIN,
                Size::new(w, size.height),
                radius.into(),
            );
            frame.fill(&path, color);
        };
        rounded(&mut frame, size.width, self.track);
        if let Some(border) = self.border {
            let path = canvas::Path::rounded_rectangle(
                Point::new(0.5, 0.5),
                Size::new(size.width - 1.0, size.height - 1.0),
                radius.into(),
            );
            frame.stroke(
                &path,
                canvas::Stroke::default().with_color(border).with_width(1.0),
            );
        }
        if self.frac > 0.0 {
            // NOTE: tiny-skia's geometry backend ignores `with_clip`
            // (draft/paste drop the clip + offset), so the fill is
            // built as its own correctly-capped shape instead.
            let path = fill_path(size, self.frac, 1.0);
            frame.fill(&path, self.fill);
        }
        if let Some(label) = &self.label {
            frame.fill_text(canvas::Text {
                content: label.clone(),
                position: Point::new(size.width / 2.0, size.height / 2.0),
                color: self.label_color,
                size: 11.0.into(),
                font: theme::BODY_BOLD,
                align_x: iced::widget::text::Alignment::Center,
                align_y: iced::alignment::Vertical::Center,
                ..canvas::Text::default()
            });
        }
        vec![frame.into_geometry()]
    }
}

/// Rounded fill strip for a pill progress bar: left caps always
/// rounded, right edge square until the fill reaches the track's
/// right cap. `inset` shrinks the fill inside the track.
pub(crate) fn fill_path(track: Size, frac: f32, inset: f32) -> iced::widget::canvas::Path {
    let h = track.height - 2.0 * inset;
    let r = h / 2.0;
    let fw = (track.width * frac).clamp(0.0, track.width) - inset;
    let fw = fw.max(2.0 * r);
    let right_r = if fw >= track.width - inset - r {
        r
    } else {
        0.0
    };
    iced::widget::canvas::Path::rounded_rectangle(
        Point::new(inset, inset),
        Size::new(fw, h),
        iced::border::Radius {
            top_left: r,
            bottom_left: r,
            top_right: right_r,
            bottom_right: right_r,
        },
    )
}

/// Uppercase 10px bold section label, fg_3.
pub fn eyebrow<'a, M: 'a>(t: &Tokens, label: &str) -> Element<'a, M> {
    text(spaced_upper(label))
        .font(theme::BODY_BOLD)
        .size(10.0)
        .color(t.fg_3)
        .into()
}

/// Uppercase 11px bold field label, fg_2.
pub fn field_label<'a, M: 'a>(t: &Tokens, label: &str) -> Element<'a, M> {
    text(spaced_upper(label))
        .font(theme::BODY_BOLD)
        .size(11.0)
        .color(t.fg_2)
        .into()
}

/// Uppercase with a hair of letter-spacing. iced text has no
/// letter-spacing; interleave U+2009 (thin space) sparingly? No —
/// that breaks copy/selection. Plain uppercase reads close enough at
/// 10–11px with Jakarta SemiBold.
pub(crate) fn spaced_upper(s: &str) -> String {
    s.to_uppercase()
}
