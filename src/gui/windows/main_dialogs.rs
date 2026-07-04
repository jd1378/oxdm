//! Main-window overlay dialogs: About (with update flow), Per-host
//! settings, Remove confirm, Conflict resolution, DB-error and
//! secrets-locked recovery. All render as centered modal layers over
//! the main view (the egui app used child viewports; one process =
//! in-window overlays here).

use iced::widget::{column, container, mouse_area, row, scrollable, text};
use iced::{Alignment, Element, Length};

use crate::data::ConflictKind;
use crate::domain::HostSetting;
use crate::gui::theme::{self, Tokens};
use crate::gui::widget::{
    Btn, BtnSize, PasswordInput, TextInput, checkbox, hairline, section_card,
};
use crate::gui::{color, icons};

use super::main::{Main, Msg, RemoveKind};

// ---------------------------------------------------------------- states

#[derive(Default, Clone)]
pub enum UpdateUi {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(crate::data::UpdateInfo),
    Downloading(String),
    Error(String),
}

#[derive(Default, Clone)]
pub struct AboutState {
    pub update: UpdateUi,
}

#[derive(Default, Clone)]
pub struct HostState {
    pub hosts: Vec<HostSetting>,
    pub search: String,
    pub selected: Option<String>,
    pub host: String,
    pub speed_enabled: bool,
    pub speed_kbs: String,
    pub threads: String,
    pub username: String,
    pub password: String,
    pub password_revealed: bool,
    pub had_password: bool,
    pub user_agent: String,
}

impl HostState {
    pub fn hydrate(&mut self, h: &HostSetting) {
        self.selected = Some(h.host.clone());
        self.host = h.host.clone();
        self.speed_enabled = h.speed_limit.is_some();
        self.speed_kbs = h
            .speed_limit
            .map(|v| (v / 1024).to_string())
            .unwrap_or_default();
        self.threads = h.thread_count.map(|v| v.to_string()).unwrap_or_default();
        self.username = h.username.clone().unwrap_or_default();
        self.password = String::new();
        self.had_password = h.has_password;
        self.user_agent = h.default_user_agent.clone().unwrap_or_default();
    }

    pub fn build(&self) -> HostSetting {
        HostSetting {
            host: self.host.trim().to_owned(),
            speed_limit: self
                .speed_enabled
                .then(|| self.speed_kbs.trim().parse::<u64>().ok().map(|k| k * 1024))
                .flatten(),
            thread_count: self.threads.trim().parse().ok(),
            username: {
                let u = self.username.trim();
                (!u.is_empty()).then(|| u.to_owned())
            },
            has_password: self.had_password || !self.password.is_empty(),
            default_user_agent: {
                let u = self.user_agent.trim();
                (!u.is_empty()).then(|| u.to_owned())
            },
        }
    }
}

#[derive(Clone)]
pub struct RemoveState {
    pub ids: Vec<crate::domain::JobId>,
    pub filename: String,
    pub completed: bool,
    /// Destructive disposition pre-selected by the context-menu morph.
    pub kind: RemoveKind,
    pub delete_on_disk: bool,
    pub dont_ask_again: bool,
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

// ---------------------------------------------------------------- about

pub fn about<'a>(m: &'a Main, base: Element<'a, Msg>) -> Element<'a, Msg> {
    let t = &m.tokens;
    let t2 = *t;
    let st = &m.about;

    let tile_bg = color::mix(t.bg_surface, t.action_primary, 0.20);
    let identity = container(
        row![
            container(
                text("OX")
                    .font(theme::DISPLAY)
                    .size(20.0)
                    .color(t.action_primary)
            )
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
            column![
                text("oxdm").font(theme::DISPLAY).size(28.0).color(t.fg_1),
                text("Open-source cross-platform download manager.")
                    .font(theme::BODY)
                    .size(13.0)
                    .color(t.fg_2),
                text(format!(
                    "Built on the odl crate · v{}",
                    env!("CARGO_PKG_VERSION")
                ))
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
        background: Some(t2.bg_raised.into()),
        border: iced::Border {
            color: t2.border_subtle,
            width: 1.0,
            radius: theme::surface::RADIUS.into(),
        },
        ..Default::default()
    });

    let status: Element<'a, Msg> = match &st.update {
        UpdateUi::Idle => text("Click \"Check for updates\" to look for a new release.")
            .font(theme::BODY)
            .size(12.0)
            .color(t.fg_3)
            .into(),
        UpdateUi::Checking => text("Checking…")
            .font(theme::BODY_BOLD)
            .size(12.0)
            .color(t.fg_2)
            .into(),
        UpdateUi::UpToDate => row![
            icons::icon("circle-check", 14.0, t.status_success),
            text("You're up to date.")
                .font(theme::BODY_BOLD)
                .size(12.0)
                .color(t.status_success),
        ]
        .spacing(6.0)
        .align_y(Alignment::Center)
        .into(),
        UpdateUi::Available(info) => column![
            crate::gui::widget::eyebrow(t, "update available"),
            text(format!("v{}", info.version))
                .font(theme::DISPLAY)
                .size(20.0)
                .color(t.fg_1),
            text(info.notes.clone().unwrap_or_default())
                .font(theme::BODY)
                .size(12.0)
                .color(t.fg_2),
            Btn::new("Download update")
                .primary()
                .icon("download")
                .on_press(Msg::AboutDownloadUpdate)
                .view(t),
        ]
        .spacing(theme::space::S2)
        .into(),
        UpdateUi::Downloading(v) => text(format!("Downloading v{v}…"))
            .font(theme::BODY_BOLD)
            .size(12.0)
            .color(t.fg_2)
            .into(),
        UpdateUi::Error(e) => row![
            icons::icon("circle-alert", 14.0, t.status_danger),
            text(e.clone())
                .font(theme::BODY_BOLD)
                .size(12.0)
                .color(t.status_danger),
        ]
        .spacing(6.0)
        .align_y(Alignment::Center)
        .into(),
    };

    let updates = section_card(
        t,
        "cloud-upload",
        "Updates",
        column![
            Btn::new("Check for updates")
                .secondary()
                .size(BtnSize::Sm)
                .icon("refresh-cw")
                .on_press(Msg::AboutCheckUpdate)
                .view(t),
            status,
        ]
        .spacing(theme::space::S2)
        .into(),
    );

    let card = column![
        identity,
        updates,
        row![
            Btn::new("Repository")
                .toolbar()
                .size(BtnSize::Sm)
                .icon("globe")
                .on_press(Msg::AboutRepository)
                .view(t),
            Btn::new("Donate")
                .toolbar()
                .size(BtnSize::Sm)
                .icon("zap")
                .on_press(Msg::AboutDonate)
                .view(t),
            iced::widget::Space::new().width(Length::Fill),
            Btn::new("Close")
                .ghost()
                .on_press(Msg::CloseOverlay)
                .view(t),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center),
    ]
    .spacing(theme::space::S3);

    modal(t, base, card.into(), 500.0, Some(Msg::CloseOverlay))
}

// ---------------------------------------------------------------- host settings

pub fn host_settings<'a>(m: &'a Main, base: Element<'a, Msg>) -> Element<'a, Msg> {
    let t = &m.tokens;
    let t2 = *t;
    let st = &m.host;

    let needle = st.search.trim().to_lowercase();
    let mut list = column![].spacing(2.0);
    for h in st
        .hosts
        .iter()
        .filter(|h| needle.is_empty() || h.host.to_lowercase().contains(&needle))
    {
        let active = st.selected.as_deref() == Some(h.host.as_str());
        let host_name = h.host.clone();
        let mut r = row![
            text(h.host.clone())
                .font(theme::BODY)
                .size(13.0)
                .color(if active { t.action_primary_fg } else { t.fg_1 }),
            iced::widget::Space::new().width(Length::Fill),
        ]
        .align_y(Alignment::Center);
        if h.has_password {
            r = r.push(icons::icon(
                "lock",
                13.0,
                if active {
                    t.action_primary_fg
                } else {
                    t.status_success
                },
            ));
        }
        list = list.push(
            mouse_area(
                container(r)
                    .width(Length::Fill)
                    .height(Length::Fixed(32.0))
                    .align_y(Alignment::Center)
                    .padding([0.0, theme::space::S2])
                    .style(move |_| container::Style {
                        background: active.then(|| t2.action_primary.into()),
                        border: iced::Border {
                            radius: theme::control::RADIUS.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
            )
            .on_press(Msg::HostSelect(host_name))
            .interaction(iced::mouse::Interaction::Pointer),
        );
    }

    let sidebar = column![
        crate::gui::widget::search_field(t, &st.search, "Search hosts…", 200.0, Msg::HostSearch),
        scrollable(list).height(Length::Fixed(280.0)),
        row![
            Btn::new("Add host")
                .ghost()
                .icon("plus")
                .on_press(Msg::HostAdd)
                .view(t),
            Btn::new("Delete")
                .toolbar()
                .size(BtnSize::Sm)
                .enabled(st.selected.is_some())
                .on_press(Msg::HostDelete)
                .view(t),
        ]
        .spacing(theme::space::S2),
    ]
    .spacing(theme::space::S2)
    .width(Length::Fixed(220.0));

    let editor: Element<'a, Msg> = if st.selected.is_some() || !st.host.is_empty() {
        column![
            section_card(
                t,
                "globe",
                "Identity",
                column![
                    crate::gui::widget::field_label(t, "host"),
                    TextInput::new(&st.host)
                        .hint("example.com")
                        .on_input(Msg::HostHost)
                        .view(t),
                ]
                .spacing(theme::space::S1)
                .into()
            ),
            section_card(
                t,
                "activity",
                "Connection",
                column![
                    row![
                        checkbox(
                            t,
                            "Speed limit (KB/s)",
                            st.speed_enabled,
                            true,
                            Msg::HostSpeedEnabled
                        ),
                        TextInput::new(&st.speed_kbs)
                            .mono()
                            .width(Length::Fixed(120.0))
                            .enabled(st.speed_enabled)
                            .on_input(Msg::HostSpeedKbs)
                            .view(t),
                    ]
                    .spacing(theme::space::S2)
                    .align_y(Alignment::Center),
                    row![
                        text("Threads").font(theme::BODY).size(13.0).color(t.fg_2),
                        TextInput::new(&st.threads)
                            .hint("auto")
                            .mono()
                            .width(Length::Fixed(80.0))
                            .on_input(Msg::HostThreads)
                            .view(t),
                    ]
                    .spacing(theme::space::S2)
                    .align_y(Alignment::Center),
                ]
                .spacing(theme::space::S2)
                .into()
            ),
            section_card(
                t,
                "key",
                "Authentication",
                column![
                    crate::gui::widget::field_label(t, "username"),
                    TextInput::new(&st.username)
                        .hint("anonymous")
                        .on_input(Msg::HostUsername)
                        .view(t),
                    crate::gui::widget::field_label(t, "password"),
                    PasswordInput::new(&st.password)
                        .hint(if st.had_password {
                            "••••••••"
                        } else {
                            ""
                        })
                        .revealed(st.password_revealed)
                        .on_input(Msg::HostPassword)
                        .on_reveal(Msg::HostReveal)
                        .view(t),
                    if st.had_password {
                        Element::from(
                            row![
                                icons::icon("lock", 12.0, t.status_success),
                                text("Stored in OS keyring")
                                    .font(theme::BODY_BOLD)
                                    .size(11.0)
                                    .color(t.status_success),
                            ]
                            .spacing(4.0)
                            .align_y(Alignment::Center),
                        )
                    } else {
                        Element::from(iced::widget::Space::new())
                    },
                ]
                .spacing(theme::space::S1)
                .into()
            ),
            section_card(
                t,
                "settings",
                "Custom user agent",
                TextInput::new(&st.user_agent)
                    .hint("Mozilla/5.0 …")
                    .on_input(Msg::HostUserAgent)
                    .view(t)
            ),
        ]
        .spacing(theme::space::S2)
        .into()
    } else {
        container(
            text("Select a host to edit, or click + to add one.")
                .font(theme::BODY)
                .size(13.0)
                .color(t.fg_3),
        )
        .width(Length::Fill)
        .height(Length::Fixed(200.0))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
    };

    let card = column![
        title_row(t, "Per host settings"),
        hairline(t.border_subtle),
        row![
            sidebar,
            scrollable(editor)
                .height(Length::Fixed(360.0))
                .width(Length::Fill)
        ]
        .spacing(theme::space::S3),
        hairline(t.border_subtle),
        row![
            Btn::new("Close")
                .ghost()
                .on_press(Msg::CloseOverlay)
                .view(t),
            iced::widget::Space::new().width(Length::Fill),
            Btn::new("Save")
                .primary()
                .icon("save")
                .enabled(!st.host.trim().is_empty())
                .on_press(Msg::HostSave)
                .view(t),
        ]
        .align_y(Alignment::Center),
    ]
    .spacing(theme::space::S3);

    modal(t, base, card.into(), 720.0, Some(Msg::CloseOverlay))
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
    let dont_label = if st.completed {
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
    if st.completed && st.kind != RemoveKind::Trash {
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
        text("Resetting deletes the download list (files on disk are kept).")
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
         comes here automatically — segmented, resumable, and queued."
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
        text("The extension reads only download URLs — never page content or browsing history.")
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
        .push(scrollable(list).height(Length::Fixed(264.0)))
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
             changed?). You can wipe stored job secrets and continue — downloads themselves \
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
