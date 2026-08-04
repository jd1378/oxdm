//! Settings window (`oxdm gui settings [--tab t] [--highlight-proxy]`):
//! left section list (General / Downloads / Categories / Network /
//! Browser / Notifications / Advanced), per-section cards,
//! footer with Cancel / Reset-tab / Save.

use std::sync::Arc;
use std::time::Duration;

use iced::widget::{column, container, mouse_area, row, text};
use iced::{Alignment, Element, Length, Subscription, Task};

use crate::domain::{Category, ProxyAdv, ProxyMode, Queue, QueueId, Settings, Theme as AppTheme};
use crate::gui::chrome::{self, WindowControl, titlebar};
use crate::gui::color;
use crate::gui::icons;
use crate::gui::ipc::DaemonSignal;
use crate::gui::shot::Shot;
use crate::gui::theme::{self, Tokens};
use crate::gui::widget::{
    Btn, BtnSize, FileInput, PasswordInput, SECTION_GAP, TextInput, combo, hairline,
    number_stepper, segmented, set_group, set_note, set_row, set_row_panel, set_row_stack,
    set_section, set_section_danger, toggle,
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
}

impl Section {
    const ALL: [(Section, &'static str, &'static str); 7] = [
        (Section::General, "sliders-horizontal", "General"),
        (Section::Downloads, "download", "Downloads"),
        (Section::Categories, "folder", "Categories"),
        (Section::Network, "globe", "Network"),
        (Section::Browser, "puzzle", "Browser"),
        (Section::Notifications, "bell", "Notifications"),
        (Section::Advanced, "terminal", "Advanced"),
    ];
    fn label(self) -> &'static str {
        Self::ALL.iter().find(|(s, _, _)| *s == self).unwrap().2
    }
    /// Muted one-line description shown under the pane-head title
    /// (design `.s-pane-head`).
    fn desc(self) -> &'static str {
        match self {
            Section::General => "Startup, appearance, and when to hold downloads back.",
            Section::Downloads => "Where files land, retry behavior, and removal confirmations.",
            Section::Categories => {
                "Categories auto-sort downloads by file extension. \
                 Edit save folders and detected types."
            }
            Section::Network => "Connections, bandwidth, proxy, and request identity.",
            Section::Browser => "Pair the browser extension and resolve capture conflicts.",
            Section::Notifications => "What oxdm tells you, and how, for each event.",
            Section::Advanced => "Reset oxdm to a clean database.",
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
    /// Options for a category's default-queue combo: the real queues,
    /// nothing else. A "Default" entry only named whichever queue the
    /// daemon would have picked anyway, so it read as a distinct choice
    /// while being one of the rows below it.
    fn queue_choices(&self) -> Vec<QueueChoice> {
        self.queues
            .iter()
            .map(|q| QueueChoice {
                id: Some(q.id),
                name: q.name.clone(),
            })
            .collect()
    }

    /// The current selection for a category. An unset category — or one
    /// pointing at a deleted queue — shows the first queue, which is the
    /// one the daemon would use.
    fn queue_choice_for(&self, sel: Option<QueueId>) -> QueueChoice {
        sel.and_then(|id| self.queues.iter().find(|q| q.id == id))
            .or_else(|| self.queues.first())
            .map(|q| QueueChoice {
                id: Some(q.id),
                name: q.name.clone(),
            })
            .unwrap_or(QueueChoice {
                id: None,
                name: String::new(),
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
    ConfirmClean(bool),
    PauseOnMetered(bool),
    PauseOnLowBattery(bool),
    // Categories
    CategoryToggle(Category),
    CategoryExts(Category, String),
    CategoryReset(Category),
    CategoryFolder(Category, String),
    BrowseCategoryFolder(Category),
    BrowsedCategoryFolder(Category, Option<std::path::PathBuf>),
    CategoryQueue(Category, QueueChoice),
    // Network
    Connections(Option<u64>),
    Concurrent(String),
    SpeedLimitOn(bool),
    SpeedLimitValue(String),
    SpeedLimitUnit(bool),
    ProxyMode(usize),
    ProxyHost(String),
    ProxyPort(String),
    ProxyAuth(bool),
    ProxyUser(String),
    ProxyPass(String),
    ConnectTimeout(String),
    InvalidCerts(bool),
    UserAgent(String),
    RandomUa(bool),
    HeaderName(usize, String),
    HeaderValue(usize, String),
    HeaderRemove(usize),
    HeaderAdd,
    // Browser
    IpcPort(String),
    CopyPairing,
    Regenerate,
    ConflictHidden(String),
    // Notifications
    ShowCompleteDialog(bool),
    NotifyComplete(bool),
    ShowFailedDialog(bool),
    NotifyFailed(bool),
    NotifyQueueFinished(bool),
    // Advanced
    ResetDbAsk,
    ResetDbCancel,
    ResetDbConfirm,
    KeyPressed(iced::keyboard::Key),
    // Footer
    ResetSection,
    Discard,
    Save,
    Saved(Result<(), String>),
    Cancel,
    WinResized(f32, f32),
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
    /// Speed-limit picker parts, same shape as the download window's
    /// limiter: on/off, a value, and the unit it is written in.
    limit_on: bool,
    limit_value: String,
    limit_unit_mb: bool,
    /// Proxy picker. The daemon stores these parts as they are; only
    /// the port is mirrored as text, since it is typed.
    proxy_mode: usize,
    proxy_host: String,
    proxy_port: String,
    proxy_auth: bool,
    proxy_user: String,
    proxy_pass: String,
    /// Edited this session. Empty + edited means "delete the stored
    /// secret"; empty + untouched leaves it alone, since the ciphertext
    /// never round-trips into the form.
    proxy_pass_edited: bool,
    /// A password is stored for the proxy (shown as a placeholder).
    has_stored_proxy_pass: bool,
    connect_timeout: String,
    user_agent: String,
    ipc_port: String,
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
    /// Custom request headers as edited: name/value pairs in the order
    /// shown, blanks included until the user fills or removes them.
    custom_headers: Vec<(String, String)>,
    shot: Option<Shot>,
    /// How many settings differ from what is saved. Drives the footer's
    /// Discard button; recomputed in `update_ready`.
    dirty: usize,
}

/// The settings this form would save: `st.s` with every string mirror
/// folded back in. Shared by Apply and the change count, so the button
/// can never disagree with what applying would write.
fn pending_settings(st: &State) -> Settings {
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
    s.speed_limit = st.limit_on.then(|| {
        let unit = if st.limit_unit_mb {
            BYTES_PER_MB
        } else {
            BYTES_PER_KB
        };
        st.limit_value.trim().parse::<u64>().unwrap_or(0) * unit
    });
    s.proxy = ProxyAdv {
        mode: PROXY_MODES[st.proxy_mode].1,
        host: st.proxy_host.trim().to_owned(),
        port: st.proxy_port.trim().to_owned(),
        auth_enabled: st.proxy_auth,
        username: st.proxy_user.trim().to_owned(),
        // Only speak about the password when the user touched
        // it: empty-and-edited deletes, empty-and-untouched
        // keeps whatever is stored.
        password: st.proxy_pass.clone(),
        clear_password: st.proxy_pass_edited && st.proxy_pass.is_empty(),
        ..s.proxy.clone()
    };
    s.connect_timeout = humantime::parse_duration(st.connect_timeout.trim()).ok();
    s.user_agent = (!st.user_agent.trim().is_empty()).then(|| st.user_agent.trim().to_owned());
    if let Ok(v) = st.ipc_port.trim().parse() {
        s.ipc_port = v;
    }
    // Only genuine overrides are stored. The panes show every category's
    // resolved extensions and folder, so writing those back verbatim
    // would turn "same as default" into a saved override — and, until
    // saved, into a change the user never made.
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
        .filter(|(c, exts)| exts.as_slice() != c.default_extensions())
        .collect();
    s.category_folders = st
        .cat_folders
        .iter()
        .map(|(c, dir)| (*c, std::path::PathBuf::from(dir.trim())))
        .filter(|(c, dir)| !dir.as_os_str().is_empty() && *dir != s.download_dir.join(c.label()))
        .collect();
    s.category_queues = st
        .cat_queues
        .iter()
        .filter_map(|(c, q)| q.map(|q| (*c, q)))
        .collect();
    // Nameless rows are still being typed; case-duplicates fold onto
    // the first spelling, so the stored map matches what the wire
    // would resolve these to.
    s.headers = crate::domain::normalize_headers(st.custom_headers.iter().cloned());
    s
}

/// Copy one section's fields from `src` onto `dst`. The list lives here
/// so "reset this section" cannot drift from what the section shows.
fn copy_section(dst: &mut Settings, src: &Settings, section: Section) {
    match section {
        Section::General => {
            dst.pause_on_metered = src.pause_on_metered;
            dst.pause_on_low_battery = src.pause_on_low_battery;
            dst.theme = src.theme;
            dst.reduce_motion = src.reduce_motion;
            dst.download_dir = src.download_dir.clone();
            dst.work_dir = src.work_dir.clone();
            dst.start_at_login = src.start_at_login;
            dst.start_to_tray = src.start_to_tray;
        }
        Section::Downloads => {
            dst.max_retries = src.max_retries;
            dst.n_fixed_retries = src.n_fixed_retries;
            dst.wait_between_retries = src.wait_between_retries;
            dst.use_server_time = src.use_server_time;
            dst.remove_confirm_incomplete = src.remove_confirm_incomplete;
            dst.remove_confirm_completed = src.remove_confirm_completed;
            dst.remove_confirm_clean = src.remove_confirm_clean;
        }
        Section::Categories => {
            dst.category_extensions = src.category_extensions.clone();
            dst.category_folders = src.category_folders.clone();
            dst.category_queues = src.category_queues.clone();
        }
        Section::Network => {
            dst.max_connections = src.max_connections;
            dst.max_concurrent_downloads = src.max_concurrent_downloads;
            dst.speed_limit = src.speed_limit;
            dst.proxy = src.proxy.clone();
            dst.connect_timeout = src.connect_timeout;
            dst.accept_invalid_certs = src.accept_invalid_certs;
            dst.user_agent = src.user_agent.clone();
            dst.randomize_user_agent = src.randomize_user_agent;
            dst.headers = src.headers.clone();
        }
        Section::Browser => {
            dst.ipc_port = src.ipc_port;
            dst.conflict_while_hidden = src.conflict_while_hidden;
        }
        Section::Notifications => {
            dst.show_complete_dialog = src.show_complete_dialog;
            dst.notify_complete = src.notify_complete;
            dst.show_failed_dialog = src.show_failed_dialog;
            dst.notify_failed = src.notify_failed;
            dst.notify_queue_finished = src.notify_queue_finished;
            dst.show_update_dialog = src.show_update_dialog;
            dst.notify_update = src.notify_update;
        }
        Section::Advanced => {}
    }
}

/// Keep only genuine category overrides. The panes show every category's
/// resolved extensions and folder, so both sides of the change count
/// have to agree on what "same as default" means — otherwise a stored
/// value equal to its default reads as an edit the moment the window
/// opens, and a shown default would be written back as an override.
fn normalize_for_editing(s: &mut Settings) {
    // The picker speaks in whole KB/s or MB/s, so a limit that is not a
    // multiple of 1 KB/s cannot survive the round trip and would read as
    // an edit. Round it to what the form can actually hold.
    if let Some(bps) = s.speed_limit.filter(|v| *v > 0) {
        s.speed_limit = Some((bps / BYTES_PER_KB).max(1) * BYTES_PER_KB);
    }
    drop_default_categories(s);
}

fn drop_default_categories(s: &mut Settings) {
    let download_dir = s.download_dir.clone();
    s.category_extensions
        .retain(|c, exts| exts.as_slice() != c.default_extensions());
    s.category_folders
        .retain(|c, dir| !dir.as_os_str().is_empty() && *dir != download_dir.join(c.label()));
}

fn mirror(st: &mut State) {
    // Whatever moved `st.s` wholesale — Reset, Discard, a reload — the
    // preview follows it, the same way picking a theme repaints on the
    // spot.
    st.tokens = Tokens::from_settings(&st.s);
    let s = &st.s;
    st.download_dir = s.download_dir.display().to_string();
    st.work_dir = s.work_dir.display().to_string();
    st.max_retries = s.max_retries.to_string();
    st.fixed_retries = s.n_fixed_retries.to_string();
    st.retry_wait = humantime::format_duration(s.wait_between_retries).to_string();
    st.concurrent = s.max_concurrent_downloads.to_string();
    // Bytes/sec on the wire; shown in whichever unit divides evenly.
    st.limit_on = s.speed_limit.is_some();
    let bps = s.speed_limit.unwrap_or(0);
    st.limit_unit_mb = bps > 0 && bps.is_multiple_of(BYTES_PER_MB);
    st.limit_value = match bps {
        0 => String::new(),
        v if st.limit_unit_mb => (v / BYTES_PER_MB).to_string(),
        v => (v / BYTES_PER_KB).to_string(),
    };
    st.proxy_mode = mode_index(s.proxy.mode);
    st.proxy_host = s.proxy.host.clone();
    st.proxy_port = s.proxy.port.clone();
    st.has_stored_proxy_pass = s.enc_proxy_password.is_some();
    st.proxy_auth = s.proxy.auth_enabled;
    st.proxy_user = s.proxy.username.clone();
    st.proxy_pass = String::new();
    st.proxy_pass_edited = false;
    st.connect_timeout = s
        .connect_timeout
        .map(|d| humantime::format_duration(d).to_string())
        .unwrap_or_default();
    st.user_agent = s.user_agent.clone().unwrap_or_default();
    st.ipc_port = s.ipc_port.to_string();
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
    // Show a real path rather than an empty field explained by a hint,
    // and make it a per-category subfolder: sorting downloads by kind is
    // the whole point of categories, so the default should already do
    // it. Writing these back on the next apply is intended.
    st.cat_folders = Category::ALL_ASSIGNABLE
        .iter()
        .map(|c| {
            let dir = s
                .category_folders
                .get(c)
                .filter(|p| !p.as_os_str().is_empty())
                .cloned()
                .unwrap_or_else(|| s.download_dir.join(c.label()));
            (*c, dir.display().to_string())
        })
        .collect();
    st.cat_queues = Category::ALL_ASSIGNABLE
        .iter()
        .map(|c| (*c, s.category_queues.get(c).copied()))
        .collect();
    st.custom_headers = s
        .headers
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
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
            let (client, mut settings, queues) = *boxed;
            // A build before this rule may have stored categories that
            // only restate their defaults; normalise on the way in so
            // the form does not open already "changed".
            normalize_for_editing(&mut settings);
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
                limit_on: false,
                limit_value: String::new(),
                limit_unit_mb: false,
                proxy_mode: 0,
                proxy_host: String::new(),
                proxy_port: String::new(),
                proxy_auth: false,
                proxy_user: String::new(),
                proxy_pass: String::new(),
                proxy_pass_edited: false,
                has_stored_proxy_pass: false,
                connect_timeout: String::new(),
                user_agent: String::new(),
                ipc_port: String::new(),
                cat_exts: Vec::new(),
                cat_folders: Vec::new(),
                cat_queues: Vec::new(),
                cat_open: None,
                queues,
                confirm_reset: false,
                custom_headers: Vec::new(),
                shot: Shot::from_env(),
                dirty: 0,
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

/// Count of settings this form would change, refreshed after every
/// message so the footer never has to diff during a render.
fn refresh_dirty(st: &mut State) {
    let changed = crate::gui::diff::changed_keys(&st.original, &pending_settings(st));
    if !changed.is_empty() {
        tracing::debug!(?changed, "settings differ from saved");
    }
    st.dirty = changed.len();
}

fn update_ready(st: &mut State, msg: Msg) -> Task<Msg> {
    let task = update_ready_inner(st, msg);
    refresh_dirty(st);
    task
}

fn update_ready_inner(st: &mut State, msg: Msg) -> Task<Msg> {
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
        // Another window changed the theme. Only the palette is adopted:
        // `st.s` is the user's unsaved edit buffer and must not be
        // overwritten from under them.
        Msg::Daemon(DaemonSignal::Event(Event::SettingsChanged)) => {
            crate::gui::theme::refresh_tokens(
                st.client.clone(),
                |t| Msg::Themed(Box::new(t)),
                Msg::Noop,
            )
        }
        Msg::Daemon(_) => Task::none(),
        Msg::Themed(t) => {
            st.tokens = *t;
            Task::none()
        }
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
        Msg::ConfirmClean(v) => {
            st.s.remove_confirm_clean = v;
            Task::none()
        }
        Msg::PauseOnMetered(v) => {
            st.s.pause_on_metered = v;
            Task::none()
        }
        Msg::PauseOnLowBattery(v) => {
            st.s.pause_on_low_battery = v;
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
        Msg::Connections(v) => {
            st.s.max_connections = v;
            Task::none()
        }
        Msg::Concurrent(v) => {
            st.concurrent = v;
            Task::none()
        }
        Msg::SpeedLimitOn(v) => {
            st.limit_on = v;
            Task::none()
        }
        Msg::SpeedLimitValue(v) => {
            st.limit_value = v.chars().filter(char::is_ascii_digit).collect();
            Task::none()
        }
        Msg::SpeedLimitUnit(mb) => {
            st.limit_unit_mb = mb;
            Task::none()
        }
        Msg::ProxyMode(i) => {
            st.proxy_mode = i;
            Task::none()
        }
        Msg::ProxyHost(v) => {
            st.proxy_host = v;
            Task::none()
        }
        Msg::ProxyPort(v) => {
            st.proxy_port = v.chars().filter(char::is_ascii_digit).collect();
            Task::none()
        }
        Msg::ProxyAuth(v) => {
            st.proxy_auth = v;
            if !v {
                st.proxy_user.clear();
                st.proxy_pass.clear();
                // Turning sign-in off is an explicit request to forget
                // the stored password, not just to hide the field.
                st.proxy_pass_edited = true;
            }
            Task::none()
        }
        Msg::ProxyUser(v) => {
            st.proxy_user = v;
            Task::none()
        }
        Msg::ProxyPass(v) => {
            st.proxy_pass = v;
            st.proxy_pass_edited = true;
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
        Msg::HeaderName(i, v) => {
            if let Some(h) = st.custom_headers.get_mut(i) {
                h.0 = v;
            }
            Task::none()
        }
        Msg::HeaderValue(i, v) => {
            if let Some(h) = st.custom_headers.get_mut(i) {
                h.1 = v;
            }
            Task::none()
        }
        Msg::HeaderRemove(i) => {
            if i < st.custom_headers.len() {
                st.custom_headers.remove(i);
            }
            Task::none()
        }
        Msg::HeaderAdd => {
            st.custom_headers.push((String::new(), String::new()));
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
        Msg::NotifyComplete(v) => {
            st.s.notify_complete = v;
            Task::none()
        }
        Msg::ShowFailedDialog(v) => {
            st.s.show_failed_dialog = v;
            Task::none()
        }
        Msg::NotifyFailed(v) => {
            st.s.notify_failed = v;
            Task::none()
        }
        Msg::NotifyQueueFinished(v) => {
            st.s.notify_queue_finished = v;
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
            // Back to oxdm's defaults, not to what is saved — reverting
            // edits is what Discard is for. The defaults land as pending
            // changes, so they can be reviewed, discarded, or applied
            // like any other edit.
            copy_section(&mut st.s, &Settings::default(), st.section);
            mirror(st);
            Task::none()
        }
        Msg::Save => {
            let s = pending_settings(st);
            // Keep `st.s` in step with what was sent: the mirrors were
            // just folded in, and `original` is rebased off it on ack.
            st.s = s.clone();
            let client = st.client.clone();
            Task::perform(async move { client.update_settings(s).await }, Msg::Saved)
        }
        Msg::Saved(Ok(())) => {
            // The window stays open, so "Reset <section>" has to mean
            // "back to what is saved" — which is now this.
            st.original = st.s.clone();
            Task::none()
        }
        Msg::Saved(Err(_)) => Task::none(),
        Msg::Discard => {
            st.s = st.original.clone();
            mirror(st);
            Task::none()
        }
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
        crate::gui::ipc::all_events(crate::ipc_local::protocol::GuiKind::Settings).map(Msg::Daemon),
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

/// Design `.settings-nav .s-item`: 500 12.5px, 600 when selected.
const NAV_FONT: f32 = 12.5;

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
            iced::widget::button(
                row![
                    icons::icon(icon, 15.0, icon_color),
                    text(label).font(label_font).size(NAV_FONT).color(fg),
                ]
                .spacing(theme::space::S2)
                // `button` does not centre its content the way the
                // styled container did — fill the row and centre in it,
                // or the label rides the top of the 34px pill.
                .height(Length::Fill)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .height(Length::Fixed(34.0))
            .padding([0.0, theme::space::S3])
            // Design `.s-item:hover { background: bg-sunken }` — a
            // styled container has no hover status to key off, so
            // the row never reacted to the pointer.
            .style(move |_th, status| iced::widget::button::Style {
                background: if active {
                    Some(t2.bg_raised.into())
                } else {
                    matches!(
                        status,
                        iced::widget::button::Status::Hovered
                            | iced::widget::button::Status::Pressed
                    )
                    .then(|| t2.bg_sunken.into())
                },
                text_color: fg,
                border: iced::Border {
                    radius: theme::control::RADIUS.into(),
                    ..Default::default()
                },
                shadow: iced::Shadow::default(),
                ..Default::default()
            })
            .on_press(Msg::SetSection(sec)),
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
    };

    let mut right = row![].spacing(theme::space::S2).align_y(Alignment::Center);
    // Outermost, because it comes and goes: the row is right-aligned, so
    // a button appearing to Reset's left leaves Reset and Apply put.
    if st.dirty > 0 {
        right = right.push(
            Btn::new(format!(
                "Discard {} change{}",
                st.dirty,
                if st.dirty == 1 { "" } else { "s" }
            ))
            .ghost()
            .accent(true)
            .icon("rotate-cw")
            .on_press(Msg::Discard)
            .view(t),
        );
    }
    // Advanced has nothing resettable.
    if !matches!(st.section, Section::Advanced) {
        right = right.push(
            Btn::new(format!("Reset {}", st.section.label()))
                .ghost()
                .icon("rotate-cw")
                .on_press(Msg::ResetSection)
                .view(t),
        );
    }
    right = right.push(
        Btn::new("Apply")
            .primary()
            .icon("save")
            // Nothing to write when nothing differs.
            .enabled(st.dirty > 0)
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
const PANE_HEAD_TITLE_LINE: f32 = 28.0;
const PANE_HEAD_DESC_LINE: f32 = 18.0;

/// `NumberStepper` clamps. Retry counts allow zero; the saved value still
/// flows through the existing string mirror + `Save` parse.
const RETRIES_MIN: i64 = 0;
const RETRIES_MAX: i64 = 20;
/// Concurrent downloads must stay ≥ 1 (zero would stall the queue).
const CONCURRENT_MIN: i64 = 1;
const CONCURRENT_MAX: i64 = 20;
/// Anything at or above this reads as Unlimited (see `UNLIMITED_CONCURRENT`).
const CONCURRENT_UNLIMITED: i64 = crate::domain::settings::UNLIMITED_CONCURRENT as i64;

/// Per-pane head (`.s-pane-head`): Fraunces h2 title + muted description
/// line + a 1px bottom rule.
fn pane_head<'a>(t: &Tokens, title: &str, desc: &str) -> Element<'a, Msg> {
    // Absolute (integer) line heights keep the rule on a whole pixel:
    // font-derived heights are fractional, and a 1px rect straddling
    // two rows is painted as a soft 2px band.
    column![
        text(title.to_owned())
            .font(theme::DISPLAY)
            .size(PANE_HEAD_TITLE_SIZE)
            .line_height(iced::widget::text::LineHeight::Absolute(
                PANE_HEAD_TITLE_LINE.into()
            ))
            .color(t.fg_1),
        text(desc.to_owned())
            .font(theme::BODY)
            .size(PANE_HEAD_DESC_SIZE)
            .line_height(iced::widget::text::LineHeight::Absolute(
                PANE_HEAD_DESC_LINE.into()
            ))
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

/// Boolean `.set-row`: label (+ hint) left, `controls::toggle` right.
fn toggle_row<'a>(
    t: &Tokens,
    label: &'a str,
    hint: Option<&'a str>,
    on: bool,
    msg: impl Fn(bool) -> Msg + 'a,
) -> Element<'a, Msg> {
    toggle_row_enabled(t, label, hint, on, true, msg)
}

/// `toggle_row` for a setting that exists but cannot be changed yet —
/// the switch reads at half opacity and swallows presses.
fn toggle_row_enabled<'a>(
    t: &Tokens,
    label: &'a str,
    hint: Option<&'a str>,
    on: bool,
    enabled: bool,
    msg: impl Fn(bool) -> Msg + 'a,
) -> Element<'a, Msg> {
    set_row(t, label, hint, toggle(t, on, enabled, msg))
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

/// Connection-count presets (design: the Queues window's concurrency
/// pills). `None` is Auto — the per-job size heuristic picks the count.
const CONN_PRESETS: [u64; 3] = [4, 8, 16];
/// A file is one part at minimum; the ceiling matches the queue window's
/// concurrency cap, since both are "how many sockets at once".
const CONN_MIN: i64 = 1;
const CONN_MAX: i64 = 16;

/// Auto / 4x / 8x / 16x pills plus a stepper for anything else.
fn connections_picker<'a>(t: &Tokens, current: Option<u64>) -> Element<'a, Msg> {
    let mut r = row![
        Btn::new("Auto")
            .secondary()
            .pill()
            .size(BtnSize::Md)
            .selected(current.is_none())
            .on_press(Msg::Connections(None))
            .view(t),
    ]
    .spacing(4.0)
    .align_y(Alignment::Center);
    for n in CONN_PRESETS {
        r = r.push(
            Btn::new(format!("{n}x"))
                .secondary()
                .pill()
                .size(BtnSize::Md)
                .selected(current == Some(n))
                .on_press(Msg::Connections(Some(n)))
                .view(t),
        );
    }
    // Custom: the stepper writes through the same message, so a value the
    // presets don't cover simply leaves them all unselected.
    r.push(number_stepper(
        t,
        current.unwrap_or(8) as i64,
        CONN_MIN,
        CONN_MAX,
        true,
        |n| Msg::Connections(Some(n as u64)),
    ))
    .into()
}

/// Speed-limit units. The daemon stores bytes/sec; the picker writes
/// whichever unit the user chose, exactly like the download window's.
const BYTES_PER_KB: u64 = 1024;
const BYTES_PER_MB: u64 = 1024 * 1024;
/// Width of the value field (download window `LIMIT_INPUT_W`).
const LIMIT_INPUT_W: f32 = 80.0;

/// Unlimited / Limit-to chips, a value field and a KB/s ‖ MB/s toggle —
/// the same control the download window uses for one job.
fn speed_limit_picker(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    // Everything on this row is the input's height: the chips and the
    // unit toggle read as one control with the field between them.
    row![
        segmented(
            t,
            &[("Unlimited", None), ("Limit to", None)],
            if st.limit_on { 1 } else { 0 },
            BtnSize::Md,
            |i| Msg::SpeedLimitOn(i == 1),
        ),
        TextInput::new(&st.limit_value)
            .width(Length::Fixed(LIMIT_INPUT_W))
            .enabled(st.limit_on)
            .on_input(Msg::SpeedLimitValue)
            .view(t),
        segmented(
            t,
            &[("KB/s", None), ("MB/s", None)],
            if st.limit_unit_mb { 1 } else { 0 },
            BtnSize::Md,
            |i| Msg::SpeedLimitUnit(i == 1),
        ),
    ]
    .spacing(theme::space::S2)
    .align_y(Alignment::Center)
    .into()
}

/// Unlimited pill + a stepper for a specific ceiling. Unlimited is not a
/// separate mode: it writes a count no queue reaches.
fn concurrent_picker<'a>(t: &Tokens, value: &str) -> Element<'a, Msg> {
    let v = value
        .trim()
        .parse::<i64>()
        .unwrap_or(CONCURRENT_UNLIMITED)
        .max(CONCURRENT_MIN);
    let unlimited = v >= CONCURRENT_UNLIMITED;
    row![
        Btn::new("Unlimited")
            .secondary()
            .pill()
            .size(BtnSize::Md)
            .selected(unlimited)
            .on_press(Msg::Concurrent(CONCURRENT_UNLIMITED.to_string()))
            .view(t),
        number_stepper(
            t,
            // The stepper only speaks in its own range: an Unlimited (or
            // stale out-of-range) value shows as the ceiling rather than
            // printing a number the buttons cannot reach.
            v.clamp(CONCURRENT_MIN, CONCURRENT_MAX),
            CONCURRENT_MIN,
            CONCURRENT_MAX,
            true,
            |n| Msg::Concurrent(n.to_string()),
        ),
    ]
    .spacing(4.0)
    .align_y(Alignment::Center)
    .into()
}

/// Global proxy modes and the `ProxyMode` each selects. "Inherit" is
/// absent by design: this *is* what a job inherits. "System" means no
/// explicit proxy, so reqwest reads the proxy environment variables.
const PROXY_MODES: &[(&str, ProxyMode)] = &[
    ("System", ProxyMode::System),
    ("HTTP", ProxyMode::Http),
    ("HTTPS", ProxyMode::Https),
    ("SOCKS5", ProxyMode::Socks5),
];
/// Width of the port field (matches the per-job `.prop-proxy-port`).
const PROXY_PORT_W: f32 = 90.0;

fn mode_index(mode: ProxyMode) -> usize {
    PROXY_MODES
        .iter()
        .position(|(_, m)| *m == mode)
        .unwrap_or(0)
}

/// Proxy rows: the same mode-then-server shape as the per-job control in
/// Properties, over the single URL the daemon stores.
fn proxy_rows(st: &State) -> Vec<Element<'_, Msg>> {
    let t = &st.tokens;
    let mut rows = vec![set_row_stack(
        t,
        "Use proxy",
        Some(
            "Routes every download. System reads your proxy environment variables; \
             a job can still override this from its Properties.",
        ),
        segmented(
            t,
            &PROXY_MODES
                .iter()
                .map(|(label, _)| (*label, None))
                .collect::<Vec<_>>(),
            st.proxy_mode,
            BtnSize::Sm,
            Msg::ProxyMode,
        ),
    )];
    if st.proxy_mode > 0 {
        rows.push(set_row_stack(
            t,
            "Server",
            None,
            row![
                TextInput::new(&st.proxy_host)
                    .width(Length::Fill)
                    .hint("proxy.example.com")
                    .on_input(Msg::ProxyHost)
                    .view(t),
                TextInput::new(&st.proxy_port)
                    .width(Length::Fixed(PROXY_PORT_W))
                    .hint("8080")
                    .on_input(Msg::ProxyPort)
                    .view(t),
            ]
            .spacing(theme::space::S2)
            .align_y(Alignment::Center)
            .into(),
        ));
        rows.push(toggle_row(
            t,
            "Proxy needs a sign-in",
            Some("The password is encrypted in the secret store, never in the proxy URL."),
            st.proxy_auth,
            Msg::ProxyAuth,
        ));
        if st.proxy_auth {
            rows.push(set_row_stack(
                t,
                "Credentials",
                None,
                row![
                    TextInput::new(&st.proxy_user)
                        .width(Length::Fill)
                        .hint("username")
                        .on_input(Msg::ProxyUser)
                        .view(t),
                    PasswordInput::new(&st.proxy_pass)
                        .hint(if st.has_stored_proxy_pass && !st.proxy_pass_edited {
                            "stored (encrypted)"
                        } else {
                            "password"
                        })
                        .on_input(Msg::ProxyPass)
                        .view(t),
                ]
                .spacing(theme::space::S2)
                .align_y(Alignment::Center)
                .into(),
            ));
        }
    }
    rows
}

fn label_input<'a>(t: &Tokens, label: &str, input: Element<'a, Msg>) -> Element<'a, Msg> {
    column![crate::gui::widget::field_label(t, label), input]
        .spacing(theme::space::S1 + 2.0)
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
    pane(
        t,
        Section::General,
        column![
            set_section(
                t,
                "Startup",
                vec![
                    toggle_row(
                        t,
                        "Launch at login",
                        Some("Start oxdm automatically when you log in."),
                        st.s.start_at_login,
                        Msg::StartAtLogin
                    ),
                    toggle_row(
                        t,
                        "Start to tray",
                        Some("Boot without opening the main window."),
                        st.s.start_to_tray,
                        Msg::StartToTray
                    ),
                ]
            ),
            set_section(
                t,
                "Appearance",
                vec![
                    set_row(
                        t,
                        "Theme",
                        Some("Color palette for the whole app. Auto follows your system."),
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
                    toggle_row(
                        t,
                        "Reduce motion",
                        Some("Skip animations and transitions across the app."),
                        st.s.reduce_motion,
                        Msg::ReduceMotion
                    ),
                ]
            ),
            set_section(
                t,
                "Schedule-aware",
                vec![
                    toggle_row(
                        t,
                        "Pause on metered networks",
                        Some("Stop downloads on cellular or a phone hotspot, and resume after."),
                        st.s.pause_on_metered,
                        Msg::PauseOnMetered
                    ),
                    toggle_row(
                        t,
                        "Pause when battery is low",
                        Some("Below 20% and not plugged in."),
                        st.s.pause_on_low_battery,
                        Msg::PauseOnLowBattery
                    ),
                ]
            ),
        ]
        .spacing(SECTION_GAP)
        .into(),
    )
}

fn downloads_section(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    pane(
        t,
        Section::Downloads,
        column![
            set_section(
                t,
                "Storage",
                vec![
                    set_row_stack(
                        t,
                        "Default download folder",
                        Some("Where finished files land unless a category overrides it."),
                        FileInput::new(&st.download_dir)
                            .on_input(Msg::DownloadDir)
                            .on_browse(Msg::BrowseDownloadDir)
                            .view(t)
                    ),
                    set_row_stack(
                        t,
                        "In-flight cache folder",
                        Some("Holds per-job .part files and metadata until a download completes."),
                        FileInput::new(&st.work_dir).on_input(Msg::WorkDir).view(t)
                    ),
                ]
            ),
            set_section(
                t,
                "Files",
                vec![toggle_row(
                    t,
                    "Use server-provided last-modified time",
                    Some("Stamp saved files with the time the server reports."),
                    st.s.use_server_time,
                    Msg::UseServerTime
                )]
            ),
            set_section(
                t,
                "Retries",
                vec![
                    set_row(
                        t,
                        "Max retries",
                        Some("How many times a failed segment is retried before the job fails."),
                        stepper(
                            t,
                            &st.max_retries,
                            3,
                            RETRIES_MIN,
                            RETRIES_MAX,
                            Msg::MaxRetries
                        )
                    ),
                    set_row(
                        t,
                        "Fixed retries before backoff",
                        Some("Early attempts reuse the same wait; later ones back off."),
                        stepper(
                            t,
                            &st.fixed_retries,
                            3,
                            RETRIES_MIN,
                            RETRIES_MAX,
                            Msg::FixedRetries
                        )
                    ),
                    set_row(
                        t,
                        "Wait between retries",
                        None,
                        TextInput::new(&st.retry_wait)
                            .width(Length::Fixed(120.0))
                            .on_input(Msg::RetryWait)
                            .view(t)
                    ),
                ]
            ),
            set_section(
                t,
                "Remove behavior",
                vec![
                    toggle_row(
                        t,
                        "Confirm removing incomplete downloads",
                        Some("Ask before discarding a job that has not finished."),
                        st.s.remove_confirm_incomplete,
                        Msg::ConfirmIncomplete
                    ),
                    toggle_row(
                        t,
                        "Confirm removing completed downloads",
                        Some("Ask before clearing a finished job from the list."),
                        st.s.remove_confirm_completed,
                        Msg::ConfirmCompleted
                    ),
                    toggle_row(
                        t,
                        "Confirm cleaning finished downloads",
                        Some("Ask before the toolbar's Clean clears every finished job at once."),
                        st.s.remove_confirm_clean,
                        Msg::ConfirmClean
                    ),
                ]
            ),
        ]
        .spacing(SECTION_GAP)
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
    let mut cards = column![].spacing(theme::space::S2);

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
        let summary = if cat == Category::Other {
            "Everything the other categories don't claim".to_owned()
        } else {
            format!("{n_exts} extensions")
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
            // The hover fill is a plain rect inside a rounded, bordered
            // card, so it has to carry the card's corners itself — a
            // square fill paints over them and the border with them.
            // Open, the body continues below, so only the top corners
            // round.
            border: iced::Border {
                radius: if open {
                    iced::border::Radius::default()
                        .top(theme::surface::RADIUS)
                        .bottom(0.0)
                } else {
                    theme::surface::RADIUS.into()
                },
                ..Default::default()
            },
            ..Default::default()
        });

        let mut card = column![header];
        if open {
            let body = column![
                label_input(
                    t,
                    "extensions, comma-separated, no dots",
                    // "Other" is the overflow bucket, not a list: it
                    // takes whatever the named categories don't claim,
                    // so there is nothing to edit.
                    if cat == Category::Other {
                        TextInput::new("")
                            .mono()
                            .enabled(false)
                            .hint("Everything the other categories don't claim lands here")
                            .view(t)
                    } else {
                        TextInput::new(exts)
                            .mono()
                            .on_input(move |v| Msg::CategoryExts(cat, v))
                            .view(t)
                    }
                ),
                label_input(
                    t,
                    "save folder",
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
                        .enabled(cat != Category::Other)
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
                // Inset by the border so the header's hover fill cannot
                // paint over it.
                .padding(1.0)
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
        set_group(t, "Categories", cards.into()),
    )
}

fn network_section(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    pane(
        t,
        Section::Network,
        column![
            set_section(
                t,
                "Connections",
                vec![
                    set_row(
                        t,
                        "Connections per file",
                        Some(
                            "How many parts a file is split into. Auto picks the count from \
                             the file size."
                        ),
                        connections_picker(t, st.s.max_connections)
                    ),
                    set_row(
                        t,
                        "Concurrent downloads",
                        Some("How many jobs run at the same time."),
                        concurrent_picker(t, &st.concurrent)
                    ),
                    set_row(
                        t,
                        "Speed limit",
                        Some("Applies across every download at once."),
                        speed_limit_picker(st)
                    ),
                    set_row(
                        t,
                        "Connect timeout",
                        Some("How long to wait for a server to answer before giving up."),
                        TextInput::new(&st.connect_timeout)
                            .width(Length::Fixed(100.0))
                            .on_input(Msg::ConnectTimeout)
                            .view(t)
                    ),
                ]
            ),
            set_section(t, "Proxy", proxy_rows(st)),
            set_section(
                t,
                "TLS",
                vec![toggle_row(
                    t,
                    "Accept invalid TLS certificates",
                    Some("Dangerous: disables certificate verification for every host."),
                    st.s.accept_invalid_certs,
                    Msg::InvalidCerts
                )]
            ),
            set_section(
                t,
                "Identity",
                vec![
                    set_row_stack(
                        t,
                        "Custom User-Agent",
                        Some("Sent with every request."),
                        TextInput::new(&st.user_agent)
                            .width(Length::Fill)
                            // Blank really does mean blank: oxdm sets no
                            // User-Agent of its own, so the header is
                            // simply absent unless randomising is on.
                            .hint(if st.s.randomize_user_agent {
                                "A random browser User-Agent per request"
                            } else {
                                "No User-Agent sent"
                            })
                            .on_input(Msg::UserAgent)
                            .view(t)
                    ),
                    toggle_row(
                        t,
                        "Randomize User-Agent per request",
                        None,
                        st.s.randomize_user_agent,
                        Msg::RandomUa
                    ),
                ]
            ),
            set_section(t, "Custom headers", header_rows(st)),
        ]
        .spacing(SECTION_GAP)
        .into(),
    )
}

/// Custom request headers, on the same name/value editor the Properties
/// window's Headers tab uses — one row per header, plus an add button.
/// Same job, same controls; a free-text `Name: value` blob here and a
/// table there was two answers to one question.
fn header_rows(st: &State) -> Vec<Element<'_, Msg>> {
    let t = &st.tokens;
    let mut rows: Vec<Element<'_, Msg>> = vec![set_note(
        t,
        "Sent alongside the defaults on every request. Useful for API keys, \
         Origin overrides, or signed URLs.",
    )];
    for (i, (name, value)) in st.custom_headers.iter().enumerate() {
        rows.push(set_row_panel(
            row![
                TextInput::new(name)
                    .hint("Name")
                    .on_input(move |v| Msg::HeaderName(i, v))
                    .view(t),
                TextInput::new(value)
                    .hint("Value")
                    .on_input(move |v| Msg::HeaderValue(i, v))
                    .view(t),
                Btn::new("")
                    .toolbar()
                    .icon_only("trash-2")
                    .on_press(Msg::HeaderRemove(i))
                    .view(t),
            ]
            .spacing(theme::space::S2)
            .align_y(Alignment::Center)
            .into(),
        ));
    }
    rows.push(set_row_panel(
        Btn::new("Add header")
            .ghost()
            .icon("plus")
            .accent(true)
            .font_size(11.0)
            .on_press(Msg::HeaderAdd)
            .view(t),
    ));
    rows
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
        set_section(
            t,
            "Browser integration",
            vec![
                set_row(
                    t,
                    "IPC port",
                    Some("Local port the browser extension connects to."),
                    TextInput::new(&st.ipc_port)
                        .width(Length::Fixed(100.0))
                        .on_input(Msg::IpcPort)
                        .view(t),
                ),
                set_row_stack(
                    t,
                    "Pairing code",
                    Some("Paste this into the extension to authorize it."),
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
                    .into(),
                ),
                set_row_stack(
                    t,
                    "Conflict while the dialog is hidden",
                    Some("What happens when a capture arrives with no visible window."),
                    combo(
                        t,
                        vec!["auto_popup".to_owned(), "notify_and_park".to_owned()],
                        Some(conflict.to_owned()),
                        Msg::ConflictHidden,
                        Length::Fill,
                    ),
                ),
            ],
        ),
    )
}

fn notifications_section(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    pane(
        t,
        Section::Notifications,
        column![
            set_section(
                t,
                "Download complete",
                vec![
                    toggle_row(
                        t,
                        "Show dialog",
                        Some("Opens the job's window with a summary and what to do next."),
                        st.s.show_complete_dialog,
                        Msg::ShowCompleteDialog,
                    ),
                    toggle_row(
                        t,
                        "System notification",
                        Some("Reports the finished file without taking focus."),
                        st.s.notify_complete,
                        Msg::NotifyComplete,
                    ),
                ],
            ),
            set_section(
                t,
                "Download failed",
                vec![
                    toggle_row(
                        t,
                        "Show dialog",
                        Some("Opens the job's window on the error, where you can retry."),
                        st.s.show_failed_dialog,
                        Msg::ShowFailedDialog,
                    ),
                    toggle_row(
                        t,
                        "System notification",
                        Some("Reports the failure without taking focus."),
                        st.s.notify_failed,
                        Msg::NotifyFailed,
                    ),
                ],
            ),
            set_section(
                t,
                "Queue finished",
                vec![
                    toggle_row(
                        t,
                        "System notification",
                        Some("Fires when every download in a queue has finished."),
                        st.s.notify_queue_finished,
                        Msg::NotifyQueueFinished,
                    ),
                    set_note(
                        t,
                        "A finished queue has no dialog. For an action instead of a report (run a \
                         command, sleep, shut down), use the queue's on-finish hooks in \
                         Queues & scheduling.",
                    ),
                ],
            ),
            set_section(
                t,
                "New version available",
                vec![
                    toggle_row_enabled(
                        t,
                        "Show dialog",
                        None,
                        st.s.show_update_dialog,
                        false,
                        |_| Msg::Noop,
                    ),
                    toggle_row_enabled(
                        t,
                        "System notification",
                        None,
                        st.s.notify_update,
                        false,
                        |_| Msg::Noop,
                    ),
                    set_note(
                        t,
                        "Unavailable: oxdm only checks for updates when you ask it to, from \
                         About. Nothing raises this event yet.",
                    ),
                ],
            ),
        ]
        .spacing(SECTION_GAP)
        .into(),
    )
}

fn advanced_section(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    pane(t, Section::Advanced, danger_section(st))
}

/// Rust-headed danger block (design §3.7 Advanced: own Reset section;
/// tokens follow the download-window completion warning).
fn danger_section(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    set_section_danger(
        t,
        "Danger zone",
        vec![set_row(
            t,
            "Reset oxdm",
            Some(
                "Backs up and clears the database: all jobs, queues and settings. \
                 Downloaded files stay on disk. The daemon exits and must be relaunched.",
            ),
            Btn::new("Reset oxdm…")
                .danger_filled()
                .icon("rotate-cw")
                .on_press(Msg::ResetDbAsk)
                .view(t),
        )],
    )
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
                 erased. Downloaded files are not touched. The daemon exits; relaunch \
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

pub fn launch_settings() {
    let mut app = iced::application(boot, update, view)
        .title(|_: &App| "oxdm: Settings".to_owned())
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
