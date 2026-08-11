//! Main-window overlay dialogs: About (with update flow), browser
//! extensions, Remove confirm, Conflict resolution, DB-error and
//! secrets-locked recovery. All render as centered modal layers over
//! the main view (the egui app used child viewports; one process =
//! in-window overlays here).

use iced::widget::{column, container, mouse_area, row, stack, text};
use iced::{Alignment, Element, Length};

use crate::data::ConflictKind;
use crate::gui::format::format_bytes;
use crate::gui::theme::{self, Tokens};
use crate::gui::widget::{Btn, BtnSize, checkbox, vscroll};
use crate::gui::{color, icons};

use super::main::{Main, Msg, RemoveKind};

// ---------------------------------------------------------------- states

#[derive(Clone)]
pub struct RemoveState {
    pub ids: Vec<crate::domain::JobId>,
    pub filename: String,
    pub completed: bool,
    /// Every selected job left a file on disk. Not the same as
    /// `completed`: a download that failed its integrity check has all
    /// of its bytes and a file to deal with, and refusing to offer that
    /// deletion leaves the user to find it by hand.
    pub has_files: bool,
    /// Destructive disposition pre-selected by the context-menu morph.
    pub kind: RemoveKind,
    pub delete_on_disk: bool,
    pub dont_ask_again: bool,
    /// Raised by the toolbar's Clean rather than by a selection: the
    /// set was assembled for the user, so the dialog says how many and
    /// the "don't ask again" answers for Clean alone.
    pub clean: bool,
}

impl RemoveState {
    /// Whether the removal has a finished file at stake. A download
    /// that failed its integrity check is not `Completed`, but every
    /// byte of it is on disk — for the question this dialog asks, it
    /// belongs with the finished ones, not with the half-transferred.
    pub fn finished(&self) -> bool {
        self.completed || self.has_files
    }
}

// ---------------------------------------------------------------- scaffolding

pub fn modal<'a>(
    t: &Tokens,
    base: Element<'a, Msg>,
    card: Element<'a, Msg>,
    width: f32,
    on_dismiss: Option<Msg>,
) -> Element<'a, Msg> {
    let t2 = *t;
    let boxed = container(card)
        .width(Length::Fixed(width))
        .padding(theme::space::S4)
        .style(move |_| container::Style {
            background: Some(t2.bg_surface.into()),
            border: iced::Border {
                color: t2.border_default,
                width: 1.0,
                radius: theme::surface::RADIUS.into(),
            },
            shadow: iced::Shadow {
                color: color::with_alpha(iced::Color::BLACK, 80.0 / 255.0),
                offset: iced::Vector::new(0.0, 4.0),
                blur_radius: 16.0,
            },
            ..Default::default()
        });

    // `opaque` swallows every event so nothing reaches the base
    // layer; the mouse_area on top of it turns a backdrop click into
    // dismiss (or a no-op for terminal modals).
    let scrim = iced::widget::opaque(
        mouse_area(
            container(iced::widget::Space::new())
                .width(Length::Fill)
                .height(Length::Fill)
                .style(|_| container::Style {
                    background: Some(color::with_alpha(iced::Color::BLACK, 120.0 / 255.0).into()),
                    ..Default::default()
                }),
        )
        .on_press(on_dismiss.unwrap_or(Msg::Noop)),
    );

    iced::widget::stack![
        base,
        scrim,
        container(iced::widget::opaque(boxed))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center),
    ]
    .into()
}

fn title_row<'a>(t: &Tokens, title: &str) -> Element<'a, Msg> {
    text(title.to_owned())
        .font(theme::BODY_BOLD)
        .size(14.0)
        .color(t.fg_1)
        .into()
}

/// `one` when a single entry is at stake, `many` otherwise. Copy that
/// says "the file" while three are selected is how a user deletes two
/// downloads they meant to keep.
fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        one.to_owned()
    } else {
        many.to_owned()
    }
}

// ---------------------------------------------------------------- remove

pub fn remove_confirm<'a>(m: &'a Main, base: Element<'a, Msg>) -> Element<'a, Msg> {
    let t = &m.tokens;
    let t2 = *t;
    // Defensive: the overlay is only shown with state present, but a
    // future edit could set `Overlay::Remove` without it — degrade to
    // the base view instead of panicking.
    let Some(st) = m.remove.as_ref() else {
        return base;
    };

    // Headline/accent/CTA morph with the pre-selected kind (B4: the
    // modifier picked the option, this dialog still confirms it).
    let n = st.ids.len();
    let (hero_icon, hero_color, message, cta_label, cta_icon): (
        &str,
        iced::Color,
        String,
        &str,
        &str,
    ) = match st.kind {
        RemoveKind::Trash => (
            "trash-2",
            color::ochre::O400,
            plural(
                n,
                "The file will be moved to your system Trash (recoverable).",
                &format!("All {n} files will be moved to your system Trash (recoverable)."),
            ),
            "Move to Trash",
            "trash-2",
        ),
        RemoveKind::Permanent => (
            "triangle-alert",
            color::rust::R300,
            plural(
                n,
                "The file will be permanently deleted from disk. This cannot be undone.",
                &format!(
                    "All {n} files will be permanently deleted from disk. This cannot be undone."
                ),
            ),
            "Delete permanently",
            "trash-2",
        ),
        RemoveKind::Entry if st.clean => (
            "trash-2",
            t.status_danger,
            "Clears every completed entry from the list. Files stay on disk.".to_owned(),
            "Clean",
            "trash-2",
        ),
        RemoveKind::Entry if st.finished() => (
            "triangle-alert",
            t.status_danger,
            plural(
                n,
                "This only removes the entry from oxdm.",
                &format!("This only removes the {n} entries from oxdm."),
            ),
            "Remove",
            "trash-2",
        ),
        RemoveKind::Entry => (
            "triangle-alert",
            t.status_danger,
            plural(
                n,
                "Partial (.part) files will be deleted from disk.",
                &format!("Partial (.part) files for all {n} will be deleted from disk."),
            ),
            "Remove",
            "trash-2",
        ),
    };
    let dont_label = if st.clean {
        "Don't ask again when cleaning"
    } else if st.finished() {
        "Don't ask again for completed downloads"
    } else {
        "Don't ask again for incomplete downloads"
    };

    let mut card = column![
        container(
            row![
                icons::icon(hero_icon, 22.0, hero_color),
                column![
                    text(st.filename.clone())
                        .font(theme::BODY_BOLD)
                        .size(13.0)
                        .color(t.fg_1),
                    text(message).font(theme::BODY).size(12.0).color(t.fg_2),
                ]
                .spacing(2.0),
            ]
            .spacing(theme::space::S3)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .padding(theme::space::S3)
        .style(move |_| container::Style {
            background: Some(t2.status_danger_bg.into()),
            border: iced::Border {
                color: t2.border_subtle,
                width: 1.0,
                radius: theme::surface::RADIUS.into(),
            },
            ..Default::default()
        }),
    ]
    .spacing(theme::space::S3);

    // On-disk delete toggle: meaningful only when EVERY selected entry
    // has a finished file (a mixed selection has partials, and there is
    // nothing to offer for those) and they aren't going to Trash, which
    // moves the files itself.
    if st.has_files && !st.clean && st.kind != RemoveKind::Trash {
        card = card.push(checkbox(
            t,
            &plural(
                n,
                "Also delete file on disk",
                &format!("Also delete all {n} files on disk"),
            ),
            st.delete_on_disk,
            true,
            Msg::RemoveDeleteOnDisk,
        ));
    }
    // "Don't ask again" only applies to the safe entry-only removal —
    // irreversible kinds always confirm (B4), so don't offer to skip it.
    if st.kind == RemoveKind::Entry {
        card = card.push(checkbox(
            t,
            dont_label,
            st.dont_ask_again,
            true,
            Msg::RemoveDontAsk,
        ));
    }
    card = card.push(
        row![
            iced::widget::Space::new().width(Length::Fill),
            Btn::new("Cancel")
                .ghost()
                .icon("x")
                .on_press(Msg::CloseOverlay)
                .view(t),
            Btn::new(cta_label)
                .danger_filled()
                .icon(cta_icon)
                .on_press(Msg::RemoveConfirm)
                .view(t),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center),
    );

    modal(t, base, card.into(), 440.0, Some(Msg::CloseOverlay))
}

// ---------------------------------------------------------------- conflict

pub fn conflict<'a>(m: &'a Main, base: Element<'a, Msg>) -> Element<'a, Msg> {
    let t = &m.tokens;
    let t2 = *t;
    let Some((id, kind, token)) = m.snap.conflict_head else {
        return base;
    };

    let (title, desc) = match kind {
        ConflictKind::FileChanged => (
            "The file on the server changed",
            "The partially-downloaded data no longer matches. Restart from scratch or abort.",
        ),
        ConflictKind::NotResumable => (
            "This download cannot be resumed",
            "The server does not support range requests. Restart from scratch or abort.",
        ),
        ConflictKind::SameDownloadExists => (
            "Same download already exists",
            "A download for this URL/file already exists.",
        ),
        ConflictKind::FinalFileExists => (
            "A file with this name already exists",
            "Replace it, keep both (numbered), or abort.",
        ),
        ConflictKind::UrlBroken => (
            "The link appears to be broken",
            "The URL no longer resolves.",
        ),
        ConflictKind::CredentialsInvalid => (
            "Credentials rejected",
            "The stored username/password were refused by the server.",
        ),
    };

    let hero = container(
        row![
            icons::icon("triangle-alert", 24.0, t.status_warning),
            column![
                text(title).font(theme::BODY_BOLD).size(14.0).color(t.fg_1),
                text(desc).font(theme::BODY).size(12.0).color(t.fg_2),
                text(format!("Job: {id}"))
                    .font(theme::MONO)
                    .size(11.0)
                    .color(t.fg_3),
            ]
            .spacing(2.0),
        ]
        .spacing(theme::space::S3)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding(theme::space::S3)
    .style(move |_| container::Style {
        background: Some(t2.status_warning_bg.into()),
        border: iced::Border {
            color: t2.border_subtle,
            width: 1.0,
            radius: theme::surface::RADIUS.into(),
        },
        ..Default::default()
    });

    use super::main::ConflictChoice::*;
    use Msg::Conflict as C;
    let buttons: Element<'a, Msg> = match kind {
        ConflictKind::FileChanged | ConflictKind::NotResumable => row![
            Btn::new("Restart")
                .primary()
                .icon("rotate-cw")
                .on_press(C(id, token, kind, Restart))
                .view(t),
            Btn::new("Abort")
                .danger_filled()
                .icon("x")
                .on_press(C(id, token, kind, Abort))
                .view(t),
        ]
        .spacing(theme::space::S2)
        .into(),
        ConflictKind::SameDownloadExists => row![
            Btn::new("Resume")
                .primary()
                .icon("play")
                .on_press(C(id, token, kind, Resume))
                .view(t),
            Btn::new("Number suffix")
                .secondary()
                .icon("plus")
                .on_press(C(id, token, kind, Numbered))
                .view(t),
            Btn::new("Abort")
                .danger_filled()
                .icon("x")
                .on_press(C(id, token, kind, Abort))
                .view(t),
        ]
        .spacing(theme::space::S2)
        .into(),
        ConflictKind::FinalFileExists => row![
            Btn::new("Replace")
                .primary()
                .icon("rotate-cw")
                .on_press(C(id, token, kind, Replace))
                .view(t),
            Btn::new("Number suffix")
                .secondary()
                .icon("plus")
                .on_press(C(id, token, kind, Numbered))
                .view(t),
            Btn::new("Abort")
                .danger_filled()
                .icon("x")
                .on_press(C(id, token, kind, Abort))
                .view(t),
        ]
        .spacing(theme::space::S2)
        .into(),
        ConflictKind::UrlBroken | ConflictKind::CredentialsInvalid => Btn::new("OK")
            .primary()
            .icon("check")
            .on_press(C(id, token, kind, Ack))
            .view(t),
    };

    let card = column![
        hero,
        row![iced::widget::Space::new().width(Length::Fill), buttons].align_y(Alignment::Center),
    ]
    .spacing(theme::space::S3);

    // Terminal modal — no backdrop dismiss.
    modal(t, base, card.into(), 520.0, None)
}

// ---------------------------------------------------------------- recovery

pub fn db_error<'a>(m: &'a Main, base: Element<'a, Msg>, error: &str) -> Element<'a, Msg> {
    let t = &m.tokens;
    let card = column![
        row![
            icons::icon("database", 20.0, t.status_danger),
            title_row(t, "Database problem"),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center),
        text(error.to_owned())
            .font(theme::BODY)
            .size(12.0)
            .color(t.fg_2),
        text(
            "Resetting deletes the download list and every unfinished download \
             (completed files are kept). A copy of the damaged database is left \
             behind."
        )
        .font(theme::BODY)
        .size(11.0)
        .color(t.fg_3),
        row![
            iced::widget::Space::new().width(Length::Fill),
            Btn::new("Exit").ghost().on_press(Msg::DbExit).view(t),
            Btn::new("Reset database")
                .danger_filled()
                .icon("trash-2")
                .on_press(Msg::DbReset)
                .view(t),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center),
    ]
    .spacing(theme::space::S3);
    modal(t, base, card.into(), 460.0, None)
}

// -------------------------------------------------- restart confirm

/// "Start over?" — asked before a restart because every byte already
/// fetched is thrown away, and for a download that finished, the file
/// on disk goes with it. The same question the download window's own
/// Restart puts, in the same words.
pub fn restart_confirm<'a>(m: &'a Main, base: Element<'a, Msg>) -> Element<'a, Msg> {
    let t = &m.tokens;
    let n = m.restart_ids.len();
    let what = if n == 1 {
        m.snap
            .jobs
            .iter()
            .find(|j| j.id == m.restart_ids[0])
            .and_then(|j| j.filename.clone())
            .unwrap_or_else(|| "This download".to_owned())
    } else {
        format!("{n} downloads")
    };
    let bytes: u64 = m
        .snap
        .jobs
        .iter()
        .filter(|j| m.restart_ids.contains(&j.id))
        .map(|j| j.status.downloaded)
        .sum();
    let already = if bytes > 0 {
        format!(" The {} already fetched is discarded.", format_bytes(bytes))
    } else {
        String::new()
    };

    let card = column![
        row![
            icons::icon("rotate-cw", 20.0, t.action_primary),
            title_row(
                t,
                &plural(
                    n,
                    "Start this download over?",
                    "Start these downloads over?"
                )
            ),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center),
        text(format!(
            "{what} is deleted from your disk if it finished, and fetched again from the \
             beginning.{already}"
        ))
        .font(theme::BODY)
        .size(12.0)
        .color(t.fg_2),
        row![
            iced::widget::Space::new().width(Length::Fill),
            Btn::new("Cancel")
                .ghost()
                .on_press(Msg::CloseOverlay)
                .view(t),
            Btn::new("Restart download")
                .primary()
                .icon("rotate-cw")
                .on_press(Msg::RestartConfirmed)
                .view(t),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center),
    ]
    .spacing(theme::space::S3);
    modal(t, base, card.into(), 440.0, Some(Msg::CloseOverlay))
}

// -------------------------------------------------- remove warning

/// Shown after a removal that could not take a file with it. The
/// entries are already gone — this is a report, not a choice, so the
/// only action is to acknowledge it.
pub fn remove_warning<'a>(m: &'a Main, base: Element<'a, Msg>) -> Element<'a, Msg> {
    let t = &m.tokens;
    let t2 = *t;
    let n = m.remove_problems.len();
    let headline = if n == 1 {
        "A file could not be deleted".to_owned()
    } else {
        format!("{n} files could not be deleted")
    };

    let mut list: iced::widget::Column<'_, Msg> = column![].spacing(theme::space::S2);
    for p in &m.remove_problems {
        list = list.push(
            text(p.clone())
                .font(theme::MONO)
                .size(11.0)
                .color(t.fg_2)
                .wrapping(text::Wrapping::WordOrGlyph),
        );
    }

    let card = column![
        row![
            icons::icon("triangle-alert", 20.0, t.status_warning),
            title_row(t, &headline),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center),
        text(
            "The download was removed from the list, but the file is still on \
             disk. It may be open in another program, on a read-only or \
             disconnected volume, or owned by another user."
        )
        .font(theme::BODY)
        .size(12.0)
        .color(t.fg_2),
        container(vscroll(Element::from(list)))
            .max_height(160.0)
            .width(Length::Fill)
            .padding(theme::space::S3)
            .style(move |_| container::Style {
                background: Some(t2.bg_sunken.into()),
                border: iced::Border {
                    color: t2.border_subtle,
                    width: 1.0,
                    radius: theme::surface::RADIUS.into(),
                },
                ..Default::default()
            }),
        row![
            iced::widget::Space::new().width(Length::Fill),
            Btn::new("Close")
                .primary()
                .on_press(Msg::CloseOverlay)
                .view(t),
        ]
        .align_y(Alignment::Center),
    ]
    .spacing(theme::space::S3);
    modal(t, base, card.into(), 460.0, Some(Msg::CloseOverlay))
}

// ------------------------------------------------------ browser extensions

/// `radial-gradient(ellipse at center, <tint> 0%, transparent 70%)`,
/// as an SVG because that is the one renderer in the stack that can
/// draw it. Stretched to the band, so the ellipse follows its width.
fn radial_wash(tint: iced::Color) -> String {
    let hex = format!(
        "#{:02X}{:02X}{:02X}",
        (tint.r * 255.0).round() as u8,
        (tint.g * 255.0).round() as u8,
        (tint.b * 255.0).round() as u8,
    );
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 100 100\" \
         preserveAspectRatio=\"none\">\
         <defs><radialGradient id=\"g\" cx=\"50%\" cy=\"50%\" r=\"70%\">\
         <stop offset=\"0%\" stop-color=\"{hex}\" stop-opacity=\"1\"/>\
         <stop offset=\"70%\" stop-color=\"{hex}\" stop-opacity=\"0\"/>\
         </radialGradient></defs>\
         <rect width=\"100\" height=\"100\" fill=\"url(#g)\"/></svg>"
    )
}

/// The art band: the 72px glyphs plus the room the wash needs to fade
/// out in.
const HERO_BAND_H: f32 = 104.0;

/// The browser glyph in the hero art (design `.fr-browser-glyph`).
const GLYPH_RADIUS: f32 = 10.0;
const GLYPH_BORDER: f32 = 2.0;

// Each vendor's extension store landing page (design §3.8). We do NOT
// fake an "Installed ✓" state — there is no reliable detection — so the
// button always reads "Open store page". Only the two stores the
// extension is actually published to: a row per Chromium repackaging
// pointed six of them at the same Chrome Web Store page, which is a
// longer list saying the same thing twice.
const BROWSER_STORES: [(&str, &str, &str); 2] = [
    (
        "Chrome",
        "Chrome Web Store",
        "https://chromewebstore.google.com/",
    ),
    (
        "Firefox",
        "Firefox Add-ons",
        "https://addons.mozilla.org/firefox/",
    ),
];

/// Browser-extensions dialog, "manage" mode (opened from Tools).
pub fn browser_extensions<'a>(m: &'a Main, base: Element<'a, Msg>) -> Element<'a, Msg> {
    extensions_dialog(m, base, false)
}

/// First-run welcome overlay (design §3.8 / `first-run-dialog.jsx`
/// `welcome` mode): same honest body, plus the "Welcome to oxdm"
/// heading and the "Maybe later" / "Done" footer. Every dismissal
/// routes through `Msg::WelcomeDismiss` so `first_run_seen` persists.
pub fn welcome<'a>(m: &'a Main, base: Element<'a, Msg>) -> Element<'a, Msg> {
    extensions_dialog(m, base, true)
}

fn extensions_dialog<'a>(
    m: &'a Main,
    base: Element<'a, Msg>,
    welcome_mode: bool,
) -> Element<'a, Msg> {
    let t = &m.tokens;
    let t2 = *t;

    // Sub-copy: welcome uses the jsx `.fr-sub` wording; manage keeps
    // the shorter helper line.
    let sub = if welcome_mode {
        "Install the oxdm extension and every download started in your browser \
         comes here automatically: segmented, resumable, and queued."
    } else {
        "Install the oxdm helper extension to send links straight to oxdm."
    };

    // Hero band (design `.fr-hero`): the flow the extension creates,
    // drawn rather than described — a browser window, an arrow, and
    // the app's own mark — over a clay wash, with the copy beneath it.
    let tile_bg = color::mix(t.bg_surface, t.action_primary, 0.20);

    // `.fr-browser-glyph`: a 96×72 window with a chrome bar of three
    // dots and a download arrow in the page. Deliberately no vendor's
    // browser: this stands for whichever one the user has.
    let dot = |c: iced::Color| {
        container(iced::widget::Space::new())
            .width(Length::Fixed(5.0))
            .height(Length::Fixed(5.0))
            .style(move |_| container::Style {
                background: Some(c.into()),
                border: iced::Border {
                    radius: theme::radius::PILL.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
    };
    // The chrome bar in the theme's own warm ramp: the design's
    // earth-100 strip is right on a near-white page and reads as a
    // light bar taped to a dark dialog anywhere else.
    let (bar_bg, dot_color) = match t.theme {
        crate::gui::theme::ResolvedTheme::Dark => (color::earth::E700, color::earth::E500),
        _ => (color::earth::E100, color::earth::E300),
    };
    let browser_glyph = container(
        column![
            container(
                row![dot(dot_color), dot(dot_color), dot(dot_color)]
                    .spacing(4.0)
                    .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .height(Length::Fixed(14.0))
            .align_y(Alignment::Center)
            .padding(iced::Padding {
                left: 6.0,
                ..Default::default()
            })
            .style(move |_| container::Style {
                background: Some(bar_bg.into()),
                // The chrome bar carries the window's own top corners.
                // A child's background is painted square regardless of
                // the parent's radius, so without this the bar's fill
                // squares off the two corners it sits in.
                border: iced::Border {
                    radius: iced::border::radius(0.0)
                        .top_left(GLYPH_RADIUS - GLYPH_BORDER)
                        .top_right(GLYPH_RADIUS - GLYPH_BORDER),
                    ..Default::default()
                },
                ..Default::default()
            }),
            container(icons::icon("download", 28.0, t.action_primary))
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Alignment::Center)
                .align_y(Alignment::Center),
        ]
        .spacing(0.0),
    )
    .width(Length::Fixed(96.0))
    .height(Length::Fixed(72.0))
    .clip(true)
    .style(move |_| container::Style {
        background: Some(t2.bg_surface.into()),
        border: iced::Border {
            color: t2.border_default,
            width: GLYPH_BORDER,
            radius: GLYPH_RADIUS.into(),
        },
        ..Default::default()
    });

    // The design draws the arrow as a 56px stroke; an icon at the same
    // width is the same picture without a canvas for one line.
    let art = row![
        browser_glyph,
        // 56×20, the design's own proportions. `icons::icon` is square
        // by construction, so this one goes through the svg widget:
        // a long thin arrow is the whole point of the drawing, and an
        // arrow glyph scaled to 56px carries a 4px stroke with it.
        iced::widget::svg(iced::widget::svg::Handle::from_memory(
            icons::raw_svg("flow-arrow").unwrap_or_default(),
        ))
        .width(Length::Fixed(56.0))
        .height(Length::Fixed(20.0))
        .style(move |_, _| iced::widget::svg::Style {
            color: Some(t2.action_primary),
        }),
        // The mark the tray and the About window use, at the design's
        // 64px. The dialog says "your downloads end up *here*", so the
        // "here" has to be the app's real face.
        container(crate::gui::widget::app_mark(t, 64.0))
            .width(Length::Fixed(72.0))
            .height(Length::Fixed(72.0))
            .align_x(Alignment::Center)
            .align_y(Alignment::Center),
    ]
    .spacing(18.0)
    .align_y(Alignment::Center);

    // `.fr-hero` is not a card: the art sits on the dialog's own
    // background under a radial clay wash. iced draws linear gradients
    // only, so the wash is an SVG — resvg has radial gradients, and one
    // rect is cheaper than faking the falloff with stacked shapes.
    let hero = container(
        column![
            // A band, not a fill: `stack` takes the height of its
            // tallest layer, and a wash asking for Fill would grow to
            // whatever the dialog had left and push the copy off the
            // bottom.
            stack![
                iced::widget::svg(iced::widget::svg::Handle::from_memory(
                    radial_wash(color::mix(t.bg_page, t.action_primary, 0.20)).into_bytes(),
                ))
                .width(Length::Fill)
                .height(Length::Fixed(HERO_BAND_H))
                .content_fit(iced::ContentFit::Fill),
                container(art)
                    .width(Length::Fill)
                    .height(Length::Fixed(HERO_BAND_H))
                    .align_x(Alignment::Center)
                    .align_y(Alignment::Center),
            ],
            text("Capture downloads from your browser")
                .font(theme::DISPLAY)
                .size(22.0)
                .color(t.fg_1)
                .width(Length::Fill)
                .align_x(Alignment::Center),
            container(
                text(sub)
                    .font(theme::BODY)
                    .size(13.0)
                    .color(t.fg_2)
                    .width(Length::Fill)
                    .align_x(Alignment::Center)
                    .wrapping(text::Wrapping::WordOrGlyph),
            )
            .max_width(420.0),
        ]
        // Fill, not shrink: a column sized to its widest child centres
        // its rows against *itself* and then sits at the card's left
        // edge, which reads as a left-aligned hero with an odd indent.
        .width(Length::Fill)
        .spacing(theme::space::S2)
        .align_x(Alignment::Center),
    )
    .width(Length::Fill)
    .padding(iced::Padding {
        top: 8.0,
        right: theme::space::S4,
        bottom: theme::space::S3,
        left: theme::space::S4,
    });

    let mut list = column![].spacing(6.0);
    for (name, store, url) in BROWSER_STORES {
        let mark = container(
            text(name.chars().next().unwrap_or('?').to_string())
                .font(theme::DISPLAY)
                .size(15.0)
                .color(t.action_primary),
        )
        .width(Length::Fixed(32.0))
        .height(Length::Fixed(32.0))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(move |_| container::Style {
            background: Some(tile_bg.into()),
            border: iced::Border {
                radius: theme::radius::SM.into(),
                ..Default::default()
            },
            ..Default::default()
        });
        let r = row![
            mark,
            column![
                text(name).font(theme::BODY_BOLD).size(13.0).color(t.fg_1),
                text(store).font(theme::MONO).size(10.5).color(t.fg_3),
            ]
            .spacing(2.0),
            iced::widget::Space::new().width(Length::Fill),
            Btn::new("Open store page")
                .secondary()
                .size(BtnSize::Sm)
                .icon("globe")
                .on_press(Msg::OpenStore(url))
                .view(t),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center);
        list = list.push(
            container(r)
                .width(Length::Fill)
                .padding([9.0, 12.0])
                .style(move |_| container::Style {
                    background: Some(t2.bg_raised.into()),
                    border: iced::Border {
                        color: t2.border_subtle,
                        width: 1.0,
                        radius: theme::radius::SM.into(),
                    },
                    ..Default::default()
                }),
        );
    }

    let privacy = row![
        icons::icon("shield", 14.0, t.status_success),
        text("The extension reads only download URLs, never page content or browsing history.")
            .font(theme::BODY)
            .size(11.0)
            .color(t.fg_3),
    ]
    .spacing(6.0)
    .align_y(Alignment::Center);

    // Footer adapts per §3.8: welcome → "Maybe later" / "Done"
    // (both persist `first_run_seen`); manage → just "Close". No fake
    // "Installed" state — installs can't be detected.
    let footer: Element<'a, Msg> = if welcome_mode {
        row![
            Btn::new("Maybe later")
                .ghost()
                .on_press(Msg::WelcomeDismiss)
                .view(t),
            iced::widget::Space::new().width(Length::Fill),
            Btn::new("Done")
                .primary()
                .icon("check")
                .on_press(Msg::WelcomeDismiss)
                .view(t),
        ]
        .align_y(Alignment::Center)
        .into()
    } else {
        row![
            iced::widget::Space::new().width(Length::Fill),
            Btn::new("Close")
                .ghost()
                .on_press(Msg::CloseOverlay)
                .view(t),
        ]
        .align_y(Alignment::Center)
        .into()
    };

    let mut card = column![].spacing(theme::space::S3);
    if welcome_mode {
        card = card.push(title_row(t, "Welcome to oxdm"));
    }
    let card = card
        .push(hero)
        // Two rows fit as they are; the scroller was sized for a list
        // that no longer exists, and left a hand's width of empty
        // panel under them.
        .push(list)
        .push(privacy)
        .push(footer);

    let dismiss = if welcome_mode {
        Msg::WelcomeDismiss
    } else {
        Msg::CloseOverlay
    };
    modal(t, base, card.into(), 560.0, Some(dismiss))
}

pub fn secrets_locked<'a>(m: &'a Main, base: Element<'a, Msg>) -> Element<'a, Msg> {
    let t = &m.tokens;
    let card = column![
        row![
            icons::icon("key", 20.0, t.status_warning),
            title_row(t, "Secrets locked"),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center),
        text(
            "The encryption key for stored passwords/cookies is unavailable (OS keyring \
             changed?). You can wipe stored job secrets and continue. Downloads themselves \
             are not affected."
        )
        .font(theme::BODY)
        .size(12.0)
        .color(t.fg_2),
        row![
            iced::widget::Space::new().width(Length::Fill),
            Btn::new("Exit").ghost().on_press(Msg::DbExit).view(t),
            Btn::new("Wipe and continue")
                .danger_filled()
                .icon("trash-2")
                .on_press(Msg::SecretsWipe)
                .view(t),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center),
    ]
    .spacing(theme::space::S3);
    modal(t, base, card.into(), 480.0, None)
}

/// "This did not start, and here is why." The daemon refuses before it
/// touches anything — no partial file, no half-started queue — so this
/// reports a decision rather than a failure.
pub fn refused<'a>(m: &'a Main, base: Element<'a, Msg>) -> Element<'a, Msg> {
    let t = &m.tokens;
    let reason = m.refusal.clone().unwrap_or_default();

    let card = column![
        row![
            icons::icon("triangle-alert", 20.0, t.status_warning),
            title_row(t, "Nothing was started"),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center),
        text(reason)
            .font(theme::BODY)
            .size(12.0)
            .color(t.fg_2)
            .wrapping(text::Wrapping::WordOrGlyph),
        text(
            "Free up room, or send this download to a folder on another drive from \
             Properties → General. A file is assembled from its parts, so the cache \
             folder and the save folder both need room for it while it finishes."
        )
        .font(theme::BODY)
        .size(11.5)
        .color(t.fg_3)
        .wrapping(text::Wrapping::WordOrGlyph),
        row![
            iced::widget::Space::new().width(Length::Fill),
            Btn::new("Close")
                .primary()
                .on_press(Msg::CloseOverlay)
                .view(t),
        ]
        .align_y(Alignment::Center),
    ]
    .spacing(theme::space::S3);
    modal(t, base, card.into(), 460.0, Some(Msg::CloseOverlay))
}

// ---------------------------------------------------- watch limit

/// A kernel limit stopped oxdm watching the download folders.
///
/// Nothing about the downloads themselves is wrong, so this is not a
/// recovery dialog: it names the one thing that stopped working, shows
/// the exact change that restores it, and offers to make it. The
/// button is only shown where it can work — elsewhere the command
/// stands on its own, copyable.
pub fn watch_limit<'a>(m: &'a Main, base: Element<'a, Msg>) -> Element<'a, Msg> {
    let t = &m.tokens;
    let Some(limit) = m.watch_limit.as_ref() else {
        return base;
    };

    let mut card = column![
        row![
            icons::icon("eye-off", 20.0, t.status_warning),
            title_row(t, "Not watching your download folders"),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center),
        text(limit.kind.consequence())
            .font(theme::BODY)
            .size(12.0)
            .color(t.fg_2)
            .wrapping(text::Wrapping::WordOrGlyph),
        text(match limit.current {
            Some(n) => format!(
                "Your system allows {n} of these at a time ({}), and they are all in \
                 use — usually by a browser, which takes one per tab process, or an \
                 editor watching a large project.",
                limit.kind.sysctl_key()
            ),
            None => format!(
                "The system limit {} is used up — usually by a browser, which takes \
                 one per tab process, or an editor watching a large project.",
                limit.kind.sysctl_key()
            ),
        })
        .font(theme::BODY)
        .size(11.5)
        .color(t.fg_3)
        .wrapping(text::Wrapping::WordOrGlyph),
    ]
    .spacing(theme::space::S3);

    // The change itself, in the open: it is a system-wide setting, and
    // a dialog that asks for a password without saying what it will run
    // is asking for trust it has not earned.
    if let Some(line) = limit.sysctl_line() {
        card = card.push(
            column![
                row![
                    text(
                        "Raising it writes this to /etc/sysctl.d/90-oxdm-inotify.conf \
                          and applies it now:"
                    )
                    .font(theme::BODY)
                    .size(11.0)
                    .color(t.fg_3)
                    .wrapping(text::Wrapping::WordOrGlyph),
                ],
                container(
                    row![
                        text(line)
                            .font(theme::MONO)
                            .size(11.0)
                            .color(t.fg_1)
                            .wrapping(text::Wrapping::WordOrGlyph),
                        iced::widget::Space::new().width(Length::Fill),
                        crate::gui::widget::copy::copy_btn(
                            "",
                            m.watch_limit_copied,
                            Msg::WatchLimitCopy
                        )
                        .toolbar()
                        .size(BtnSize::Sm)
                        .view(t),
                    ]
                    .spacing(theme::space::S2)
                    .align_y(Alignment::Center)
                )
                .width(Length::Fill)
                .padding([8.0, 10.0])
                .style(move |_| container::Style {
                    background: Some(t.bg_page.into()),
                    border: iced::Border {
                        color: t.border_subtle,
                        width: 1.0,
                        radius: theme::radius::SM.into(),
                    },
                    ..Default::default()
                }),
            ]
            .spacing(theme::space::S2),
        );
    }

    if let Some(err) = m.watch_limit_error.as_deref() {
        card = card.push(
            text(err)
                .font(theme::BODY)
                .size(11.5)
                .color(t.status_danger)
                .wrapping(text::Wrapping::WordOrGlyph),
        );
    }

    let can_raise = limit.suggested.is_some() && crate::platform::watch_limit::can_raise();
    let mut actions = row![
        Btn::new("Don't warn again")
            .ghost()
            .on_press(Msg::WatchLimitNever)
            .view(t),
        iced::widget::Space::new().width(Length::Fill),
        Btn::new("Not now")
            .ghost()
            .on_press(Msg::CloseOverlay)
            .view(t),
    ]
    .spacing(theme::space::S2)
    .align_y(Alignment::Center);
    if can_raise {
        actions = actions.push(
            Btn::new("Raise the limit…")
                .primary()
                .icon("shield-check")
                .on_press(Msg::WatchLimitRaise)
                .view(t),
        );
    }
    card = card.push(actions);
    modal(t, base, card.into(), 520.0, Some(Msg::CloseOverlay))
}
