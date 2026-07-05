//! Settings window (`oxdm gui settings [--tab t] [--highlight-proxy]`):
//! left section list (General / Downloads / Categories / Network /
//! Browser / Notifications / Advanced / About), per-section cards,
//! footer with Cancel / Reset-tab / Save.

use std::sync::Arc;
use std::time::Duration;

use iced::widget::{column, container, mouse_area, row, text, text_editor};
use iced::{Alignment, Element, Length, Subscription, Task};

use crate::domain::{Category, Density, Queue, QueueId, Settings, Theme as AppTheme};
use crate::gui::chrome::{self, WindowControl, titlebar};
use crate::gui::color;
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
            Section::Categories => {
                "Categories auto-sort downloads by file extension. \
                 Edit save folders and detected types."
            }
            Section::Network => "Connections, bandwidth, proxy, and request identity.",
            Section::Browser => "Pair the browser extension and resolve capture conflicts.",
            Section::Notifications => "What oxdm tells you when a download finishes.",
            Section::Advanced => "Theme overrides, the update feed, and reset.",
            Section::About => "Version and project information.",
        }
    }
}

/// Default-queue option for a category `combo`. Equality is id-only so
/// duplicate queue names still select correctly.
#[derive(Debug, Clone)]
pub struct QueueChoice {
    id: Option<QueueId>,
    name: String,
}

impl PartialEq for QueueChoice {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl std::fmt::Display for QueueChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

impl State {
    /// Options for a category's default-queue combo: "Default" (daemon
    /// picks) plus every known queue.
    fn queue_choices(&self) -> Vec<QueueChoice> {
        let mut v = vec![QueueChoice {
            id: None,
            name: "Default".to_owned(),
        }];
        v.extend(self.queues.iter().map(|q| QueueChoice {
            id: Some(q.id),
            name: q.name.clone(),
        }));
        v
    }

    /// The current selection for a category (falls back to "Default"
    /// when the stored queue no longer exists).
    fn queue_choice_for(&self, sel: Option<QueueId>) -> QueueChoice {
        sel.and_then(|id| self.queues.iter().find(|q| q.id == id))
            .map(|q| QueueChoice {
                id: Some(q.id),
                name: q.name.clone(),
            })
            .unwrap_or(QueueChoice {
                id: None,
                name: "Default".to_owned(),
            })
    }
}

/// Boot payload: client + settings snapshot + queue list (for the
/// category default-queue pickers).
type BootPayload = Box<(Arc<Client>, Settings, Vec<Queue>)>;

#[derive(Clone)]
pub enum Msg {
    Connected(Result<BootPayload, String>),
    Daemon(DaemonSignal),
    Window(WindowControl),
    QueuesLoaded(Vec<Queue>),
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
    CategoryToggle(Category),
    CategoryExts(Category, String),
    CategoryReset(Category),
    CategoryFolder(Category, String),
    BrowseCategoryFolder(Category),
    BrowsedCategoryFolder(Category, Option<std::path::PathBuf>),
    CategoryQueue(Category, QueueChoice),
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
    ResetDbAsk,
    ResetDbCancel,
    ResetDbConfirm,
    KeyPressed(iced::keyboard::Key),
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
    /// Per-category save folder mirror (blank = inherit the default
    /// download folder; `Settings.category_folders`).
    cat_folders: Vec<(Category, String)>,
    /// Per-category default queue (`None` = daemon default;
    /// `Settings.category_queues`).
    cat_queues: Vec<(Category, Option<QueueId>)>,
    /// Which category accordion is expanded (one at a time, design
    /// `CategoryCard`).
    cat_open: Option<Category>,
    /// Queue snapshot for the default-queue pickers.
    queues: Vec<Queue>,
    /// Reset-oxdm confirm overlay (Advanced danger section).
    confirm_reset: bool,
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
    st.cat_folders = Category::ALL_ASSIGNABLE
        .iter()
        .map(|c| {
            let dir = s
                .category_folders
                .get(c)
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            (*c, dir)
        })
        .collect();
    st.cat_queues = Category::ALL_ASSIGNABLE
        .iter()
        .map(|c| (*c, s.category_queues.get(c).copied()))
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
                Ok(Box::new((client, snap.settings, snap.queues)))
            },
            Msg::Connected,
        ),
    )
}

pub fn update(app: &mut App, msg: Msg) -> Task<Msg> {
    match msg {
        Msg::Connected(Ok(boxed)) => {
            let (client, settings, queues) = *boxed;
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
                cat_folders: Vec::new(),
                cat_queues: Vec::new(),
                cat_open: None,
                queues,
                confirm_reset: false,
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
        Msg::Daemon(DaemonSignal::Event(Event::QueuesChanged)) => {
            let client = st.client.clone();
            Task::perform(async move { client.snapshot().await }, |r| match r {
                Ok(s) => Msg::QueuesLoaded(s.queues),
                Err(_) => Msg::Noop,
            })
        }
        Msg::Daemon(_) => Task::none(),
        Msg::QueuesLoaded(qs) => {
            // Drop stale default-queue picks (queue deleted meanwhile);
            // the daemon ignores stale ids anyway — mirror that honestly.
            for (_, sel) in &mut st.cat_queues {
                if sel.is_some_and(|id| !qs.iter().any(|q| q.id == id)) {
                    *sel = None;
                }
            }
            st.queues = qs;
            Task::none()
        }
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
        Msg::CategoryToggle(cat) => {
            st.cat_open = (st.cat_open != Some(cat)).then_some(cat);
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
        Msg::CategoryFolder(cat, v) => {
            if let Some(e) = st.cat_folders.iter_mut().find(|(c, _)| *c == cat) {
                e.1 = v;
            }
            Task::none()
        }
        Msg::BrowseCategoryFolder(cat) => Task::perform(
            async {
                rfd::AsyncFileDialog::new()
                    .pick_folder()
                    .await
                    .map(|h| h.path().to_path_buf())
            },
            move |p| Msg::BrowsedCategoryFolder(cat, p),
        ),
        Msg::BrowsedCategoryFolder(cat, Some(d)) => {
            if let Some(e) = st.cat_folders.iter_mut().find(|(c, _)| *c == cat) {
                e.1 = d.display().to_string();
            }
            Task::none()
        }
        Msg::BrowsedCategoryFolder(_, None) => Task::none(),
        Msg::CategoryQueue(cat, choice) => {
            if let Some(e) = st.cat_queues.iter_mut().find(|(c, _)| *c == cat) {
                e.1 = choice.id;
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
        Msg::ResetDbAsk => {
            st.confirm_reset = true;
            Task::none()
        }
        Msg::ResetDbCancel => {
            st.confirm_reset = false;
            Task::none()
        }
        Msg::ResetDbConfirm => {
            st.confirm_reset = false;
            let client = st.client.clone();
            // The daemon backs up the DB, spawns its replacement and
            // exits; this window then closes via the DaemonSignal::Lost
            // path. The reply (or a dropped connection) needs no
            // handling beyond not crashing.
            Task::perform(async move { client.reset_database().await }, |_| Msg::Noop)
        }
        // Confirm-dialog keys (design `confirm-dialog.jsx`): Enter
        // confirms, Escape cancels — gated on the overlay being open.
        Msg::KeyPressed(key) => {
            use iced::keyboard::key::Named;
            if st.confirm_reset {
                match key.as_ref() {
                    iced::keyboard::Key::Named(Named::Enter) => {
                        return update_ready(st, Msg::ResetDbConfirm);
                    }
                    iced::keyboard::Key::Named(Named::Escape) => {
                        return update_ready(st, Msg::ResetDbCancel);
                    }
                    _ => {}
                }
            }
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
                Section::Categories => {
                    st.s.category_extensions = orig.category_extensions;
                    st.s.category_folders = orig.category_folders;
                    st.s.category_queues = orig.category_queues;
                }
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
            s.category_folders = st
                .cat_folders
                .iter()
                .filter(|(_, dir)| !dir.trim().is_empty())
                .map(|(c, dir)| (*c, std::path::PathBuf::from(dir.trim())))
                .collect();
            s.category_queues = st
                .cat_queues
                .iter()
                .filter_map(|(c, q)| q.map(|q| (*c, q)))
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
            iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { key, .. }) => {
                Some(Msg::KeyPressed(key))
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

fn ready_view(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let t2 = *t;

    // Left section list.
    let mut list = column![].spacing(2.0).padding(theme::space::S2);
    for (sec, icon, label) in Section::ALL {
        let active = st.section == sec;
        let fg = if active { t.fg_1 } else { t.fg_2 };
        // Design `.settings-nav .s-item.on`: font-weight 600 label +
        // clay-500 icon (clay-500 is theme-invariant in tokens.css).
        let label_font = if active {
            theme::BODY_BOLD
        } else {
            theme::BODY_MEDIUM
        };
        let icon_color = if active { color::clay::C500 } else { t.fg_3 };
        list = list.push(
            mouse_area(
                container(
                    row![
                        icons::icon(icon, 15.0, icon_color),
                        text(label).font(label_font).size(13.0).color(fg),
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

    let overlaid: Element<'_, Msg> = if st.confirm_reset {
        reset_overlay(st, page.into())
    } else {
        page.into()
    };

    let content = container(overlaid)
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

/// Icon tile size of the accordion header (design `CategoryCard` icon
/// tile, settings-dialog.jsx).
const CAT_ICON_TILE: f32 = 28.0;

/// Sidebar icon per category (same glyphs as the main-window sidebar).
fn cat_icon_name(cat: Category) -> &'static str {
    match cat {
        Category::Compressed => "archive",
        Category::Programs => "package",
        Category::Videos => "film",
        Category::Music => "music",
        Category::Pictures => "image",
        Category::Documents => "file-text",
        Category::Other => "file",
    }
}

/// Category tint (mirrors the main-window ext-pill/sidebar tints; the
/// mock's `CATEGORIES` lack tint fields — design-intent §3.7 correction
/// says to source them from the ext-pill map).
fn cat_tint(t: &Tokens, cat: Category) -> iced::Color {
    match cat {
        Category::Compressed => t.cat_compressed,
        Category::Programs => t.cat_programs,
        Category::Videos => t.cat_videos,
        Category::Music => t.cat_music,
        Category::Pictures => t.cat_pictures,
        Category::Documents => t.cat_documents,
        Category::Other => t.fg_3,
    }
}

/// Accordion `CategoryCard`s (design §3.7): header = tinted icon tile +
/// name + summary, one card expanded at a time; body = extensions
/// editor, save folder, default queue. No "Add custom category" tile
/// (Category is a fixed enum — documented BLOCKED) and no auto-extract
/// (no extraction engine).
fn categories_section(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let mut cards = column![
        text("Categories auto-sort by extension. Expand a card to edit extensions, save folder and default queue.")
            .font(theme::BODY)
            .size(12.0)
            .color(t.fg_3),
    ]
    .spacing(theme::space::S2);

    for (cat, exts) in &st.cat_exts {
        let cat = *cat;
        let open = st.cat_open == Some(cat);
        let tint = cat_tint(t, cat);
        let t2 = *t;
        let folder: &str = st
            .cat_folders
            .iter()
            .find(|(c, _)| *c == cat)
            .map(|(_, d)| d.as_str())
            .unwrap_or("");
        let queue_sel = st
            .cat_queues
            .iter()
            .find(|(c, _)| *c == cat)
            .and_then(|(_, q)| *q);

        let n_exts = exts.split(',').filter(|e| !e.trim().is_empty()).count();
        let summary = {
            let dest = if folder.trim().is_empty() {
                "Default folder".to_owned()
            } else {
                folder.to_owned()
            };
            format!("{n_exts} extensions · {dest}")
        };

        let icon_tile = container(icons::icon(cat_icon_name(cat), 14.0, tint))
            .width(Length::Fixed(CAT_ICON_TILE))
            .height(Length::Fixed(CAT_ICON_TILE))
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .style(move |_| container::Style {
                background: Some(color::with_alpha(tint, 0.12).into()),
                border: iced::Border {
                    radius: theme::control::RADIUS.into(),
                    ..Default::default()
                },
                ..Default::default()
            });

        let header = iced::widget::button(
            row![
                icon_tile,
                column![
                    text(cat.label())
                        .font(theme::BODY_MEDIUM)
                        .size(13.0)
                        .color(t.fg_1),
                    text(summary).font(theme::BODY).size(11.0).color(t.fg_3),
                ]
                .spacing(1.0),
                iced::widget::Space::new().width(Length::Fill),
                icons::icon(
                    if open { "chevron-up" } else { "chevron-down" },
                    14.0,
                    t.fg_3
                ),
            ]
            .spacing(theme::space::S3)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .padding(theme::space::S3)
        .on_press(Msg::CategoryToggle(cat))
        .style(move |_th, status| iced::widget::button::Style {
            background: Some(
                if matches!(status, iced::widget::button::Status::Hovered) && !open {
                    t2.bg_sunken.into()
                } else {
                    iced::Color::TRANSPARENT.into()
                },
            ),
            text_color: t2.fg_1,
            ..Default::default()
        });

        let mut card = column![header];
        if open {
            let body = column![
                label_input(
                    t,
                    "extensions — comma-separated, no dots",
                    TextInput::new(exts)
                        .mono()
                        .on_input(move |v| Msg::CategoryExts(cat, v))
                        .view(t)
                ),
                label_input(
                    t,
                    "save folder — blank inherits the default download folder",
                    FileInput::new(folder)
                        .on_input(move |v| Msg::CategoryFolder(cat, v))
                        .on_browse(Msg::BrowseCategoryFolder(cat))
                        .view(t)
                ),
                label_input(
                    t,
                    "default queue",
                    combo(
                        t,
                        st.queue_choices(),
                        Some(st.queue_choice_for(queue_sel)),
                        move |c| Msg::CategoryQueue(cat, c),
                        Length::Fill,
                    )
                ),
                row![
                    iced::widget::Space::new().width(Length::Fill),
                    Btn::new("Reset extensions")
                        .ghost()
                        .accent(true)
                        .size(BtnSize::Sm)
                        .on_press(Msg::CategoryReset(cat))
                        .view(t),
                ],
            ]
            .spacing(theme::space::S3);
            card = card.push(container(body).width(Length::Fill).padding(iced::Padding {
                top: 0.0,
                right: theme::space::S3,
                bottom: theme::space::S3,
                left: theme::space::S3,
            }));
        }

        cards = cards.push(
            container(card)
                .width(Length::Fill)
                .style(move |_| container::Style {
                    background: Some(t2.bg_raised.into()),
                    border: iced::Border {
                        color: if open {
                            t2.border_default
                        } else {
                            t2.border_subtle
                        },
                        width: 1.0,
                        radius: theme::surface::RADIUS.into(),
                    },
                    ..Default::default()
                }),
        );
    }

    pane(
        t,
        Section::Categories,
        section_card(t, "folder", "Categories", cards.into()),
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
            danger_section(st),
        ]
        .spacing(theme::space::S3)
        .into(),
    )
}

/// Rust-headed danger block (design §3.7 Advanced: own Reset section;
/// tokens follow the download-window completion warning).
fn danger_section(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let t2 = *t;
    container(
        column![
            row![
                icons::icon("triangle-alert", 14.0, t.status_danger),
                text("Danger zone")
                    .font(theme::BODY_BOLD)
                    .size(13.0)
                    .color(t.status_danger),
            ]
            .spacing(theme::space::S2)
            .align_y(Alignment::Center),
            text(
                "Reset oxdm backs up and clears the database — all jobs, queues and \
                 settings. Downloaded files stay on disk. The daemon exits and must be \
                 relaunched.",
            )
            .font(theme::BODY)
            .size(12.0)
            .color(t.fg_2),
            row![
                iced::widget::Space::new().width(Length::Fill),
                Btn::new("Reset oxdm…")
                    .danger_filled()
                    .icon("rotate-cw")
                    .on_press(Msg::ResetDbAsk)
                    .view(t),
            ],
        ]
        .spacing(theme::space::S3),
    )
    .width(Length::Fill)
    .padding(theme::space::S4)
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

/// Confirm overlay for the danger Reset (pattern: queues delete_overlay;
/// Enter/Escape handled in `Msg::KeyPressed`).
fn reset_overlay<'a>(st: &'a State, base: Element<'a, Msg>) -> Element<'a, Msg> {
    let t = &st.tokens;
    let t2 = *t;
    let card = container(
        column![
            text("Reset oxdm?")
                .font(theme::BODY_BOLD)
                .size(14.0)
                .color(t.fg_1),
            text(
                "The database is backed up, then all jobs, queues and settings are \
                 erased. Downloaded files are not touched. The daemon exits — relaunch \
                 oxdm to start fresh.",
            )
            .font(theme::BODY)
            .size(12.0)
            .color(t.fg_2),
            row![
                iced::widget::Space::new().width(Length::Fill),
                Btn::new("Cancel")
                    .ghost()
                    .on_press(Msg::ResetDbCancel)
                    .view(t),
                Btn::new("Reset oxdm")
                    .danger_filled()
                    .icon("rotate-cw")
                    .on_press(Msg::ResetDbConfirm)
                    .view(t),
            ]
            .spacing(theme::space::S2)
            .align_y(Alignment::Center),
        ]
        .spacing(theme::space::S3),
    )
    .width(Length::Fixed(400.0))
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
        .on_press(Msg::ResetDbCancel),
    );

    iced::widget::stack![
        base,
        scrim,
        container(iced::widget::opaque(card))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center),
    ]
    .into()
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
