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

    /// Whether this removal takes the file with it.
    ///
    /// The single place that question is answered, because the answer
    /// has to hold on every path into a removal, not only the one the
    /// dialog draws. Deleting a file is a per-answer choice and never a
    /// stored one: an answer being remembered ("don't ask again") can
    /// only ever mean "take the entry off the list", so it cancels the
    /// disk deletion even if some other path set the flag. Trash is
    /// excluded because it moved the file itself, and deleting after
    /// that would empty the Trash behind the user.
    pub fn deletes_file(&self) -> bool {
        self.has_files
            && self.delete_on_disk
            && !self.dont_ask_again
            && self.kind != RemoveKind::Trash
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

/// The round tinted disc the design puts a confirm dialog's icon in
/// (`.cd-icon`: 36px, danger-tinted, danger-coloured glyph).
fn confirm_disc<'a>(t: &Tokens, name: &'a str) -> Element<'a, Msg> {
    let t2 = *t;
    container(icons::icon(name, 18.0, t.status_danger))
        .width(Length::Fixed(36.0))
        .height(Length::Fixed(36.0))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(move |_| container::Style {
            background: Some(t2.status_danger_bg.into()),
            border: iced::Border {
                radius: 18.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

/// A sentence with the download's own name set in bold, the way the
/// design writes every confirm body. The name is the thing the user
/// checks before answering, so it carries the weight.
fn named_body<'a>(t: &Tokens, name: &str, rest: &str) -> Element<'a, Msg> {
    iced::widget::rich_text::<(), Msg, _, _>([
        iced::widget::span(name.to_owned())
            .font(theme::BODY_BOLD)
            .color(t.fg_1),
        iced::widget::span(rest.to_owned()),
    ])
    .font(theme::BODY)
    .size(12.0)
    .line_height(1.5)
    .color(t.fg_2)
    .into()
}

pub fn remove_confirm<'a>(m: &'a Main, base: Element<'a, Msg>) -> Element<'a, Msg> {
    let t = &m.tokens;
    // Defensive: the overlay is only shown with state present, but a
    // future edit could set `Overlay::Remove` without it — degrade to
    // the base view instead of panicking.
    let Some(st) = m.remove.as_ref() else {
        return base;
    };

    // Headline/icon/body/CTA morph with the pre-selected kind (B4: the
    // modifier picked the option, this dialog still confirms it). The
    // body always names what is at stake and what happens to the bytes,
    // in that order, because that is the question being answered.
    let n = st.ids.len();
    let (icon_name, title, body, cta_label): (&str, String, Element<'a, Msg>, &str) = match st.kind
    {
        RemoveKind::Trash => (
            "trash-2",
            plural(n, "Move to Trash?", &format!("Move {n} files to Trash?")),
            named_body(
                t,
                &st.filename,
                &plural(
                    n,
                    " moves to your system Trash and leaves the list. You can put it back \
                     from there.",
                    " move to your system Trash and leave the list. You can put them back \
                     from there.",
                ),
            ),
            "Move to Trash",
        ),
        RemoveKind::Permanent => (
            "triangle-alert",
            plural(
                n,
                "Delete permanently?",
                &format!("Delete {n} files permanently?"),
            ),
            named_body(
                t,
                &st.filename,
                &plural(
                    n,
                    " is deleted from disk and leaves the list. This cannot be undone, and \
                     it does not go to the Trash.",
                    " are deleted from disk and leave the list. This cannot be undone, and \
                     they do not go to the Trash.",
                ),
            ),
            "Delete permanently",
        ),
        // Clean speaks in counts, not names: the title says how many
        // and the body says what survives it.
        RemoveKind::Entry if st.clean => (
            "trash-2",
            plural(
                n,
                "Clear 1 finished download?",
                &format!("Clear {n} finished downloads?"),
            ),
            text("This clears finished downloads from the list. The files stay on disk.")
                .font(theme::BODY)
                .size(12.0)
                .line_height(1.5)
                .color(t.fg_2)
                .into(),
            "Clear list",
        ),
        // The file is still there. Whether it stays is the checkbox
        // below, so the sentence points at it rather than promising.
        RemoveKind::Entry if st.has_files => (
            "trash-2",
            plural(
                n,
                "Remove from list?",
                &format!("Remove {n} downloads from the list?"),
            ),
            named_body(
                t,
                &st.filename,
                &plural(
                    n,
                    " leaves the list. The file stays on disk unless you also delete it below.",
                    " leave the list. The files stay on disk unless you also delete them below.",
                ),
            ),
            "Remove",
        ),
        RemoveKind::Entry if st.finished() => (
            "trash-2",
            plural(
                n,
                "Remove from list?",
                &format!("Remove {n} downloads from the list?"),
            ),
            named_body(
                t,
                &st.filename,
                &plural(
                    n,
                    " leaves the list. Nothing on disk is touched.",
                    " leave the list. Nothing on disk is touched.",
                ),
            ),
            "Remove",
        ),
        RemoveKind::Entry => (
            "trash-2",
            plural(
                n,
                "Remove from list?",
                &format!("Remove {n} downloads from the list?"),
            ),
            named_body(
                t,
                &st.filename,
                &plural(
                    n,
                    " leaves the list, and the partly downloaded data is discarded.",
                    " leave the list, and the partly downloaded data is discarded.",
                ),
            ),
            "Remove",
        ),
    };
    let dont_label = if st.clean {
        "Don't ask again when cleaning"
    } else if st.finished() {
        "Don't ask again for completed downloads"
    } else {
        "Don't ask again for incomplete downloads"
    };

    let mut said = column![
        text(title).font(theme::DISPLAY).size(14.0).color(t.fg_1),
        body,
    ]
    .spacing(5.0);

    // On-disk delete toggle: meaningful only when EVERY selected entry
    // has a finished file (a mixed selection has partials, and there is
    // nothing to offer for those) and they aren't going to Trash, which
    // moves the files itself.
    let mut checks = column![].spacing(6.0);
    let mut any_check = false;
    if st.has_files && !st.clean && st.kind != RemoveKind::Trash {
        any_check = true;
        checks = checks.push(checkbox(
            t,
            plural(
                n,
                "Also delete file on disk",
                &format!("Also delete all {n} files on disk"),
            ),
            st.delete_on_disk,
            // Off the table once the answer is being remembered: what
            // gets stored is "remove the entry", never "and delete the
            // file", so the box cannot be left ticked under a
            // preference that will not carry it.
            !st.dont_ask_again,
            Msg::RemoveDeleteOnDisk,
        ));
    }
    // "Don't ask again" only applies to the safe entry-only removal —
    // irreversible kinds always confirm (B4), so don't offer to skip it.
    if st.kind == RemoveKind::Entry {
        any_check = true;
        checks = checks.push(checkbox(
            t,
            dont_label,
            st.dont_ask_again,
            true,
            Msg::RemoveDontAsk,
        ));
    }
    if any_check {
        said = said.push(container(checks).padding(iced::Padding {
            top: 10.0,
            ..iced::Padding::ZERO
        }));
    }

    let card = column![
        row![confirm_disc(t, icon_name), said]
            .spacing(14.0)
            .align_y(Alignment::Start),
        row![
            iced::widget::Space::new().width(Length::Fill),
            Btn::new("Cancel")
                .ghost()
                .icon("x")
                .on_press(Msg::CloseOverlay)
                .view(t),
            Btn::new(cta_label)
                .danger()
                .icon("trash-2")
                .on_press(Msg::RemoveConfirm)
                .view(t),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center),
    ]
    .spacing(14.0);

    modal(t, base, card.into(), 420.0, Some(Msg::CloseOverlay))
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

/// The art band: the 72px glyphs plus the room the wash needs to fade
/// out in. Much taller than the art on purpose — the glow reads as
/// light on the page only if it has room to let go in; boxed tightly
/// around the artwork it reads as a smudge behind it.
const HERO_BAND_H: f32 = 156.0;

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
        "https://chromewebstore.google.com/detail/oxdm-download-manager-int/bfefefnlghppdcgjjimkllklpifkcokj",
    ),
    (
        "Firefox",
        "Firefox Add-ons",
        "https://addons.mozilla.org/addon/oxdm-download-manager-bridge/",
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
    let (ledge, hairline_c) = match t.theme {
        crate::gui::theme::ResolvedTheme::Dark => (color::earth::E900, color::earth::E600),
        _ => (color::earth::E300, t.border_subtle),
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
                // Top corners only: the bar meets the window's frame
                // above and a straight rule below. A child's background
                // is painted square whatever the parent's radius, so
                // without this it squares off the corners it sits in.
                border: iced::Border {
                    radius: iced::border::radius(0.0)
                        .top_left(GLYPH_RADIUS - GLYPH_BORDER)
                        .top_right(GLYPH_RADIUS - GLYPH_BORDER),
                    ..Default::default()
                },
                ..Default::default()
            }),
            // `border-bottom: 1px` on the bar — iced borders are
            // all-or-nothing, so the one side that exists is a rule.
            container(iced::widget::Space::new())
                .width(Length::Fill)
                .height(Length::Fixed(1.0))
                .style(move |_| container::Style {
                    background: Some(hairline_c.into()),
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
    // The frame is drawn on the bounds, not around them: without this
    // pad the bar covers the two pixels of border it should sit inside.
    .padding(GLYPH_BORDER)
    .clip(true)
    .style(move |_| container::Style {
        background: Some(t2.bg_surface.into()),
        border: iced::Border {
            color: t2.border_default,
            width: GLYPH_BORDER,
            radius: GLYPH_RADIUS.into(),
        },
        // The design's `0 2px 0` ledge: a hard offset, no blur, which
        // is what gives the little window its weight.
        shadow: iced::Shadow {
            color: ledge,
            offset: iced::Vector::new(0.0, 2.0),
            blur_radius: 0.0,
        },
        ..Default::default()
    });

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
            // bottom. The glow is drawn as pixels rather than an SVG —
            // see `widget::wash`.
            stack![
                iced::widget::image(crate::gui::widget::wash::radial(color::mix(
                    t.bg_page,
                    t.action_primary,
                    0.22,
                )))
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
        // No mark: a browser's own logo is theirs, and a tile with the
        // first letter of a name that is written beside it in full is
        // a placeholder standing in for nothing.
        let r = row![
            column![
                text(name).font(theme::BODY_BOLD).size(13.0).color(t.fg_1),
                text(store).font(theme::MONO).size(10.5).color(t.fg_3),
            ]
            .spacing(2.0),
            iced::widget::Space::new().width(Length::Fill),
            // The design's `.fr-install-btn`: a quiet bordered button
            // with a clay label. The icon says it leaves the app, which
            // is the honest promise — oxdm cannot install anything, and
            // cannot tell whether the extension is already there.
            Btn::new("Install")
                .secondary()
                .accent(true)
                .size(BtnSize::Sm)
                .font_size(11.5)
                // The design's 5/11 padding around an 11.5px label,
                // which lands between the Sm and Md heights.
                .pad_x(11.0)
                .height(26.0)
                .icon("external-link")
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
        iced::widget::Space::new().width(Length::Fill),
        icons::icon("shield", 14.0, t.status_success),
        text(
            "The extension only acts on download URLs. It never stores or transmits \
             anything about the pages you visit.",
        )
        .font(theme::BODY)
        .size(11.0)
        .color(t.fg_3)
        .align_x(Alignment::Center),
        iced::widget::Space::new().width(Length::Fill),
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
                .primary()
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
    // Wider than the design's 560: the privacy line is one sentence
    // and reads as one, and at 560 it wrapped mid-clause. Still well
    // inside the 900px minimum window width.
    modal(t, base, card.into(), 500.0, Some(dismiss))
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
                 use, usually by a browser, which takes one per tab process, or an \
                 editor watching a large project.",
                limit.kind.sysctl_key()
            ),
            None => format!(
                "The system limit {} is used up, usually by a browser, which takes \
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

#[cfg(test)]
mod tests {
    use super::*;

    fn state(kind: RemoveKind) -> RemoveState {
        RemoveState {
            ids: vec![crate::domain::JobId::new()],
            filename: "f.bin".into(),
            completed: true,
            has_files: true,
            kind,
            delete_on_disk: true,
            dont_ask_again: false,
            clean: false,
        }
    }

    #[test]
    fn a_ticked_box_deletes_the_file() {
        assert!(state(RemoveKind::Entry).deletes_file());
    }

    /// The stored preference means "take the entry off the list" and
    /// nothing else, so it cancels the deletion however the flag got
    /// set: a state restored from an older version, or a future path
    /// that forgets to clear it.
    #[test]
    fn a_remembered_answer_never_deletes_the_file() {
        let mut st = state(RemoveKind::Entry);
        st.dont_ask_again = true;
        assert!(!st.deletes_file());
    }

    /// Trash already moved it; deleting after that empties the Trash
    /// behind the user.
    #[test]
    fn trash_does_not_delete_a_second_time() {
        assert!(!state(RemoveKind::Trash).deletes_file());
    }

    #[test]
    fn nothing_on_disk_means_nothing_to_delete() {
        let mut st = state(RemoveKind::Entry);
        st.has_files = false;
        assert!(!st.deletes_file());
    }
}
