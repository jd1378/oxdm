//! About window (`oxdm gui about`): who made this, what exactly is
//! running, and whether a newer release exists. A window of its own
//! rather than a main-window overlay — it is reachable from the tray
//! with no main window on screen, and its update check outlives any
//! particular view.

use std::sync::Arc;
use std::time::Duration;

use iced::widget::{column, container, row, text};
use iced::{Alignment, Element, Length, Subscription, Task};

use crate::gui::chrome::{self, WindowControl, titlebar};
use crate::gui::ipc::DaemonSignal;
use crate::gui::shot::Shot;
use crate::gui::theme::{self, Tokens};
use crate::gui::widget::{Btn, BtnSize, hairline, set_row_panel, set_rows, vdivider};
use crate::gui::{color, icons};
use crate::ipc_local::Client;
use crate::ipc_local::protocol::{Event, GuiKind};

const WIN_W: f32 = 468.0;
/// Sized to the content: identity header, the four body cards with the
/// page's own padding under the last of them, and the footer band. The
/// painted titlebar is added on top where the user opted into it.
///
/// The last card ends in padding that paints nothing, so a height read
/// off a screenshot lands a few pixels short and the page scrolls by
/// exactly that much; the 5 is that shortfall.
const WIN_H: f32 = 517.0;

/// Facts cargo does not expose to the crate itself; `build.rs` resolves
/// them (each degrades to "unknown", never to a build failure).
const ODL_VERSION: &str = env!("OXDM_ODL_VERSION");
const GIT_COMMIT: &str = env!("OXDM_GIT_COMMIT");
const RUSTC_VERSION: &str = env!("OXDM_RUSTC");

const REPOSITORY_URL: &str = "https://github.com/jd1378/oxdm";
const DONATE_URL: &str = "https://github.com/sponsors/jd1378";
const RELEASES_URL: &str = "https://github.com/jd1378/oxdm/releases";

/// How long "Copy build info" shows its confirmation before reverting.
const COPIED_FOR: Duration = Duration::from_millis(1600);

#[derive(Default, Clone)]
pub enum UpdateUi {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(crate::data::UpdateInfo),
    /// Fetching the artifact through oxdm's own download machinery, so
    /// the user can watch it arrive rather than stare at a spinner.
    Downloading {
        version: String,
        job: crate::domain::JobId,
        done: u64,
        total: Option<u64>,
    },
    /// Fetched, and its SHA-256 matched what the feed published. The
    /// swap happens on the user's word, not before.
    Staged(String),
    Error(String),
}

#[derive(Clone)]
pub enum Msg {
    Connected(Result<Box<(Arc<Client>, crate::domain::Settings)>, String>),
    Daemon(DaemonSignal),
    Window(WindowControl),
    CheckUpdate,
    Checked(Result<Option<crate::data::UpdateInfo>, String>),
    DownloadUpdate,
    DownloadStarted(Result<crate::domain::JobId, String>),
    /// Ask the daemon how far the update download has got.
    ProgressTick,
    Progress(Option<(u64, Option<u64>)>),
    /// Restart into the new version.
    InstallNow,
    Installed(Result<(), String>),
    ReleaseNotes,
    Repository,
    Donate,
    CopyBuildInfo,
    /// Flips "Copied" back to "Copy build info" once the confirmation
    /// has been readable for a moment.
    CopyExpired,
    KeyPressed(iced::keyboard::Key),
    ShotTick,
    Shot(iced::window::Screenshot),
    Themed(Box<Tokens>),
    Noop,
}

pub enum App {
    Connecting,
    Failed(String),
    Ready(Box<State>),
}

pub struct State {
    client: Arc<Client>,
    tokens: Tokens,
    update: UpdateUi,
    copied: bool,
    shot: Option<Shot>,
}

// ---------------------------------------------------------------- data

fn platform() -> String {
    format!("{} · {}", std::env::consts::OS, std::env::consts::ARCH)
}

/// One line an issue report can paste verbatim ("Copy build info").
fn build_info() -> String {
    format!(
        "oxdm {} ({}) · odl {} · rustc {} · {}",
        env!("CARGO_PKG_VERSION"),
        GIT_COMMIT,
        ODL_VERSION,
        RUSTC_VERSION,
        platform(),
    )
}

// ---------------------------------------------------------------- app

pub fn boot() -> (App, Task<Msg>) {
    (
        App::Connecting,
        Task::perform(
            async {
                let client = Client::connect_retry(Duration::from_secs(8))
                    .await
                    .map_err(|e| e.to_string())?;
                client.hello(GuiKind::About).await?;
                let snap = client.snapshot().await?;
                Ok(Box::new((client, snap.settings)))
            },
            Msg::Connected,
        ),
    )
}

pub fn update(app: &mut App, msg: Msg) -> Task<Msg> {
    match msg {
        Msg::Connected(Ok(boxed)) => {
            let (client, settings) = *boxed;
            *app = App::Ready(Box::new(State {
                tokens: Tokens::from_settings(&settings),
                client,
                update: UpdateUi::Idle,
                copied: false,
                shot: Shot::from_env(),
            }));
            Task::none()
        }
        Msg::Connected(Err(e)) => {
            *app = App::Failed(e);
            Task::none()
        }
        Msg::Window(ctl) => chrome::window_task(ctl),
        msg => {
            let App::Ready(st) = app else {
                return Task::none();
            };
            update_ready(st, msg)
        }
    }
}

fn update_ready(st: &mut State, msg: Msg) -> Task<Msg> {
    match msg {
        Msg::Daemon(DaemonSignal::Lost) => iced::exit(),
        Msg::Daemon(DaemonSignal::Event(ev)) => match ev {
            Event::SettingsChanged => crate::gui::theme::refresh_tokens(
                st.client.clone(),
                |t| Msg::Themed(Box::new(t)),
                Msg::Noop,
            ),
            Event::UpdateStaged { version } => {
                st.update = UpdateUi::Staged(version);
                Task::none()
            }
            Event::UpdateFailed { message } => {
                st.update = UpdateUi::Error(message);
                Task::none()
            }
            Event::Close => iced::exit(),
            Event::Focus => iced::window::latest().and_then(iced::window::gain_focus),
            _ => Task::none(),
        },
        Msg::CheckUpdate => {
            st.update = UpdateUi::Checking;
            let client = st.client.clone();
            Task::perform(async move { client.update_check().await }, Msg::Checked)
        }
        Msg::Checked(res) => {
            st.update = match res {
                Ok(Some(info)) => UpdateUi::Available(info),
                Ok(None) => UpdateUi::UpToDate,
                Err(e) => UpdateUi::Error(e),
            };
            Task::none()
        }
        Msg::DownloadUpdate => {
            let UpdateUi::Available(info) = st.update.clone() else {
                return Task::none();
            };
            let client = st.client.clone();
            Task::perform(
                async move { client.add_update_job(info).await },
                Msg::DownloadStarted,
            )
        }
        Msg::DownloadStarted(Ok(job)) => {
            let version = match &st.update {
                UpdateUi::Available(info) => info.version.clone(),
                _ => String::new(),
            };
            st.update = UpdateUi::Downloading {
                version,
                job,
                done: 0,
                total: None,
            };
            Task::none()
        }
        Msg::DownloadStarted(Err(e)) => {
            st.update = UpdateUi::Error(e);
            Task::none()
        }
        Msg::ProgressTick => {
            let UpdateUi::Downloading { job, .. } = &st.update else {
                return Task::none();
            };
            let (client, job) = (st.client.clone(), *job);
            Task::perform(
                async move {
                    client
                        .job_entry(job)
                        .await
                        .ok()
                        .flatten()
                        .map(|e| (e.counters.downloaded, e.counters.total))
                },
                Msg::Progress,
            )
        }
        Msg::Progress(Some((got, size))) => {
            if let UpdateUi::Downloading { done, total, .. } = &mut st.update {
                *done = got;
                *total = size;
            }
            Task::none()
        }
        Msg::Progress(None) => Task::none(),
        Msg::InstallNow => {
            let client = st.client.clone();
            Task::perform(async move { client.install_update().await }, Msg::Installed)
        }
        // The daemon is on its way out and the helper takes over from
        // here; the window closes with everything else.
        Msg::Installed(Ok(())) => Task::none(),
        Msg::Installed(Err(e)) => {
            st.update = UpdateUi::Error(e);
            Task::none()
        }
        Msg::ReleaseNotes => {
            crate::platform::open_url(RELEASES_URL);
            Task::none()
        }
        Msg::Repository => {
            crate::platform::open_url(REPOSITORY_URL);
            Task::none()
        }
        Msg::Donate => {
            crate::platform::open_url(DONATE_URL);
            Task::none()
        }
        Msg::CopyBuildInfo => {
            st.copied = true;
            Task::batch([
                iced::clipboard::write(build_info()),
                Task::perform(tokio::time::sleep(COPIED_FOR), |()| Msg::CopyExpired),
            ])
        }
        Msg::CopyExpired => {
            st.copied = false;
            Task::none()
        }
        Msg::KeyPressed(key) => {
            if matches!(
                key.as_ref(),
                iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape)
            ) {
                return chrome::window_task(WindowControl::Close);
            }
            Task::none()
        }
        Msg::ShotTick => {
            if let Some(shot) = &mut st.shot
                && let Some(task) = shot.tick()
            {
                return task.map(Msg::Shot);
            }
            Task::none()
        }
        Msg::Shot(s) => match &st.shot {
            Some(shot) => shot.save_and_exit(s),
            None => Task::none(),
        },
        Msg::Themed(t) => {
            st.tokens = *t;
            Task::none()
        }
        Msg::Connected(_) | Msg::Window(_) | Msg::Noop => Task::none(),
    }
}

pub fn subscription(app: &App) -> Subscription<Msg> {
    let App::Ready(st) = app else {
        return Subscription::none();
    };
    let mut subs = vec![
        iced::event::listen_with(|event, _status, _id| match event {
            iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { key, .. }) => {
                Some(Msg::KeyPressed(key))
            }
            _ => None,
        }),
        crate::gui::ipc::lifecycle_events(GuiKind::About).map(Msg::Daemon),
    ];
    if matches!(st.update, UpdateUi::Downloading { .. }) {
        // Polled rather than subscribed to the counter pump: this is
        // one hidden job, and the About window has no other use for a
        // stream of every download's counters.
        subs.push(iced::time::every(Duration::from_millis(500)).map(|_| Msg::ProgressTick));
    }
    if st.shot.is_some() {
        subs.push(Shot::frames().map(|_| Msg::ShotTick));
    }
    Subscription::batch(subs)
}

// ---------------------------------------------------------------- view

/// Identity chip: version (mono, accented) and licence.
fn chip<'a>(t: &Tokens, label: String, accent: bool) -> Element<'a, Msg> {
    let t2 = *t;
    let accent_bg = color::mix(t.bg_surface, t.action_primary, 0.14);
    container(
        text(label)
            .font(if accent {
                theme::MONO_SEMIBOLD
            } else {
                theme::BODY_BOLD
            })
            .size(CHIP_TEXT)
            // Pin the line box to the glyphs: iced's default leaves
            // descender room an all-caps label never uses, which reads
            // as a chip whose text sits high.
            .line_height(iced::widget::text::LineHeight::Absolute(12.0.into()))
            .color(if accent { t.fg_1 } else { t.fg_2 }),
    )
    .padding([3.0, 8.0])
    .style(move |_| container::Style {
        background: Some(if accent {
            accent_bg.into()
        } else {
            t2.bg_surface.into()
        }),
        border: iced::Border {
            color: if accent {
                t2.border_brand
            } else {
                t2.border_default
            },
            width: 1.0,
            radius: theme::radius::PILL.into(),
        },
        ..Default::default()
    })
    .into()
}

const CHIP_TEXT: f32 = 10.5;

/// Build-facts cells sit on the settings-row grid; the row is pinned
/// because `vdivider` needs a concrete height.
const FACT_PAD_Y: f32 = 12.0;
const FACT_PAD_X: f32 = 14.0;
const FACT_LABEL_SIZE: f32 = 12.5;
const FACT_VALUE_SIZE: f32 = 11.5;
/// Padding plus one 12.5px line box.
const FACT_H: f32 = FACT_PAD_Y * 2.0 + 18.0;

/// Status-glyph tile in the update row.
const MARK_TILE: f32 = 26.0;

/// Gap between every stacked block, and the body's inset. One value so
/// the sections read as an even rhythm rather than a set of cards.
const GAP: f32 = theme::space::S3;
/// How far the mark sits below the top of the header band.
const LOGO_DROP: f32 = 8.0;
const BODY_PAD: f32 = theme::space::S5;

/// One cell of the build-facts row: label left, mono value right.
fn fact<'a>(t: &Tokens, label: &str, value: String) -> Element<'a, Msg> {
    container(
        row![
            text(label.to_owned())
                .font(theme::BODY_MEDIUM)
                .size(FACT_LABEL_SIZE)
                .color(t.fg_1),
            iced::widget::Space::new().width(Length::Fill),
            text(value)
                .font(theme::MONO)
                .size(FACT_VALUE_SIZE)
                .color(t.fg_2),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .align_y(Alignment::Center)
    .padding([FACT_PAD_Y, FACT_PAD_X])
    .into()
}

/// Project link: a settings row that happens to be clickable — mark,
/// label over a hint, and a leaves-the-app arrow. Borderless, because
/// the section surface around it already draws the frame.
fn link<'a>(t: &Tokens, mark: &str, label: &str, hint: &str, msg: Msg) -> Element<'a, Msg> {
    let t2 = *t;
    iced::widget::button(
        row![
            icons::icon(mark, 15.0, t.fg_2),
            column![
                text(label.to_owned())
                    .font(theme::BODY_MEDIUM)
                    .size(FACT_LABEL_SIZE)
                    .color(t.fg_1),
                text(hint.to_owned())
                    .font(theme::BODY)
                    .size(11.0)
                    .color(t.fg_3),
            ]
            .spacing(3.0),
            iced::widget::Space::new().width(Length::Fill),
            icons::icon("arrow-up-right", 12.0, t.fg_3),
        ]
        .spacing(theme::space::S3)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding([FACT_PAD_Y, FACT_PAD_X])
    .on_press(msg)
    .style(move |_th, status| iced::widget::button::Style {
        background: matches!(
            status,
            iced::widget::button::Status::Hovered | iced::widget::button::Status::Pressed
        )
        .then(|| t2.bg_surface_hover.into()),
        text_color: t2.fg_1,
        border: iced::Border {
            radius: theme::surface::RADIUS.into(),
            ..Default::default()
        },
        shadow: iced::Shadow::default(),
        ..Default::default()
    })
    .into()
}

pub fn view(app: &App) -> Element<'_, Msg> {
    chrome::framed(match app {
        App::Connecting => splash("Connecting…".to_owned()),
        App::Failed(e) => splash(e.clone()),
        App::Ready(st) => ready_view(st),
    })
}

fn splash<'a>(msg: String) -> Element<'a, Msg> {
    let t = Tokens::dark();
    container(text(msg).color(t.fg_2))
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(move |_| container::Style {
            background: Some(t.bg_page.into()),
            ..Default::default()
        })
        .into()
}

fn identity(t: &Tokens) -> Element<'_, Msg> {
    let t2 = *t;
    container(
        row![
            // Dropped against the wordmark rather than aligned to its
            // cap height: "oxdm" is set large enough that a top-aligned
            // mark reads as sitting above the name it belongs to. A
            // spacer, not padding — padding would take the drop out of
            // the mark's own 64px and shrink it.
            column![
                iced::widget::Space::new().height(Length::Fixed(LOGO_DROP)),
                crate::gui::widget::app_mark(t, 64.0),
            ],
            column![
                text("oxdm").font(theme::DISPLAY).size(32.0).color(t.fg_1),
                text("An open-source download manager for every desktop.")
                    .font(theme::BODY)
                    .size(12.5)
                    .color(t.fg_2),
                row![
                    chip(t, format!("v{}", env!("CARGO_PKG_VERSION")), true),
                    chip(t, env!("CARGO_PKG_LICENSE").to_owned(), false),
                ]
                .spacing(6.0),
            ]
            .spacing(7.0),
        ]
        .spacing(theme::space::S4)
        .align_y(Alignment::Start),
    )
    .width(Length::Fill)
    .padding(iced::Padding {
        top: 22.0,
        right: 22.0,
        bottom: 20.0,
        left: 22.0,
    })
    .style(move |_| container::Style {
        background: Some(t2.bg_sunken.into()),
        ..Default::default()
    })
    .into()
}

/// Update card: one row of copy, one row of actions, tinted when there
/// is something to install.
fn updates(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let t2 = *t;
    let version = env!("CARGO_PKG_VERSION");
    let (mark, mark_fg, headline, detail): (&str, iced::Color, String, String) = match &st.update {
        UpdateUi::Idle => (
            "refresh-cw",
            t.fg_2,
            "Not checked yet".into(),
            "Automatic checks are off. Look for a new release whenever you want.".into(),
        ),
        UpdateUi::Checking => (
            "refresh-cw",
            t.fg_2,
            "Checking for updates…".into(),
            "Asking the release server.".into(),
        ),
        UpdateUi::UpToDate => (
            "circle-check",
            t.status_success,
            "You're on the latest release".into(),
            format!("Last checked just now · v{version}"),
        ),
        UpdateUi::Available(info) => (
            "download",
            t.action_primary,
            format!("Version {} is ready", info.version),
            info.notes
                .clone()
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| "A newer release is available to install.".into()),
        ),
        UpdateUi::Downloading {
            version,
            done,
            total,
            ..
        } => (
            "download",
            t.action_primary,
            match total {
                Some(size) if *size > 0 => format!(
                    "Downloading v{version}… {}%",
                    (*done as f64 / *size as f64 * 100.0).round() as u64
                ),
                _ => format!("Downloading v{version}…"),
            },
            match total {
                Some(size) => format!(
                    "{} of {}",
                    crate::gui::format::format_bytes(*done),
                    crate::gui::format::format_bytes(*size)
                ),
                None => crate::gui::format::format_bytes(*done),
            },
        ),
        UpdateUi::Staged(v) => (
            "circle-check",
            t.status_success,
            format!("Version {v} is ready to install"),
            "Downloaded and checked against the release checksum. oxdm restarts to \
             finish, and your downloads pause first."
                .into(),
        ),
        UpdateUi::Error(e) => (
            "circle-alert",
            t.status_danger,
            "Update check failed".into(),
            e.clone(),
        ),
    };

    let actions: Element<'_, Msg> = match &st.update {
        UpdateUi::Available(_) => row![
            Btn::new("Install update")
                .primary()
                .size(BtnSize::Md)
                .icon("download")
                .on_press(Msg::DownloadUpdate)
                .view(t),
            Btn::new("Release notes")
                .ghost()
                .size(BtnSize::Md)
                .icon("file-text")
                .on_press(Msg::ReleaseNotes)
                .view(t),
        ]
        .spacing(theme::space::S2)
        .into(),
        UpdateUi::Downloading { .. } => Btn::new("Release notes")
            .ghost()
            .size(BtnSize::Md)
            .icon("file-text")
            .on_press(Msg::ReleaseNotes)
            .view(t),
        UpdateUi::Staged(_) => row![
            Btn::new("Restart and install")
                .primary()
                .size(BtnSize::Md)
                .icon("refresh-cw")
                .on_press(Msg::InstallNow)
                .view(t),
            Btn::new("Release notes")
                .ghost()
                .size(BtnSize::Md)
                .icon("file-text")
                .on_press(Msg::ReleaseNotes)
                .view(t),
        ]
        .spacing(theme::space::S2)
        .into(),
        UpdateUi::UpToDate | UpdateUi::Error(_) => Btn::new("Check again")
            .ghost()
            .size(BtnSize::Md)
            .icon("refresh-cw")
            .on_press(Msg::CheckUpdate)
            .view(t),
        UpdateUi::Checking => Btn::new("Checking…")
            .secondary()
            .size(BtnSize::Md)
            .icon("refresh-cw")
            .enabled(false)
            .view(t),
        UpdateUi::Idle => Btn::new("Check for updates")
            .secondary()
            .size(BtnSize::Md)
            .icon("refresh-cw")
            .on_press(Msg::CheckUpdate)
            .view(t),
    };

    let mark_bg = color::mix(t.bg_sunken, mark_fg, 0.14);
    set_row_panel(
        column![
            row![
                container(icons::icon(mark, 15.0, mark_fg))
                    .width(Length::Fixed(MARK_TILE))
                    .height(Length::Fixed(MARK_TILE))
                    .align_x(Alignment::Center)
                    .align_y(Alignment::Center)
                    .style(move |_| container::Style {
                        background: Some(mark_bg.into()),
                        border: iced::Border {
                            color: t2.border_default,
                            width: 1.0,
                            radius: theme::radius::XS.into(),
                        },
                        ..Default::default()
                    }),
                column![
                    text(headline)
                        .font(theme::BODY_MEDIUM)
                        .size(FACT_LABEL_SIZE)
                        .color(t.fg_1),
                    text(detail).font(theme::BODY).size(11.0).color(t.fg_3),
                ]
                .spacing(3.0),
            ]
            .spacing(theme::space::S3)
            .align_y(Alignment::Start),
            actions,
        ]
        .spacing(GAP)
        .into(),
    )
}

fn ready_view(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let t2 = *t;

    // Build facts share the settings surface, split side by side so the
    // divider runs vertically between the two cells.
    let facts: Element<'_, Msg> = row![
        fact(t, "Engine", format!("odl {ODL_VERSION}")),
        vdivider(t.border_subtle, FACT_H),
        fact(t, "Platform", platform()),
    ]
    .height(Length::Fixed(FACT_H))
    .into();

    let repository = set_rows(
        t,
        vec![link(
            t,
            "git-branch",
            "Repository",
            "Source, issues, releases",
            Msg::Repository,
        )],
    );
    let donate = set_rows(
        t,
        vec![link(
            t,
            "heart",
            "Donate",
            "Keep the project independent",
            Msg::Donate,
        )],
    );

    // Scrolls because the window is sized to the content it normally
    // has: a release note long enough to wrap several times grows the
    // update card, and growing past the window should not cut it off.
    let body = crate::gui::widget::vscroll(
        container(
            column![
                set_rows(t, vec![updates(st)]),
                set_rows(t, vec![facts]),
                repository,
                donate,
            ]
            .spacing(GAP),
        )
        .width(Length::Fill)
        .padding(iced::Padding {
            top: BODY_PAD,
            bottom: BODY_PAD,
            left: BODY_PAD,
            right: BODY_PAD - crate::gui::widget::SCROLL_GUTTER,
        }),
    )
    .height(Length::Fill);

    let footer = container(
        row![
            text("© 2026 oxdm contributors")
                .font(theme::MONO)
                .size(11.0)
                .color(t.fg_3),
            iced::widget::Space::new().width(Length::Fill),
            Btn::new(if st.copied {
                "Copied"
            } else {
                "Copy build info"
            })
            .ghost()
            .icon(if st.copied { "check" } else { "clipboard" })
            .on_press(Msg::CopyBuildInfo)
            .view(t),
            Btn::new("Close")
                .primary()
                .on_press(Msg::Window(WindowControl::Close))
                .view(t),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding([GAP, BODY_PAD])
    .style(move |_| container::Style {
        background: Some(t2.bg_sunken.into()),
        ..Default::default()
    });

    container(column![
        titlebar::titlebar(t, "About oxdm", false, Msg::Window),
        identity(t),
        hairline(t.border_subtle),
        body,
        hairline(t.border_subtle),
        footer,
    ])
    .width(Length::Fill)
    .height(Length::Fill)
    .style(move |_| container::Style {
        background: Some(t2.bg_page.into()),
        ..Default::default()
    })
    .into()
}

pub fn launch_about() {
    let mut app = iced::application(boot, update, view)
        .title(|_: &App| "About oxdm".to_owned())
        .theme(|app: &App| match app {
            App::Ready(st) => st.tokens.iced_theme(),
            _ => Tokens::dark().iced_theme(),
        })
        .subscription(subscription)
        .default_font(theme::BODY)
        .antialiasing(true)
        .window(chrome::window_settings(
            iced::Size::new(WIN_W, WIN_H + chrome::overhead_h()),
            iced::Size::new(WIN_W, WIN_H + chrome::overhead_h()),
        ));
    for f in theme::fonts::ALL {
        app = app.font(*f);
    }
    if let Err(e) = app.run() {
        eprintln!("gui error: {e}");
        std::process::exit(1);
    }
}
