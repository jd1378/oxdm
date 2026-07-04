//! Shared severe-error grammar (design §3.2 / §3.3): friendly title →
//! detail → things-to-check → quiet monospace error-code chip + Copy.
//! Extracted from the Download window so the Add dialog's probe-error
//! panel reads identically (feature #3), plus the stacked Expected/Got
//! hash-mismatch panel (design §3.4) shared with the Properties
//! Checksums tab.

use iced::widget::{column, container, row, text};
use iced::{Alignment, Element, Length};

use crate::domain::JobError;
use crate::gui::icons;
use crate::gui::theme::{self, Tokens};

use super::{Btn, BtnSize, eyebrow, hairline};

/// Square icon tile in the error head (design `.eb-icon` = 36px).
const ICON_TILE: f32 = 36.0;
/// Icon-tile corner radius (design `.eb-icon` border-radius = 8px).
const ICON_TILE_RADIUS: f32 = 8.0;
/// Mid-truncation width for long hex digests in mismatch panels.
pub const HASH_TRUNCATE_CHARS: usize = 40;

/// Friendly title, leading icon, short code, and a static "things to
/// check" hint for each `JobError` variant. The detail line uses the
/// error's own `Display` text. (Design §3.3 severe-error grammar.)
pub fn error_meta(err: &JobError) -> (&'static str, &'static str, &'static str, &'static str) {
    match err {
        JobError::Network(_) => (
            "wifi",
            "Connection problem",
            "NETWORK",
            "Check your internet connection, then resume. If it persists, the server may be down or rate-limiting.",
        ),
        JobError::Dns { .. } => (
            "globe",
            "Couldn't reach the server",
            "DNS",
            "The hostname couldn't be resolved. Verify the URL spelling and your DNS / VPN settings.",
        ),
        JobError::ServerConflict(_) => (
            "triangle-alert",
            "The file on the server changed",
            "SERVER_CONFLICT",
            "The remote file changed since this download began. Restart from zero to fetch the current version.",
        ),
        JobError::SaveConflict(_) => (
            "hard-drive",
            "Couldn't save the file",
            "SAVE_CONFLICT",
            "A naming conflict came up while writing. Free up the filename or pick a different folder.",
        ),
        JobError::DuplicateActive { .. } => (
            "copy",
            "Already downloading",
            "DUPLICATE",
            "A download with this name is already in progress in the same folder. Wait for it or rename this one.",
        ),
        JobError::ChecksumMismatch { .. } => (
            "shield-alert",
            "Integrity check failed",
            "CHECKSUM_MISMATCH",
            "The data doesn't match the expected hash. Don't open the file; re-download from a trusted source.",
        ),
        JobError::Cancelled => (
            "circle-x",
            "Download cancelled",
            "CANCELLED",
            "This download was cancelled. Start it again to retry.",
        ),
        JobError::Io(_) => (
            "hard-drive",
            "Disk write error",
            "IO",
            "Couldn't write to disk. Check free space and folder permissions, or save to a different folder.",
        ),
        JobError::ConflictPending(_) => (
            "triangle-alert",
            "Paused — needs your attention",
            "CONFLICT_PENDING",
            "A conflict came up while running in the background. Resume to retry the download.",
        ),
        JobError::Other(_) => (
            "circle-alert",
            "Something went wrong",
            "ERROR",
            "An unexpected error occurred. Try again; if it keeps failing, check the daemon logs.",
        ),
    }
}

/// Severe-error block (Download window shape): rust-tinted card with
/// title + detail, a small "things to check" hint paragraph, and a
/// quiet monospace code footer with copy.
pub fn error_block<'a, M: Clone + 'a>(t: &Tokens, err: &JobError, on_copy: M) -> Element<'a, M> {
    let (_, _, _, hint) = error_meta(err);
    let checks = column![
        eyebrow(t, "things to check"),
        text(hint)
            .font(theme::BODY)
            .size(12.0)
            .color(t.fg_2)
            .line_height(iced::widget::text::LineHeight::Relative(1.4)),
    ]
    .spacing(theme::space::S1)
    .into();
    panel(t, err, None, checks, on_copy)
}

/// Severe-error block with a bulleted static checklist and an optional
/// "Try again" head action (Add dialog probe-error shape, design §3.2:
/// `.error-block` head → `.eb-checklist` → `.eb-foot`).
pub fn error_checklist_block<'a, M: Clone + 'a>(
    t: &Tokens,
    err: &JobError,
    items: &'a [&'a str],
    on_retry: Option<M>,
    on_copy: M,
) -> Element<'a, M> {
    let mut list = column![].spacing(theme::space::S1);
    for item in items {
        list = list.push(
            row![
                text("•")
                    .font(theme::BODY_BOLD)
                    .size(12.0)
                    .color(t.status_danger),
                text(*item)
                    .font(theme::BODY)
                    .size(12.0)
                    .color(t.fg_2)
                    .line_height(iced::widget::text::LineHeight::Relative(1.4)),
            ]
            .spacing(theme::space::S2),
        );
    }
    let checks = column![eyebrow(t, "a few things to check"), list]
        .spacing(theme::space::S1)
        .into();
    let retry = on_retry.map(|msg| {
        Btn::new("Try again")
            .secondary()
            .size(BtnSize::Sm)
            .icon("rotate-cw")
            .on_press(msg)
            .view(t)
    });
    panel(t, err, retry, checks, on_copy)
}

/// Shared card: head (icon tile + title + detail [+ action]) →
/// hairline → checks → quiet code footer.
fn panel<'a, M: Clone + 'a>(
    t: &Tokens,
    err: &JobError,
    head_action: Option<Element<'a, M>>,
    checks: Element<'a, M>,
    on_copy: M,
) -> Element<'a, M> {
    let t2 = *t;
    let (icon_name, title, code, _) = error_meta(err);
    let detail = err.to_string();

    let mut head = row![
        container(icons::icon(icon_name, 20.0, t.status_danger))
            .width(Length::Fixed(ICON_TILE))
            .height(Length::Fixed(ICON_TILE))
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .style(move |_| container::Style {
                background: Some(t2.status_danger_bg.into()),
                border: iced::Border {
                    radius: ICON_TILE_RADIUS.into(),
                    ..Default::default()
                },
                ..Default::default()
            }),
        column![
            text(title)
                .font(theme::BODY_BOLD)
                .size(14.0)
                .color(t.status_danger),
            text(detail).font(theme::BODY).size(12.0).color(t.fg_2),
        ]
        .spacing(2.0)
        .width(Length::Fill),
    ]
    .spacing(theme::space::S3)
    .align_y(Alignment::Center);
    if let Some(action) = head_action {
        head = head.push(action);
    }

    // Quiet monospace error-code footer (label + chip + copy).
    let code_chip = container(text(code).font(theme::MONO).size(11.0).color(t2.fg_2))
        .padding([2.0, 8.0])
        .style(move |_| container::Style {
            background: Some(t2.bg_sunken.into()),
            border: iced::Border {
                color: t2.border_subtle,
                width: 1.0,
                radius: theme::radius::XS.into(),
            },
            ..Default::default()
        });
    let code_footer = row![
        text("Error code")
            .font(theme::BODY)
            .size(11.0)
            .color(t.fg_3),
        code_chip,
        iced::widget::Space::new().width(Length::Fill),
        Btn::new("Copy")
            .toolbar()
            .size(BtnSize::Sm)
            .icon("copy")
            .on_press(on_copy)
            .view(t),
    ]
    .spacing(theme::space::S2)
    .align_y(Alignment::Center);

    container(
        column![head, hairline(t.border_subtle), checks, code_footer].spacing(theme::space::S3),
    )
    .width(Length::Fill)
    .padding(theme::space::S3)
    .style(move |_| container::Style {
        background: Some(t2.status_danger_bg.into()),
        border: iced::Border {
            color: t2.status_danger,
            width: 1.0,
            radius: theme::surface::RADIUS.into(),
        },
        ..Default::default()
    })
    .into()
}

/// Middle-ellipsis truncation for long mono strings (paths, hashes).
pub fn mid_truncate(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_owned();
    }
    let keep = max.saturating_sub(1);
    let head = keep.div_ceil(2);
    let tail = keep - head;
    let head_s: String = chars[..head].iter().collect();
    let tail_s: String = chars[chars.len() - tail..].iter().collect();
    format!("{head_s}…{tail_s}")
}

/// Rust stacked Expected/Got mismatch panel (design §3.4). Shared by
/// the Download window's paste-verify / local-compute paths and the
/// Properties Checksums row diff so they read identically. `expected`
/// is the saved (publisher) hash; `got` is the digest computed from
/// the file on disk.
pub fn hash_mismatch<'a, M: 'a>(
    t: &Tokens,
    algo_label: &str,
    expected: &str,
    got: &str,
) -> Element<'a, M> {
    let t2 = *t;
    container(
        column![
            row![
                icons::icon("shield-alert", 17.0, t.status_danger),
                text(format!("Doesn't match the saved {algo_label} hash."))
                    .font(theme::BODY_BOLD)
                    .size(12.0)
                    .color(t.status_danger),
            ]
            .spacing(theme::space::S2)
            .align_y(Alignment::Center),
            text(format!(
                "Expected  {}",
                mid_truncate(expected, HASH_TRUNCATE_CHARS)
            ))
            .font(theme::MONO)
            .size(11.0)
            .color(t.fg_2),
            text(format!(
                "Got       {}",
                mid_truncate(got, HASH_TRUNCATE_CHARS)
            ))
            .font(theme::MONO)
            .size(11.0)
            .color(t.status_danger),
        ]
        .spacing(theme::space::S1),
    )
    .width(Length::Fill)
    .padding(theme::space::S3)
    .style(move |_| container::Style {
        background: Some(t2.status_danger_bg.into()),
        border: iced::Border {
            color: t2.status_danger,
            width: 1.0,
            radius: theme::radius::XS.into(),
        },
        ..Default::default()
    })
    .into()
}
