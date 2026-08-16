//! Settings-pane primitives (design `.set-section` / `.set-row`).
//!
//! A section is an uppercase eyebrow header sitting *outside* a bordered
//! surface, plus that surface holding hairline-separated rows. Every
//! settings tab is built from these so the panes stay coherent.

use iced::widget::{column, container, row, text};
use iced::{Alignment, Color, Element, Length, Padding};

use crate::gui::color;
use crate::gui::theme::{self, Tokens};
use crate::gui::widget::cards::hairline;

// ---- design constants (no magic numbers) ----------------------------

/// `.set-section .head`: 700 10.5px uppercase, fg_3, 10px above the rows,
/// nudged 2px right so it optically aligns with the row labels below.
const HEAD_SIZE: f32 = 10.5;
const HEAD_GAP: f32 = 10.0;
const HEAD_INDENT: f32 = 2.0;

/// `.set-section { margin-bottom: 22px }` — gap between stacked sections.
pub const SECTION_GAP: f32 = 22.0;

/// `.set-row { padding: 12px 14px; gap: 16px }`.
const ROW_PAD_Y: f32 = 12.0;
const ROW_PAD_X: f32 = 14.0;
const ROW_GAP: f32 = theme::space::S4;

/// `.set-row .lbl` 500 12.5px fg_1 · `.hint` 400 11px fg_3, 3px under the
/// label, 1.4 line-height.
const LBL_SIZE: f32 = 12.5;
const HINT_SIZE: f32 = 11.0;
const HINT_GAP: f32 = 3.0;
const HINT_LINE: f32 = 1.4;

/// `.set-section.danger .rows` border — the danger hue at 30% alpha.
const DANGER_BORDER_ALPHA: f32 = 0.3;

// ---- rows -----------------------------------------------------------

/// One `.set-row`: label (+ optional hint) on the left, control right.
pub fn set_row<'a, M: 'a>(
    t: &Tokens,
    label: &str,
    hint: Option<&str>,
    control: Element<'a, M>,
) -> Element<'a, M> {
    labelled_row(t, label, hint, control, ROW_PAD_X)
}

/// `set_row` for a pane that has no card around it: the same row on the
/// same vertical rhythm, with its label flush to the pane's own edge
/// rather than inset by a border that isn't there.
pub fn set_row_flat<'a, M: 'a>(
    t: &Tokens,
    label: &str,
    hint: Option<&str>,
    control: Element<'a, M>,
) -> Element<'a, M> {
    labelled_row(t, label, hint, control, 0.0)
}

fn labelled_row<'a, M: 'a>(
    t: &Tokens,
    label: &str,
    hint: Option<&str>,
    control: Element<'a, M>,
    pad_x: f32,
) -> Element<'a, M> {
    container(
        row![label_col(t, label, hint), control]
            .spacing(ROW_GAP)
            .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding(Padding::from([ROW_PAD_Y, pad_x]))
    .into()
}

/// `.set-row.stack`: label (+ hint) above a full-width control. For
/// inputs, editors and pickers that cannot share a line with their label.
pub fn set_row_stack<'a, M: 'a>(
    t: &Tokens,
    label: &str,
    hint: Option<&str>,
    control: Element<'a, M>,
) -> Element<'a, M> {
    container(
        column![label_col(t, label, hint), control]
            .spacing(theme::space::S2)
            .width(Length::Fill),
    )
    .width(Length::Fill)
    .padding(Padding::from([ROW_PAD_Y, ROW_PAD_X]))
    .into()
}

/// A row whose content spans the full width, on the same padding grid
/// as the labelled rows. For panels that belong inside a group but
/// carry their own surface (e.g. a warning about the row above).
pub fn set_row_panel<'a, M: 'a>(content: Element<'a, M>) -> Element<'a, M> {
    container(content)
        .width(Length::Fill)
        .padding(Padding::from([ROW_PAD_Y, ROW_PAD_X]))
        .into()
}

/// Content belonging to the row above: the same horizontal grid, and no
/// top padding of its own, so it reads as part of that row rather than
/// as another row.
///
/// For what an action in a row reported. Put in the row's control
/// column instead, a sentence of it widens that column and squeezes the
/// label and hint into a ribbon down the left.
pub fn set_row_footnote<'a, M: 'a>(content: Element<'a, M>) -> Element<'a, M> {
    container(content)
        .width(Length::Fill)
        .padding(Padding {
            top: 0.0,
            right: ROW_PAD_X,
            bottom: ROW_PAD_Y,
            left: ROW_PAD_X,
        })
        .into()
}

/// A row that is only prose (footnotes under a group of settings).
pub fn set_note<'a, M: 'a>(t: &Tokens, note: &str) -> Element<'a, M> {
    container(
        text(note.to_owned())
            .font(theme::BODY)
            .size(HINT_SIZE)
            .line_height(HINT_LINE)
            .color(t.fg_3),
    )
    .width(Length::Fill)
    .padding(Padding::from([ROW_PAD_Y, ROW_PAD_X]))
    .into()
}

fn label_col<'a, M: 'a>(t: &Tokens, label: &str, hint: Option<&str>) -> Element<'a, M> {
    let mut col = column![
        text(label.to_owned())
            .font(theme::BODY_MEDIUM)
            .size(LBL_SIZE)
            .color(t.fg_1),
    ]
    .spacing(HINT_GAP)
    .width(Length::Fill);
    if let Some(hint) = hint {
        col = col.push(
            text(hint.to_owned())
                .font(theme::BODY)
                .size(HINT_SIZE)
                .line_height(HINT_LINE)
                .color(t.fg_3),
        );
    }
    col.into()
}

// ---- sections -------------------------------------------------------

/// `.set-section`: uppercase header outside a bordered surface whose rows
/// are separated by hairlines.
pub fn set_section<'a, M: 'a>(
    t: &Tokens,
    title: &str,
    rows: Vec<Element<'a, M>>,
) -> Element<'a, M> {
    section(t, title, t.fg_3, t.border_subtle, rows)
}

/// Danger variant: rust header and border (design `.set-section.danger`).
pub fn set_section_danger<'a, M: 'a>(
    t: &Tokens,
    title: &str,
    rows: Vec<Element<'a, M>>,
) -> Element<'a, M> {
    let border = color::with_alpha(t.status_danger, DANGER_BORDER_ALPHA);
    section(t, title, t.status_danger, border, rows)
}

/// The `.rows` surface on its own, with no eyebrow header. For a pane
/// short enough that a header would only name what the rows already say.
pub fn set_rows<'a, M: 'a>(t: &Tokens, rows: Vec<Element<'a, M>>) -> Element<'a, M> {
    rows_surface(t, t.border_subtle, rows)
}

/// Rows with no surface at all: they sit on the page's own background,
/// divided by a dashed rule.
///
/// For panes that are nothing but settings, where a card around every
/// group draws a box around the obvious — the tab is already the group.
/// A dashed divider says "these are separate" without the solid line's
/// claim to be the edge of something.
pub fn set_rows_flat<'a, M: 'a>(t: &Tokens, rows: Vec<Element<'a, M>>) -> Element<'a, M> {
    set_row_groups(t, rows.into_iter().map(|r| vec![r]).collect())
}

/// Flat rows in groups: a dashed rule between the groups, nothing but
/// the rows' own spacing inside one.
///
/// For settings that are one another's detail — a limit and the presets
/// that set it, an action and the warning about what it will do. A rule
/// between those says they are separate things to decide, when the
/// second only exists because of the first.
pub fn set_row_groups<'a, M: 'a>(t: &Tokens, groups: Vec<Vec<Element<'a, M>>>) -> Element<'a, M> {
    let mut body = column![].width(Length::Fill);
    for (i, group) in groups.into_iter().enumerate() {
        if group.is_empty() {
            continue;
        }
        if i > 0 {
            body = body.push(crate::gui::widget::dashed_rule(t.border_default));
        }
        for row in group {
            body = body.push(row);
        }
    }
    body.width(Length::Fill).into()
}

/// Header + arbitrary body, with no rows surface. For groups whose body
/// already carries its own surfaces (e.g. the category accordions).
pub fn set_group<'a, M: 'a>(t: &Tokens, title: &str, body: Element<'a, M>) -> Element<'a, M> {
    column![head(title, t.fg_3), body]
        .spacing(HEAD_GAP)
        .width(Length::Fill)
        .into()
}

fn section<'a, M: 'a>(
    t: &Tokens,
    title: &str,
    head_color: Color,
    border_color: Color,
    rows: Vec<Element<'a, M>>,
) -> Element<'a, M> {
    column![head(title, head_color), rows_surface(t, border_color, rows)]
        .spacing(HEAD_GAP)
        .width(Length::Fill)
        .into()
}

fn rows_surface<'a, M: 'a>(
    t: &Tokens,
    border_color: Color,
    rows: Vec<Element<'a, M>>,
) -> Element<'a, M> {
    let mut body = column![].width(Length::Fill);
    for (i, r) in rows.into_iter().enumerate() {
        if i > 0 {
            body = body.push(hairline(border_color));
        }
        body = body.push(r);
    }
    // Rows carry their own padding, so the surface adds none.
    crate::gui::widget::surface(t.bg_surface, border_color, 0.0, body.into())
}

fn head<'a, M: 'a>(title: &str, color: Color) -> Element<'a, M> {
    container(crate::gui::widget::tracked_caps(
        title,
        HEAD_SIZE,
        crate::gui::widget::TRACKING_EM,
        color,
    ))
    .padding(Padding {
        top: 0.0,
        right: 0.0,
        bottom: 0.0,
        left: HEAD_INDENT,
    })
    .into()
}
