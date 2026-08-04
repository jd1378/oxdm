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

/// Error-code chip: 11px mono pinned to a line box its caps actually
/// fill, padded back out to the height the design draws.
const CODE_TEXT: f32 = 11.0;
const CODE_LINE: f32 = 11.0;
const CODE_PAD_Y: f32 = 4.0;
const CODE_PAD_X: f32 = 8.0;
/// Even with the line box pinned, the renderer leaves this much more
/// room above the caps than below — 11px of box for 8px of ink, and the
/// remainder does not split evenly. A row centres *boxes*, so without
/// compensating the chip's caps sit low inside their chip and the label
/// beside it sits low in the row. Both are lifted by the same amount:
/// take it off the top padding and give it back at the bottom.
const CODE_INK_LIFT: f32 = 2.0;

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
        JobError::HttpStatus { code, .. } => match code {
            401 | 407 => (
                "key",
                "This download needs credentials",
                "HTTP_UNAUTHORIZED",
                "The server asked for a sign-in that oxdm doesn't have.",
            ),
            403 => (
                "lock",
                "The server refused the request",
                "HTTP_FORBIDDEN",
                "The server understood the request and declined it.",
            ),
            404 | 410 => (
                "file",
                "The file isn't at this address",
                "HTTP_NOT_FOUND",
                "The server has nothing at this URL any more.",
            ),
            429 => (
                "clock",
                "The server is rate-limiting you",
                "HTTP_TOO_MANY_REQUESTS",
                "Too many requests arrived from here; the server is asking you to slow down.",
            ),
            500..=599 => (
                "triangle-alert",
                "The server had a problem",
                "HTTP_SERVER_ERROR",
                "The failure is on the server's side, not yours.",
            ),
            _ => (
                "circle-alert",
                "The server refused the request",
                "HTTP_ERROR",
                "The server answered with an error instead of the file.",
            ),
        },
        JobError::NotResumable(_) => (
            "plug-zap",
            "Server refused to resume",
            "NOT_RESUMABLE",
            "The server rejected the request to continue from where this download stopped.",
        ),
        JobError::FileChanged(_) => (
            "rotate-ccw",
            "The file on the server has changed",
            "FILE_CHANGED",
            "The server reports a different file than when this download started. Continuing would corrupt the result.",
        ),
        JobError::DiskFull(_) => (
            "hard-drive",
            "Out of disk space",
            "DISK_FULL",
            "The destination drive ran out of room. Your progress is safe.",
        ),
        JobError::PermissionDenied(_) => (
            "hard-drive",
            "Can't write to this folder",
            "PERMISSION_DENIED",
            "The destination folder rejected the write. Permissions may have changed since the download started.",
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

/// How tall this error's card will be, so a window opening on it can be
/// sized to fit instead of opening scrolled — or, worse, opening with a
/// screenful of empty surface under a three-line block.
///
/// Derived from the copy the card actually renders rather than measured
/// per variant: editing the wording, or adding a step to a list, moves
/// the height with it.
pub fn error_block_height(err: &JobError) -> f32 {
    /// Everything in the card that does not scale with the text: 12px
    /// padding top and bottom, the 36px icon tile, the hairline, the
    /// eyebrow, the code footer, and the gaps between them.
    const CARD_CHROME: f32 = 112.0;
    /// One rendered line of 12px body copy at line-height 1.4, rounded
    /// to whole pixels the way the renderer lays it out.
    const LINE: f32 = 17.0;
    /// `theme::space::S1` between bullets.
    const BULLET_GAP: f32 = 4.0;
    /// Characters per line of the card at the download window's fixed
    /// width, read off the rendered panel (lines break between 78 and
    /// 88 characters depending on the words). Deliberately the low end:
    /// over-estimating a line costs ~17px of slack, under-estimating
    /// opens the window already scrolled.
    const CHARS_PER_LINE: usize = 78;

    fn lines(s: &str) -> f32 {
        (s.chars().count().div_ceil(CHARS_PER_LINE)).max(1) as f32
    }

    let detail = lines(&error_detail(err));
    match recovery_copy(err) {
        Some((_, _, items)) => {
            let text: f32 = items.iter().map(|i| lines(i)).sum();
            CARD_CHROME + LINE * (detail + text) + BULLET_GAP * (items.len() as f32 - 1.0)
        }
        // The plain block is one paragraph of hint text.
        None => CARD_CHROME + LINE * (detail + lines(error_meta(err).3)),
    }
}

/// What Copy puts on the clipboard: the short code plus the raw error
/// text. The panel shows a written sentence, which is no use in a bug
/// report — the code says which case it was, the raw text says what the
/// server or OS actually returned.
pub fn error_report(err: &JobError) -> String {
    format!("{}: {err}", error_meta(err).2)
}

/// The sentence under the title. Ours, not the error's: an error type
/// is written for whoever debugs it, and `Display` output like
/// "io error: No space left on device (os error 28)" repeats the title
/// and then talks about errno. The raw text still travels — Copy puts
/// it on the clipboard.
pub fn error_detail(err: &JobError) -> String {
    match err {
        JobError::Network(_) => {
            "The connection to the server dropped or timed out before the file finished.".into()
        }
        JobError::Dns { host, .. } => match host {
            Some(h) => format!("`{h}` could not be looked up, so no connection was ever made."),
            None => "The address could not be looked up, so no connection was ever made.".into(),
        },
        JobError::HttpStatus { code, reason, .. } => {
            let what = match code {
                401 | 407 => "asked for credentials",
                403 => "refused access to this file",
                404 | 410 => "has nothing at this address",
                429 => "is asking for fewer requests",
                500..=599 => "reported a problem on its own side",
                _ => "answered with an error instead of the file",
            };
            match reason {
                Some(r) => format!("The server {what} — HTTP {code} {r}."),
                None => format!("The server {what} — HTTP {code}."),
            }
        }
        JobError::ServerConflict(_) => {
            "The file on the server no longer matches the one this download started.".into()
        }
        JobError::NotResumable(_) => {
            "This server won't send part of a file, so the bytes already downloaded can't be \
             continued — only discarded."
                .into()
        }
        JobError::FileChanged(_) => {
            "The server reports a different size or version than when this download started. \
             Continuing would stitch two different files together."
                .into()
        }
        JobError::SaveConflict(_) => {
            "Something already occupies the name this download wants to save under.".into()
        }
        JobError::DuplicateActive { filename, save_dir } => {
            format!("`{filename}` is already downloading into {save_dir}.")
        }
        JobError::ChecksumMismatch { .. } => {
            "The bytes that arrived don't match the hash they were supposed to have, so the \
             file cannot be trusted."
                .into()
        }
        JobError::Cancelled => "This download was stopped before it finished.".into(),
        JobError::DiskFull(_) => {
            "The drive this download saves to has no room left. What has been downloaded so \
             far is kept."
                .into()
        }
        JobError::PermissionDenied(_) => {
            "The destination folder refused to be written to. Permissions may have changed \
             since the download started."
                .into()
        }
        JobError::Io(_) => "The download stopped while reading or writing the file on disk.".into(),
        JobError::ConflictPending(_) => {
            "This download hit a conflict while running in the background and is waiting for \
             you to decide."
                .into()
        }
        JobError::Other(_) => {
            "The download stopped for a reason oxdm doesn't have a better explanation for.".into()
        }
    }
}

/// Tone, list heading and recovery steps for the failures worth
/// spelling out. `None` for every other kind — those keep the plain
/// [`error_block`]. Shared with [`error_block_height`], so the window
/// that opens on this block is sized from the very copy it renders.
fn recovery_copy(err: &JobError) -> Option<(Tone, &'static str, &'static [&'static str])> {
    const TRY: &str = "what you can try";
    let picked: (Tone, &'static str, &'static [&'static str]) = match err {
        // The status class decides the advice: a 403 is never fixed by
        // waiting, and a 429 is never fixed by re-authenticating.
        JobError::HttpStatus { code, .. } => match code {
            401 | 407 => (
                Tone::Danger,
                TRY,
                &[
                    "Add the sign-in under Properties → Connection, then retry.",
                    "If the link came from a logged-in page, copy it again from that session.",
                    "Some hosts also need the referring page — set it under Properties.",
                ],
            ),
            403 => (
                Tone::Danger,
                TRY,
                &[
                    "Open the link in the browser you got it from — many hosts only serve it \
                     to that session.",
                    "Signed links expire; fetch a fresh one from the source.",
                    "Some hosts reject unknown clients — set a browser User-Agent under \
                     Settings → Network.",
                ],
            ),
            404 | 410 => (
                Tone::Danger,
                TRY,
                &[
                    "Check the address for typos, then open it in a browser.",
                    "The file may have moved or been taken down — look for a current link.",
                    "If the URL was copied from a page, copy it again; some are one-time.",
                ],
            ),
            429 => (
                Tone::Warning,
                TRY,
                &[
                    "Wait a few minutes before retrying — the limit is time-based.",
                    "Lower the connections per file under Properties → Connection.",
                    "Avoid running several downloads from this host at once.",
                ],
            ),
            500..=599 => (
                Tone::Danger,
                TRY,
                &[
                    "Retry in a few minutes — server-side failures are usually temporary.",
                    "Check the host's status page if it has one.",
                    "If it persists, the file may need to be fetched from a mirror.",
                ],
            ),
            _ => (
                Tone::Danger,
                TRY,
                &[
                    "Open the link in a browser to see what the server says.",
                    "Fetch a fresh link from the source if this one was generated for you.",
                ],
            ),
        },
        // Not a design variant: DNS is the one network failure where the
        // useful moves are specific (typo, VPN/custom resolver, fresh
        // domain) rather than "check your connection".
        JobError::Dns { .. } => (
            Tone::Danger,
            TRY,
            &[
                "Check the address for typos — the hostname did not resolve.",
                "Open the site in a browser; if that fails too, the problem is upstream of oxdm.",
                "On a VPN or a custom DNS server, switch it off and retry.",
                "A domain registered or moved in the last day or two may still be propagating.",
            ],
        ),
        JobError::NotResumable(_) => (
            Tone::Danger,
            TRY,
            &[
                "Get a fresh URL from the source — signed links expire.",
                "Check whether your sign-in or session is still valid.",
                "If the file changed on the server, restart from the beginning; the bytes \
                 already downloaded are discarded.",
            ],
        ),
        // Nothing is broken — the download simply cannot continue from
        // where it stopped, so the list says what restarting costs
        // rather than offering remedies.
        JobError::FileChanged(_) => (
            Tone::Warning,
            "what will happen",
            &[
                "This starts over from byte 0.",
                "Everything downloaded so far is fetched again.",
                "The file you would get now is not the file this download started.",
            ],
        ),
        JobError::DiskFull(_) => (
            Tone::Danger,
            TRY,
            &[
                "Free up space on the destination drive, then try again.",
                "Or save this download to a different folder — your progress carries over.",
                "Check that the drive isn't being unmounted or going to sleep.",
            ],
        ),
        JobError::PermissionDenied(_) => (
            Tone::Danger,
            TRY,
            &[
                "Check that you can write to the destination folder.",
                "Or save this download to a different folder — your progress carries over.",
                "If another program is holding the file open, close it and try again.",
            ],
        ),
        _ => return None,
    };
    Some(picked)
}

/// The failures the user can act on (design §3.3 severe-error
/// variants): a bulleted recovery list instead of the one-line hint,
/// and ochre when the only way forward is starting over.
///
/// Returns `None` for every other error kind — those keep the plain
/// [`error_block`].
pub fn error_recovery_block<'a, M: Clone + 'a>(
    t: &Tokens,
    err: &JobError,
    on_copy: M,
) -> Option<Element<'a, M>> {
    let (tone, title, items) = recovery_copy(err)?;
    let mut list = column![].spacing(theme::space::S1);
    for item in items {
        list = list.push(
            row![
                text("•")
                    .font(theme::BODY_BOLD)
                    .size(12.0)
                    .color(tone.fg(t)),
                text(*item)
                    .font(theme::BODY)
                    .size(12.0)
                    .color(t.fg_2)
                    .line_height(iced::widget::text::LineHeight::Relative(1.4)),
            ]
            .spacing(theme::space::S2),
        );
    }
    let checks = column![eyebrow(t, title), list]
        .spacing(theme::space::S1)
        .into();
    Some(panel_toned(t, err, tone, None, checks, on_copy))
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

/// Card tone. Design §3.3 draws the restart-required state in ochre:
/// nothing is broken, the download simply has to start over.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Danger,
    Warning,
}

impl Tone {
    fn fg(self, t: &Tokens) -> iced::Color {
        match self {
            Tone::Danger => t.status_danger,
            Tone::Warning => t.status_warning,
        }
    }
    fn bg(self, t: &Tokens) -> iced::Color {
        match self {
            Tone::Danger => t.status_danger_bg,
            Tone::Warning => t.status_warning_bg,
        }
    }
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
    panel_toned(t, err, Tone::Danger, head_action, checks, on_copy)
}

fn panel_toned<'a, M: Clone + 'a>(
    t: &Tokens,
    err: &JobError,
    tone: Tone,
    head_action: Option<Element<'a, M>>,
    checks: Element<'a, M>,
    on_copy: M,
) -> Element<'a, M> {
    let t2 = *t;
    let (tone_fg, tone_bg) = (tone.fg(t), tone.bg(t));
    let (icon_name, title, code, _) = error_meta(err);
    let detail = error_detail(err);

    let mut head = row![
        container(icons::icon(icon_name, 20.0, tone_fg))
            .width(Length::Fixed(ICON_TILE))
            .height(Length::Fixed(ICON_TILE))
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .style(move |_| container::Style {
                background: Some(tone_bg.into()),
                border: iced::Border {
                    radius: ICON_TILE_RADIUS.into(),
                    ..Default::default()
                },
                ..Default::default()
            }),
        column![
            text(title).font(theme::BODY_BOLD).size(14.0).color(tone_fg),
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
    //
    // The chip's line box is pinned to the glyphs (as `pills::chip`
    // does): iced's default line height reserves descender room that an
    // all-caps code never uses, so centring the *box* against the label
    // leaves the ink sitting high. Padding absorbs the difference.
    let code_chip = container(
        text(code)
            .font(theme::MONO)
            .size(CODE_TEXT)
            .line_height(iced::widget::text::LineHeight::Absolute(CODE_LINE.into()))
            .color(t2.fg_2),
    )
    .padding(iced::Padding {
        top: CODE_PAD_Y - CODE_INK_LIFT,
        right: CODE_PAD_X,
        bottom: CODE_PAD_Y + CODE_INK_LIFT,
        left: CODE_PAD_X,
    })
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
        // Same lift as the chip, applied as padding under the label so
        // the row's centring moves its ink up by half of it.
        container(
            text("Error code")
                .font(theme::BODY)
                .size(CODE_TEXT)
                .line_height(iced::widget::text::LineHeight::Absolute(CODE_LINE.into()))
                .color(t.fg_3),
        )
        .padding(iced::Padding {
            bottom: CODE_INK_LIFT * 2.0,
            ..iced::Padding::ZERO
        }),
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

    super::surface(
        tone_bg,
        tone_fg,
        theme::space::S3,
        column![head, hairline(t.border_subtle), checks, code_footer]
            .spacing(theme::space::S3)
            .into(),
    )
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
