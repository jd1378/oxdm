//! The integrity table: one row per saved hash, algorithm and verdict
//! on the first line, the expected/got pair beside them.
//!
//! Shared so a mismatch reads the same wherever it is reported — the
//! download window's completion page and the Properties dialog's
//! Checksums tab are the same news about the same file, and two
//! layouts for it made them look like two different findings.
//!
//! The download window keeps its own value lines: theirs track hover
//! and a per-line copy confirmation. What lives here is everything that
//! decides how the table *looks* — the column metrics, the verdict
//! chip, and a self-contained row for callers that need no hover state.

use iced::widget::{container, row, text};
use iced::{Alignment, Element, Length};

use crate::gui::color;
use crate::gui::icons;
use crate::gui::theme::{self, Tokens};
use crate::gui::widget::button::BtnSize;
use crate::gui::widget::error_panel::mid_truncate;

/// `.checksum-box` table metrics: the algorithm and status columns are
/// fixed so the hashes line up down the box, and a hash is
/// mid-truncated rather than wrapped.
pub const ALGO_W: f32 = 64.0;
pub const STATUS_W: f32 = 100.0;
pub const LABEL_W: f32 = 64.0;
pub const ALGO_SIZE: f32 = 11.0;
pub const STATUS_SIZE: f32 = 10.0;
pub const HASH_SIZE: f32 = 11.0;
pub const LABEL_SIZE: f32 = 9.0;
/// Design truncates to `12…8`. Ours fits a little more, but the line
/// must never wrap: the copy button shares the row, and a second line
/// pushes it out of the box.
pub const HASH_CHARS: usize = 24;
/// One line of a row. Pinned so the algorithm, the chip and the first
/// value line share a centre — a stacked pair otherwise leaves them
/// each aligned to something different.
pub const LINE_H: f32 = 22.0;
/// Row padding — tighter than a settings row's 12/14.
pub const PAD_Y: f32 = 8.0;
pub const PAD_X: f32 = 12.0;

/// The edge every failed-integrity panel carries, and the fill they
/// share (`Tokens::status_danger_bg`). One warning across three panels
/// reads as one thing; three shades of rust read as three.
pub const DANGER_EDGE: iced::Color = color::rust::R300;

/// What a row says about its hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Verified,
    Mismatch,
    Unverified,
}

impl Verdict {
    /// Chip fill, chip ink, word, glyph. Solid pairs, not a wash: the
    /// chip sits on a panel that is itself tinted, and an alpha of the
    /// same hue reads as a smudge of the background rather than a
    /// label. Fixed across themes for the same reason a warning sign is
    /// not repainted per room.
    pub fn chip(self, t: &Tokens) -> (iced::Color, iced::Color, &'static str, &'static str) {
        match self {
            Verdict::Mismatch => (color::rust::R100, color::rust::R500, "mismatch", "x"),
            Verdict::Verified => (color::moss::M50, color::moss::M600, "verified", "check"),
            Verdict::Unverified => (t.bg_page, t.fg_3, "unverified", "minus"),
        }
    }
}

/// The verdict pill.
pub fn chip<'a, M: 'a>(
    icon: &'a str,
    label: &'a str,
    bg: iced::Color,
    fg: iced::Color,
) -> Element<'a, M> {
    container(
        row![
            icons::icon(icon, 10.0, fg),
            text(label).font(theme::BODY).size(STATUS_SIZE).color(fg),
        ]
        .spacing(theme::space::S1)
        .align_y(Alignment::Center),
    )
    .padding([2.0, 6.0])
    .style(move |_| container::Style {
        background: Some(bg.into()),
        border: iced::Border {
            radius: theme::radius::PILL.into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .into()
}

/// One `EXPECTED` / `GOT` line: the label, the hash, and a button that
/// copies it.
///
/// `bad` strikes the value through — those are the bytes to discard —
/// and `text` has no strikethrough, so it goes through a span.
pub fn hash_line<'a, M: Clone + 'a>(
    t: &Tokens,
    label: &'a str,
    hash: &str,
    bad: bool,
    copied: bool,
    on_copy: M,
) -> Element<'a, M> {
    let color = if bad { t.status_danger } else { t.fg_2 };
    let shown: Element<'a, M> = if bad {
        iced::widget::rich_text::<(), M, _, _>([
            iced::widget::span(mid_truncate(hash, HASH_CHARS)).strikethrough(true)
        ])
        .font(theme::MONO)
        .size(HASH_SIZE)
        .wrapping(iced::widget::text::Wrapping::None)
        .color(color)
        .into()
    } else {
        text(mid_truncate(hash, HASH_CHARS))
            .font(theme::MONO)
            .size(HASH_SIZE)
            .wrapping(iced::widget::text::Wrapping::None)
            .color(color)
            .into()
    };
    row![
        container(
            text(label.to_uppercase())
                .font(theme::BODY_BOLD)
                .size(LABEL_SIZE)
                .color(if bad { t.status_danger } else { t.fg_3 })
        )
        .width(Length::Fixed(LABEL_W))
        .height(Length::Fixed(LINE_H))
        .align_y(Alignment::Center),
        shown,
        iced::widget::Space::new().width(Length::Fill),
        crate::gui::widget::copy::copy_btn("", copied, on_copy)
            .toolbar()
            .size(BtnSize::Sm)
            .view(t),
    ]
    .spacing(theme::space::S2)
    .align_y(Alignment::Center)
    .height(Length::Fixed(LINE_H))
    .into()
}
