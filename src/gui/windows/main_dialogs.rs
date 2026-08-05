//! Main-window overlay dialogs: About (with update flow), browser
//! extensions, Remove confirm, Conflict resolution, DB-error and
//! secrets-locked recovery. All render as centered modal layers over
//! the main view (the egui app used child viewports; one process =
//! in-window overlays here).

use iced::widget::{column, container, mouse_area, row, text};
use iced::{Alignment, Element, Length};

use crate::data::ConflictKind;
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
    /// Destructive disposition pre-selected by the context-menu morph.
    pub kind: RemoveKind,
    pub delete_on_disk: bool,
    pub dont_ask_again: bool,
    /// Raised by the toolbar's Clean rather than by a selection: the
    /// set was assembled for the user, so the dialog says how many and
    /// the "don't ask again" answers for Clean alone.
    pub clean: bool,
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
    let (hero_icon, hero_color, message, cta_label, cta_icon): (
        &str,
        iced::Color,
        &str,
        &str,
        &str,
    ) = match st.kind {
        RemoveKind::Trash => (
            "trash-2",
            color::ochre::O400,
            "The file will be moved to your system Trash (recoverable).",
            "Move to Trash",
            "trash-2",
        ),
        RemoveKind::Permanent => (
            "triangle-alert",
            color::rust::R300,
            "The file will be permanently deleted from disk. This cannot be undone.",
            "Delete permanently",
            "trash-2",
        ),
        RemoveKind::Entry if st.clean => (
            "trash-2",
            t.status_danger,
            "Clears every completed entry from the list. Files stay on disk.",
            "Clean",
            "trash-2",
        ),
        RemoveKind::Entry if st.completed => (
            "triangle-alert",
            t.status_danger,
            "This only removes the entry from oxdm.",
            "Remove",
            "trash-2",
        ),
        RemoveKind::Entry => (
            "triangle-alert",
            t.status_danger,
            "Partial (.part) files will be deleted from disk.",
            "Remove",
            "trash-2",
        ),
    };
    let dont_label = if st.clean {
        "Don't ask again when cleaning"
    } else if st.completed {
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

    // On-disk delete toggle: meaningful only for completed entries that
    // aren't going to Trash (Trash moves the file itself).
    if st.completed && !st.clean && st.kind != RemoveKind::Trash {
        card = card.push(checkbox(
            t,
            "Also delete file on disk",
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

// ------------------------------------------------------ browser extensions

// Each vendor's extension store landing page (design §3.8). We do NOT
// fake an "Installed ✓" state — there is no reliable detection — so the
// button always reads "Open store page". Brave/Arc reuse the Chrome
// Web Store; Safari ships via the Mac App Store.
const BROWSER_STORES: [(&str, &str, &str); 7] = [
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
    (
        "Edge",
        "Edge Add-ons",
        "https://microsoftedge.microsoft.com/addons/",
    ),
    (
        "Brave",
        "Chrome Web Store",
        "https://chromewebstore.google.com/",
    ),
    (
        "Opera",
        "Opera Add-ons",
        "https://addons.opera.com/extensions/",
    ),
    (
        "Arc",
        "Chrome Web Store",
        "https://chromewebstore.google.com/",
    ),
    ("Safari", "Mac App Store", "https://apps.apple.com/"),
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

    // Hero band (design `.fr-hero`): clay-tinted glow + flow title.
    let glow = color::mix(t.bg_surface, t.action_primary, 0.12);
    let tile_bg = color::mix(t.bg_surface, t.action_primary, 0.20);
    let hero = container(
        column![
            container(icons::icon("puzzle", 30.0, t.action_primary))
                .width(Length::Fixed(56.0))
                .height(Length::Fixed(56.0))
                .align_x(Alignment::Center)
                .align_y(Alignment::Center)
                .style(move |_| container::Style {
                    background: Some(tile_bg.into()),
                    border: iced::Border {
                        radius: theme::radius::SM.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
            text("Capture downloads from your browser")
                .font(theme::DISPLAY)
                .size(20.0)
                .color(t.fg_1),
            text(sub).font(theme::BODY).size(13.0).color(t.fg_2),
        ]
        .spacing(theme::space::S2)
        .align_x(Alignment::Center),
    )
    .width(Length::Fill)
    .padding(theme::space::S4)
    .style(move |_| container::Style {
        background: Some(glow.into()),
        border: iced::Border {
            color: t2.border_subtle,
            width: 1.0,
            radius: theme::surface::RADIUS.into(),
        },
        ..Default::default()
    });

    let mut list = column![].spacing(theme::space::S1);
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
                text(store).font(theme::MONO).size(10.0).color(t.fg_3),
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
        .spacing(theme::space::S3)
        .align_y(Alignment::Center);
        list = list.push(
            container(r)
                .width(Length::Fill)
                .padding([theme::space::S1, theme::space::S2])
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
        .push(vscroll(list).height(Length::Fixed(264.0)))
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
