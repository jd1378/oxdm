//! Settings window (`oxdm gui settings [--tab t] [--highlight-proxy]`):
//! left section list (General / Downloads / Categories / Network /
//! Browser / Notifications / Advanced / About), per-section cards,
//! footer with Cancel / Reset-tab / Save.

use std::sync::Arc;
use std::time::Duration;

use iced::widget::{column, container, mouse_area, row, text, text_editor};
use iced::{Alignment, Element, Length, Subscription, Task};

use crate::domain::{Category, Density, Settings, Theme as AppTheme};
use crate::gui::chrome::{self, WindowControl, titlebar};
use crate::gui::icons;
use crate::gui::ipc::DaemonSignal;
use crate::gui::shot::Shot;
use crate::gui::theme::{self, Tokens};
use crate::gui::widget::{
    Btn, BtnSize, FileInput, TextInput, combo, hairline, number_stepper, section_card, segmented,
    toggle,
};
use crate::ipc_local::Client;
use crate::ipc_local::protocol::Event;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    General,
    Downloads,
    Categories,
    Network,
    Browser,
    Notifications,
    Advanced,
    About,
}

impl Section {
    const ALL: [(Section, &'static str, &'static str); 8] = [
        (Section::General, "sliders-horizontal", "General"),
        (Section::Downloads, "download", "Downloads"),
        (Section::Categories, "folder", "Categories"),
        (Section::Network, "globe", "Network"),
        (Section::Browser, "puzzle", "Browser"),
        (Section::Notifications, "bell", "Notifications"),
        (Section::Advanced, "terminal", "Advanced"),
        (Section::About, "info", "About"),
    ];
    fn label(self) -> &'static str {
        Self::ALL.iter().find(|(s, _, _)| *s == self).unwrap().2
    }
    /// Muted one-line description shown under the pane-head title
    /// (design `.s-pane-head`).
    fn desc(self) -> &'static str {
        match self {
            Section::General => "Appearance, storage locations, and startup behavior.",
            Section::Downloads => "Retry behavior and removal confirmations.",
            Section::Categories => "Map file extensions to download categories.",
            Section::Network => "Connections, bandwidth, proxy, and request identity.",
            Section::Browser => "Pair the browser extension and resolve capture conflicts.",
            Section::Notifications => "What oxdm tells you when a download finishes.",
            Section::Advanced => "Theme overrides and the update feed.",
            Section::About => "Version and project information.",
        }
    }
}

#[derive(Clone)]
pub enum Msg {
    Connected(Result<Box<(Arc<Client>, Settings)>, String>),
    Daemon(DaemonSignal),
    Window(WindowControl),
    SetSection(Section),
    // General
    SetTheme(String),
    SetDensity(Density),
    ReduceMotion(bool),
    DownloadDir(String),
    BrowseDownloadDir,
    BrowsedDownloadDir(Option<std::path::PathBuf>),
    WorkDir(String),
    StartAtLogin(bool),
    StartToTray(bool),
    // Downloads
    MaxRetries(String),
    FixedRetries(String),
    RetryWait(String),
    UseServerTime(bool),
    ConfirmIncomplete(bool),
    ConfirmCompleted(bool),
    // Categories
    CategoryExts(Category, String),
    CategoryReset(Category),
    // Network
    AutoConnections(bool),
    Concurrent(String),
    SpeedLimit(String),
    Proxy(String),
    ConnectTimeout(String),
    InvalidCerts(bool),
    UserAgent(String),
    RandomUa(bool),
    CustomHeaders(text_editor::Action),
    // Browser
    IpcPort(String),
    CopyPairing,
    Regenerate,
    ConflictHidden(String),
    // Notifications
    ShowCompleteDialog(bool),
    // Advanced
    ThemeOverrides(text_editor::Action),
    UpdateFeed(String),
    // Footer
    ResetSection,
    Save,
    Saved(Result<(), String>),
    Cancel,
    WinResized(f32, f32),
    ShotTick,
    Shot(iced::window::Screenshot),
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
    section: Section,
    original: Settings,
    s: Settings,
    // string mirrors for numeric fields
    download_dir: String,
    work_dir: String,
    max_retries: String,
    fixed_retries: String,
    retry_wait: String,
    concurrent: String,
    speed_limit: String,
    proxy: String,
    connect_timeout: String,
    user_agent: String,
    ipc_port: String,
    update_feed: String,
    cat_exts: Vec<(Category, String)>,
    custom_headers: text_editor::Content,
    theme_overrides: text_editor::Content,
    shot: Option<Shot>,
}

fn mirror(st: &mut State) {
    let s = &st.s;
    st.download_dir = s.download_dir.display().to_string();
    st.work_dir = s.work_dir.display().to_string();
    st.max_retries = s.max_retries.to_string();
    st.fixed_retries = s.n_fixed_retries.to_string();
    st.retry_wait = humantime::format_duration(s.wait_between_retries).to_string();
    st.concurrent = s.max_concurrent_downloads.to_string();
    st.speed_limit = s.speed_limit.map(|v| v.to_string()).unwrap_or_default();
    st.proxy = s.proxy.clone().unwrap_or_default();
    st.connect_timeout = s
        .connect_timeout
        .map(|d| humantime::format_duration(d).to_string())
        .unwrap_or_default();
    st.user_agent = s.user_agent.clone().unwrap_or_default();
    st.ipc_port = s.ipc_port.to_string();
    st.update_feed = s.update_feed_url.clone();
    st.cat_exts = Category::ALL_ASSIGNABLE
        .iter()
        .map(|c| {
            let exts = s
                .category_extensions
                .get(c)
                .map(|v| v.join(", "))
                .unwrap_or_else(|| c.default_extensions().join(", "));
            (*c, exts)
        })
        .collect();
    let headers = s
        .headers
        .iter()
        .map(|(k, v)| format!("{k}: {v}"))
        .collect::<Vec<_>>()
        .join("\n");
    st.custom_headers = text_editor::Content::with_text(&headers);
    let overrides = s
        .theme_overrides
        .iter()
        .map(|(k, v)| format!("{k}: {v}"))
        .collect::<Vec<_>>()
        .join("\n");
    st.theme_overrides = text_editor::Content::with_text(&overrides);
}

fn section_arg() -> Section {
    let mut args = std::env::args().skip(3);
    while let Some(a) = args.next() {
        if a == "--tab" {
            return match args.next().as_deref() {
                Some("downloads") => Section::Downloads,
                Some("categories") => Section::Categories,
                Some("network") => Section::Network,
                Some("browser") => Section::Browser,
                Some("notifications") => Section::Notifications,
                Some("advanced") => Section::Advanced,
                Some("about") => Section::About,
                _ => Section::General,
            };
        }
    }
    Section::General
}

pub fn boot() -> (App, Task<Msg>) {
    (
        App::Connecting,
        Task::perform(
            async {
                let client = Client::connect_retry(Duration::from_secs(8))
                    .await
                    .map_err(|e| e.to_string())?;
                client
                    .hello(crate::ipc_local::protocol::GuiKind::Settings)
                    .await?;
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
            let mut st = State {
                tokens: Tokens::from_settings(&settings),
                section: section_arg(),
                original: settings.clone(),
                s: settings,
                download_dir: String::new(),
                work_dir: String::new(),
                max_retries: String::new(),
                fixed_retries: String::new(),
                retry_wait: String::new(),
                concurrent: String::new(),
                speed_limit: String::new(),
                proxy: String::new(),
                connect_timeout: String::new(),
                user_agent: String::new(),
                ipc_port: String::new(),
                update_feed: String::new(),
                cat_exts: Vec::new(),
                custom_headers: text_editor::Content::new(),
                theme_overrides: text_editor::Content::new(),
                shot: Shot::from_env(),
                client,
            };
            mirror(&mut st);
            *app = App::Ready(Box::new(st));
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
        Msg::Daemon(DaemonSignal::Event(Event::Close)) => iced::exit(),
        Msg::Daemon(_) => Task::none(),
        Msg::SetSection(sec) => {
            st.section = sec;
            Task::none()
        }
        Msg::SetTheme(v) => {
            st.s.theme = match v.as_str() {
                "light" => AppTheme::Light,
                "dark" => AppTheme::Dark,
                "warm" => AppTheme::Warm,
                _ => AppTheme::System,
            };
            st.tokens = Tokens::from_settings(&st.s);
            Task::none()
        }
        Msg::SetDensity(v) => {
            st.s.ui_density = v;
            Task::none()
        }
        Msg::ReduceMotion(v) => {
            st.s.reduce_motion = v;
            Task::none()
        }
        Msg::DownloadDir(v) => {
            st.download_dir = v;
            Task::none()
        }
        Msg::BrowseDownloadDir => Task::perform(
            async {
                rfd::AsyncFileDialog::new()
                    .pick_folder()
                    .await
                    .map(|h| h.path().to_path_buf())
            },
            Msg::BrowsedDownloadDir,
        ),
        Msg::BrowsedDownloadDir(Some(d)) => {
            st.download_dir = d.display().to_string();
            Task::none()
        }
        Msg::BrowsedDownloadDir(None) => Task::none(),
        Msg::WorkDir(v) => {
            st.work_dir = v;
            Task::none()
        }
        Msg::StartAtLogin(v) => {
            st.s.start_at_login = v;
            Task::none()
        }
        Msg::StartToTray(v) => {
            st.s.start_to_tray = v;
            Task::none()
        }
        Msg::MaxRetries(v) => {
            st.max_retries = v;
            Task::none()
        }
        Msg::FixedRetries(v) => {
            st.fixed_retries = v;
            Task::none()
        }
        Msg::RetryWait(v) => {
            st.retry_wait = v;
            Task::none()
        }
        Msg::UseServerTime(v) => {
            st.s.use_server_time = v;
            Task::none()
        }
        Msg::ConfirmIncomplete(v) => {
            st.s.remove_confirm_incomplete = v;
            Task::none()
        }
        Msg::ConfirmCompleted(v) => {
            st.s.remove_confirm_completed = v;
            Task::none()
        }
        Msg::CategoryExts(cat, v) => {
            if let Some(e) = st.cat_exts.iter_mut().find(|(c, _)| *c == cat) {
                e.1 = v;
            }
            Task::none()
        }
        Msg::CategoryReset(cat) => {
            if let Some(e) = st.cat_exts.iter_mut().find(|(c, _)| *c == cat) {
                e.1 = cat.default_extensions().join(", ");
            }
            Task::none()
        }
        Msg::AutoConnections(v) => {
            st.s.max_connections = if v { None } else { Some(8) };
            Task::none()
        }
        Msg::Concurrent(v) => {
            st.concurrent = v;
            Task::none()
        }
        Msg::SpeedLimit(v) => {
            st.speed_limit = v;
            Task::none()
        }
        Msg::Proxy(v) => {
            st.proxy = v;
            Task::none()
        }
        Msg::ConnectTimeout(v) => {
            st.connect_timeout = v;
            Task::none()
        }
        Msg::InvalidCerts(v) => {
            st.s.accept_invalid_certs = v;
            Task::none()
        }
        Msg::UserAgent(v) => {
            st.user_agent = v;
            Task::none()
        }
        Msg::RandomUa(v) => {
            st.s.randomize_user_agent = v;
            Task::none()
        }
        Msg::CustomHeaders(a) => {
            st.custom_headers.perform(a);
            Task::none()
        }
        Msg::IpcPort(v) => {
            st.ipc_port = v;
            Task::none()
        }
        Msg::CopyPairing => iced::clipboard::write(st.s.ext_token.clone()),
        Msg::Regenerate => {
            let client = st.client.clone();
            Task::perform(async move { client.regenerate_ext_token().await }, |_| {
                Msg::Noop
            })
        }
        Msg::ConflictHidden(v) => {
            use crate::domain::ConflictWhileHidden;
            st.s.conflict_while_hidden = match v.as_str() {
                "notify_and_park" => ConflictWhileHidden::NotifyAndPark,
                _ => ConflictWhileHidden::AutoPopup,
            };
            Task::none()
        }
        Msg::ShowCompleteDialog(v) => {
            st.s.show_complete_dialog = v;
            Task::none()
        }
        Msg::ThemeOverrides(a) => {
            st.theme_overrides.perform(a);
            Task::none()
        }
        Msg::UpdateFeed(v) => {
            st.update_feed = v;
            Task::none()
        }
        Msg::ResetSection => {
            let orig = st.original.clone();
            match st.section {
                Section::General => {
                    st.s.theme = orig.theme;
                    st.s.ui_density = orig.ui_density;
                    st.s.reduce_motion = orig.reduce_motion;
                    st.s.download_dir = orig.download_dir;
                    st.s.work_dir = orig.work_dir;
                    st.s.start_at_login = orig.start_at_login;
                    st.s.start_to_tray = orig.start_to_tray;
                }
                Section::Downloads => {
                    st.s.max_retries = orig.max_retries;
                    st.s.n_fixed_retries = orig.n_fixed_retries;
                    st.s.wait_between_retries = orig.wait_between_retries;
                    st.s.use_server_time = orig.use_server_time;
                    st.s.remove_confirm_incomplete = orig.remove_confirm_incomplete;
                    st.s.remove_confirm_completed = orig.remove_confirm_completed;
                }
                Section::Categories => st.s.category_extensions = orig.category_extensions,
                Section::Network => {
                    st.s.max_connections = orig.max_connections;
                    st.s.max_concurrent_downloads = orig.max_concurrent_downloads;
                    st.s.speed_limit = orig.speed_limit;
                    st.s.proxy = orig.proxy;
                    st.s.connect_timeout = orig.connect_timeout;
                    st.s.accept_invalid_certs = orig.accept_invalid_certs;
                    st.s.user_agent = orig.user_agent;
                    st.s.randomize_user_agent = orig.randomize_user_agent;
                    st.s.headers = orig.headers;
                }
                Section::Browser => {
                    st.s.ipc_port = orig.ipc_port;
                    st.s.conflict_while_hidden = orig.conflict_while_hidden;
                }
                Section::Notifications => st.s.show_complete_dialog = orig.show_complete_dialog,
                Section::Advanced | Section::About => {}
            }
            mirror(st);
            Task::none()
        }
        Msg::Save => {
            // fold string mirrors into Settings
            let mut s = st.s.clone();
            s.download_dir = std::path::PathBuf::from(st.download_dir.trim());
            s.work_dir = std::path::PathBuf::from(st.work_dir.trim());
            if let Ok(v) = st.max_retries.trim().parse() {
                s.max_retries = v;
            }
            if let Ok(v) = st.fixed_retries.trim().parse() {
                s.n_fixed_retries = v;
            }
            if let Ok(v) = humantime::parse_duration(st.retry_wait.trim()) {
                s.wait_between_retries = v;
            }
            if let Ok(v) = st.concurrent.trim().parse() {
                s.max_concurrent_downloads = v;
            }
            s.speed_limit = st.speed_limit.trim().parse().ok();
            s.proxy = (!st.proxy.trim().is_empty()).then(|| st.proxy.trim().to_owned());
            s.connect_timeout = humantime::parse_duration(st.connect_timeout.trim()).ok();
            s.user_agent =
                (!st.user_agent.trim().is_empty()).then(|| st.user_agent.trim().to_owned());
            if let Ok(v) = st.ipc_port.trim().parse() {
                s.ipc_port = v;
            }
            s.update_feed_url = st.update_feed.trim().to_owned();
            s.category_extensions = st
                .cat_exts
                .iter()
                .map(|(c, txt)| {
                    (
                        *c,
                        txt.split(',')
                            .map(|e| e.trim().to_lowercase())
                            .filter(|e| !e.is_empty())
                            .collect::<Vec<_>>(),
                    )
                })
                .collect();
            s.headers = st
                .custom_headers
                .text()
                .lines()
                .filter_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    Some((k.trim().to_owned(), v.trim().to_owned()))
                })
                .collect();
            s.theme_overrides = st
                .theme_overrides
                .text()
                .lines()
                .filter_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    Some((k.trim().to_owned(), v.trim().to_owned()))
                })
                .collect();
            let client = st.client.clone();
            Task::perform(async move { client.update_settings(s).await }, Msg::Saved)
        }
        Msg::Saved(Ok(())) => iced::exit(),
        Msg::Saved(Err(_)) => Task::none(),
        Msg::Cancel => iced::exit(),
        Msg::WinResized(w, h) => {
            chrome::enforce_min_size(iced::Size::new(w, h), iced::Size::new(640.0, 558.0))
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
        Msg::Connected(_) | Msg::Window(_) | Msg::Noop => Task::none(),
    }
}

pub fn subscription(app: &App) -> Subscription<Msg> {
    let App::Ready(st) = app else {
        return Subscription::none();
    };
    let mut subs = vec![
        iced::event::listen_with(|event, _status, _id| match event {
            iced::Event::Window(iced::window::Event::Resized(size)) => {
                Some(Msg::WinResized(size.width, size.height))
            }
            _ => None,
        }),
        crate::gui::ipc::all_events().map(Msg::Daemon),
    ];
    if st.shot.is_some() {
        subs.push(Shot::frames().map(|_| Msg::ShotTick));
    }
    Subscription::batch(subs)
}

// ---------------------------------------------------------------- view

pub fn view(app: &App) -> Element<'_, Msg> {
    match app {
        App::Connecting => splash("Connecting…".to_owned()),
        App::Failed(e) => splash(e.clone()),
        App::Ready(st) => ready_view(st),
    }
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

fn ready_view(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let t2 = *t;

    // Left section list.
    let mut list = column![].spacing(2.0).padding(theme::space::S2);
    for (sec, icon, label) in Section::ALL {
        let active = st.section == sec;
        let fg = if active { t.fg_1 } else { t.fg_2 };
        list = list.push(
            mouse_area(
                container(
                    row![
                        icons::icon(icon, 15.0, if active { t.action_primary } else { t.fg_3 }),
                        text(label).font(theme::BODY_MEDIUM).size(13.0).color(fg),
                    ]
                    .spacing(theme::space::S2)
                    .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .height(Length::Fixed(34.0))
                .align_y(Alignment::Center)
                .padding([0.0, theme::space::S3])
                .style(move |_| container::Style {
                    background: active.then(|| t2.bg_raised.into()),
                    border: iced::Border {
                        radius: theme::control::RADIUS.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
            )
            .on_press(Msg::SetSection(sec))
            .interaction(iced::mouse::Interaction::Pointer),
        );
    }
    let sidebar = container(list)
        .width(Length::Fixed(200.0))
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(t2.bg_sidebar.into()),
            ..Default::default()
        });

    let body: Element<'_, Msg> = match st.section {
        Section::General => general_section(st),
        Section::Downloads => downloads_section(st),
        Section::Categories => categories_section(st),
        Section::Network => network_section(st),
        Section::Browser => browser_section(st),
        Section::Notifications => notifications_section(st),
        Section::Advanced => advanced_section(st),
        Section::About => about_section(st),
    };

    let mut right = row![].spacing(theme::space::S2).align_y(Alignment::Center);
    if !matches!(st.section, Section::Advanced | Section::About) {
        right = right.push(
            Btn::new(format!("Reset {}", st.section.label()))
                .ghost()
                .icon("rotate-cw")
                .on_press(Msg::ResetSection)
                .view(t),
        );
    }
    right = right.push(
        Btn::new("Save")
            .primary()
            .icon("save")
            .on_press(Msg::Save)
            .view(t),
    );

    let footer_el = crate::gui::windows::add::footer(
        t,
        Btn::new("Cancel").ghost().on_press(Msg::Cancel).view(t),
        right.into(),
    );

    let page = column![
        titlebar::titlebar(t, "Settings", false, Msg::Window),
        hairline(t.border_subtle),
        row![
            sidebar,
            crate::gui::widget::vscroll(
                container(body)
                    .padding(iced::Padding {
                        top: theme::space::S4,
                        bottom: theme::space::S4,
                        left: theme::space::S4,
                        right: theme::space::S4 - crate::gui::widget::SCROLL_GUTTER,
                    })
                    .width(Length::Fill)
            )
            .height(Length::Fill)
        ]
        .height(Length::Fill),
        hairline(t.border_subtle),
        footer_el,
    ];

    let content = container(page)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(t2.bg_page.into()),
            text_color: Some(t2.fg_1),
            ..Default::default()
        });
    chrome::resize::resizable(t, content.into(), true, Msg::Window)
}

// ---- design constants (no magic numbers) ----------------------------

/// `.s-pane-head` title — Fraunces h2 (design h2; opsz 72 not exposed on
/// tiny-skia, so the bundled SemiBold face stands in).
const PANE_HEAD_TITLE_SIZE: f32 = 22.0;
/// `.s-pane-head` muted description line (body, fg_3).
const PANE_HEAD_DESC_SIZE: f32 = 13.0;

/// `NumberStepper` clamps. Retry counts allow zero; the saved value still
/// flows through the existing string mirror + `Save` parse.
const RETRIES_MIN: i64 = 0;
const RETRIES_MAX: i64 = 20;
/// Concurrent downloads must stay ≥ 1 (zero would stall the queue).
const CONCURRENT_MIN: i64 = 1;
const CONCURRENT_MAX: i64 = 20;

/// Per-pane head (`.s-pane-head`): Fraunces h2 title + muted description
/// line + a 1px bottom rule.
fn pane_head<'a>(t: &Tokens, title: &str, desc: &str) -> Element<'a, Msg> {
    column![
        text(title.to_owned())
            .font(theme::DISPLAY)
            .size(PANE_HEAD_TITLE_SIZE)
            .color(t.fg_1),
        text(desc.to_owned())
            .font(theme::BODY)
            .size(PANE_HEAD_DESC_SIZE)
            .color(t.fg_3),
        hairline(t.border_subtle),
    ]
    .spacing(theme::space::S2)
    .into()
}

/// Wrap a section's body with its pane-head.
fn pane<'a>(t: &Tokens, section: Section, body: Element<'a, Msg>) -> Element<'a, Msg> {
    column![pane_head(t, section.label(), section.desc()), body]
        .spacing(theme::space::S4)
        .into()
}

/// Boolean `.set-row`: label left (fills), `controls::toggle` right.
fn toggle_row<'a>(
    t: &Tokens,
    label: &'a str,
    on: bool,
    msg: impl Fn(bool) -> Msg + 'a,
) -> Element<'a, Msg> {
    row![
        text(label)
            .font(theme::BODY)
            .size(13.0)
            .color(t.fg_1)
            .width(Length::Fill),
        toggle(t, on, true, msg),
    ]
    .spacing(theme::space::S2)
    .align_y(Alignment::Center)
    .into()
}

/// Bounded numeric `.set-row` control: a `NumberStepper` whose value reads
/// from / writes to the existing string mirror (message wiring preserved).
fn stepper<'a>(
    t: &Tokens,
    value_str: &str,
    default: i64,
    min: i64,
    max: i64,
    msg: impl Fn(String) -> Msg + 'a,
) -> Element<'a, Msg> {
    let v = value_str
        .trim()
        .parse::<i64>()
        .unwrap_or(default)
        .clamp(min, max);
    number_stepper(t, v, min, max, true, move |n| msg(n.to_string()))
}

fn label_input<'a>(t: &Tokens, label: &str, input: Element<'a, Msg>) -> Element<'a, Msg> {
    column![crate::gui::widget::field_label(t, label), input]
        .spacing(theme::space::S1 + 2.0)
        .into()
}

fn inline_input<'a>(t: &Tokens, label: &'a str, input: Element<'a, Msg>) -> Element<'a, Msg> {
    row![
        text(label).font(theme::BODY).size(13.0).color(t.fg_2),
        input
    ]
    .spacing(theme::space::S2)
    .align_y(Alignment::Center)
    .into()
}

fn general_section(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    // Design `.s-seg` segments, extended with an Auto segment so every
    // `AppTheme` stays reachable: Auto (System / follow-OS) / Utility
    // (Light palette) / Warm / Dark. Wired through the unchanged
    // `Msg::SetTheme(String)` parser (incl. its live `_ => System` arm).
    let theme_idx = match st.s.theme {
        AppTheme::System => 0,
        AppTheme::Light => 1,
        AppTheme::Warm => 2,
        AppTheme::Dark => 3,
    };
    // `.s-seg` density segments, mirroring the Theme control: writes back
    // to `Settings.ui_density` via `Msg::SetDensity`, persisted on Save.
    let density_idx = match st.s.ui_density {
        Density::Comfortable => 0,
        Density::Compact => 1,
    };
    pane(
        t,
        Section::General,
        column![
            section_card(
                t,
                "moon",
                "Appearance",
                column![
                    label_input(
                        t,
                        "theme",
                        segmented(
                            t,
                            &[
                                ("Auto", None),
                                ("Utility", None),
                                ("Warm", None),
                                ("Dark", None)
                            ],
                            theme_idx,
                            BtnSize::Md,
                            |i| Msg::SetTheme(
                                match i {
                                    1 => "light",
                                    2 => "warm",
                                    3 => "dark",
                                    _ => "system",
                                }
                                .to_owned()
                            ),
                        )
                    ),
                    label_input(
                        t,
                        "density",
                        column![
                            segmented(
                                t,
                                &[("Comfortable", None), ("Compact", None)],
                                density_idx,
                                BtnSize::Md,
                                |i| Msg::SetDensity(match i {
                                    1 => Density::Compact,
                                    _ => Density::Comfortable,
                                }),
                            ),
                            text("Comfortable spacing or compact rows.")
                                .font(theme::BODY)
                                .size(12.0)
                                .color(t.fg_3),
                        ]
                        .spacing(theme::space::S1 + 2.0)
                        .into()
                    ),
                    toggle_row(
                        t,
                        "Reduce motion (skip animations)",
                        st.s.reduce_motion,
                        Msg::ReduceMotion
                    ),
                ]
                .spacing(theme::space::S3)
                .into()
            ),
            section_card(
                t,
                "save",
                "Storage",
                column![
                    label_input(
                        t,
                        "default download folder",
                        FileInput::new(&st.download_dir)
                            .on_input(Msg::DownloadDir)
                            .on_browse(Msg::BrowseDownloadDir)
                            .view(t)
                    ),
                    label_input(
                        t,
                        "in-flight cache folder (per-job .part + metadata)",
                        FileInput::new(&st.work_dir).on_input(Msg::WorkDir).view(t)
                    ),
                ]
                .spacing(theme::space::S3)
                .into()
            ),
            section_card(
                t,
                "settings",
                "Misc",
                column![
                    toggle_row(
                        t,
                        "Start oxdm on system login",
                        st.s.start_at_login,
                        Msg::StartAtLogin
                    ),
                    toggle_row(
                        t,
                        "Start to tray (no main window on boot)",
                        st.s.start_to_tray,
                        Msg::StartToTray
                    ),
                ]
                .spacing(theme::space::S3)
                .into()
            ),
        ]
        .spacing(theme::space::S3)
        .into(),
    )
}

fn downloads_section(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    pane(
        t,
        Section::Downloads,
        column![
            section_card(
                t,
                "rotate-cw",
                "Behavior",
                column![
                    inline_input(
                        t,
                        "Max retries",
                        stepper(
                            t,
                            &st.max_retries,
                            3,
                            RETRIES_MIN,
                            RETRIES_MAX,
                            Msg::MaxRetries
                        )
                    ),
                    inline_input(
                        t,
                        "Fixed retries before backoff",
                        stepper(
                            t,
                            &st.fixed_retries,
                            3,
                            RETRIES_MIN,
                            RETRIES_MAX,
                            Msg::FixedRetries
                        )
                    ),
                    inline_input(
                        t,
                        "Wait between retries",
                        TextInput::new(&st.retry_wait)
                            .width(Length::Fixed(120.0))
                            .on_input(Msg::RetryWait)
                            .view(t)
                    ),
                    toggle_row(
                        t,
                        "Use server-provided last-modified time",
                        st.s.use_server_time,
                        Msg::UseServerTime
                    ),
                ]
                .spacing(theme::space::S3)
                .into()
            ),
            section_card(
                t,
                "trash-2",
                "Remove behavior",
                column![
                    toggle_row(
                        t,
                        "Confirm before removing incomplete downloads",
                        st.s.remove_confirm_incomplete,
                        Msg::ConfirmIncomplete
                    ),
                    toggle_row(
                        t,
                        "Confirm before removing completed downloads",
                        st.s.remove_confirm_completed,
                        Msg::ConfirmCompleted
                    ),
                ]
                .spacing(theme::space::S3)
                .into()
            ),
        ]
        .spacing(theme::space::S3)
        .into(),
    )
}

fn categories_section(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let mut rows = column![
        text("Override file extensions per category. Comma-separated, no dots.")
            .font(theme::BODY)
            .size(12.0)
            .color(t.fg_3),
    ]
    .spacing(theme::space::S3);
    for (cat, exts) in &st.cat_exts {
        let cat = *cat;
        rows = rows.push(
            row![
                container(
                    text(format!("{}:", cat.label()))
                        .font(theme::BODY)
                        .size(13.0)
                        .color(t.fg_1)
                )
                .width(Length::Fixed(110.0)),
                TextInput::new(exts)
                    .mono()
                    .on_input(move |v| Msg::CategoryExts(cat, v))
                    .view(t),
                Btn::new("Reset")
                    .ghost()
                    .accent(true)
                    .size(BtnSize::Sm)
                    .on_press(Msg::CategoryReset(cat))
                    .view(t),
            ]
            .spacing(theme::space::S2)
            .align_y(Alignment::Center),
        );
    }
    pane(
        t,
        Section::Categories,
        section_card(t, "folder", "Categories", rows.into()),
    )
}

fn network_section(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let t3 = *t;
    pane(
        t,
        Section::Network,
        column![
            section_card(
                t,
                "activity",
                "Network",
                column![
                    toggle_row(
                        t,
                        "Determine connections per file automatically (by file size)",
                        st.s.max_connections.is_none(),
                        Msg::AutoConnections
                    ),
                    inline_input(
                        t,
                        "Concurrent downloads",
                        stepper(
                            t,
                            &st.concurrent,
                            3,
                            CONCURRENT_MIN,
                            CONCURRENT_MAX,
                            Msg::Concurrent
                        )
                    ),
                    inline_input(
                        t,
                        "Speed limit (B/s — blank for unlimited)",
                        TextInput::new(&st.speed_limit)
                            .width(Length::Fixed(140.0))
                            .on_input(Msg::SpeedLimit)
                            .view(t)
                    ),
                    inline_input(
                        t,
                        "Proxy URL",
                        TextInput::new(&st.proxy)
                            .width(Length::Fill)
                            .on_input(Msg::Proxy)
                            .view(t)
                    ),
                    inline_input(
                        t,
                        "Connect timeout",
                        TextInput::new(&st.connect_timeout)
                            .width(Length::Fixed(100.0))
                            .on_input(Msg::ConnectTimeout)
                            .view(t)
                    ),
                    toggle_row(
                        t,
                        "Accept invalid TLS certificates (dangerous)",
                        st.s.accept_invalid_certs,
                        Msg::InvalidCerts
                    ),
                ]
                .spacing(theme::space::S3)
                .into()
            ),
            section_card(
                t,
                "user",
                "Identity",
                column![
                    inline_input(
                        t,
                        "Custom User-Agent",
                        TextInput::new(&st.user_agent)
                            .width(Length::Fill)
                            .on_input(Msg::UserAgent)
                            .view(t)
                    ),
                    toggle_row(
                        t,
                        "Randomize User-Agent per request",
                        st.s.randomize_user_agent,
                        Msg::RandomUa
                    ),
                    label_input(
                        t,
                        "custom headers",
                        text_editor::TextEditor::new(&st.custom_headers)
                            .font(theme::MONO)
                            .size(12.0)
                            .height(Length::Fixed(64.0))
                            .on_action(Msg::CustomHeaders)
                            .style(move |_th, _| text_editor::Style {
                                background: t3.bg_raised.into(),
                                border: iced::Border {
                                    color: t3.border_subtle,
                                    width: 1.0,
                                    radius: theme::control::RADIUS.into(),
                                },
                                placeholder: t3.fg_4,
                                value: t3.fg_1,
                                selection: t3.selection_bg(),
                            })
                            .into()
                    ),
                ]
                .spacing(theme::space::S3)
                .into()
            ),
        ]
        .spacing(theme::space::S3)
        .into(),
    )
}

fn browser_section(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let t2 = *t;
    let conflict = match st.s.conflict_while_hidden {
        crate::domain::ConflictWhileHidden::AutoPopup => "auto_popup",
        crate::domain::ConflictWhileHidden::NotifyAndPark => "notify_and_park",
    };
    pane(
        t,
        Section::Browser,
        section_card(
            t,
            "puzzle",
            "Browser integration",
            column![
                inline_input(
                    t,
                    "IPC port",
                    TextInput::new(&st.ipc_port)
                        .width(Length::Fixed(100.0))
                        .on_input(Msg::IpcPort)
                        .view(t)
                ),
                label_input(
                    t,
                    "pairing code",
                    row![
                        container(
                            text(st.s.ext_token.clone())
                                .font(theme::MONO)
                                .size(11.0)
                                .color(t.fg_2)
                        )
                        .width(Length::Fill)
                        .height(Length::Fixed(theme::control::H_MD))
                        .align_y(Alignment::Center)
                        .padding([0.0, theme::control::INPUT_PAD_X])
                        .style(move |_| container::Style {
                            background: Some(t2.bg_sunken.into()),
                            border: iced::Border {
                                color: t2.border_subtle,
                                width: 1.0,
                                radius: theme::control::RADIUS.into(),
                            },
                            ..Default::default()
                        }),
                        Btn::new("Copy")
                            .toolbar()
                            .icon("copy")
                            .on_press(Msg::CopyPairing)
                            .view(t),
                        Btn::new("Regenerate")
                            .toolbar()
                            .icon("rotate-cw")
                            .on_press(Msg::Regenerate)
                            .view(t),
                    ]
                    .spacing(theme::space::S2)
                    .align_y(Alignment::Center)
                    .into()
                ),
                label_input(
                    t,
                    "conflict while dialog hidden",
                    combo(
                        t,
                        vec!["auto_popup".to_owned(), "notify_and_park".to_owned()],
                        Some(conflict.to_owned()),
                        Msg::ConflictHidden,
                        Length::Fill,
                    )
                ),
            ]
            .spacing(theme::space::S3)
            .into(),
        ),
    )
}

fn notifications_section(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    pane(
        t,
        Section::Notifications,
        section_card(
        t,
        "bell",
        "Notifications",
        column![
            toggle_row(
                t,
                "Show download-complete dialog when a download finishes",
                st.s.show_complete_dialog,
                Msg::ShowCompleteDialog
            ),
            text("System notifications follow your queue's on-finish hooks (see Queues & scheduling).")
                .font(theme::BODY)
                .size(11.0)
                .color(t.fg_3),
        ]
        .spacing(theme::space::S2)
        .into(),
        ),
    )
}

fn advanced_section(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let t3 = *t;
    pane(
        t,
        Section::Advanced,
        column![
            section_card(
                t,
                "palette",
                "Theme overrides",
                label_input(
                    t,
                    "overrides — accent / bg / text (one per line)",
                    text_editor::TextEditor::new(&st.theme_overrides)
                        .font(theme::MONO)
                        .size(12.0)
                        .height(Length::Fixed(64.0))
                        .on_action(Msg::ThemeOverrides)
                        .style(move |_th, _| text_editor::Style {
                            background: t3.bg_raised.into(),
                            border: iced::Border {
                                color: t3.border_subtle,
                                width: 1.0,
                                radius: theme::control::RADIUS.into(),
                            },
                            placeholder: t3.fg_4,
                            value: t3.fg_1,
                            selection: t3.selection_bg(),
                        })
                        .into()
                )
            ),
            section_card(
                t,
                "cloud-upload",
                "Updates",
                inline_input(
                    t,
                    "Update feed URL",
                    TextInput::new(&st.update_feed)
                        .width(Length::Fill)
                        .on_input(Msg::UpdateFeed)
                        .view(t)
                )
            ),
        ]
        .spacing(theme::space::S3)
        .into(),
    )
}

fn about_section(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    pane(
        t,
        Section::About,
        section_card(
            t,
            "info",
            "About oxdm",
            column![
                text("oxdm").font(theme::DISPLAY).size(22.0).color(t.fg_1),
                text(format!("Version {}", env!("CARGO_PKG_VERSION")))
                    .font(theme::MONO)
                    .size(11.0)
                    .color(t.fg_2),
                text("A focused, native download manager.")
                    .font(theme::BODY)
                    .size(12.0)
                    .color(t.fg_3),
            ]
            .spacing(theme::space::S1)
            .into(),
        ),
    )
}

pub fn launch_settings() {
    let mut app = iced::application(boot, update, view)
        .title(|_: &App| "oxdm — Settings".to_owned())
        .theme(|app: &App| match app {
            App::Ready(st) => st.tokens.iced_theme(),
            _ => Tokens::dark().iced_theme(),
        })
        .subscription(subscription)
        .default_font(theme::BODY)
        .antialiasing(true)
        .window(chrome::window_settings(
            // Design `.dialog-settings` = 920×640; min stays 640×558 so
            // 920 only sets the default size and never risks clipping the
            // resizable window.
            iced::Size::new(920.0, 660.0),
            iced::Size::new(640.0, 558.0),
        ));
    for f in theme::fonts::ALL {
        app = app.font(*f);
    }
    if let Err(e) = app.run() {
        eprintln!("gui error: {e}");
        std::process::exit(1);
    }
}
