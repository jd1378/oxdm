//! Main window: sidebar (categories / queues / tools), toolbar, tab
//! strip, jobs table, statusbar — plus in-window overlays (context
//! menu, remove/about/host/conflict dialogs, db/secrets recovery).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use iced::widget::{column, container, mouse_area, row, scrollable, text};
use iced::{Alignment, Element, Length, Subscription, Task};

use super::main_dialogs::{self, AboutState, HostState, RemoveState, UpdateUi};
use crate::domain::{Category, Density, JobId, Phase, QueueId};
use crate::gui::chrome::{self, WindowControl, titlebar};
use crate::gui::format::{format_bytes, format_eta, format_speed};
use crate::gui::ipc::DaemonSignal;
use crate::gui::shot::Shot;
use crate::gui::theme::{self, Tokens};
use crate::gui::widget::{
    Btn, BtnSize, TabBtn, col_header_sortable, hairline, inline_progress, search_field, status_dot,
    swatch, vdivider,
};
use crate::gui::{color, icons};
use crate::ipc_local::Client;
use crate::ipc_local::protocol::{Event, JobCounters, SnapshotData};

const RESIZE_HANDLE_W: f32 = 6.0;
const HEADER_H: f32 = 22.0;

// Row heights per UI `Density` (design `--density`). Comfortable keeps
// the roomy default; Compact tightens the vertical rhythm. Applied to
// the jobs-table row and the sidebar/list nav rows only.
const ROW_H_COMFORTABLE: f32 = 48.0;
const ROW_H_COMPACT: f32 = 40.0;
const SIDEBAR_ROW_H_COMFORTABLE: f32 = 26.0;
const SIDEBAR_ROW_H_COMPACT: f32 = 22.0;

// Queue live-dot (design `.q-live-dot`): a small moss dot shown next to
// a queue's color chip while that queue has ≥1 running job.
const LIVE_DOT_SIZE: f32 = 7.0;

// Toast (design `.toast`): bottom-right surface card with a 3px left
// accent border, auto-dismissed after `TOAST_TTL_MS`.
const TOAST_TTL_MS: u64 = 3000;
const TOAST_ACCENT_W: f32 = 3.0;
const TOAST_W: f32 = 320.0;
const TOAST_GAP: f32 = 8.0;
const TOAST_MARGIN: f32 = 16.0;

// Pulse clock (design `pulse` keyframe). Drives the live-dot's alpha;
// the whole subscription is gated on `!reduce_motion` (W6).
const PULSE_TICK_MS: u64 = 60;
const PULSE_SPEED: f32 = 3.2; // radians/sec of the sine pulse
const PULSE_MIN_ALPHA: f32 = 0.35;

// Drag-to-add overlay (design `.drag-overlay`): full-window clay wash
// (`rgba(201,112,63,.88)` ≈ clay-400 @ .88) with a centered glyph tile.
const DRAG_WASH_ALPHA: f32 = 0.88;

// Resize grip (design `ResizableHeader`): 1px quiet idle grip, 3px
// clay grip at ~70% height on hover, 3px clay-500 while dragging.
const GRIP_W_IDLE: f32 = 1.0;
const GRIP_W_ACTIVE: f32 = 3.0;
const GRIP_ACTIVE_RATIO: f32 = 0.7;

// Name-cell ext pill (design `.fname` ext tag): 28×22, radius 4, mono
// 700 ~9px, category-tinted; 10px gap to the title stack.
const EXT_PILL_W: f32 = 28.0;
const EXT_PILL_H: f32 = 22.0;
const EXT_PILL_RADIUS: f32 = 4.0;
const EXT_PILL_FONT: f32 = 9.0;
const NAME_PILL_GAP: f32 = 10.0;

// Column-resize guideline (design §4 `ResizableHeader`: "dragging →
// clay-500 grip + a full-height clay-300 guideline"): a 1px clay-300
// vertical rule over the table body at the dragged boundary.
const GUIDELINE_W: f32 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarFilter {
    All,
    Category(Category),
    Queue(QueueId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    All,
    Active,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortColumn {
    Name,
    Size,
    Status,
    Speed,
    Eta,
    Date,
}

#[derive(Clone)]
pub enum Msg {
    Connected(Result<(Arc<Client>, SnapshotData, Option<String>, bool), String>),
    Snapshot(SnapshotData),
    Daemon(DaemonSignal),
    Window(WindowControl),
    SetFilter(SidebarFilter),
    ToggleSection(u8),
    SetTab(Tab),
    SetSort(SortColumn),
    SetSearch(String),
    RowClick(JobId, bool, bool),
    RowDoubleClick(JobId),
    RowRightClick(JobId),
    Toolbar(ToolbarAction),
    Tool(ToolAction),
    CloseOverlay,
    Context(ContextAction),
    KeyPressed(iced::keyboard::Key, iced::keyboard::Modifiers),
    Modifiers(iced::keyboard::Modifiers),
    CursorMoved(f32, f32),
    MouseReleased,
    ColHandleHover(SortColumn, bool),
    TableScrolled(f32),
    WindowResized(f32, f32),
    ColResizeStart(SortColumn),
    HeaderRightClick,
    ColToggle(SortColumn),
    // About overlay
    AboutCheckUpdate,
    AboutChecked(Result<Option<crate::data::UpdateInfo>, String>),
    AboutDownloadUpdate,
    AboutRepository,
    AboutDonate,
    // Host settings overlay
    HostsLoaded(Vec<crate::domain::HostSetting>),
    HostSearch(String),
    HostSelect(String),
    HostAdd,
    HostDelete,
    HostHost(String),
    HostSpeedEnabled(bool),
    HostSpeedKbs(String),
    HostThreads(String),
    HostUsername(String),
    HostPassword(String),
    HostReveal(bool),
    HostUserAgent(String),
    HostSave,
    // Remove overlay
    RemoveAs(RemoveKind),
    RemoveDeleteOnDisk(bool),
    RemoveDontAsk(bool),
    RemoveConfirm,
    // Drag-to-add (design `.drag-overlay`)
    DragHover(bool),
    DragDropped(std::path::PathBuf),
    // Toasts (design `.toast`)
    Toast(ToastSeverity, String),
    ToastExpired(u64),
    // Pulse clock for the queue live-dot (gated on !reduce_motion)
    AnimTick,
    // Browser-extensions overlay store link
    OpenStore(&'static str),
    // First-run welcome overlay: either dismissal persists
    // `first_run_seen` (design §3.8 welcome mode)
    WelcomeDismiss,
    // Conflict / recovery
    Conflict(JobId, u64, crate::data::ConflictKind, ConflictChoice),
    DbExit,
    DbReset,
    SecretsWipe,
    SecretsWiped(Result<(), String>),
    ShotTick,
    Shot(iced::window::Screenshot),
    Noop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolbarAction {
    AddUrl,
    /// Start/Stop toggle (design §3.1): queue scope starts/stops the
    /// queue by `active_queues` membership; other scopes pause/resume
    /// everything. The handler re-derives the direction from state.
    ToggleRun,
    StopAll,
    Clean,
    Schedule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolAction {
    Scheduler,
    Settings,
    BrowserExtension,
    PerHost,
    About,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextAction {
    Open,
    OpenFolder,
    Resume,
    Pause,
    Restart,
    CopyUrl,
    Properties,
}

/// How the destructive context-menu row resolves once confirmed.
/// Modifiers morph the row (Finder-like); confirmation is never skipped
/// (B4) — the kind only PRE-SELECTS the destructive option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveKind {
    /// "Remove from list" — entry only (purges `.part` for incomplete).
    Entry,
    /// ⇧ "Move to Trash" — recoverable; final file to the OS trash.
    Trash,
    /// ⇧⌥ "Delete permanently" — irreversible on-disk delete.
    Permanent,
}

/// Toast severity drives the 3px left-accent color (design `.toast`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastSeverity {
    Info,
    Success,
    Error,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub id: u64,
    pub severity: ToastSeverity,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictChoice {
    Restart,
    Abort,
    Resume,
    Numbered,
    Replace,
    Ack,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    #[default]
    None,
    Context,
    About,
    Host,
    Remove,
    BrowserExtensions,
    /// First-run welcome variant of the browser-extensions dialog
    /// (design §3.8 `welcome` mode).
    Welcome,
    DbError,
    SecretsLocked,
}

pub enum App {
    Connecting,
    Failed(String),
    Ready(Box<Main>),
}

pub struct Main {
    pub client: Arc<Client>,
    pub snap: SnapshotData,
    pub counters: HashMap<JobId, JobCounters>,
    pub tokens: Tokens,
    pub filter: SidebarFilter,
    pub tab: Tab,
    pub sort: (SortColumn, bool), // (column, descending)
    pub search: String,
    pub selection: HashSet<JobId>,
    pub select_anchor: Option<JobId>,
    pub collapsed_sections: HashSet<u8>,
    pub maximized: bool,
    pub context_menu: Option<JobId>,
    pub overlay: Overlay,
    pub about: AboutState,
    pub host: HostState,
    pub remove: Option<RemoveState>,
    pub db_error: Option<String>,
    pub modifiers: iced::keyboard::Modifiers,
    pub cursor: (f32, f32),
    /// Cursor position captured when a popup menu opened — menus
    /// must not follow the moving mouse.
    pub menu_anchor: (f32, f32),
    pub win_size: (f32, f32),
    pub last_size_save: Option<std::time::Instant>,
    pub columns: crate::gui::ui_prefs::ColumnsState,
    /// Active header drag: (column, cursor x at start, width at start).
    pub col_drag: Option<(SortColumn, f32, f32)>,
    pub col_handle_hover: Option<SortColumn>,
    /// Horizontal scroll offset of the table body (mirrored on every
    /// `TableScrolled`); corrects the resize guideline x.
    pub table_scroll_x: f32,
    pub columns_menu: bool,
    pub shot: Option<Shot>,
    /// Active toasts, newest last (rendered bottom-right).
    pub toasts: Vec<Toast>,
    pub next_toast_id: u64,
    /// A URL/file is being dragged over the window (drag-to-add wash).
    pub drag_hover: bool,
    /// Accumulating pulse clock for the queue live-dot (seconds).
    pub anim_t: f32,
    /// Job count at the last snapshot — used to toast genuine adds.
    pub prev_job_count: usize,
    /// First-run welcome overlay already shown this session — never
    /// re-show even if the settings flag hasn't round-tripped yet.
    pub welcome_shown: bool,
}

impl Main {
    fn new(client: Arc<Client>, snap: SnapshotData) -> Self {
        let tokens = Tokens::from_settings(&snap.settings);
        let counters = snap.counters.iter().map(|c| (c.id, c.clone())).collect();
        let main_q = snap
            .queues
            .iter()
            .find(|q| q.builtin)
            .map(|q| q.id)
            .or_else(|| snap.queues.first().map(|q| q.id));
        Self {
            client,
            tokens,
            counters,
            filter: main_q
                .map(SidebarFilter::Queue)
                .unwrap_or(SidebarFilter::All),
            tab: Tab::All,
            sort: (SortColumn::Date, true),
            search: String::new(),
            selection: HashSet::new(),
            select_anchor: None,
            collapsed_sections: HashSet::new(),
            maximized: false,
            context_menu: None,
            overlay: Overlay::None,
            about: AboutState::default(),
            host: HostState::default(),
            remove: None,
            db_error: None,
            modifiers: iced::keyboard::Modifiers::default(),
            cursor: (0.0, 0.0),
            menu_anchor: (0.0, 0.0),
            win_size: (0.0, 0.0),
            last_size_save: None,
            columns: crate::gui::ui_prefs::load().columns.unwrap_or_default(),
            col_drag: None,
            col_handle_hover: None,
            table_scroll_x: 0.0,
            columns_menu: false,
            shot: Shot::from_env(),
            toasts: Vec::new(),
            next_toast_id: 0,
            drag_hover: false,
            anim_t: 0.0,
            prev_job_count: snap.jobs.len(),
            welcome_shown: false,
            snap,
        }
    }

    /// Table row height for the active UI density.
    fn row_h(&self) -> f32 {
        match self.snap.settings.ui_density {
            Density::Comfortable => ROW_H_COMFORTABLE,
            Density::Compact => ROW_H_COMPACT,
        }
    }

    /// Sidebar/list nav row height for the active UI density.
    fn sidebar_row_h(&self) -> f32 {
        match self.snap.settings.ui_density {
            Density::Comfortable => SIDEBAR_ROW_H_COMFORTABLE,
            Density::Compact => SIDEBAR_ROW_H_COMPACT,
        }
    }

    /// Queue ids that currently host ≥1 running job (N2: keyed off
    /// `Phase::is_running`, not `== Downloading`).
    fn live_queues(&self) -> HashSet<QueueId> {
        self.snap
            .jobs
            .iter()
            .filter(|j| self.phase(j.id).is_running())
            .map(|j| j.queue_id)
            .collect()
    }

    /// Any job currently running (Pause all / Resume all direction).
    fn any_running(&self) -> bool {
        self.snap.jobs.iter().any(|j| self.phase(j.id).is_running())
    }

    /// Whether the toolbar Start/Stop toggle has anything to act on
    /// (design §3.1: "Start disabled when nothing resumable").
    /// Queue scope: pausing an active queue is always actionable;
    /// starting needs ≥1 non-terminal job in the queue. Other scopes:
    /// pausing needs something running; resuming needs ≥1 job that is
    /// neither running nor terminal (Queued/Paused/Cancelled).
    fn toggle_actionable(&self) -> bool {
        match self.filter {
            SidebarFilter::Queue(q) => {
                self.snap.active_queues.contains(&q)
                    || self
                        .snap
                        .jobs
                        .iter()
                        .any(|j| j.queue_id == q && !self.phase(j.id).is_terminal())
            }
            _ => {
                self.any_running()
                    || self.snap.jobs.iter().any(|j| {
                        let p = self.phase(j.id);
                        !p.is_running() && !p.is_terminal()
                    })
            }
        }
    }

    fn push_toast(&mut self, severity: ToastSeverity, message: String) -> u64 {
        let id = self.next_toast_id;
        self.next_toast_id += 1;
        self.toasts.push(Toast {
            id,
            severity,
            message,
        });
        id
    }

    fn phase(&self, id: JobId) -> Phase {
        self.counters
            .get(&id)
            .map(|c| c.phase)
            .or_else(|| {
                self.snap
                    .jobs
                    .iter()
                    .find(|j| j.id == id)
                    .map(|j| j.status.phase)
            })
            .unwrap_or(Phase::Queued)
    }

    /// Jobs passing the sidebar filter + search (before tab filter).
    fn sidebar_filtered(&self) -> Vec<&crate::domain::Job> {
        let needle = self.search.trim().to_lowercase();
        self.snap
            .jobs
            .iter()
            .filter(|j| match self.filter {
                SidebarFilter::All => true,
                SidebarFilter::Category(c) => j.category == c,
                SidebarFilter::Queue(q) => j.queue_id == q,
            })
            .filter(|j| {
                needle.is_empty()
                    || j.filename
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&needle)
                    || j.url.as_str().to_lowercase().contains(&needle)
            })
            .collect()
    }

    fn visible_jobs(&self) -> Vec<&crate::domain::Job> {
        let mut jobs: Vec<_> = self
            .sidebar_filtered()
            .into_iter()
            .filter(|j| match self.tab {
                Tab::All => true,
                Tab::Active => !self.phase(j.id).is_terminal(),
                Tab::Finished => self.phase(j.id) == Phase::Completed,
            })
            .collect();
        let (col, desc) = self.sort;
        jobs.sort_by(|a, b| {
            let ord = match col {
                SortColumn::Name => a
                    .filename
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .cmp(&b.filename.as_deref().unwrap_or("").to_lowercase()),
                SortColumn::Size => {
                    let sa = self.counters.get(&a.id).and_then(|c| c.total).unwrap_or(0);
                    let sb = self.counters.get(&b.id).and_then(|c| c.total).unwrap_or(0);
                    sa.cmp(&sb)
                }
                SortColumn::Status => (self.phase(a.id) as u8).cmp(&(self.phase(b.id) as u8)),
                SortColumn::Speed => {
                    let sa = self.counters.get(&a.id).map(|c| c.speed_bps).unwrap_or(0.0);
                    let sb = self.counters.get(&b.id).map(|c| c.speed_bps).unwrap_or(0.0);
                    sa.total_cmp(&sb)
                }
                SortColumn::Eta => {
                    let ea = eta_of(self.counters.get(&a.id)).unwrap_or(u64::MAX);
                    let eb = eta_of(self.counters.get(&b.id)).unwrap_or(u64::MAX);
                    ea.cmp(&eb)
                }
                SortColumn::Date => a.created_at.cmp(&b.created_at),
            };
            if desc { ord.reverse() } else { ord }
        });
        jobs
    }

    fn cat_count(&self, cat: Option<Category>) -> u64 {
        self.snap
            .jobs
            .iter()
            .filter(|j| cat.is_none_or(|c| j.category == c))
            .count() as u64
    }
}

fn eta_of(c: Option<&JobCounters>) -> Option<u64> {
    let c = c?;
    let total = c.total?;
    if c.speed_bps <= 1.0 || total <= c.downloaded {
        return None;
    }
    Some(((total - c.downloaded) as f64 / c.speed_bps) as u64)
}

pub fn boot() -> (App, Task<Msg>) {
    (
        App::Connecting,
        Task::perform(
            async {
                let client = Client::connect_retry(std::time::Duration::from_secs(8))
                    .await
                    .map_err(|e| e.to_string())?;
                client
                    .hello(crate::ipc_local::protocol::GuiKind::Main)
                    .await?;
                let snap = client.snapshot().await?;
                let db_error = client.db_status().await.ok().flatten();
                // `secrets_status` returns `locked` directly (true =
                // keyring/master key unavailable).
                let secrets_locked = client.secrets_status().await.unwrap_or(false);
                Ok((client, snap, db_error, secrets_locked))
            },
            Msg::Connected,
        ),
    )
}

fn act<F>(fut: F) -> Task<Msg>
where
    F: std::future::Future<Output = Result<(), String>> + Send + 'static,
{
    Task::perform(
        async move {
            if let Err(e) = fut.await {
                tracing::warn!("ipc action failed: {e}");
            }
        },
        |_| Msg::Noop,
    )
}

fn refresh(client: Arc<Client>) -> Task<Msg> {
    Task::perform(async move { client.snapshot().await }, |r| match r {
        Ok(snap) => Msg::Snapshot(snap),
        Err(_) => Msg::Noop,
    })
}

/// Push a toast and schedule its expiry. The TTL timer is a one-shot
/// task keyed to the toast id (no periodic subscription needed).
fn spawn_toast(m: &mut Main, severity: ToastSeverity, message: String) -> Task<Msg> {
    let id = m.push_toast(severity, message);
    Task::perform(
        async move {
            tokio::time::sleep(std::time::Duration::from_millis(TOAST_TTL_MS)).await;
        },
        move |_| Msg::ToastExpired(id),
    )
}

pub fn update(app: &mut App, msg: Msg) -> Task<Msg> {
    match msg {
        Msg::Connected(Ok((client, snap, db_error, secrets_locked))) => {
            let mut m = Main::new(client, snap);
            if let Some(e) = db_error {
                m.db_error = Some(e);
                m.overlay = Overlay::DbError;
            } else if secrets_locked {
                m.overlay = Overlay::SecretsLocked;
            } else if !m.snap.settings.first_run_seen {
                // First launch: welcome variant of the extensions
                // dialog (design §3.8). Recovery overlays win.
                m.overlay = Overlay::Welcome;
                m.welcome_shown = true;
            }
            *app = App::Ready(Box::new(m));
            Task::none()
        }
        Msg::Connected(Err(e)) => {
            *app = App::Failed(e);
            Task::none()
        }
        Msg::Window(ctl) => {
            if let (App::Ready(m), WindowControl::ToggleMaximize) = (&mut *app, ctl) {
                m.maximized = !m.maximized;
            }
            chrome::window_task(ctl)
        }
        msg => {
            let App::Ready(main) = app else {
                return Task::none();
            };
            update_main(main, msg)
        }
    }
}

fn update_main(m: &mut Main, msg: Msg) -> Task<Msg> {
    match msg {
        Msg::Connected(_) | Msg::Window(_) => unreachable!(),
        Msg::Snapshot(snap) => {
            m.tokens = Tokens::from_settings(&snap.settings);
            m.counters = snap.counters.iter().map(|c| (c.id, c.clone())).collect();
            let new_count = snap.jobs.len();
            // Toast genuine adds (count grew). Removals are toasted from
            // the confirm path, so only surface increases here.
            let added = new_count.saturating_sub(m.prev_job_count);
            m.prev_job_count = new_count;
            m.snap = snap;
            m.selection
                .retain(|id| m.snap.jobs.iter().any(|j| j.id == *id));
            if !m.welcome_shown && !m.snap.settings.first_run_seen && m.overlay == Overlay::None {
                m.overlay = Overlay::Welcome;
                m.welcome_shown = true;
            }
            if added > 0 {
                let msg = if added == 1 {
                    "Download added".to_owned()
                } else {
                    format!("{added} downloads added")
                };
                return spawn_toast(m, ToastSeverity::Info, msg);
            }
            Task::none()
        }
        Msg::Daemon(DaemonSignal::Lost) => iced::exit(),
        Msg::Daemon(DaemonSignal::Event(ev)) => match ev {
            Event::Counters(list) => {
                for c in list {
                    m.counters.insert(c.id, c);
                }
                Task::none()
            }
            Event::JobsChanged
            | Event::QueuesChanged
            | Event::SettingsChanged
            | Event::ActiveQueuesChanged
            | Event::ConflictChanged => refresh(m.client.clone()),
            // The grace countdown lives in its own window
            // (`gui power`); the main window ignores these.
            Event::ShutdownPending { .. } | Event::ShutdownCancelled => Task::none(),
            Event::Close => iced::exit(),
            Event::Focus | Event::ShowMainWindow => iced::window::latest().and_then(|id| {
                Task::batch([
                    iced::window::minimize(id, false),
                    iced::window::gain_focus(id),
                ])
            }),
            Event::OpenDownloadDialog(id) => {
                let client = m.client.clone();
                act(async move { client.open_download_window(id).await })
            }
            _ => Task::none(),
        },
        Msg::SetFilter(f) => {
            m.filter = f;
            m.selection.clear();
            Task::none()
        }
        Msg::ToggleSection(s) => {
            if !m.collapsed_sections.remove(&s) {
                m.collapsed_sections.insert(s);
            }
            Task::none()
        }
        Msg::SetTab(tab) => {
            m.tab = tab;
            Task::none()
        }
        Msg::SetSort(col) => {
            if m.sort.0 == col {
                m.sort.1 = !m.sort.1;
            } else {
                m.sort = (col, matches!(col, SortColumn::Date));
            }
            Task::none()
        }
        Msg::SetSearch(s) => {
            m.search = s;
            Task::none()
        }
        Msg::RowClick(id, ctrl, shift) => {
            m.context_menu = None;
            if ctrl {
                if !m.selection.remove(&id) {
                    m.selection.insert(id);
                }
                m.select_anchor = Some(id);
            } else if shift && m.select_anchor.is_some() {
                let order: Vec<JobId> = m.visible_jobs().iter().map(|j| j.id).collect();
                let a = order.iter().position(|x| Some(*x) == m.select_anchor);
                let b = order.iter().position(|x| *x == id);
                if let (Some(a), Some(b)) = (a, b) {
                    let (lo, hi) = (a.min(b), a.max(b));
                    m.selection = order[lo..=hi].iter().copied().collect();
                }
            } else {
                m.selection.clear();
                m.selection.insert(id);
                m.select_anchor = Some(id);
            }
            Task::none()
        }
        Msg::RowDoubleClick(id) => {
            let client = m.client.clone();
            if m.phase(id) == Phase::Completed {
                if let Some(job) = m.snap.jobs.iter().find(|j| j.id == id) {
                    let path = job.save_dir.join(job.filename.as_deref().unwrap_or(""));
                    crate::platform::open_path(&path);
                }
                Task::none()
            } else {
                act(async move { client.open_download_window(id).await })
            }
        }
        Msg::RowRightClick(id) => {
            if !m.selection.contains(&id) {
                m.selection.clear();
                m.selection.insert(id);
                m.select_anchor = Some(id);
            }
            m.context_menu = Some(id);
            m.menu_anchor = m.cursor;
            Task::none()
        }
        Msg::CloseOverlay => {
            m.context_menu = None;
            m.columns_menu = false;
            // Escape/backdrop on the welcome overlay is a dismissal
            // too — it must persist `first_run_seen` like the buttons.
            if m.overlay == Overlay::Welcome {
                return update_main(m, Msg::WelcomeDismiss);
            }
            if !matches!(m.overlay, Overlay::DbError | Overlay::SecretsLocked) {
                m.overlay = Overlay::None;
            }
            Task::none()
        }
        Msg::WelcomeDismiss => {
            m.overlay = Overlay::None;
            if m.snap.settings.first_run_seen {
                return Task::none();
            }
            // Optimistic local flip; the daemon echoes SettingsChanged.
            m.snap.settings.first_run_seen = true;
            let settings = m.snap.settings.clone();
            let client = m.client.clone();
            act(async move { client.update_settings(settings).await })
        }
        Msg::KeyPressed(key, mods) => handle_key(m, key, mods),
        Msg::Modifiers(mods) => {
            m.modifiers = mods;
            Task::none()
        }
        Msg::CursorMoved(x, y) => {
            m.cursor = (x, y);
            if let Some((col, start_x, start_w)) = m.col_drag {
                m.columns.set_width(col as usize, start_w + (x - start_x));
            }
            Task::none()
        }
        Msg::MouseReleased => {
            if m.col_drag.take().is_some() {
                crate::gui::ui_prefs::save_columns(&m.columns);
            }
            Task::none()
        }
        Msg::ColResizeStart(col) => {
            m.col_drag = Some((col, m.cursor.0, m.columns.width(col as usize)));
            Task::none()
        }
        Msg::TableScrolled(x) => {
            m.table_scroll_x = x;
            iced::widget::operation::scroll_to(
                iced::widget::Id::new("tbl-header"),
                iced::widget::scrollable::AbsoluteOffset {
                    x: Some(x),
                    y: None,
                },
            )
        }
        Msg::ColHandleHover(col, on) => {
            if on {
                m.col_handle_hover = Some(col);
            } else if m.col_handle_hover == Some(col) {
                m.col_handle_hover = None;
            }
            Task::none()
        }
        Msg::HeaderRightClick => {
            m.columns_menu = true;
            m.menu_anchor = m.cursor;
            Task::none()
        }
        Msg::ColToggle(col) => {
            m.columns.toggle(col as usize);
            crate::gui::ui_prefs::save_columns(&m.columns);
            Task::none()
        }
        Msg::WindowResized(w, h) => {
            m.win_size = (w, h);
            let clamp =
                chrome::enforce_min_size(iced::Size::new(w, h), iced::Size::new(820.0, 520.0));
            let due = m
                .last_size_save
                .is_none_or(|t| t.elapsed().as_millis() > 1000);
            if due {
                m.last_size_save = Some(std::time::Instant::now());
                crate::gui::ui_prefs::save_window(crate::gui::ui_prefs::WindowPrefs {
                    width: w,
                    height: h,
                });
            }
            clamp
        }
        Msg::AboutCheckUpdate => {
            m.about.update = UpdateUi::Checking;
            let client = m.client.clone();
            Task::perform(
                async move { client.update_check().await },
                Msg::AboutChecked,
            )
        }
        Msg::AboutChecked(res) => {
            m.about.update = match res {
                Ok(Some(info)) => UpdateUi::Available(info),
                Ok(None) => UpdateUi::UpToDate,
                Err(e) => UpdateUi::Error(e),
            };
            Task::none()
        }
        Msg::AboutDownloadUpdate => {
            if let UpdateUi::Available(info) = m.about.update.clone() {
                m.about.update = UpdateUi::Downloading(info.version.clone());
                let client = m.client.clone();
                Task::perform(
                    async move {
                        let name = format!("oxdm-update-{}", info.version);
                        client.add_update_job(info.url.clone(), Some(name)).await
                    },
                    |_| Msg::Noop,
                )
            } else {
                Task::none()
            }
        }
        Msg::AboutRepository => {
            crate::platform::open_url("https://github.com/jd1378/oxdm");
            Task::none()
        }
        Msg::AboutDonate => {
            crate::platform::open_url("https://github.com/sponsors/jd1378");
            Task::none()
        }
        Msg::HostsLoaded(hosts) => {
            m.host.hosts = hosts;
            Task::none()
        }
        Msg::HostSearch(v) => {
            m.host.search = v;
            Task::none()
        }
        Msg::HostSelect(host) => {
            if let Some(h) = m.host.hosts.iter().find(|h| h.host == host).cloned() {
                m.host.hydrate(&h);
            }
            Task::none()
        }
        Msg::HostAdd => {
            let hosts = std::mem::take(&mut m.host.hosts);
            let search = std::mem::take(&mut m.host.search);
            m.host = HostState {
                hosts,
                search,
                ..Default::default()
            };
            Task::none()
        }
        Msg::HostDelete => {
            let Some(host) = m.host.selected.clone() else {
                return Task::none();
            };
            m.host.hosts.retain(|h| h.host != host);
            let reload = m.host.hosts.clone();
            m.host = HostState {
                hosts: reload,
                ..Default::default()
            };
            let client = m.client.clone();
            Task::perform(async move { client.delete_host(host).await }, |_| Msg::Noop)
        }
        Msg::HostHost(v) => {
            m.host.host = v;
            Task::none()
        }
        Msg::HostSpeedEnabled(v) => {
            m.host.speed_enabled = v;
            Task::none()
        }
        Msg::HostSpeedKbs(v) => {
            m.host.speed_kbs = v;
            Task::none()
        }
        Msg::HostThreads(v) => {
            m.host.threads = v;
            Task::none()
        }
        Msg::HostUsername(v) => {
            m.host.username = v;
            Task::none()
        }
        Msg::HostPassword(v) => {
            m.host.password = v;
            Task::none()
        }
        Msg::HostReveal(v) => {
            m.host.password_revealed = v;
            Task::none()
        }
        Msg::HostUserAgent(v) => {
            m.host.user_agent = v;
            Task::none()
        }
        Msg::HostSave => {
            let setting = m.host.build();
            let old = m.host.selected.clone();
            let client = m.client.clone();
            Task::perform(
                async move {
                    if let Some(old) = old
                        && old != setting.host
                    {
                        let _ = client.delete_host(old).await;
                    }
                    client.upsert_host(setting).await?;
                    client.host_list().await
                },
                |r| match r {
                    Ok(hosts) => Msg::HostsLoaded(hosts),
                    Err(_) => Msg::Noop,
                },
            )
        }
        Msg::RemoveAs(kind) => {
            m.context_menu = None;
            request_remove(m, kind)
        }
        Msg::DragHover(on) => {
            m.drag_hover = on;
            Task::none()
        }
        Msg::DragDropped(path) => {
            m.drag_hover = false;
            // iced only delivers file drops (paths); a path string is a
            // valid Add prefill (the Add flow resolves URLs vs files).
            let prefill = path.to_string_lossy().into_owned();
            if prefill.trim().is_empty() {
                return Task::none();
            }
            let client = m.client.clone();
            act(async move { client.open_add_window(None, Some(prefill)).await })
        }
        Msg::Toast(severity, message) => spawn_toast(m, severity, message),
        Msg::ToastExpired(id) => {
            m.toasts.retain(|t| t.id != id);
            Task::none()
        }
        Msg::AnimTick => {
            m.anim_t += PULSE_TICK_MS as f32 / 1000.0;
            Task::none()
        }
        Msg::OpenStore(url) => {
            crate::platform::open_url(url);
            Task::none()
        }
        Msg::RemoveDeleteOnDisk(v) => {
            if let Some(r) = &mut m.remove {
                r.delete_on_disk = v;
            }
            Task::none()
        }
        Msg::RemoveDontAsk(v) => {
            if let Some(r) = &mut m.remove {
                r.dont_ask_again = v;
            }
            Task::none()
        }
        Msg::RemoveConfirm => {
            m.overlay = Overlay::None;
            let Some(r) = m.remove.take() else {
                return Task::none();
            };
            let trash = matches!(r.kind, RemoveKind::Trash);
            // Resolve final paths up-front (the snapshot is borrowed
            // here; the async block must own its data).
            let trash_paths: Vec<std::path::PathBuf> = if trash {
                r.ids
                    .iter()
                    .filter_map(|id| {
                        m.snap.jobs.iter().find(|j| j.id == *id).and_then(|j| {
                            j.status
                                .final_path
                                .clone()
                                .or_else(|| j.filename.as_ref().map(|f| j.save_dir.join(f)))
                        })
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let n = r.ids.len();
            let client = m.client.clone();
            let mut settings = m.snap.settings.clone();
            Task::perform(
                async move {
                    // N4: surface the FIRST trash failure as an error
                    // toast (no DBus / cross-device → never silent). The
                    // entry is still removed below so the list stays sane.
                    let mut trash_err: Option<String> = None;
                    for p in trash_paths {
                        let res = tokio::task::spawn_blocking(move || trash::delete(&p))
                            .await
                            .map_err(|e| e.to_string())
                            .and_then(|r| r.map_err(|e| e.to_string()));
                        if let Err(e) = res {
                            trash_err.get_or_insert(e);
                        }
                    }
                    for id in &r.ids {
                        let _ = client
                            .remove(
                                *id,
                                crate::data::RemoveOpts {
                                    purge_partial: !r.completed,
                                    // Trash already moved the file; never
                                    // double-delete on disk.
                                    delete_final_file: r.completed && r.delete_on_disk && !trash,
                                },
                            )
                            .await;
                    }
                    if r.dont_ask_again {
                        if r.completed {
                            settings.remove_confirm_completed = false;
                        } else {
                            settings.remove_confirm_incomplete = false;
                        }
                        let _ = client.update_settings(settings).await;
                    }
                    trash_err
                },
                move |trash_err| match trash_err {
                    Some(e) => {
                        Msg::Toast(ToastSeverity::Error, format!("Couldn't move to Trash: {e}"))
                    }
                    None => {
                        let what = if n == 1 {
                            "Removed download".to_owned()
                        } else {
                            format!("Removed {n} downloads")
                        };
                        Msg::Toast(ToastSeverity::Success, what)
                    }
                },
            )
        }
        Msg::Conflict(id, token, kind, choice) => {
            let client = m.client.clone();
            Task::perform(
                async move {
                    use crate::data::ConflictKind as K;
                    use crate::ipc_local::protocol::{
                        FileChangedRes, FinalFileRes, NotResumableRes, SameDownloadRes,
                    };
                    let r = match (kind, choice) {
                        (K::FileChanged, ConflictChoice::Restart) => {
                            client
                                .resolve_file_changed(id, token, FileChangedRes::Restart)
                                .await
                        }
                        (K::FileChanged, _) => {
                            client
                                .resolve_file_changed(id, token, FileChangedRes::Abort)
                                .await
                        }
                        (K::NotResumable, ConflictChoice::Restart) => {
                            client
                                .resolve_not_resumable(id, token, NotResumableRes::Restart)
                                .await
                        }
                        (K::NotResumable, _) => {
                            client
                                .resolve_not_resumable(id, token, NotResumableRes::Abort)
                                .await
                        }
                        (K::SameDownloadExists, ConflictChoice::Resume) => {
                            client
                                .resolve_same_download(id, token, SameDownloadRes::Resume)
                                .await
                        }
                        (K::SameDownloadExists, ConflictChoice::Numbered) => {
                            client
                                .resolve_same_download(
                                    id,
                                    token,
                                    SameDownloadRes::AddNumberAndContinue,
                                )
                                .await
                        }
                        (K::SameDownloadExists, _) => {
                            client
                                .resolve_same_download(id, token, SameDownloadRes::Abort)
                                .await
                        }
                        (K::FinalFileExists, ConflictChoice::Replace) => {
                            client
                                .resolve_final_file(id, token, FinalFileRes::Replace)
                                .await
                        }
                        (K::FinalFileExists, ConflictChoice::Numbered) => {
                            client
                                .resolve_final_file(id, token, FinalFileRes::AddNumberAndContinue)
                                .await
                        }
                        (K::FinalFileExists, _) => {
                            client
                                .resolve_final_file(id, token, FinalFileRes::Abort)
                                .await
                        }
                        (K::UrlBroken | K::CredentialsInvalid, _) => Ok(()),
                    };
                    let _ = r;
                    let _ = client.pop_conflict().await;
                },
                |_| Msg::Noop,
            )
        }
        Msg::DbExit => {
            let client = m.client.clone();
            Task::perform(async move { client.daemon_quit().await }, |_| Msg::Noop)
                .chain(iced::exit())
        }
        Msg::DbReset => {
            m.overlay = Overlay::None;
            m.db_error = None;
            let client = m.client.clone();
            Task::perform(
                async move {
                    let _ = client.reset_database().await;
                },
                |_| Msg::Noop,
            )
            .chain(iced::exit())
        }
        Msg::SecretsWipe => {
            let client = m.client.clone();
            Task::perform(
                async move { client.wipe_job_secrets().await },
                Msg::SecretsWiped,
            )
        }
        Msg::SecretsWiped(res) => match res {
            Ok(()) => {
                m.overlay = Overlay::None;
                refresh(m.client.clone())
            }
            Err(e) => {
                tracing::warn!("wipe_job_secrets failed: {e}");
                m.db_error = Some(format!("Could not wipe job secrets: {e}"));
                m.overlay = Overlay::DbError;
                Task::none()
            }
        },
        Msg::Context(action) => {
            m.context_menu = None;
            context_action(m, action)
        }
        Msg::Toolbar(action) => {
            let client = m.client.clone();
            match action {
                ToolbarAction::AddUrl => {
                    act(async move { client.open_add_window(None, None).await })
                }
                ToolbarAction::ToggleRun => match m.filter {
                    // Queue scope: direction keyed on `active_queues`
                    // membership (design §3.1 Start/Stop queue).
                    SidebarFilter::Queue(q) => {
                        if m.snap.active_queues.contains(&q) {
                            act(async move { client.stop_queue(q).await })
                        } else {
                            act(async move { client.start_queue(q).await })
                        }
                    }
                    // Other scopes: pause/resume everything, keyed on
                    // whether anything is running.
                    _ => {
                        if m.any_running() {
                            act(async move { client.pause_all().await })
                        } else {
                            act(async move { client.resume_all().await })
                        }
                    }
                },
                ToolbarAction::StopAll => act(async move { client.pause_all().await }),
                ToolbarAction::Clean => {
                    let done: Vec<JobId> = m
                        .snap
                        .jobs
                        .iter()
                        .filter(|j| m.phase(j.id) == Phase::Completed)
                        .map(|j| j.id)
                        .collect();
                    act(async move {
                        for id in done {
                            client
                                .remove(id, crate::data::RemoveOpts::default())
                                .await?;
                        }
                        Ok(())
                    })
                }
                ToolbarAction::Schedule => act(async move { client.open_queues_window().await }),
            }
        }
        Msg::Tool(tool) => {
            let client = m.client.clone();
            match tool {
                ToolAction::Scheduler => act(async move { client.open_queues_window().await }),
                ToolAction::Settings => {
                    act(async move { client.open_settings_window(None, false).await })
                }
                ToolAction::BrowserExtension => {
                    m.overlay = Overlay::BrowserExtensions;
                    Task::none()
                }
                ToolAction::PerHost => {
                    m.overlay = Overlay::Host;
                    m.host = HostState::default();
                    let client = m.client.clone();
                    Task::perform(async move { client.host_list().await }, |r| match r {
                        Ok(hosts) => Msg::HostsLoaded(hosts),
                        Err(_) => Msg::Noop,
                    })
                }
                ToolAction::About => {
                    m.overlay = Overlay::About;
                    m.about = AboutState::default();
                    Task::none()
                }
            }
        }
        Msg::ShotTick => {
            if let Some(shot) = &mut m.shot {
                if let Some(task) = shot.tick() {
                    return task.map(Msg::Shot);
                }
            }
            Task::none()
        }
        Msg::Shot(s) => match &m.shot {
            Some(shot) => shot.save_and_exit(s),
            None => Task::none(),
        },
        Msg::Noop => Task::none(),
    }
}

fn handle_key(
    m: &mut Main,
    key: iced::keyboard::Key,
    mods: iced::keyboard::Modifiers,
) -> Task<Msg> {
    use iced::keyboard::Key;
    use iced::keyboard::key::Named;
    m.modifiers = mods;
    match key.as_ref() {
        Key::Character("n") if mods.command() => {
            update_main(m, Msg::Toolbar(ToolbarAction::AddUrl))
        }
        Key::Character("q") if mods.command() => {
            let client = m.client.clone();
            Task::perform(async move { client.daemon_quit().await }, |_| Msg::Noop)
                .chain(iced::exit())
        }
        Key::Named(Named::Delete) if !m.selection.is_empty() && m.overlay == Overlay::None => {
            // Keyboard Delete is the SAFE default (entry only); the
            // destructive escalation lives on the context menu.
            request_remove(m, RemoveKind::Entry)
        }
        // Confirm-dialog keys (design `confirm-dialog.jsx`): Enter
        // confirms, Escape cancels. `listen_with` ignores capture
        // status, so Enter is gated on the confirm overlay being open.
        Key::Named(Named::Enter) if m.overlay == Overlay::Remove => {
            update_main(m, Msg::RemoveConfirm)
        }
        Key::Named(Named::Escape) => update_main(m, Msg::CloseOverlay),
        _ => Task::none(),
    }
}

/// Delete request: show the confirm overlay when settings demand it,
/// else remove immediately. `kind` PRE-SELECTS the destructive option
/// (B4) — confirmation is NEVER skipped for the irreversible kinds.
fn request_remove(m: &mut Main, kind: RemoveKind) -> Task<Msg> {
    let ids: Vec<JobId> = m.selection.iter().copied().collect();
    if ids.is_empty() {
        return Task::none();
    }
    let completed = ids.iter().all(|id| m.phase(*id) == Phase::Completed);
    let need_confirm = if completed {
        m.snap.settings.remove_confirm_completed
    } else {
        m.snap.settings.remove_confirm_incomplete
    };
    let filename = if ids.len() == 1 {
        m.snap
            .jobs
            .iter()
            .find(|j| j.id == ids[0])
            .and_then(|j| j.filename.clone())
            .unwrap_or_else(|| "download".to_owned())
    } else {
        format!("{} downloads", ids.len())
    };
    m.remove = Some(RemoveState {
        ids,
        filename,
        completed,
        kind,
        // Permanent pre-checks "also delete file on disk" for completed
        // entries; the user can still untick before confirming.
        delete_on_disk: matches!(kind, RemoveKind::Permanent) && completed,
        dont_ask_again: false,
    });
    // Trash / Permanent are irreversible → ALWAYS confirm (B4); only the
    // safe entry-only removal honors the "don't ask again" preference.
    let force_confirm = !matches!(kind, RemoveKind::Entry);
    if need_confirm || force_confirm {
        m.overlay = Overlay::Remove;
        Task::none()
    } else {
        update_main(m, Msg::RemoveConfirm)
    }
}

fn context_action(m: &mut Main, action: ContextAction) -> Task<Msg> {
    let ids: Vec<JobId> = m.selection.iter().copied().collect();
    let client = m.client.clone();
    match action {
        ContextAction::Open | ContextAction::OpenFolder => {
            for id in &ids {
                if let Some(job) = m.snap.jobs.iter().find(|j| j.id == *id) {
                    let path = match action {
                        ContextAction::Open => {
                            job.save_dir.join(job.filename.as_deref().unwrap_or(""))
                        }
                        _ => job.save_dir.clone(),
                    };
                    crate::platform::open_path(&path);
                }
            }
            Task::none()
        }
        ContextAction::Resume => act(async move {
            for id in ids {
                client.resume(id).await?;
            }
            Ok(())
        }),
        ContextAction::Pause => act(async move {
            for id in ids {
                client.pause(id).await?;
            }
            Ok(())
        }),
        ContextAction::Restart => act(async move {
            for id in ids {
                client.restart_job(id).await?;
            }
            Ok(())
        }),
        ContextAction::CopyUrl => {
            let urls: Vec<String> = m
                .snap
                .jobs
                .iter()
                .filter(|j| ids.contains(&j.id))
                .map(|j| j.url.to_string())
                .collect();
            iced::clipboard::write(urls.join("\n"))
        }
        ContextAction::Properties => act(async move {
            for id in ids {
                client.open_properties_window(id).await?;
            }
            Ok(())
        }),
    }
}

pub fn subscription(app: &App) -> Subscription<Msg> {
    let mut subs = vec![];
    if let App::Ready(m) = app {
        subs.push(crate::gui::ipc::all_events().map(Msg::Daemon));
        subs.push(iced::event::listen_with(
            |event, _status, _id| match event {
                iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                    key, modifiers, ..
                }) => Some(Msg::KeyPressed(key, modifiers)),
                iced::Event::Keyboard(iced::keyboard::Event::ModifiersChanged(mods)) => {
                    Some(Msg::Modifiers(mods))
                }
                iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
                    Some(Msg::CursorMoved(position.x, position.y))
                }
                iced::Event::Mouse(iced::mouse::Event::ButtonReleased(
                    iced::mouse::Button::Left,
                )) => Some(Msg::MouseReleased),
                iced::Event::Window(iced::window::Event::Resized(size)) => {
                    Some(Msg::WindowResized(size.width, size.height))
                }
                // Drag-to-add (design `.drag-overlay`). NOTE: file-drop
                // events are compositor-dependent and may not deliver on
                // all Wayland/X11 setups — the code is correct but cannot
                // be verified headless.
                iced::Event::Window(iced::window::Event::FileHovered(_)) => {
                    Some(Msg::DragHover(true))
                }
                iced::Event::Window(iced::window::Event::FilesHoveredLeft) => {
                    Some(Msg::DragHover(false))
                }
                iced::Event::Window(iced::window::Event::FileDropped(path)) => {
                    Some(Msg::DragDropped(path))
                }
                _ => None,
            },
        ));
        if m.shot.is_some() {
            subs.push(Shot::frames().map(|_| Msg::ShotTick));
        }
        // Pulse clock for the queue live-dot. Gated on !reduce_motion
        // (W6) and only while a queue actually has running work.
        if !m.snap.settings.reduce_motion && m.snap.jobs.iter().any(|j| m.phase(j.id).is_running())
        {
            subs.push(
                iced::time::every(std::time::Duration::from_millis(PULSE_TICK_MS))
                    .map(|_| Msg::AnimTick),
            );
        }
    }
    Subscription::batch(subs)
}

/// Pulsing alpha for the live-dot (design `pulse` keyframe). Static at
/// full opacity when motion is reduced.
fn pulse_alpha(m: &Main) -> f32 {
    if m.snap.settings.reduce_motion {
        return 1.0;
    }
    let s = (m.anim_t * PULSE_SPEED).sin() * 0.5 + 0.5; // 0..1
    PULSE_MIN_ALPHA + (1.0 - PULSE_MIN_ALPHA) * s
}

pub fn theme_of(app: &App) -> iced::Theme {
    match app {
        App::Ready(m) => m.tokens.iced_theme(),
        _ => default_tokens().iced_theme(),
    }
}

fn default_tokens() -> Tokens {
    match crate::gui::theme::system_theme() {
        theme::ResolvedTheme::Light => Tokens::light(),
        theme::ResolvedTheme::Warm => Tokens::warm(),
        theme::ResolvedTheme::Dark => Tokens::dark(),
    }
}

pub fn view(app: &App) -> Element<'_, Msg> {
    match app {
        App::Connecting => splash("Connecting to the oxdm daemon…".to_owned()),
        App::Failed(e) => splash(format!("Could not reach the daemon: {e}")),
        App::Ready(m) => main_view(m),
    }
}

fn splash<'a>(message: String) -> Element<'a, Msg> {
    let t = default_tokens();
    container(
        text(message)
            .font(theme::BODY_MEDIUM)
            .size(14.0)
            .color(t.fg_2),
    )
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

fn main_view(m: &Main) -> Element<'_, Msg> {
    let t = &m.tokens;

    // Overlays/modals cover the body only — the titlebar stays above
    // them (matches the egui app, whose scrim started below the bar).
    let body = column![].push(
        column![
            row![
                sidebar(m),
                vdivider(t.border_subtle, f32::MAX),
                column![
                    toolbar(m),
                    hairline(t.border_subtle),
                    tab_strip(m),
                    hairline(t.border_subtle),
                    table(m),
                ]
                .width(Length::Fill)
                .height(Length::Fill),
            ]
            .height(Length::Fill),
            hairline(t.border_subtle),
            statusbar(m),
        ]
        .height(Length::Fill),
    );

    let base: Element<'_, Msg> = container(body)
        .width(Length::Fill)
        .height(Length::Fill)
        .into();

    let overlaid: Element<'_, Msg> = if let Some(id) = m.context_menu {
        context_menu_overlay(m, base, id)
    } else if m.columns_menu {
        columns_menu_overlay(m, base)
    } else if m.snap.conflict_head.is_some()
        && matches!(m.overlay, Overlay::None | Overlay::Context)
    {
        main_dialogs::conflict(m, base)
    } else {
        match m.overlay {
            Overlay::About => main_dialogs::about(m, base),
            Overlay::Host => main_dialogs::host_settings(m, base),
            Overlay::Remove => main_dialogs::remove_confirm(m, base),
            Overlay::BrowserExtensions => main_dialogs::browser_extensions(m, base),
            Overlay::Welcome => main_dialogs::welcome(m, base),
            Overlay::DbError => {
                let err = m.db_error.clone().unwrap_or_default();
                main_dialogs::db_error(m, base, &err)
            }
            Overlay::SecretsLocked => main_dialogs::secrets_locked(m, base),
            _ => base,
        }
    };

    // Drag-to-add wash sits above any modal; toasts sit above that
    // (non-modal, never block input). Both live below the titlebar so
    // the window chrome stays interactive.
    let with_drag: Element<'_, Msg> = if m.drag_hover {
        drag_overlay(m, overlaid)
    } else {
        overlaid
    };
    let with_toasts: Element<'_, Msg> = if m.toasts.is_empty() {
        with_drag
    } else {
        toast_layer(m, with_drag)
    };

    let content = container(column![
        titlebar::titlebar(t, "oxdm", m.maximized, Msg::Window),
        hairline(t.border_subtle),
        with_toasts,
    ])
    .width(Length::Fill)
    .height(Length::Fill)
    .style({
        let t = *t;
        move |_| container::Style {
            background: Some(t.bg_page.into()),
            text_color: Some(t.fg_1),
            ..Default::default()
        }
    });

    chrome::resize::resizable(t, content.into(), true, Msg::Window)
}

// ------------------------------------------------------- drag-to-add wash

/// Full-window clay wash with a centered glyph tile (design
/// `.drag-overlay`). Non-interactive — purely a hint while hovering.
fn drag_overlay<'a>(_m: &'a Main, base: Element<'a, Msg>) -> Element<'a, Msg> {
    let wash = color::with_alpha(color::clay::C400, DRAG_WASH_ALPHA);
    let tile = container(
        column![
            icons::icon("download", 44.0, iced::Color::WHITE),
            text("Drop a file to add")
                .font(theme::DISPLAY)
                .size(22.0)
                .color(iced::Color::WHITE),
            text("Release to open the Add dialog")
                .font(theme::BODY)
                .size(13.0)
                .color(color::with_alpha(iced::Color::WHITE, 0.85)),
        ]
        .spacing(theme::space::S2)
        .align_x(Alignment::Center),
    )
    .padding(theme::space::S5)
    .style(|_| container::Style {
        background: Some(color::with_alpha(iced::Color::WHITE, 0.12).into()),
        border: iced::Border {
            color: color::with_alpha(iced::Color::WHITE, 0.45),
            width: 1.0,
            radius: theme::surface::RADIUS.into(),
        },
        ..Default::default()
    });
    let layer = container(tile)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(move |_| container::Style {
            background: Some(wash.into()),
            ..Default::default()
        });
    iced::widget::stack![base, layer].into()
}

// ---------------------------------------------------------------- toasts

fn toast_card<'a>(t: &Tokens, toast: &'a Toast) -> Element<'a, Msg> {
    let (accent, icon) = match toast.severity {
        ToastSeverity::Info => (color::clay::C400, "info"),
        ToastSeverity::Success => (t.status_success, "circle-check"),
        ToastSeverity::Error => (t.status_danger, "circle-alert"),
    };
    let t2 = *t;
    let body = row![
        icons::icon(icon, 15.0, accent),
        text(toast.message.clone())
            .font(theme::BODY)
            .size(12.5)
            .color(t.fg_1),
    ]
    .spacing(theme::space::S2)
    .align_y(Alignment::Center);
    // 3px left accent rail + surface card.
    container(
        row![
            container(iced::widget::Space::new())
                .width(Length::Fixed(TOAST_ACCENT_W))
                .height(Length::Fill)
                .style(move |_| container::Style {
                    background: Some(accent.into()),
                    ..Default::default()
                }),
            container(body)
                .padding(theme::space::S2)
                .width(Length::Fill),
        ]
        .align_y(Alignment::Center),
    )
    .width(Length::Fixed(TOAST_W))
    .style(move |_| container::Style {
        background: Some(t2.bg_raised.into()),
        border: iced::Border {
            color: t2.border_default,
            width: 1.0,
            radius: theme::radius::SM.into(),
        },
        shadow: iced::Shadow {
            color: color::with_alpha(iced::Color::BLACK, 70.0 / 255.0),
            offset: iced::Vector::new(0.0, 3.0),
            blur_radius: 14.0,
        },
        ..Default::default()
    })
    .into()
}

/// Bottom-right toast stack (design `.toast`). Non-modal: the layer
/// fills the body but only the cards paint, so input passes through.
fn toast_layer<'a>(m: &'a Main, base: Element<'a, Msg>) -> Element<'a, Msg> {
    let t = &m.tokens;
    let mut col = column![].spacing(TOAST_GAP);
    for toast in &m.toasts {
        col = col.push(toast_card(t, toast));
    }
    let anchored = container(col)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::End)
        .align_y(Alignment::End)
        .padding(TOAST_MARGIN);
    iced::widget::stack![base, anchored].into()
}

// ---------------------------------------------------------------- sidebar

fn sidebar_row<'a>(
    t: &Tokens,
    leader: Element<'a, Msg>,
    label: &str,
    count: Option<u64>,
    active: bool,
    height: f32,
    msg: Msg,
) -> Element<'a, Msg> {
    let t2 = *t;
    let fg = if active { t.action_primary_fg } else { t.fg_2 };
    let mut r = row![
        leader,
        text(label.to_owned())
            .font(theme::BODY)
            .size(12.0)
            .color(fg)
    ]
    .spacing(theme::space::S2)
    .align_y(Alignment::Center);
    if let Some(n) = count {
        let count_fg = if active {
            color::with_alpha(iced::Color::WHITE, 0.85)
        } else {
            t.fg_3
        };
        r = r.push(iced::widget::Space::new().width(Length::Fill)).push(
            text(n.to_string())
                .font(theme::MONO)
                .size(11.0)
                .color(count_fg),
        );
    }
    // Nav rows get hover feedback (design: hover -> bg_sunken).
    // Selected (`active`) rows keep the clay fill. A `button` gives us
    // the per-status hover the plain container couldn't.
    iced::widget::button(
        container(r)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .height(Length::Fixed(height))
    .padding(iced::Padding {
        left: 12.0,
        right: 10.0,
        ..Default::default()
    })
    .on_press(msg)
    .style(move |_, status| {
        use iced::widget::button::Status;
        let background = if active {
            Some(t2.action_primary.into())
        } else if matches!(status, Status::Hovered | Status::Pressed) {
            Some(t2.bg_sunken.into())
        } else {
            None
        };
        iced::widget::button::Style {
            background,
            text_color: fg,
            border: iced::Border {
                radius: theme::control::RADIUS.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    })
    .into()
}

fn section_header<'a>(
    t: &Tokens,
    label: &'a str,
    idx: u8,
    open: bool,
    add: Option<Msg>,
) -> Element<'a, Msg> {
    let chev = if open {
        "chevron-down"
    } else {
        "chevron-right"
    };
    let t2 = *t;
    let mut head = row![
        icons::icon(chev, 14.0, color::with_alpha(t.fg_3, 0.85)),
        text(label.to_uppercase())
            .font(theme::BODY_BOLD)
            .size(10.0)
            .color(t.fg_3),
    ]
    .spacing(6.0)
    .align_y(Alignment::Center);
    // Section "+" add affordance (design: Queues header opens the
    // Queue dialog). Nested button captures its own click so the
    // surrounding toggle mouse_area doesn't also fire.
    if let Some(add_msg) = add {
        head = head
            .push(iced::widget::Space::new().width(Length::Fill))
            .push(
                iced::widget::button(icons::icon("plus", 14.0, t.fg_3))
                    .padding(2)
                    .on_press(add_msg)
                    .style(move |_, status| {
                        use iced::widget::button::Status;
                        iced::widget::button::Style {
                            background: matches!(status, Status::Hovered | Status::Pressed)
                                .then(|| t2.bg_sunken.into()),
                            border: iced::Border {
                                radius: theme::radius::XS.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    }),
            );
    }
    mouse_area(
        container(head)
            .width(Length::Fill)
            .height(Length::Fixed(28.0))
            .align_y(Alignment::Center)
            .padding(iced::Padding {
                left: 10.0,
                right: 8.0,
                ..Default::default()
            }),
    )
    .on_press(Msg::ToggleSection(idx))
    .interaction(iced::mouse::Interaction::Pointer)
    .into()
}

fn queue_color(t: &Tokens, name: &str, builtin: bool) -> iced::Color {
    if builtin {
        return t.action_primary;
    }
    let palette = [
        t.cat_music,
        t.cat_programs,
        t.cat_pictures,
        t.cat_videos,
        t.cat_documents,
        t.cat_compressed,
        t.status_info,
        t.status_success,
    ];
    let mut h: u32 = 0;
    for b in name.bytes() {
        h = h.wrapping_mul(131).wrapping_add(b as u32);
    }
    palette[(h as usize) % palette.len()]
}

fn sidebar(m: &Main) -> Element<'_, Msg> {
    let t = &m.tokens;
    let rh = m.sidebar_row_h();
    let live = m.live_queues();
    let pa = pulse_alpha(m);
    let mut col = column![]
        .spacing(2.0)
        .padding(iced::Padding::new(theme::space::S1));

    // CATEGORIES
    let cats_open = !m.collapsed_sections.contains(&0);
    col = col.push(section_header(t, "Categories", 0, cats_open, None));
    if cats_open {
        let all_active = m.filter == SidebarFilter::All;
        col = col.push(sidebar_row(
            t,
            icons::icon("layers", 17.0, leader_fg(t, all_active)),
            "All downloads",
            Some(m.cat_count(None)),
            all_active,
            rh,
            Msg::SetFilter(SidebarFilter::All),
        ));
        for (cat, icon, label) in [
            (Category::Compressed, "archive", "Compressed"),
            (Category::Programs, "package", "Programs"),
            (Category::Videos, "film", "Videos"),
            (Category::Music, "music", "Music"),
            (Category::Pictures, "image", "Pictures"),
            (Category::Documents, "file-text", "Documents"),
        ] {
            let active = m.filter == SidebarFilter::Category(cat);
            col = col.push(sidebar_row(
                t,
                icons::icon(icon, 17.0, leader_fg(t, active)),
                label,
                Some(m.cat_count(Some(cat))),
                active,
                rh,
                Msg::SetFilter(SidebarFilter::Category(cat)),
            ));
        }
    }

    // QUEUES
    let queues_open = !m.collapsed_sections.contains(&1);
    col = col.push(section_header(
        t,
        "Queues",
        1,
        queues_open,
        Some(Msg::Tool(ToolAction::Scheduler)),
    ));
    if queues_open {
        for q in &m.snap.queues {
            let active = m.filter == SidebarFilter::Queue(q.id);
            let count = m.snap.jobs.iter().filter(|j| j.queue_id == q.id).count() as u64;
            let chip = swatch(8.0, 2.0, queue_color(t, &q.name, q.builtin));
            // Live-dot (design `.q-live-dot`): pulsing moss dot when the
            // queue has ≥1 running job (N2). Pulse gated on !reduce_motion.
            let leader: Element<'_, Msg> = if live.contains(&q.id) {
                row![
                    chip,
                    crate::gui::widget::dot(
                        LIVE_DOT_SIZE,
                        color::with_alpha(color::moss::M400, pa),
                    ),
                ]
                .spacing(5.0)
                .align_y(Alignment::Center)
                .into()
            } else {
                chip
            };
            col = col.push(sidebar_row(
                t,
                leader,
                &q.name,
                Some(count),
                active,
                rh,
                Msg::SetFilter(SidebarFilter::Queue(q.id)),
            ));
        }
    }

    // TOOLS
    let tools_open = !m.collapsed_sections.contains(&2);
    col = col.push(section_header(t, "Tools", 2, tools_open, None));
    if tools_open {
        for (action, icon, label) in [
            (ToolAction::Scheduler, "calendar", "Scheduler"),
            (ToolAction::Settings, "settings", "Settings"),
            (ToolAction::BrowserExtension, "puzzle", "Browser extension"),
            (ToolAction::PerHost, "globe", "Per host settings"),
            (ToolAction::About, "info", "About"),
        ] {
            col = col.push(sidebar_row(
                t,
                icons::icon(icon, 17.0, t.fg_2),
                label,
                None,
                false,
                rh,
                Msg::Tool(action),
            ));
        }
    }

    let t2 = *t;
    container(scrollable(col).height(Length::Fill))
        .width(Length::Fixed(theme::size::SIDEBAR_W))
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(t2.bg_sidebar.into()),
            ..Default::default()
        })
        .into()
}

fn leader_fg(t: &Tokens, active: bool) -> iced::Color {
    if active { t.action_primary_fg } else { t.fg_2 }
}

// ---------------------------------------------------------------- toolbar

fn toolbar(m: &Main) -> Element<'_, Msg> {
    let t = &m.tokens;
    // Start/Stop toggle (design §3.1): label/icon follow the action the
    // press would take — queue scope keys off `active_queues`
    // membership, other scopes off "anything running".
    let (toggle_label, toggle_icon) = match m.filter {
        SidebarFilter::Queue(q) if m.snap.active_queues.contains(&q) => ("Pause queue", "pause"),
        SidebarFilter::Queue(_) => ("Start queue", "play"),
        _ if m.any_running() => ("Pause all", "pause"),
        _ => ("Resume all", "play"),
    };
    let bar = row![
        Btn::new("Add URL")
            .primary()
            .size(BtnSize::Lg) // design: hero CTA is the lg button
            .icon("plus")
            .on_press(Msg::Toolbar(ToolbarAction::AddUrl))
            .view(t),
        container(vdivider(t.border_subtle, 24.0)).padding([0.0, theme::space::S1]),
        Btn::new(toggle_label)
            .toolbar()
            .icon(toggle_icon)
            .enabled(m.toggle_actionable())
            .on_press(Msg::Toolbar(ToolbarAction::ToggleRun))
            .view(t),
        Btn::new("Stop all")
            .toolbar()
            // design `octagon-x`; not in the icon set, `circle-x` is the
            // closest stop-with-X glyph available.
            .icon("circle-x")
            .on_press(Msg::Toolbar(ToolbarAction::StopAll))
            .view(t),
        Btn::new("Clean")
            .toolbar()
            .danger_hover() // design `.tb-btn.danger`: borderless, rust on hover only
            .icon("trash-2")
            .on_press(Msg::Toolbar(ToolbarAction::Clean))
            .view(t),
        container(vdivider(t.border_subtle, 24.0)).padding([0.0, theme::space::S1]),
        Btn::new("Schedule")
            .toolbar()
            .icon("calendar")
            .on_press(Msg::Toolbar(ToolbarAction::Schedule))
            .view(t),
        iced::widget::Space::new().width(Length::Fill),
        search_field(
            t,
            &m.search,
            "Search downloads\u{2026}",
            200.0,
            Msg::SetSearch
        ),
    ]
    .spacing(theme::space::S2)
    .align_y(Alignment::Center);

    container(bar)
        .width(Length::Fill)
        .padding([theme::space::S2 - 2.0, theme::space::S4])
        .into()
}

// ---------------------------------------------------------------- tabs

fn tab_strip(m: &Main) -> Element<'_, Msg> {
    let t = &m.tokens;
    let base = m.sidebar_filtered();
    let n_all = base.len() as u64;
    let n_active = base.iter().filter(|j| !m.phase(j.id).is_terminal()).count() as u64;
    // Tab-meta active count keys off `is_running()` (N2) — Queued/Paused
    // don't count as "active" in the live readout, unlike the Active tab
    // badge above (which is legitimate tab semantics: not-yet-finished).
    let n_running = base.iter().filter(|j| m.phase(j.id).is_running()).count() as u64;
    let n_done = base
        .iter()
        .filter(|j| m.phase(j.id) == Phase::Completed)
        .count() as u64;

    let mut bar = row![
        TabBtn::new("All")
            .count(n_all)
            .active(m.tab == Tab::All)
            .on_press(Msg::SetTab(Tab::All))
            .view(t),
        TabBtn::new("Active")
            .count(n_active)
            .active(m.tab == Tab::Active)
            .on_press(Msg::SetTab(Tab::Active))
            .view(t),
        TabBtn::new("Finished")
            .count(n_done)
            .active(m.tab == Tab::Finished)
            .on_press(Msg::SetTab(Tab::Finished))
            .view(t),
    ]
    .align_y(Alignment::Center);

    // Right-side live readout (design `.tab-meta`): "● N active · speed"
    // in clay-500 mono, shown only while downloads are running (N2).
    if n_running > 0 {
        let active_speed: f64 = base
            .iter()
            .filter(|j| m.phase(j.id) == Phase::Downloading)
            .filter_map(|j| m.counters.get(&j.id))
            .map(|c| c.speed_bps)
            .sum();
        let label = if active_speed > 1.0 {
            format!("{n_running} active \u{00b7} {}", format_speed(active_speed))
        } else {
            format!("{n_running} active")
        };
        bar = bar
            .push(iced::widget::Space::new().width(Length::Fill))
            .push(
                row![
                    crate::gui::widget::dot(6.0, color::clay::C500),
                    text(label)
                        .font(theme::MONO)
                        .size(11.0)
                        .color(color::clay::C500),
                ]
                .spacing(6.0)
                .align_y(Alignment::Center),
            );
    }

    container(bar)
        .padding(iced::Padding {
            left: theme::space::S4,
            right: theme::space::S4,
            ..Default::default()
        })
        .width(Length::Fill)
        .into()
}

// ---------------------------------------------------------------- table

fn header_cell<'a>(m: &Main, label: &'a str, col: SortColumn, width: f32) -> Element<'a, Msg> {
    let (active_col, desc) = m.sort;
    let t2 = m.tokens;
    // Resize grip (design `ResizableHeader`): 1px quiet idle, 3px
    // clay-400 at ~70% height on hover, 3px clay-500 while dragging.
    let dragging = matches!(m.col_drag, Some((c, _, _)) if c == col);
    let hovering = m.col_handle_hover == Some(col);
    let (line_w, line_h, line_color) = if dragging {
        (GRIP_W_ACTIVE, HEADER_H, color::clay::C500)
    } else if hovering {
        (
            GRIP_W_ACTIVE,
            HEADER_H * GRIP_ACTIVE_RATIO,
            color::clay::C400,
        )
    } else {
        (GRIP_W_IDLE, HEADER_H, t2.border_subtle)
    };
    let handle = mouse_area(
        container(
            container(iced::widget::Space::new())
                .width(Length::Fixed(line_w))
                .height(Length::Fixed(line_h))
                .style(move |_| container::Style {
                    background: Some(line_color.into()),
                    ..Default::default()
                }),
        )
        .width(Length::Fixed(RESIZE_HANDLE_W))
        .height(Length::Fixed(HEADER_H))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center),
    )
    .on_press(Msg::ColResizeStart(col))
    .on_enter(Msg::ColHandleHover(col, true))
    .on_exit(Msg::ColHandleHover(col, false))
    .interaction(iced::mouse::Interaction::ResizingHorizontally);
    mouse_area(
        row![
            container(col_header_sortable(
                &m.tokens,
                label,
                active_col == col,
                desc,
                Msg::SetSort(col),
            ))
            .width(Length::Fixed(width - RESIZE_HANDLE_W))
            .padding([0.0, theme::space::S2])
            .align_y(Alignment::Center)
            .height(Length::Fixed(HEADER_H)),
            handle,
        ]
        .align_y(Alignment::Center),
    )
    .on_right_press(Msg::HeaderRightClick)
    .into()
}

const TABLE_COLS: [(SortColumn, &str); 6] = [
    (SortColumn::Name, "Name"),
    (SortColumn::Size, "Size"),
    (SortColumn::Status, "Status"),
    (SortColumn::Speed, "Speed"),
    (SortColumn::Eta, "Time left"),
    (SortColumn::Date, "Date added"),
];

fn table(m: &Main) -> Element<'_, Msg> {
    let t = &m.tokens;
    let mut header_row = row![];
    for (col, label) in TABLE_COLS {
        if !m.columns.is_visible(col as usize) {
            continue;
        }
        header_row = header_row.push(header_cell(m, label, col, m.columns.width(col as usize)));
    }
    // Header scrolls horizontally in lockstep with the body (synced
    // via TableScrolled -> scroll_to); its own scrollbar is hidden.
    let header = container(
        mouse_area(
            scrollable(header_row)
                .id(iced::widget::Id::new("tbl-header"))
                .direction(scrollable::Direction::Horizontal(
                    scrollable::Scrollbar::new()
                        .width(0.0)
                        .scroller_width(0.0)
                        .margin(0.0),
                ))
                .width(Length::Fill)
                .height(Length::Fixed(HEADER_H)),
        )
        .on_right_press(Msg::HeaderRightClick),
    )
    .width(Length::Fill);

    let jobs = m.visible_jobs();
    let body: Element<'_, Msg> = if jobs.is_empty() {
        empty_state(m)
    } else {
        let mut rows = column![];
        for job in &jobs {
            rows = rows.push(job_row(m, job));
        }
        // Design-spec scrollbars (§4): 10px rail, thin rounded thumb.
        let bar = || {
            scrollable::Scrollbar::new()
                .width(theme::size::SCROLLBAR_W)
                .scroller_width(theme::scroll::THUMB_W)
                .margin(0.0)
        };
        scrollable(rows)
            .direction(scrollable::Direction::Both {
                vertical: bar(),
                horizontal: bar(),
            })
            .style(theme::scrollbar_style)
            .on_scroll(|vp| Msg::TableScrolled(vp.absolute_offset().x))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    };

    // Column-resize guideline (design §4 ResizableHeader): while a
    // header grip is dragged, a full-height 1px clay-300 rule over the
    // body marks the dragged boundary.
    let body: Element<'_, Msg> = match m.col_drag {
        Some((col, _, _)) => match drag_guideline_x(m, col) {
            Some(x) => {
                let rule = container(iced::widget::Space::new())
                    .width(Length::Fixed(GUIDELINE_W))
                    .height(Length::Fill)
                    .style(|_| container::Style {
                        background: Some(color::clay::C300.into()),
                        ..Default::default()
                    });
                iced::widget::stack![
                    body,
                    container(rule)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .padding(iced::Padding {
                            left: x,
                            ..Default::default()
                        }),
                ]
                .into()
            }
            None => body,
        },
        None => body,
    };

    column![header, hairline(t.border_subtle), body,]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// X of the dragged column's right boundary in table-body coordinates:
/// the visible-column widths up to and including the dragged column,
/// corrected by the horizontal scroll offset. `None` when the boundary
/// is scrolled out of view to the left (or the column is hidden).
fn drag_guideline_x(m: &Main, dragged: SortColumn) -> Option<f32> {
    let mut x = 0.0;
    for (col, _) in TABLE_COLS {
        if !m.columns.is_visible(col as usize) {
            continue;
        }
        x += m.columns.width(col as usize);
        if col == dragged {
            let x = x - m.table_scroll_x;
            return (x >= 0.0).then_some(x);
        }
    }
    None
}

fn empty_state(m: &Main) -> Element<'_, Msg> {
    let t = &m.tokens;
    let (title, hint) = match m.tab {
        _ if !m.search.trim().is_empty() => ("No matches", "Try a different search."),
        Tab::Active => (
            "Nothing active",
            "Queued and running downloads appear here.",
        ),
        Tab::Finished => ("Nothing finished yet", "Completed downloads appear here."),
        Tab::All => ("No downloads yet", "Add a URL above to start."),
    };
    container(
        column![
            icons::icon("download", 39.0, t.fg_3),
            text(title).font(theme::DISPLAY).size(20.0).color(t.fg_1),
            text(hint).font(theme::BODY).size(13.0).color(t.fg_3),
        ]
        .spacing(theme::space::S2)
        .align_x(Alignment::Center),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .align_x(Alignment::Center)
    .padding(iced::Padding {
        top: 90.0,
        ..Default::default()
    })
    .into()
}

fn job_row<'a>(m: &'a Main, job: &'a crate::domain::Job) -> Element<'a, Msg> {
    let t = &m.tokens;
    let id = job.id;
    let selected = m.selection.contains(&id);
    let c = m.counters.get(&id);
    let phase = m.phase(id);

    let name = job.filename.clone().unwrap_or_else(|| job.url.to_string());
    let host = job.url.host_str().unwrap_or("").to_owned();

    // Category-tinted ext pill (design `.fname` tag), before the title.
    let ext = std::path::PathBuf::from(&name)
        .extension()
        .map(|e| e.to_string_lossy().to_uppercase())
        .unwrap_or_else(|| "FILE".into());
    let cat = cat_color(t, job.category);
    let pill_bg = color::mix(t.bg_surface, cat, 0.20);
    let ext_pill = container(
        text(ext)
            .font(theme::MONO_BOLD)
            .size(EXT_PILL_FONT)
            .color(cat)
            .wrapping(iced::widget::text::Wrapping::None),
    )
    .width(Length::Fixed(EXT_PILL_W))
    .height(Length::Fixed(EXT_PILL_H))
    .clip(true)
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .style(move |_| container::Style {
        background: Some(pill_bg.into()),
        border: iced::Border {
            radius: EXT_PILL_RADIUS.into(),
            ..Default::default()
        },
        ..Default::default()
    });

    let name_cell = container(
        row![
            ext_pill,
            column![
                text(name)
                    .font(theme::BODY_BOLD)
                    .size(13.0)
                    .color(t.fg_1)
                    .wrapping(iced::widget::text::Wrapping::None),
                text(host)
                    .font(theme::MONO)
                    .size(10.0)
                    .color(t.fg_3)
                    .wrapping(iced::widget::text::Wrapping::None),
            ]
            .spacing(2.0),
        ]
        .spacing(NAME_PILL_GAP)
        .align_y(Alignment::Center),
    )
    .width(Length::Fixed(m.columns.width(SortColumn::Name as usize)))
    .clip(true)
    .padding([0.0, theme::space::S2])
    .align_y(Alignment::Center)
    .height(Length::Fill);

    let total = c.and_then(|c| c.total);
    let size_cell = cell(
        text(total.map(format_bytes).unwrap_or_else(|| "—".into()))
            .font(theme::MONO)
            .size(12.0)
            .color(t.fg_2)
            .wrapping(iced::widget::text::Wrapping::None)
            .into(),
        Length::Fixed(m.columns.width(SortColumn::Size as usize)),
        Alignment::End, // design: numeric columns right-align
    );

    let status_cell: Element<'_, Msg> = if phase.is_terminal()
        || matches!(phase, Phase::Paused | Phase::Cancelled | Phase::Queued)
    {
        let (color, label) = phase_style(t, phase);
        cell(
            status_dot(color, label, 12.0),
            Length::Fixed(m.columns.width(SortColumn::Status as usize)),
            Alignment::Start,
        )
    } else {
        let frac = match (c.map(|c| c.downloaded), total) {
            (Some(d), Some(tot)) if tot > 0 => d as f64 / tot as f64,
            _ => 0.0,
        };
        let (_, label) = phase_style(t, phase);
        cell(
            inline_progress(t, frac as f32, label, selected, Length::Fill, 22.0),
            Length::Fixed(m.columns.width(SortColumn::Status as usize)),
            Alignment::Start,
        )
    };

    let speed = c.map(|c| c.speed_bps).unwrap_or(0.0);
    let speed_cell = cell(
        text(if phase == Phase::Downloading {
            format_speed(speed)
        } else {
            "—".into()
        })
        .font(theme::MONO)
        .size(12.0)
        .color(t.fg_2)
        .wrapping(iced::widget::text::Wrapping::None)
        .into(),
        Length::Fixed(m.columns.width(SortColumn::Speed as usize)),
        Alignment::End,
    );

    let eta_cell = cell(
        text(eta_of(c).map(format_eta).unwrap_or_else(|| "—".into()))
            .font(theme::MONO)
            .size(12.0)
            .color(t.fg_2)
            .wrapping(iced::widget::text::Wrapping::None)
            .into(),
        Length::Fixed(m.columns.width(SortColumn::Eta as usize)),
        Alignment::End,
    );

    let date_cell = cell(
        text(format_short_date(&job.created_at))
            .font(theme::MONO)
            .size(11.0)
            .color(t.fg_3)
            .wrapping(iced::widget::text::Wrapping::None)
            .into(),
        Length::Fixed(m.columns.width(SortColumn::Date as usize)),
        Alignment::End,
    );

    // Design `.dl-table tbody tr`: selected → clay-50, selected+hover →
    // clay-100, hover → bg-sunken (per-theme values live on `Tokens`).
    let t2 = *t;
    let bg = move |hovered: bool| {
        if selected && hovered {
            Some(t2.row_selhover_bg)
        } else if selected {
            Some(t2.row_selected_bg)
        } else if hovered {
            Some(t2.row_hover_bg)
        } else {
            None
        }
    };

    let mut cells = row![].align_y(Alignment::Center).height(Length::Fill);
    let vis = |c: SortColumn| m.columns.is_visible(c as usize);
    cells = cells.push(name_cell);
    if vis(SortColumn::Size) {
        cells = cells.push(size_cell);
    }
    if vis(SortColumn::Status) {
        cells = cells.push(status_cell);
    }
    if vis(SortColumn::Speed) {
        cells = cells.push(speed_cell);
    }
    if vis(SortColumn::Eta) {
        cells = cells.push(eta_cell);
    }
    if vis(SortColumn::Date) {
        cells = cells.push(date_cell);
    }

    // NOTE: width must be Shrink — Fill resolves to zero inside the
    // horizontally-unbounded table scrollable and collapses the row.
    let row_el = container(cells)
        .height(Length::Fixed(m.row_h()))
        .width(Length::Shrink)
        .style(move |_| container::Style {
            background: bg(false).map(Into::into),
            ..Default::default()
        });

    let (ctrl, shift) = (m.modifiers.command(), m.modifiers.shift());
    let row_area = mouse_area(row_el)
        .on_press(Msg::RowClick(id, ctrl, shift))
        .on_double_click(Msg::RowDoubleClick(id))
        .on_right_press(Msg::RowRightClick(id));

    // 1px bottom row separator (design `.tr` border-subtle hairline).
    // Fixed width = sum of visible columns so it tracks the Shrink row
    // (a Fill hairline would collapse in the unbounded scrollable).
    let total_w: f32 = TABLE_COLS
        .iter()
        .filter(|(c, _)| m.columns.is_visible(*c as usize))
        .map(|(c, _)| m.columns.width(*c as usize))
        .sum();
    let separator = container(iced::widget::Space::new())
        .width(Length::Fixed(total_w))
        .height(Length::Fixed(1.0))
        .style(move |_| container::Style {
            background: Some(t.border_subtle.into()),
            ..Default::default()
        });

    column![row_area, separator].width(Length::Shrink).into()
}

fn cell(content: Element<'_, Msg>, width: Length, align: Alignment) -> Element<'_, Msg> {
    container(content)
        .width(width)
        .padding([0.0, theme::space::S2])
        .align_x(align)
        .align_y(Alignment::Center)
        .height(Length::Fill)
        .into()
}

/// Category accent color for the Name-cell ext pill (mirrors the
/// sidebar/category tints in `download.rs`).
fn cat_color(t: &Tokens, cat: Category) -> iced::Color {
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

fn phase_style(t: &Tokens, phase: Phase) -> (iced::Color, String) {
    match phase {
        Phase::Evaluating
        | Phase::ResolvingConflicts
        | Phase::Downloading
        | Phase::Assembling
        | Phase::Flushing
        | Phase::Verifying => (t.action_primary, "Downloading".to_owned()),
        Phase::Reconnecting => (t.action_primary, "Reconnecting".to_owned()),
        Phase::Queued => (t.status_info, "Queued".to_owned()),
        Phase::Paused => (t.fg_3, "Paused".to_owned()),
        Phase::Cancelled => (t.fg_3, "Cancelled".to_owned()),
        Phase::Completed => (t.status_success, "Complete".to_owned()),
        Phase::Failed => (t.status_danger, "Failed".to_owned()),
    }
}

fn format_short_date(dt: &chrono::DateTime<chrono::Utc>) -> String {
    use chrono::Datelike;
    let local = dt.with_timezone(&chrono::Local);
    let now = chrono::Local::now();
    let today = now.date_naive();
    let date = local.date_naive();
    let hm = local.format("%H:%M").to_string();
    if date == today {
        format!("Today, {hm}")
    } else if date == today.pred_opt().unwrap_or(today) {
        format!("Yesterday, {hm}")
    } else if date.year() == today.year() {
        local.format("%b %d").to_string()
    } else {
        local.format("%b %d, %Y").to_string()
    }
}

// ---------------------------------------------------------------- statusbar

fn statusbar(m: &Main) -> Element<'_, Msg> {
    let t = &m.tokens;
    let n_downloading = m
        .snap
        .jobs
        .iter()
        .filter(|j| m.phase(j.id) == Phase::Downloading)
        .count();
    let total_speed: f64 = m
        .counters
        .values()
        .filter(|c| c.phase == Phase::Downloading)
        .map(|c| c.speed_bps)
        .sum();

    let (dot_color, label) = if n_downloading > 0 {
        (t.action_primary, format!("{n_downloading} downloading"))
    } else {
        // design: idle dot is moss (active stays action_primary).
        (t.status_success, "Idle".to_owned())
    };

    let queue_name = match m.filter {
        SidebarFilter::Queue(q) => m
            .snap
            .queues
            .iter()
            .find(|x| x.id == q)
            .map(|x| x.name.clone())
            .unwrap_or_default(),
        _ => "—".to_owned(),
    };
    let max_x = m
        .snap
        .queues
        .iter()
        .find(|q| matches!(m.filter, SidebarFilter::Queue(id) if id == q.id))
        .and_then(|q| q.max_concurrent)
        .unwrap_or(m.snap.settings.max_concurrent_downloads);

    let sep = || {
        container(crate::gui::widget::dot(3.0, m.tokens.fg_4))
            .padding([0.0, theme::space::S1])
            .align_y(Alignment::Center)
    };

    let mut left = row![
        container(status_dot(dot_color, label, 11.0)).align_y(Alignment::Center),
        sep(),
        icons::icon(
            "activity",
            14.0,
            if total_speed > 1.0 { t.fg_2 } else { t.fg_4 }
        ),
        // design: aggregate speed value is clay-500 mono.
        text(if total_speed > 1.0 {
            format_speed(total_speed)
        } else {
            "—".into()
        })
        .font(theme::MONO_BOLD)
        .size(11.0)
        .color(if total_speed > 1.0 {
            color::clay::C500
        } else {
            t.fg_4
        }),
        sep(),
        // design: "Queue:" / "max N×" set in mono.
        text(format!("Queue: {queue_name}"))
            .font(theme::MONO)
            .size(11.0)
            .color(t.fg_3),
        sep(),
        text(format!("max {max_x}\u{00d7}"))
            .font(theme::MONO)
            .size(11.0)
            .color(t.fg_3),
    ]
    .spacing(theme::space::S2)
    .align_y(Alignment::Center);

    if !m.selection.is_empty() {
        left = left.push(sep()).push(
            text(format!(
                "{}/{} selected",
                m.selection.len(),
                m.visible_jobs().len()
            ))
            .font(theme::BODY)
            .size(11.0)
            .color(t.fg_3),
        );
    }

    let proxy_set = m
        .snap
        .settings
        .proxy
        .as_deref()
        .map(str::trim)
        .is_some_and(|s| !s.is_empty());
    let (proxy_icon, proxy_label) = if proxy_set {
        ("shield", "Proxied")
    } else {
        ("globe", "Direct")
    };
    let free = free_disk_str(&m.snap.settings.download_dir);

    let right = row![
        Btn::new(free)
            .toolbar()
            .icon("hard-drive")
            .size(BtnSize::Sm)
            .on_press(Msg::Noop)
            .view(t),
        sep(),
        Btn::new(proxy_label)
            .toolbar()
            .icon(proxy_icon)
            .size(BtnSize::Sm)
            .on_press(Msg::Tool(ToolAction::Settings))
            .view(t),
    ]
    .spacing(theme::space::S2)
    .align_y(Alignment::Center);

    let t2 = *t;
    container(
        row![left, iced::widget::Space::new().width(Length::Fill), right]
            .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .height(Length::Fixed(28.0))
    .padding([0.0, theme::space::S3])
    .align_y(Alignment::Center)
    .style(move |_| container::Style {
        background: Some(t2.bg_sidebar.into()),
        ..Default::default()
    })
    .into()
}

fn free_disk_str(path: &std::path::Path) -> String {
    let mut probe = path;
    let probe = loop {
        if probe.exists() {
            break probe;
        }
        match probe.parent() {
            Some(p) => probe = p,
            None => break path,
        }
    };
    match fs4::available_space(probe) {
        Ok(free) => format!("{} free", format_bytes(free)),
        Err(_) => "— free".to_owned(),
    }
}

// ---------------------------------------------------------------- context menu

fn context_menu_overlay<'a>(m: &'a Main, base: Element<'a, Msg>, id: JobId) -> Element<'a, Msg> {
    let t = &m.tokens;
    let phase = m.phase(id);
    let t2 = *t;

    let item = |icon: &'a str, label: &'a str, kbd: Option<&'a str>, enabled: bool, msg: Msg| {
        let fg = if enabled { t2.fg_1 } else { t2.fg_4 };
        let mut r = row![
            icons::icon(icon, 15.0, fg),
            text(label).font(theme::BODY).size(13.0).color(fg),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center);
        if let Some(kbd) = kbd {
            r = r.push(iced::widget::Space::new().width(Length::Fill)).push(
                text(kbd).font(theme::MONO).size(11.0).color(if enabled {
                    t2.fg_3
                } else {
                    t2.fg_4
                }),
            );
        }
        let inner = container(r)
            .width(Length::Fill)
            .height(Length::Fixed(28.0))
            .align_y(Alignment::Center)
            .padding([0.0, theme::space::S2]);
        if enabled {
            iced::widget::button(inner)
                .padding(0)
                .width(Length::Fill)
                .style(move |_th, status| iced::widget::button::Style {
                    background: matches!(
                        status,
                        iced::widget::button::Status::Hovered
                            | iced::widget::button::Status::Pressed
                    )
                    .then(|| t2.bg_sunken.into()),
                    text_color: fg,
                    border: iced::Border {
                        radius: theme::radius::XS.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .on_press(msg)
                .into()
        } else {
            Element::from(inner)
        }
    };
    let separator = || {
        container(hairline(t2.border_subtle))
            .padding([theme::space::S0, 0.0])
            .width(Length::Fill)
    };

    let can_resume = matches!(phase, Phase::Paused | Phase::Cancelled | Phase::Failed);
    let can_pause = phase.is_running();
    let done = phase == Phase::Completed;

    // Destructive row morphs with live modifiers (design: Finder-like):
    // default neutral "Remove from list" → ⇧ ochre "Move to Trash" →
    // ⇧⌥ rust "Delete permanently". Every kind routes through the Remove
    // confirm (B4) — the modifier only PRE-SELECTS the option.
    //
    // Trash/Permanent act on the FINISHED file, so they're only offered
    // when EVERY selected job is Completed. For any non-completed job
    // there is no final file and removal would purge its `.part`
    // (irrecoverable) — which would betray the "recoverable" promise —
    // so non-completed selections get entry-only removal regardless of
    // modifiers.
    let all_done = !m.selection.is_empty()
        && m.selection
            .iter()
            .all(|sid| m.phase(*sid) == Phase::Completed);
    let destruct: Element<'a, Msg> = {
        let morph = if all_done {
            (m.modifiers.shift(), m.modifiers.alt())
        } else {
            (false, false)
        };
        let (label, kbd, fg, kind) = match morph {
            (true, true) => (
                "Delete permanently",
                "\u{21e7}\u{2325}",
                color::rust::R300,
                RemoveKind::Permanent,
            ),
            (true, false) => (
                "Move to Trash",
                "\u{21e7}",
                color::ochre::O400,
                RemoveKind::Trash,
            ),
            _ => ("Remove from list", "Del", t2.fg_1, RemoveKind::Entry),
        };
        let r = row![
            icons::icon("trash-2", 15.0, fg),
            text(label).font(theme::BODY).size(13.0).color(fg),
            iced::widget::Space::new().width(Length::Fill),
            text(kbd).font(theme::MONO).size(11.0).color(t2.fg_3),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center);
        let inner = container(r)
            .width(Length::Fill)
            .height(Length::Fixed(28.0))
            .align_y(Alignment::Center)
            .padding([0.0, theme::space::S2]);
        iced::widget::button(inner)
            .padding(0)
            .width(Length::Fill)
            .style(move |_th, status| iced::widget::button::Style {
                background: matches!(
                    status,
                    iced::widget::button::Status::Hovered | iced::widget::button::Status::Pressed
                )
                .then(|| t2.bg_sunken.into()),
                text_color: fg,
                border: iced::Border {
                    radius: theme::radius::XS.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .on_press(Msg::RemoveAs(kind))
            .into()
    };

    let menu = container(
        column![
            item(
                "file",
                "Open",
                Some("Ctrl+O"),
                done,
                Msg::Context(ContextAction::Open)
            ),
            item(
                "folder",
                "Open Containing Folder",
                Some("Ctrl+F"),
                true,
                Msg::Context(ContextAction::OpenFolder)
            ),
            item(
                "play",
                "Resume",
                Some("Ctrl+R"),
                can_resume,
                Msg::Context(ContextAction::Resume)
            ),
            item(
                "pause",
                "Pause",
                Some("Ctrl+P"),
                can_pause,
                Msg::Context(ContextAction::Pause)
            ),
            separator(),
            destruct,
            item(
                "rotate-cw",
                "Restart Download",
                None,
                true,
                Msg::Context(ContextAction::Restart)
            ),
            item(
                "copy",
                "Copy URL",
                None,
                true,
                Msg::Context(ContextAction::CopyUrl)
            ),
            separator(),
            item(
                "info",
                "Show Properties",
                None,
                true,
                Msg::Context(ContextAction::Properties)
            ),
        ]
        .width(Length::Fixed(260.0)),
    )
    .padding(theme::space::S1)
    .style(move |_| container::Style {
        background: Some(t2.bg_raised.into()),
        border: iced::Border {
            color: t2.border_default,
            width: 1.0,
            radius: theme::radius::SM.into(),
        },
        shadow: iced::Shadow {
            color: color::with_alpha(iced::Color::BLACK, 80.0 / 255.0),
            offset: iced::Vector::new(0.0, 4.0),
            blur_radius: 16.0,
        },
        ..Default::default()
    });

    let scrim = mouse_area(
        container(iced::widget::Space::new())
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .on_press(Msg::CloseOverlay)
    .on_right_press(Msg::CloseOverlay);

    // Anchor at the cursor (egui opens context menus at the click
    // point); clamp so the menu stays inside the window.
    let (cx, cy) = m.menu_anchor;
    let cy = cy - titlebar::HEIGHT - 1.0; // overlay stack starts below the bar
    let (mw, mh) = (268.0, 290.0);
    let (ww, wh) = if m.win_size.0 > 0.0 {
        (m.win_size.0, m.win_size.1 - titlebar::HEIGHT - 1.0)
    } else {
        (1240.0, 760.0)
    };
    let left = cx.min(ww - mw).max(0.0);
    let top = cy.min(wh - mh).max(0.0);
    iced::widget::stack![
        base,
        scrim,
        container(iced::widget::opaque(menu)).padding(iced::Padding {
            left,
            top,
            ..Default::default()
        }),
    ]
    .into()
}

fn columns_menu_overlay<'a>(m: &'a Main, base: Element<'a, Msg>) -> Element<'a, Msg> {
    let t = &m.tokens;
    let t2 = *t;

    let mut items = column![
        container(text("Columns").font(theme::BODY).size(11.0).color(t.fg_3))
            .padding([2.0, theme::space::S2]),
        hairline(t.border_subtle),
    ]
    .width(Length::Fixed(180.0));
    for (col, label) in TABLE_COLS {
        let enabled = col != SortColumn::Name;
        items = items.push(
            container(crate::gui::widget::checkbox(
                t,
                label,
                m.columns.is_visible(col as usize),
                enabled,
                move |_| Msg::ColToggle(col),
            ))
            .padding([4.0, theme::space::S2]),
        );
    }

    let menu = container(items)
        .padding(theme::space::S1)
        .style(move |_| container::Style {
            background: Some(t2.bg_raised.into()),
            border: iced::Border {
                color: t2.border_default,
                width: 1.0,
                radius: theme::radius::SM.into(),
            },
            shadow: iced::Shadow {
                color: color::with_alpha(iced::Color::BLACK, 80.0 / 255.0),
                offset: iced::Vector::new(0.0, 4.0),
                blur_radius: 16.0,
            },
            ..Default::default()
        });

    let scrim = mouse_area(
        container(iced::widget::Space::new())
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .on_press(Msg::CloseOverlay)
    .on_right_press(Msg::CloseOverlay);

    let (cx, cy) = m.menu_anchor;
    let cy = cy - titlebar::HEIGHT - 1.0;
    let (mw, mh) = (188.0, 200.0);
    let (ww, wh) = if m.win_size.0 > 0.0 {
        (m.win_size.0, m.win_size.1 - titlebar::HEIGHT - 1.0)
    } else {
        (1240.0, 760.0)
    };
    let left = cx.min(ww - mw).max(0.0);
    let top = cy.min(wh - mh).max(0.0);
    iced::widget::stack![
        base,
        scrim,
        container(iced::widget::opaque(menu)).padding(iced::Padding {
            left,
            top,
            ..Default::default()
        }),
    ]
    .into()
}

// ---------------------------------------------------------------- launch

pub fn launch_main() {
    let saved = crate::gui::ui_prefs::load().window;
    let size = saved
        .map(|w| iced::Size::new(w.width.max(820.0), w.height.max(520.0)))
        .unwrap_or(iced::Size::new(1240.0, 760.0));
    let mut app = iced::application(boot, update, view)
        .title(|_: &App| "oxdm".to_owned())
        .theme(theme_of)
        .subscription(subscription)
        .default_font(theme::BODY)
        .antialiasing(true)
        .window(chrome::window_settings(size, iced::Size::new(820.0, 520.0)));
    for f in theme::fonts::ALL {
        app = app.font(*f);
    }
    if let Err(e) = app.run() {
        eprintln!("gui error: {e}");
        std::process::exit(1);
    }
}
