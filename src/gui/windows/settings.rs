//! Settings window (`oxdm gui settings [--tab t] [--highlight-proxy]`):
//! left section list (General / Downloads / Categories / Network /
//! Browser / Notifications / Advanced / About), per-section cards,
//! footer with Cancel / Reset-tab / Save.

use std::sync::Arc;
use std::time::Duration;

use iced::widget::{column, container, mouse_area, row, scrollable, text, text_editor};
use iced::{Alignment, Element, Length, Subscription, Task};

use crate::domain::{Category, Settings, Theme as AppTheme};
use crate::gui::chrome::{self, WindowControl, titlebar};
use crate::gui::icons;
use crate::gui::ipc::DaemonSignal;
use crate::gui::shot::Shot;
use crate::gui::theme::{self, Tokens};
use crate::gui::widget::{
    Btn, BtnSize, FileInput, TextInput, checkbox, combo, hairline, section_card,
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
}

#[derive(Clone)]
pub enum Msg {
    Connected(Result<Box<(Arc<Client>, Settings)>, String>),
    Daemon(DaemonSignal),
    Window(WindowControl),
    SetSection(Section),
    // General
    SetTheme(String),
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
            chrome::enforce_min_size(iced::Size::new(w, h), iced::Size::new(640.0, 480.0))
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
            scrollable(
                container(body)
                    .padding(theme::space::S4)
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
    let theme_label = match st.s.theme {
        AppTheme::Light => "light",
        AppTheme::Dark => "dark",
        AppTheme::Warm => "warm",
        AppTheme::System => "system",
    };
    column![
        section_card(
            t,
            "moon",
            "Appearance",
            column![
                label_input(
                    t,
                    "theme",
                    combo(
                        t,
                        vec![
                            "system".to_owned(),
                            "light".to_owned(),
                            "dark".to_owned(),
                            "warm".to_owned()
                        ],
                        Some(theme_label.to_owned()),
                        Msg::SetTheme,
                        Length::Fill,
                    )
                ),
                checkbox(
                    t,
                    "Reduce motion (skip animations)",
                    st.s.reduce_motion,
                    true,
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
                checkbox(
                    t,
                    "Start oxdm on system login",
                    st.s.start_at_login,
                    true,
                    Msg::StartAtLogin
                ),
                checkbox(
                    t,
                    "Start to tray (no main window on boot)",
                    st.s.start_to_tray,
                    true,
                    Msg::StartToTray
                ),
            ]
            .spacing(theme::space::S3)
            .into()
        ),
    ]
    .spacing(theme::space::S3)
    .into()
}

fn downloads_section(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    column![
        section_card(
            t,
            "rotate-cw",
            "Behavior",
            column![
                inline_input(
                    t,
                    "Max retries",
                    TextInput::new(&st.max_retries)
                        .width(Length::Fixed(80.0))
                        .on_input(Msg::MaxRetries)
                        .view(t)
                ),
                inline_input(
                    t,
                    "Fixed retries before backoff",
                    TextInput::new(&st.fixed_retries)
                        .width(Length::Fixed(80.0))
                        .on_input(Msg::FixedRetries)
                        .view(t)
                ),
                inline_input(
                    t,
                    "Wait between retries",
                    TextInput::new(&st.retry_wait)
                        .width(Length::Fixed(120.0))
                        .on_input(Msg::RetryWait)
                        .view(t)
                ),
                checkbox(
                    t,
                    "Use server-provided last-modified time",
                    st.s.use_server_time,
                    true,
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
                checkbox(
                    t,
                    "Confirm before removing incomplete downloads",
                    st.s.remove_confirm_incomplete,
                    true,
                    Msg::ConfirmIncomplete
                ),
                checkbox(
                    t,
                    "Confirm before removing completed downloads",
                    st.s.remove_confirm_completed,
                    true,
                    Msg::ConfirmCompleted
                ),
            ]
            .spacing(theme::space::S3)
            .into()
        ),
    ]
    .spacing(theme::space::S3)
    .into()
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
    section_card(t, "folder", "Categories", rows.into())
}

fn network_section(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let t3 = *t;
    column![
        section_card(
            t,
            "activity",
            "Network",
            column![
                checkbox(
                    t,
                    "Determine connections per file automatically (by file size)",
                    st.s.max_connections.is_none(),
                    true,
                    Msg::AutoConnections
                ),
                inline_input(
                    t,
                    "Concurrent downloads",
                    TextInput::new(&st.concurrent)
                        .width(Length::Fixed(80.0))
                        .on_input(Msg::Concurrent)
                        .view(t)
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
                checkbox(
                    t,
                    "Accept invalid TLS certificates (dangerous)",
                    st.s.accept_invalid_certs,
                    true,
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
                checkbox(
                    t,
                    "Randomize User-Agent per request",
                    st.s.randomize_user_agent,
                    true,
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
    .into()
}

fn browser_section(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let t2 = *t;
    let conflict = match st.s.conflict_while_hidden {
        crate::domain::ConflictWhileHidden::AutoPopup => "auto_popup",
        crate::domain::ConflictWhileHidden::NotifyAndPark => "notify_and_park",
    };
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
    )
}

fn notifications_section(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    section_card(
        t,
        "bell",
        "Notifications",
        column![
            checkbox(
                t,
                "Show download-complete dialog when a download finishes",
                st.s.show_complete_dialog,
                true,
                Msg::ShowCompleteDialog
            ),
            text("System notifications follow your queue's on-finish hooks (see Queues & scheduling).")
                .font(theme::BODY)
                .size(11.0)
                .color(t.fg_3),
        ]
        .spacing(theme::space::S2)
        .into(),
    )
}

fn advanced_section(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let t3 = *t;
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
    .into()
}

fn about_section(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
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
            iced::Size::new(820.0, 660.0),
            iced::Size::new(640.0, 480.0),
        ));
    for f in theme::fonts::ALL {
        app = app.font(*f);
    }
    if let Err(e) = app.run() {
        eprintln!("gui error: {e}");
        std::process::exit(1);
    }
}
