//! Main window: sidebar (categories / queues / tools), toolbar, tab
//! strip, jobs table, statusbar — plus in-window overlays (context
//! menu, remove/about/host/conflict dialogs, db/secrets recovery).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use iced::widget::{column, container, mouse_area, row, scrollable, text};
use iced::{Alignment, Element, Length, Subscription, Task};

use super::main_dialogs::{self, RemoveState};
use crate::domain::{Category, JobId, Phase, QueueId};
use crate::gui::chrome::{self, WindowControl, titlebar};
use crate::gui::format::{format_bytes, format_eta, format_speed};
use crate::gui::ipc::DaemonSignal;
use crate::gui::shot::Shot;
use crate::gui::theme::{self, Tokens};
use crate::gui::widget::{
    Btn, BtnSize, ProgressTone, TabBtn, col_header_sortable, hairline, inline_progress,
    search_field, status_dot, status_mark, swatch, tracked_caps, vdivider,
};
use crate::gui::{color, icons};
use crate::ipc_local::Client;
use crate::ipc_local::protocol::{Event, JobCounters, SnapshotData};

const RESIZE_HANDLE_W: f32 = 6.0;
const HEADER_H: f32 = 22.0;

/// 1px hairline under every row (design `.tr`), so the pitch a virtual
/// list counts in is the row plus this.
const ROW_SEPARATOR_H: f32 = 1.0;
/// Rows built above and below the viewport, so a flick has something to
/// show before the next scroll event lands.
const ROW_OVERSCAN: usize = 6;
/// Jobs-table row height (design `.tr`).
const ROW_H: f32 = 48.0;
/// Sidebar / list-nav row height (design `.nav-item`).
const SIDEBAR_ROW_H: f32 = 26.0;

// Queue live-dot (design `.q-live-dot`): a small moss dot shown next to
// a queue's color chip while that queue has ≥1 running job.
const LIVE_DOT_SIZE: f32 = 7.0;

// Toast (design `.toast`): bottom-right surface card with a 3px left
// auto-dismissed after `TOAST_TTL_MS`.
const TOAST_TTL_MS: u64 = 3000;
const TOAST_W: f32 = 320.0;
const TOAST_GAP: f32 = 8.0;
/// Design `.toast { bottom: 24px; right: 24px }` — measured from the
/// window edge, so the card floats over the status bar (z-index 400).
const TOAST_MARGIN: f32 = 24.0;
/// Status-bar height, also the toast stack's floor: the layer spans the
/// whole body, so without it the bottom toast sits on the bar.
const STATUSBAR_H: f32 = 28.0;
/// Design `.toast { padding: 10px 14px }`.
const TOAST_PAD_Y: f32 = 10.0;
const TOAST_PAD_X: f32 = 14.0;

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

// Type-column ext pill (design `.fname` ext tag): 28×22, radius 4, mono
// 700 ~9px, category-tinted.

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
    /// Press that landed on the table but not on a row — the empty
    /// space below the last row, or beside the columns. Rows capture
    /// their own presses, so reaching this means "nothing here".
    ClearSelection,
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
    ColHeaderHover(SortColumn, bool),
    /// `(x, y, viewport height)` from the table's scrollable. The x
    /// keeps the header in step; the other two are what the virtual
    /// list needs to know which rows are on screen.
    TableScrolled(f32, f32, f32),
    WindowResized(f32, f32),
    ColResizeStart(SortColumn),
    ColMoveStart(SortColumn),
    /// Pointer left the window or focus was lost mid-drag.
    DragCancelled,
    HeaderRightClick,
    /// Status-bar disk button: reveal the in-flight cache folder.
    OpenWorkDir,
    ColToggle(SortColumn),
    // Remove overlay
    RemoveAs(RemoveKind),
    RemoveDeleteOnDisk(bool),
    RemoveDontAsk(bool),
    RemoveConfirm,
    /// The restart confirmation was accepted.
    RestartConfirmed,
    /// A removal finished: what went (already phrased for the toast),
    /// plus every file the daemon (or Trash) could not get rid of.
    RemoveDone {
        what: String,
        problems: Vec<String>,
    },
    // Drag-to-add (design `.drag-overlay`)
    DragHover(bool),
    DragDropped(std::path::PathBuf),
    /// Ctrl+V, or a dropped text file: whatever links the text held.
    LinksPasted(Vec<url::Url>),
    // Toasts (design `.toast`)
    Toast(ToastSeverity, String),
    ToastExpired(u64),
    /// Clicked away before its TTL ran out. Same removal as expiry —
    /// the pending `ToastExpired` for this id then finds nothing.
    ToastDismissed(u64),
    /// Pointer entered a table row.
    RowHovered(JobId),
    /// Pointer left a table row. Carries the row's own id: a fast move
    /// makes the entered and exited rows report in the SAME batch, and
    /// rows update top-down, so moving upwards the new row's enter
    /// arrives before the old row's exit. An id-less exit would clear
    /// the highlight that had just been set.
    RowUnhovered(JobId),
    /// The debounce for resize generation `n` elapsed.
    ResizeSettled(u64),
    /// Resize settled: `(width, height, maximized)`. Only a
    /// non-maximized size is worth remembering.
    WindowSizeSettled(f32, f32, bool),
    SectionHovered(u8),
    SectionUnhovered(u8),
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
    About,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextAction {
    /// Open the per-job window (`oxdm gui download <id>`): live
    /// progress while the job runs, the completion view once it is
    /// done. Design calls this "Show progress…".
    ShowProgress,
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
    Remove,
    BrowserExtensions,
    /// First-run welcome variant of the browser-extensions dialog
    /// (design §3.8 `welcome` mode).
    Welcome,
    DbError,
    SecretsLocked,
    /// The entries were removed, but some of their files are still on
    /// disk. Shown after the fact — the removal already happened.
    RemoveWarning,
    /// "Start these downloads over?" — asked before, because it throws
    /// away every byte already fetched.
    RestartConfirm,
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
    /// Sidebar section head under the pointer. The design only reveals
    /// the section's "+" on hover (`.sec-head:hover .add { opacity: 1 }`),
    /// and a view cannot read hover status, so track it.
    pub hovered_section: Option<u8>,
    /// Row under the pointer, for the design's `tr:hover` background.
    /// iced containers have no hover status, so the table row tracks it
    /// explicitly via `mouse_area` enter/exit.
    pub hovered_row: Option<JobId>,
    pub overlay: Overlay,
    pub remove: Option<RemoveState>,
    /// Files a removal was asked to delete and could not, one line each.
    pub remove_problems: Vec<String>,
    /// Downloads a confirmed restart will start over from zero.
    pub restart_ids: Vec<JobId>,
    pub db_error: Option<String>,
    pub modifiers: iced::keyboard::Modifiers,
    pub cursor: (f32, f32),
    /// Cursor position captured when a popup menu opened — menus
    /// must not follow the moving mouse.
    pub menu_anchor: (f32, f32),
    pub win_size: (f32, f32),
    /// Bumped on every resize event; a settle callback only persists if
    /// it still owns the latest generation. Throttling instead of
    /// debouncing dropped the *final* size of any drag shorter than the
    /// throttle window, so the app remembered a mid-drag size — or, for
    /// a quick drag, the size it started from.
    pub resize_gen: u64,
    pub columns: crate::gui::ui_prefs::ColumnsState,
    /// Active header drag: (column, cursor x at start, width at start).
    pub col_drag: Option<(SortColumn, f32, f32)>,
    /// Column order the header row previews while a drag is in flight.
    /// The body keeps rendering the committed order until release —
    /// re-laying every row on each pointer move is the expensive half of
    /// a reorder, and the header alone carries the feedback.
    pub col_preview: Option<[usize; crate::gui::ui_prefs::COLS]>,
    /// Header being dragged to a new position: `(column, press x)`.
    /// Until the pointer travels `COL_MOVE_SLOP` this is still a click,
    /// so the release sorts instead of reordering.
    /// `(column, press x, last x)` — the last x is what gives the drag
    /// its direction.
    pub col_move: Option<(SortColumn, f32, f32)>,
    /// How far into the cell the press landed, captured once: recomputing
    /// it from the live layout would move the ghost every time the
    /// preview reorders underneath it.
    pub col_grab: f32,
    pub col_handle_hover: Option<SortColumn>,
    /// Header cell under the pointer — sortable headers tint on hover so
    /// they read as clickable (design `tbody tr:hover` treatment).
    pub col_header_hover: Option<SortColumn>,
    /// Horizontal scroll offset of the table body (mirrored on every
    /// `TableScrolled`); corrects the resize guideline x.
    pub table_scroll_x: f32,
    /// Vertical scroll offset and the height of the scrollable's own
    /// viewport, both from `TableScrolled`. The virtual list turns them
    /// into the slice of rows worth building.
    pub table_scroll_y: f32,
    pub table_viewport_h: f32,
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
            hovered_section: None,
            hovered_row: None,
            overlay: Overlay::None,
            remove: None,
            remove_problems: Vec::new(),
            restart_ids: Vec::new(),
            db_error: None,
            modifiers: iced::keyboard::Modifiers::default(),
            cursor: (0.0, 0.0),
            menu_anchor: (0.0, 0.0),
            win_size: (0.0, 0.0),
            resize_gen: 0,
            columns: crate::gui::ui_prefs::load().columns.unwrap_or_default(),
            col_drag: None,
            col_move: None,
            col_grab: 0.0,
            col_preview: None,
            col_handle_hover: None,
            col_header_hover: None,
            table_scroll_x: 0.0,
            table_scroll_y: 0.0,
            table_viewport_h: 0.0,
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
    /// starting needs ≥1 startable job. Other scopes: pausing needs
    /// something running; resuming needs ≥1 startable job.
    ///
    /// "Startable" is the daemon's own rule (`Phase::is_startable`),
    /// failed jobs included — the button must not refuse work that
    /// `start_queue` / `resume_all` would happily do.
    fn toggle_actionable(&self) -> bool {
        let any_startable = |f: &dyn Fn(&crate::domain::Job) -> bool| -> bool {
            self.snap
                .jobs
                .iter()
                .any(|j| f(j) && self.phase(j.id).is_startable())
        };
        match self.filter {
            SidebarFilter::Queue(q) => {
                self.snap.active_queues.contains(&q) || any_startable(&|j| j.queue_id == q)
            }
            _ => self.any_running() || any_startable(&|_| true),
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

/// A filename cut to what a toast can hold, ending in an ellipsis.
///
/// Head-truncated, not middle: what distinguishes two downloads in a
/// list is almost always the start of the name, and a toast is read in
/// passing rather than studied.
fn clipped(name: &str) -> String {
    const MAX: usize = 36;
    let mut chars = name.chars();
    let head: String = chars.by_ref().take(MAX).collect();
    if chars.next().is_none() {
        head
    } else {
        format!("{head}\u{2026}")
    }
}

/// Turn an IPC result into a message: nothing to say on success, a
/// toast naming the reason on failure.
fn paste_failure(result: Result<(), String>, what: &str) -> Msg {
    match result {
        Ok(()) => Msg::Noop,
        Err(e) => Msg::Toast(ToastSeverity::Error, format!("{what}: {e}")),
    }
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
            // Covers changes that did not come from this machine's
            // Settings window (import, another host, a hand-edited DB).
            crate::gui::ui_prefs::sync_custom_window_chrome(snap.settings.custom_window_chrome);
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
            // `JobFailed` included: the row's phase rides the counter
            // stream, but *why* it failed lives on the job, and without
            // a re-fetch the list keeps rendering the job as it was
            // before it failed — a progress bar under a download that
            // has already given up.
            Event::JobsChanged
            | Event::JobFailed { .. }
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
        Msg::ClearSelection => {
            m.selection.clear();
            m.select_anchor = None;
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
        Msg::SectionHovered(idx) => {
            m.hovered_section = Some(idx);
            Task::none()
        }
        Msg::SectionUnhovered(idx) => {
            // Same ownership rule as the table rows: a newer enter for
            // another head must win regardless of message order.
            if m.hovered_section == Some(idx) {
                m.hovered_section = None;
            }
            Task::none()
        }
        Msg::RowHovered(id) => {
            m.hovered_row = Some(id);
            Task::none()
        }
        Msg::RowUnhovered(id) => {
            // Only clear if this row still owns the slot; a newer enter
            // for another row must win regardless of message order.
            if m.hovered_row == Some(id) {
                m.hovered_row = None;
            }
            Task::none()
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
            if m.overlay == Overlay::RemoveWarning {
                m.remove_problems.clear();
            }
            if m.overlay == Overlay::RestartConfirm {
                m.restart_ids.clear();
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
            if let Some((col, press_x, last_x)) = m.col_move
                && (x - press_x).abs() >= COL_MOVE_SLOP
            {
                // Direction decides which threshold applies, so a
                // sub-pixel wobble must not flip it.
                let dx = x - last_x;
                if dx.abs() >= DRAG_DIR_DEADBAND {
                    m.col_move = Some((col, press_x, x));
                    drag_step(m, col, dx > 0.0);
                }
            }
            Task::none()
        }
        Msg::MouseReleased => {
            if m.col_drag.take().is_some() {
                crate::gui::ui_prefs::save_columns(&m.columns);
            }
            if let Some((col, press_x, _)) = m.col_move.take() {
                if (m.cursor.0 - press_x).abs() < COL_MOVE_SLOP {
                    // The pointer never left the header cell: a plain
                    // click, so sort (same toggle as `Msg::SetSort`).
                    if m.sort.0 == col {
                        m.sort.1 = !m.sort.1;
                    } else {
                        m.sort = (col, matches!(col, SortColumn::Date));
                    }
                    return Task::none();
                }
                // The header previewed the new order; commit it to the
                // table now, which is the only point the body re-lays
                // out.
                let _ = col;
                if let Some(order) = m.col_preview.take() {
                    m.columns.order = order;
                }
                crate::gui::ui_prefs::save_columns(&m.columns);
            }
            Task::none()
        }
        Msg::DragCancelled => {
            // Abandon the move (the preview was never committed), but
            // keep a resize — its width is already applied, and undoing
            // it would be the surprise.
            m.col_move = None;
            m.col_preview = None;
            m.col_handle_hover = None;
            if m.col_drag.take().is_some() {
                crate::gui::ui_prefs::save_columns(&m.columns);
            }
            Task::none()
        }
        Msg::ColMoveStart(col) => {
            let mut left = 0.0;
            for c in visible_cols(m) {
                if c == col {
                    break;
                }
                left += m.columns.width(c as usize);
            }
            let cell_x = theme::size::SIDEBAR_W + left - m.table_scroll_x;
            m.col_grab = (m.cursor.0 - cell_x).clamp(0.0, m.columns.width(col as usize));
            m.col_move = Some((col, m.cursor.0, m.cursor.0));
            m.col_preview = Some(m.columns.order);
            // A grip left highlighted under the pointer would keep
            // glowing for the whole drag.
            m.col_handle_hover = None;
            Task::none()
        }
        Msg::ColResizeStart(col) => {
            m.col_drag = Some((col, m.cursor.0, m.columns.width(col as usize)));
            Task::none()
        }
        Msg::TableScrolled(x, y, viewport_h) => {
            m.table_scroll_x = x;
            m.table_scroll_y = y;
            m.table_viewport_h = viewport_h;
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
        Msg::ColHeaderHover(col, on) => {
            // Guarded so the exit of the cell being left cannot clear the
            // enter of the one being entered, whichever order they arrive.
            if on {
                m.col_header_hover = Some(col);
            } else if m.col_header_hover == Some(col) {
                m.col_header_hover = None;
            }
            Task::none()
        }
        Msg::OpenWorkDir => {
            crate::platform::open_path(&m.snap.settings.work_dir);
            Task::none()
        }
        Msg::HeaderRightClick => {
            m.columns_menu = true;
            m.menu_anchor = m.cursor;
            Task::none()
        }
        Msg::WindowSizeSettled(w, h, maximized) => {
            if !maximized {
                crate::gui::ui_prefs::save_window(crate::gui::ui_prefs::WindowPrefs {
                    width: w,
                    height: h,
                });
            }
            Task::none()
        }
        Msg::ColToggle(col) => {
            m.columns.toggle(col as usize);
            crate::gui::ui_prefs::save_columns(&m.columns);
            Task::none()
        }
        Msg::WindowResized(w, h) => {
            m.win_size = (w, h);
            m.resize_gen = m.resize_gen.wrapping_add(1);
            let generation = m.resize_gen;
            let clamp =
                chrome::enforce_min_size(iced::Size::new(w, h), iced::Size::new(820.0, 520.0));
            let settle = Task::perform(
                async move {
                    tokio::time::sleep(std::time::Duration::from_millis(RESIZE_SETTLE_MS)).await;
                },
                move |()| Msg::ResizeSettled(generation),
            );
            Task::batch([clamp, settle])
        }
        Msg::ResizeSettled(generation) => {
            if generation != m.resize_gen {
                return Task::none(); // a later resize superseded this one
            }
            let (w, h) = m.win_size;
            // Ask the window whether this is its maximized size before
            // persisting: `launch_main` restores whatever is saved, so
            // storing a maximized geometry makes every later launch open
            // filling the screen.
            iced::window::latest()
                .and_then(iced::window::is_maximized)
                .map(move |maximized| Msg::WindowSizeSettled(w, h, maximized))
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
            // A dropped text file is a list of links, not a download:
            // read it and treat it exactly like a paste. Anything else
            // — or a file with no links in it — goes to the Add dialog
            // as a path, which is what it was before.
            let prefill = path.to_string_lossy().into_owned();
            if prefill.trim().is_empty() {
                return Task::none();
            }
            let client = m.client.clone();
            Task::perform(
                async move {
                    let links = tokio::task::spawn_blocking(move || {
                        crate::gui::clipboard::links_in_file(&path)
                    })
                    .await
                    .unwrap_or_default();
                    if links.is_empty() {
                        let _ = client.open_add_window(None, Some(prefill)).await;
                    }
                    links
                },
                Msg::LinksPasted,
            )
        }
        Msg::LinksPasted(urls) => {
            let client = m.client.clone();
            match urls.len() {
                // Pressing paste and having nothing happen is
                // indistinguishable from a shortcut that does not work.
                0 => update_main(m, Msg::Toast(ToastSeverity::Info, "No links to add".into())),
                // One link is a download the user is about to describe;
                // several are a list to triage, which is what the batch
                // window is for.
                //
                // Failures are said out loud rather than logged: the
                // usual cause is a daemon older than this window, which
                // does not know the request and answers with an error
                // no one would otherwise see.
                1 => {
                    let one = urls[0].to_string();
                    Task::perform(
                        async move { client.open_add_window(None, Some(one)).await },
                        |r| paste_failure(r, "Couldn't open the Add window"),
                    )
                }
                n => {
                    let what = format!("Couldn't open the list of {n} links");
                    Task::perform(
                        async move { client.open_batch_window(urls).await },
                        move |r| paste_failure(r, &what),
                    )
                }
            }
        }
        Msg::Toast(severity, message) => spawn_toast(m, severity, message),
        Msg::ToastExpired(id) | Msg::ToastDismissed(id) => {
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
            // Named while the state is still here: "Removed download"
            // tells the user nothing when three rows were selected and
            // one of them was the wrong one.
            let what = if r.ids.len() == 1 {
                format!("Removed {}", clipped(&r.filename))
            } else {
                format!("Removed {} downloads", r.ids.len())
            };
            let client = m.client.clone();
            let mut settings = m.snap.settings.clone();
            Task::perform(
                async move {
                    // N4: a file that survives the removal is never
                    // silent — no DBus, cross-device, read-only, still
                    // open. The entries go either way (a half-done
                    // removal is worse), and what is left behind is
                    // collected for the dialog afterwards.
                    let mut problems: Vec<String> = Vec::new();
                    for p in trash_paths {
                        let shown = p.display().to_string();
                        let res = tokio::task::spawn_blocking(move || trash::delete(&p))
                            .await
                            .map_err(|e| e.to_string())
                            .and_then(|r| r.map_err(|e| e.to_string()));
                        if let Err(e) = res {
                            problems.push(format!("{shown}: {e}"));
                        }
                    }
                    for id in &r.ids {
                        if let Ok(Some(w)) = client
                            .remove(
                                *id,
                                crate::data::RemoveOpts {
                                    purge_partial: !r.completed,
                                    // Trash already moved the file; never
                                    // double-delete on disk.
                                    delete_final_file: r.has_files && r.delete_on_disk && !trash,
                                },
                            )
                            .await
                        {
                            problems.push(w);
                        }
                    }
                    if r.dont_ask_again {
                        if r.clean {
                            settings.remove_confirm_clean = false;
                        } else if r.completed || r.has_files {
                            settings.remove_confirm_completed = false;
                        } else {
                            settings.remove_confirm_incomplete = false;
                        }
                        let _ = client.update_settings(settings).await;
                    }
                    problems
                },
                move |problems| Msg::RemoveDone {
                    what: what.clone(),
                    problems,
                },
            )
        }
        Msg::RestartConfirmed => {
            m.overlay = Overlay::None;
            let ids = std::mem::take(&mut m.restart_ids);
            let client = m.client.clone();
            act(async move {
                for id in ids {
                    // The file goes first: `restart_job` clears the
                    // work directory but not the finished file, and the
                    // new run would arrive at a name already taken.
                    let _ = client.delete_final_file(id).await;
                    client.restart_job(id).await?;
                }
                Ok(())
            })
        }
        Msg::RemoveDone { what, problems } => {
            if problems.is_empty() {
                return update_main(m, Msg::Toast(ToastSeverity::Success, what));
            }
            m.remove_problems = problems;
            m.overlay = Overlay::RemoveWarning;
            Task::none()
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
                // Opened by hand with a link on the clipboard: fill it
                // in. It is what the user came to paste, and the dialog
                // has always had a Paste button saying so — this saves
                // the press without taking the decision away, since the
                // field is still theirs to edit.
                ToolbarAction::AddUrl => act(async move {
                    let prefill =
                        tokio::task::spawn_blocking(crate::gui::clipboard::clipboard_first_link)
                            .await
                            .ok()
                            .flatten();
                    client.open_add_window(None, prefill).await
                }),
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
                ToolbarAction::Clean => request_clean(m),
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
                ToolAction::About => act(async move { client.open_about_window().await }),
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
        // Paste is how a link arrives from a browser, and the list is
        // where the user is looking when they copy one. Reading the
        // clipboard is blocking on X11, so it goes off the UI thread.
        Key::Character("v") if mods.command() => Task::perform(
            async {
                tokio::task::spawn_blocking(crate::gui::clipboard::clipboard_links)
                    .await
                    .unwrap_or_default()
            },
            Msg::LinksPasted,
        ),
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
        // Nothing to confirm — the removal already happened; Enter
        // acknowledges the report the same way the Close button does.
        Key::Named(Named::Enter) if m.overlay == Overlay::RemoveWarning => {
            update_main(m, Msg::CloseOverlay)
        }
        Key::Named(Named::Escape) => update_main(m, Msg::CloseOverlay),
        _ => Task::none(),
    }
}

/// Toolbar Clean: drop every completed entry from the list. Files are
/// never touched, so this is the safe entry-only removal — but it acts
/// on a set the user never picked, hence its own confirmation setting.
fn request_clean(m: &mut Main) -> Task<Msg> {
    let ids: Vec<JobId> = m
        .snap
        .jobs
        .iter()
        .filter(|j| m.phase(j.id) == Phase::Completed)
        .map(|j| j.id)
        .collect();
    if ids.is_empty() {
        return update_main(
            m,
            Msg::Toast(ToastSeverity::Info, "No finished downloads to clean".into()),
        );
    }
    let n = ids.len();
    m.remove = Some(RemoveState {
        ids,
        filename: if n == 1 {
            "1 finished download".to_owned()
        } else {
            format!("{n} finished downloads")
        },
        completed: true,
        // Clean never touches files, whatever the rows left behind.
        has_files: false,
        kind: RemoveKind::Entry,
        delete_on_disk: false,
        dont_ask_again: false,
        clean: true,
    });
    if m.snap.settings.remove_confirm_clean {
        m.overlay = Overlay::Remove;
        Task::none()
    } else {
        update_main(m, Msg::RemoveConfirm)
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
    // What can be deleted from disk is not the same question as what
    // finished: an integrity failure has the whole file, and the user
    // is more likely to want that one gone than any other.
    let has_files = ids.iter().all(|id| {
        m.snap
            .jobs
            .iter()
            .any(|j| j.id == *id && j.has_saved_file())
    });
    // A file on disk is the thing worth confirming about, whether or
    // not the download is `Completed`.
    let need_confirm = if completed || has_files {
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
        has_files,
        kind,
        // Permanent pre-checks "also delete file on disk" when there is
        // one; the user can still untick before confirming.
        delete_on_disk: matches!(kind, RemoveKind::Permanent) && has_files,
        dont_ask_again: false,
        clean: false,
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
        // Restarting throws away everything already fetched, so it
        // asks first — the same question the download window's own
        // Restart puts, in the same words.
        ContextAction::Restart => {
            m.restart_ids = ids;
            m.overlay = Overlay::RestartConfirm;
            Task::none()
        }
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
        ContextAction::ShowProgress => act(async move {
            for id in ids {
                client.open_download_window(id).await?;
            }
            Ok(())
        }),
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
        subs.push(
            crate::gui::ipc::all_events(crate::ipc_local::protocol::GuiKind::Main).map(Msg::Daemon),
        );
        subs.push(iced::event::listen_with(|event, status, _id| match event {
            // A key a widget has already used is not a shortcut:
            // Ctrl+V in the search field is a paste into the field,
            // and the window must not also read it as "add these
            // links".
            iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { key, modifiers, .. })
                if status == iced::event::Status::Ignored =>
            {
                Some(Msg::KeyPressed(key, modifiers))
            }
            iced::Event::Keyboard(iced::keyboard::Event::ModifiersChanged(mods)) => {
                Some(Msg::Modifiers(mods))
            }
            iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
                Some(Msg::CursorMoved(position.x, position.y))
            }
            iced::Event::Mouse(iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left)) => {
                Some(Msg::MouseReleased)
            }
            // A drag can end without a release ever reaching us —
            // the pointer leaves the window, or the compositor takes
            // focus away mid-gesture. Without this the header keeps
            // previewing an order the table never adopts.
            iced::Event::Mouse(iced::mouse::Event::CursorLeft)
            | iced::Event::Window(iced::window::Event::Unfocused) => Some(Msg::DragCancelled),
            iced::Event::Window(iced::window::Event::Resized(size)) => {
                Some(Msg::WindowResized(size.width, size.height))
            }
            // Drag-to-add (design `.drag-overlay`). NOTE: file-drop
            // events are compositor-dependent and may not deliver on
            // all Wayland/X11 setups — the code is correct but cannot
            // be verified headless.
            iced::Event::Window(iced::window::Event::FileHovered(_)) => Some(Msg::DragHover(true)),
            iced::Event::Window(iced::window::Event::FilesHoveredLeft) => {
                Some(Msg::DragHover(false))
            }
            iced::Event::Window(iced::window::Event::FileDropped(path)) => {
                Some(Msg::DragDropped(path))
            }
            _ => None,
        }));
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
    chrome::framed(match app {
        App::Connecting => splash("Connecting to the oxdm daemon…".to_owned()),
        App::Failed(e) => splash(format!("Could not reach the daemon: {e}")),
        App::Ready(m) => main_view(m),
    })
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
                // Clicking past the rows clears the selection, the way
                // every file list does. One `mouse_area` around the
                // whole pane covers the empty table body, the strip
                // beside the columns and the gaps in the bars above it:
                // rows, buttons, tabs, the search field and the
                // scrollbars all capture their own presses (iced's
                // `mouse_area` forwards to its content first and stops
                // if the event was captured), so this only ever sees
                // presses that hit nothing.
                mouse_area(
                    column![
                        toolbar(m),
                        hairline(t.border_subtle),
                        tab_strip(m),
                        hairline(t.border_subtle),
                        table(m),
                    ]
                    .width(Length::Fill)
                    .height(Length::Fill),
                )
                .on_press(Msg::ClearSelection),
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
            Overlay::Remove => main_dialogs::remove_confirm(m, base),
            Overlay::BrowserExtensions => main_dialogs::browser_extensions(m, base),
            Overlay::Welcome => main_dialogs::welcome(m, base),
            Overlay::DbError => {
                let err = m.db_error.clone().unwrap_or_default();
                main_dialogs::db_error(m, base, &err)
            }
            Overlay::SecretsLocked => main_dialogs::secrets_locked(m, base),
            Overlay::RemoveWarning => main_dialogs::remove_warning(m, base),
            Overlay::RestartConfirm => main_dialogs::restart_confirm(m, base),
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
    let with_toasts = header_ghost(m, with_toasts);

    let content = container(column![
        titlebar::titlebar(t, "oxdm", m.maximized, Msg::Window),
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
            text("Drop to add")
                .font(theme::DISPLAY)
                .size(22.0)
                .color(iced::Color::WHITE),
            text("A file, or a text file of links")
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
        icons::icon(icon, 14.0, accent),
        text(toast.message.clone())
            // Design `.toast { font: 500 12px }`.
            .font(theme::BODY_MEDIUM)
            .size(12.0)
            .color(t.fg_1),
    ]
    .spacing(10.0) // design `.toast { gap: 10px }`
    .align_y(Alignment::Center);
    // The severity reads from the icon alone — a coloured rail down the
    // card's edge said the same thing twice and fought the card's own
    // rounding.
    let card = container(body)
        .width(Length::Fixed(TOAST_W))
        .padding([TOAST_PAD_Y, TOAST_PAD_X])
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
        });

    // Click anywhere on the card to dismiss it early. The button is
    // styled to nothing so the card's own surface still shows through;
    // it exists for the hit box (and gives the pointer cursor).
    let id = toast.id;
    iced::widget::button(card)
        .padding(0)
        .style(|_, _| iced::widget::button::Style::default())
        .on_press(Msg::ToastDismissed(id))
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
        .padding(iced::Padding {
            bottom: TOAST_MARGIN + STATUSBAR_H,
            ..iced::Padding::from(TOAST_MARGIN)
        });
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
    // `indent`: design `.nav-item.indent` — the category rows under
    // "All downloads" hang off it, so their content starts 22px in.
    indent: bool,
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
        left: if indent { NAV_INDENT } else { NAV_PAD_X },
        right: 10.0,
        ..Default::default()
    })
    .on_press(msg)
    .style(move |_, status| {
        use iced::widget::button::Status;
        // Design `.nav-item.on { background: clay-400 }` — a literal,
        // so the selected row keeps the same clay in every theme and
        // does not shift under the pointer.
        let background = if active {
            Some(crate::gui::color::clay::C400.into())
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

/// Design `.nav-item { padding: 5px 10px }` / `.indent { padding-left: 22px }`.
const NAV_PAD_X: f32 = 10.0;
const NAV_INDENT: f32 = 22.0;
/// Design `.sec-head`: 700 10px, `letter-spacing: 0.1em`, uppercase.
const SEC_HEAD_SIZE: f32 = 10.0;
const SEC_HEAD_TRACKING: f32 = SEC_HEAD_SIZE * 0.1;

/// Quiet period after the last resize event before the size is
/// persisted — long enough to cover a drag, short enough to survive a
/// close right afterwards.
const RESIZE_SETTLE_MS: u64 = 400;

fn section_header<'a>(
    t: &Tokens,
    label: &'a str,
    idx: u8,
    open: bool,
    // `hovered`: the design keeps the section's "+" at `opacity: 0`
    // until the pointer is over the head.
    hovered: bool,
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
        tracked_caps(
            label,
            SEC_HEAD_SIZE,
            SEC_HEAD_TRACKING / SEC_HEAD_SIZE,
            t.fg_3
        ),
    ]
    .spacing(6.0)
    .align_y(Alignment::Center);
    // Section "+" add affordance (design: Queues header opens the
    // Queue dialog). Nested button captures its own click so the
    // surrounding toggle mouse_area doesn't also fire.
    if let Some(add_msg) = add.filter(|_| hovered) {
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
    .on_enter(Msg::SectionHovered(idx))
    .on_exit(Msg::SectionUnhovered(idx))
    .interaction(iced::mouse::Interaction::Pointer)
    .into()
}

fn sidebar(m: &Main) -> Element<'_, Msg> {
    let t = &m.tokens;
    let rh = SIDEBAR_ROW_H;
    let live = m.live_queues();
    let pa = pulse_alpha(m);
    let mut col = column![]
        .spacing(2.0)
        .padding(iced::Padding::new(theme::space::S1));

    // CATEGORIES
    let cats_open = !m.collapsed_sections.contains(&0);
    col = col.push(section_header(
        t,
        "Categories",
        0,
        cats_open,
        m.hovered_section == Some(0),
        None,
    ));
    if cats_open {
        let all_active = m.filter == SidebarFilter::All;
        col = col.push(sidebar_row(
            t,
            icons::icon("layers", 17.0, leader_fg(t, all_active)),
            "All downloads",
            Some(m.cat_count(None)),
            all_active,
            rh,
            false,
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
                true,
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
        m.hovered_section == Some(1),
        Some(Msg::Tool(ToolAction::Scheduler)),
    ));
    if queues_open {
        for q in &m.snap.queues {
            let active = m.filter == SidebarFilter::Queue(q.id);
            let count = m.snap.jobs.iter().filter(|j| j.queue_id == q.id).count() as u64;
            let chip = swatch(8.0, 2.0, t.queue_color(q));
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
                false,
                Msg::SetFilter(SidebarFilter::Queue(q.id)),
            ));
        }
    }

    // TOOLS
    let tools_open = !m.collapsed_sections.contains(&2);
    col = col.push(section_header(
        t,
        "Tools",
        2,
        tools_open,
        m.hovered_section == Some(2),
        None,
    ));
    if tools_open {
        for (action, icon, label) in [
            (ToolAction::Scheduler, "calendar", "Scheduler"),
            (ToolAction::Settings, "settings", "Settings"),
            (ToolAction::BrowserExtension, "puzzle", "Browser extension"),
            (ToolAction::About, "info", "About"),
        ] {
            col = col.push(sidebar_row(
                t,
                icons::icon(icon, 17.0, t.fg_2),
                label,
                None,
                false,
                rh,
                false,
                Msg::Tool(action),
            ));
        }
    }

    let t2 = *t;
    container(crate::gui::widget::vscroll(col).height(Length::Fill))
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
            // `.tb-btn.primary`: the CTA shares the toolbar's metrics so
            // it sits at the same height as the buttons beside it, with
            // the same 16px glyph.
            .tb()
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
            .icon("octagon-x")
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
    // Grips are NOT part of the cell: they must straddle the column
    // boundary (design `.col-resizer { right: -8px; width: 16px }`),
    // which flow layout cannot express. They are overlaid by
    // `header_grips` instead, so the cell owns its full width.
    let dragged = matches!(m.col_move, Some((c, press_x, _)) if c == col
        && (m.cursor.0 - press_x).abs() >= COL_MOVE_SLOP);
    // Hover tint stops at the grip: over a resizer the gesture is a
    // resize, not a sort, and lighting the whole cell would promise the
    // wrong thing.
    let hovered = !dragged
        && m.col_move.is_none()
        && m.col_header_hover == Some(col)
        && m.col_handle_hover.is_none();
    let t2 = m.tokens;
    mouse_area(
        container(col_header_sortable(
            &m.tokens,
            label,
            active_col == col,
            desc,
            Msg::ColMoveStart(col),
        ))
        .width(Length::Fixed(width))
        .padding([0.0, theme::space::S2])
        .align_y(Alignment::Center)
        .height(Length::Fixed(HEADER_H))
        // The header being carried dims, so it is obvious which one the
        // drop marker belongs to.
        .style(move |_| container::Style {
            background: if dragged {
                Some(t2.bg_sunken.into())
            } else if hovered {
                Some(t2.row_hover_bg.into())
            } else {
                None
            },
            ..Default::default()
        }),
    )
    // The whole cell is the handle, not just the text:
    // `col_header_sortable` shrink-wraps its hit area, so pressing the
    // empty part of a header did nothing. The inner area handles (and
    // captures) presses over the text, so this never fires twice.
    // Press starts a potential reorder; the release decides whether the
    // gesture was a click (sort) or a drag (move).
    .on_press(Msg::ColMoveStart(col))
    .on_right_press(Msg::HeaderRightClick)
    .on_enter(Msg::ColHeaderHover(col, true))
    .on_exit(Msg::ColHeaderHover(col, false))
    .interaction(iced::mouse::Interaction::Pointer)
    .into()
}

/// Ghost of the header being dragged: a translucent copy of the cell
/// that follows the pointer anywhere in the window, so the gesture
/// reads as carrying the column rather than only pointing at a slot.
/// Positioned in window space (not the header's), hence a layer over
/// the whole body rather than one inside the header's scrollable.
fn header_ghost<'a>(m: &'a Main, base: Element<'a, Msg>) -> Element<'a, Msg> {
    let Some((col, press_x, _)) = m.col_move else {
        return base;
    };
    let grab = m.col_grab;
    if (m.cursor.0 - press_x).abs() < COL_MOVE_SLOP {
        return base;
    }
    let t = m.tokens;
    let width = m.columns.width(col as usize);

    let ghost = container(
        text(COL_LABELS[col as usize].to_uppercase())
            .font(theme::BODY_BOLD)
            .size(11.0)
            .color(color::with_alpha(t.fg_1, GHOST_ALPHA))
            .wrapping(iced::widget::text::Wrapping::None),
    )
    .width(Length::Fixed(width))
    .height(Length::Fixed(HEADER_H))
    .padding([0.0, theme::space::S2])
    .align_y(Alignment::Center)
    .style(move |_| container::Style {
        background: Some(color::with_alpha(t.bg_raised, GHOST_ALPHA).into()),
        ..Default::default()
    });

    iced::widget::stack![
        base,
        container(iced::widget::opaque(ghost)).padding(iced::Padding {
            left: (m.cursor.0 - grab).max(0.0),
            // The layer starts below the titlebar, so window-space y has
            // to lose that; centre the ghost on the pointer.
            top: (m.cursor.1 - titlebar::chrome_h() - HEADER_H / 2.0).max(0.0),
            ..Default::default()
        }),
    ]
    .into()
}

/// Resize grips, overlaid on the header row and centered on each column
/// boundary (design `.col-resizer`: centered hit area, grip centered in
/// it, and the drag guideline at its center). Laid out as spacers +
/// fixed-width grips so each lands on its boundary without absolute
/// positioning; the layer sits above the header cells in a `stack`, so
/// a press on a grip is captured before it can reach the sort control.
fn header_grips(m: &Main) -> Element<'_, Msg> {
    const HALF: f32 = RESIZE_HANDLE_W / 2.0;
    let t2 = m.tokens;
    let mut strip = row![].align_y(Alignment::Center);
    let mut first = true;
    for col in header_cols(m) {
        let width = m.columns.width(col as usize);
        // Gap from the previous grip's right edge to this grip's left
        // edge: the first is measured from the header's left edge.
        let gap = if first {
            width - HALF
        } else {
            width - RESIZE_HANDLE_W
        };
        first = false;
        strip = strip.push(iced::widget::Space::new().width(Length::Fixed(gap.max(0.0))));

        // Design `ResizableHeader`: 1px quiet idle, 3px clay-400 at ~70%
        // height on hover, 3px clay-500 full height while dragging.
        let dragging = matches!(m.col_drag, Some((c, _, _)) if c == col);
        // A header being carried must not light up (or grab) the grips it
        // passes over: the pointer is busy, and a highlight left behind
        // it sticks for the rest of the drag.
        let moving = m.col_move.is_some();
        let hovering = !moving && m.col_handle_hover == Some(col);
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
        // Type is fixed-width: its grip still draws and lights up on
        // hover — a missing handle in the run reads as a rendering bug —
        // it just never starts a drag.
        let mut grip = mouse_area(
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
        .interaction(if moving {
            iced::mouse::Interaction::Idle
        } else {
            iced::mouse::Interaction::ResizingHorizontally
        });
        if !moving {
            grip = grip
                .on_enter(Msg::ColHandleHover(col, true))
                .on_exit(Msg::ColHandleHover(col, false));
            grip = grip.on_press(Msg::ColResizeStart(col));
        }
        strip = strip.push(grip);
    }
    strip.height(Length::Fixed(HEADER_H)).into()
}

/// Column labels, indexed by `SortColumn as usize` — display order
/// lives in `ColumnsState::order`, which the user can drag around.
const COL_LABELS: [&str; crate::gui::ui_prefs::COLS] =
    ["Name", "Size", "Status", "Speed", "Time left", "Date added"];

const COL_BY_INDEX: [SortColumn; crate::gui::ui_prefs::COLS] = [
    SortColumn::Name,
    SortColumn::Size,
    SortColumn::Status,
    SortColumn::Speed,
    SortColumn::Eta,
    SortColumn::Date,
];

/// How far the pointer must travel across a header before the gesture
/// stops being a click-to-sort and becomes a drag-to-reorder.
const COL_MOVE_SLOP: f32 = 5.0;
/// Pointer travel needed before a reorder drag changes direction.
const DRAG_DIR_DEADBAND: f32 = 3.0;
/// Opacity of the dragged-header ghost.
const GHOST_ALPHA: f32 = 0.5;

/// Slot the dragged column where the pointer is: it lands before the
/// first remaining column whose midpoint the pointer has not yet passed.
///
/// Two details make this stable and predictable. The midpoints come from
/// the layout of the *other* columns — with the dragged one removed —
/// so the choice cannot feed back into itself; comparing against the
/// dragged cell's own edges oscillates whenever widths differ (carry a
/// 58px column past a 420px one and the swap drops the pointer outside
/// the cell's new bounds, which swaps it straight back). And the
/// comparison uses the pointer rather than the ghost's centre, so the
/// swap happens where the user is looking instead of half a column
/// ahead of it.
fn drag_step(m: &mut Main, col: SortColumn, forward: bool) {
    let x = m.cursor.0 - theme::size::SIDEBAR_W + m.table_scroll_x;

    // Walk the preview as it is drawn — the dragged column included —
    // so the thresholds are the edges on screen. Measuring the other
    // columns packed together puts everything right of the dragged slot
    // one column-width too far left.
    let cols = header_cols(m);
    let mut spans: Vec<(SortColumn, f32, f32)> = Vec::with_capacity(cols.len());
    let mut edge = 0.0;
    for c in &cols {
        let w = m.columns.width(*c as usize);
        spans.push((*c, edge, w));
        edge += w;
    }
    let Some(cur) = spans.iter().position(|(c, _, _)| *c == col) else {
        return;
    };

    // Asymmetric thresholds, by request: going forward a column steps
    // aside as soon as the pointer enters it, which keeps the drag
    // feeling immediate; coming back it holds until the pointer passes
    // its middle, so the column just vacated does not snap back the
    // instant the pointer twitches.
    let mut slot = cur;
    if forward {
        for (i, (_, left, _)) in spans.iter().enumerate().skip(cur + 1) {
            if x >= *left {
                slot = i;
            } else {
                break;
            }
        }
    } else {
        for i in (0..cur).rev() {
            let (_, left, w) = spans[i];
            if x < left + w / 2.0 {
                slot = i;
            } else {
                break;
            }
        }
    }
    if slot == cur {
        return;
    }

    // Splice the dragged column into its new slot. Removing it first
    // shifts everything after it left by one, which is exactly what
    // makes `slot` the right insertion index in both directions.
    let mut seq = cols;
    seq.remove(cur);
    let anchor = seq.get(slot).copied();

    // Hidden columns ride along with the visible neighbour they follow.
    let mut order: Vec<usize> = m.col_preview.unwrap_or(m.columns.order).to_vec();
    order.retain(|&i| COL_BY_INDEX[i] != col);
    let at = anchor
        .and_then(|a| order.iter().position(|&i| COL_BY_INDEX[i] == a))
        .unwrap_or(order.len());
    order.insert(at, col as usize);
    if let Ok(order) = <[usize; crate::gui::ui_prefs::COLS]>::try_from(order) {
        m.col_preview = Some(order);
    }
}

/// Committed column order, skipping hidden ones — what the body draws.
fn visible_cols(m: &Main) -> Vec<SortColumn> {
    visible_in(m, m.columns.order)
}

/// What the *header* draws: the drag preview while one is in flight,
/// otherwise the committed order.
fn header_cols(m: &Main) -> Vec<SortColumn> {
    visible_in(m, m.col_preview.unwrap_or(m.columns.order))
}

fn visible_in(m: &Main, order: [usize; crate::gui::ui_prefs::COLS]) -> Vec<SortColumn> {
    order
        .iter()
        .map(|&i| COL_BY_INDEX[i])
        .filter(|c| m.columns.is_visible(*c as usize))
        .collect()
}

fn table(m: &Main) -> Element<'_, Msg> {
    let t = &m.tokens;
    let mut header_row = row![];
    for col in header_cols(m) {
        header_row = header_row.push(header_cell(
            m,
            COL_LABELS[col as usize],
            col,
            m.columns.width(col as usize),
        ));
    }
    // Header scrolls horizontally in lockstep with the body (synced
    // via TableScrolled -> scroll_to); its own scrollbar is hidden.
    let header = container(
        mouse_area(
            scrollable(iced::widget::stack![header_row, header_grips(m)])
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
        // Virtual list: every row is the same height, so the slice on
        // screen is arithmetic. Everything above and below collapses
        // into one spacer each, which keeps the scrollbar honest while
        // sparing iced ~10k widgets to lay out and diff per frame — the
        // difference between a 20ms frame and a multi-second one on a
        // table that size.
        let pitch = ROW_H + ROW_SEPARATOR_H;
        // Until the first scroll event the viewport height is unknown;
        // the window is a safe over-estimate (it can only be smaller).
        let viewport_h = if m.table_viewport_h > 0.0 {
            m.table_viewport_h
        } else {
            m.win_size.1
        };
        let first = ((m.table_scroll_y / pitch).floor() as usize).saturating_sub(ROW_OVERSCAN);
        let visible = (viewport_h / pitch).ceil() as usize + ROW_OVERSCAN * 2 + 1;
        let last = (first + visible).min(jobs.len());

        let mut rows = column![];
        if first > 0 {
            rows =
                rows.push(iced::widget::Space::new().height(Length::Fixed(first as f32 * pitch)));
        }
        for job in jobs.iter().take(last).skip(first) {
            rows = rows.push(job_row(m, job));
        }
        if last < jobs.len() {
            rows = rows.push(
                iced::widget::Space::new()
                    .height(Length::Fixed((jobs.len() - last) as f32 * pitch)),
            );
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
            .on_scroll(|vp| {
                Msg::TableScrolled(
                    vp.absolute_offset().x,
                    vp.absolute_offset().y,
                    vp.bounds().height,
                )
            })
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
                            // Centered on the boundary, matching the
                            // grip above it (design `translateX(-50%)`).
                            left: (x - GUIDELINE_W / 2.0).max(0.0),
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
    for col in visible_cols(m) {
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
    let hovered = m.hovered_row == Some(id);
    let c = m.counters.get(&id);
    let phase = m.phase(id);

    let name = job.filename.clone().unwrap_or_else(|| job.url.to_string());
    let host = job.url.host_str().unwrap_or("").to_owned();

    let name_cell = container(
        column![
            crate::gui::widget::ellipsized(name, theme::BODY_BOLD, 13.0, t.fg_1),
            crate::gui::widget::ellipsized(host, theme::MONO, 10.0, t.fg_3),
        ]
        .spacing(2.0),
    )
    .width(Length::Fixed(m.columns.width(SortColumn::Name as usize)))
    .clip(true)
    .padding([0.0, theme::space::S2])
    .align_y(Alignment::Center)
    .height(Length::Fill);

    let total = c.and_then(|c| c.total);
    let size_cell = cell(
        crate::gui::widget::ellipsized(
            total.map(format_bytes).unwrap_or_else(|| "—".into()),
            theme::MONO,
            12.0,
            t.fg_2,
        ),
        Length::Fixed(m.columns.width(SortColumn::Size as usize)),
        Alignment::Start,
    );

    // Status cell shows a progress bar whenever there is progress worth
    // showing (design `DLRow.showBar`, generalised): a live transfer, or
    // any stopped one that already has bytes on disk. Keyed on the bytes
    // rather than on a phase allow-list — `cancel_to_queued` parks a
    // half-finished job at `Queued` without discarding its `.part`
    // files, and an allow-list would hide progress that is still there.
    // Only Queued-at-0% and Completed (100%, already said by the label)
    // stay plain dots.
    let frac = match (c.map(|c| c.downloaded), total) {
        (Some(d), Some(tot)) if tot > 0 => d as f64 / tot as f64,
        _ => 0.0,
    };
    let tone = match phase {
        Phase::Failed => ProgressTone::Failed,
        // Anything stopped-with-bytes reads as parked, not live.
        _ if !phase.is_running() => ProgressTone::Paused,
        _ => ProgressTone::Active,
    };
    // An integrity failure has every byte on disk and nothing to
    // resume, so a bar at 100% measures work that is over and offers a
    // number where the answer is "this file is wrong".
    let integrity_failed = job.integrity_failed();
    let stopped_with_progress = frac > 0.0 && phase != Phase::Completed && !integrity_failed;
    let status_cell: Element<'_, Msg> = if phase.is_running() || stopped_with_progress {
        let (_, label) = phase_style(t, phase);
        cell(
            inline_progress(t, frac as f32, label, selected, tone, Length::Fill, 22.0),
            Length::Fixed(m.columns.width(SortColumn::Status as usize)),
            Alignment::Start,
        )
    } else {
        let (color, label) = phase_style(t, phase);
        let label = if integrity_failed {
            "Integrity check failed".to_owned()
        } else {
            label
        };
        cell(
            status_mark(phase_mark(phase), color, label, 12.0),
            Length::Fixed(m.columns.width(SortColumn::Status as usize)),
            Alignment::Start,
        )
    };

    let speed = c.map(|c| c.speed_bps).unwrap_or(0.0);
    let speed_cell = cell(
        crate::gui::widget::ellipsized(
            if phase == Phase::Downloading {
                format_speed(speed)
            } else {
                "—".into()
            },
            theme::MONO,
            12.0,
            t.fg_2,
        ),
        Length::Fixed(m.columns.width(SortColumn::Speed as usize)),
        Alignment::Start,
    );

    let eta_cell = cell(
        crate::gui::widget::ellipsized(
            eta_of(c).map(format_eta).unwrap_or_else(|| "—".into()),
            theme::MONO,
            12.0,
            t.fg_2,
        ),
        Length::Fixed(m.columns.width(SortColumn::Eta as usize)),
        Alignment::Start,
    );

    let date_cell = cell(
        crate::gui::widget::ellipsized(
            format_short_date(&job.created_at),
            theme::MONO,
            11.0,
            t.fg_3,
        ),
        Length::Fixed(m.columns.width(SortColumn::Date as usize)),
        Alignment::Start,
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
    // Cells are built up front, then pushed in the user's column order;
    // `Option::take` hands each one out exactly once.
    let mut by_col: [Option<Element<'_, Msg>>; crate::gui::ui_prefs::COLS] = [
        Some(name_cell.into()),
        Some(size_cell),
        Some(status_cell),
        Some(speed_cell),
        Some(eta_cell),
        Some(date_cell),
    ];
    for col in visible_cols(m) {
        if let Some(cell) = by_col[col as usize].take() {
            cells = cells.push(cell);
        }
    }

    // NOTE: width must be Shrink — Fill resolves to zero inside the
    // horizontally-unbounded table scrollable and collapses the row.
    let row_el = container(cells)
        .height(Length::Fixed(ROW_H))
        .width(Length::Shrink)
        .style(move |_| container::Style {
            background: bg(hovered).map(Into::into),
            ..Default::default()
        });

    let (ctrl, shift) = (m.modifiers.command(), m.modifiers.shift());
    let row_area = mouse_area(row_el)
        .on_press(Msg::RowClick(id, ctrl, shift))
        .on_double_click(Msg::RowDoubleClick(id))
        .on_right_press(Msg::RowRightClick(id))
        .on_enter(Msg::RowHovered(id))
        .on_exit(Msg::RowUnhovered(id))
        .interaction(iced::mouse::Interaction::Pointer);

    // 1px bottom row separator (design `.tr` border-subtle hairline).
    // Fixed width = sum of visible columns so it tracks the Shrink row
    // (a Fill hairline would collapse in the unbounded scrollable).
    let total_w: f32 = visible_cols(m)
        .into_iter()
        .map(|c| m.columns.width(c as usize))
        .sum();
    let separator = container(iced::widget::Space::new())
        .width(Length::Fixed(total_w))
        .height(Length::Fixed(ROW_SEPARATOR_H))
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

/// Dot treatment per phase (design §3.1): shape carries the status
/// alongside colour, so Queued and Complete no longer render as the
/// same dot in two tints.
fn phase_mark(phase: Phase) -> crate::gui::widget::Mark {
    use crate::gui::widget::Mark;
    match phase {
        Phase::Completed => Mark::Check,
        Phase::Failed => Mark::Cross,
        Phase::Queued => Mark::Dashed,
        Phase::Paused | Phase::Cancelled => Mark::Hollow,
        _ => Mark::Filled,
    }
}

fn phase_style(t: &Tokens, phase: Phase) -> (iced::Color, String) {
    let color = match phase {
        Phase::Evaluating
        | Phase::ResolvingConflicts
        | Phase::Downloading
        | Phase::Assembling
        | Phase::Flushing
        | Phase::Verifying
        | Phase::Reconnecting => t.action_primary,
        Phase::Queued => t.status_info,
        Phase::Paused | Phase::Cancelled => t.fg_3,
        Phase::Completed => t.status_success,
        Phase::Failed => t.status_danger,
    };
    (color, phase.label().to_owned())
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
        // A queue inheriting the global "Unlimited" would otherwise
        // print the ceiling that stands in for it.
        text(if max_x >= crate::domain::settings::UNLIMITED_CONCURRENT {
            "max \u{221e}".to_owned()
        } else {
            format!("max {max_x}\u{00d7}")
        })
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

    let proxy = &m.snap.settings.proxy;
    let proxy_set = matches!(
        proxy.mode,
        crate::domain::ProxyMode::Http
            | crate::domain::ProxyMode::Https
            | crate::domain::ProxyMode::Socks5
    ) && !proxy.host.trim().is_empty();
    let (proxy_icon, proxy_label) = if proxy_set {
        ("shield", "Proxied")
    } else {
        ("globe", "Direct")
    };
    // Free space is reported for the work directory, so the button opens
    // that one: the in-flight `.part` files are what consume it.
    let free = free_disk_str(&m.snap.settings.work_dir);

    let right = row![
        Btn::new(free)
            .toolbar()
            .icon("hard-drive")
            .size(BtnSize::Sm)
            .on_press(Msg::OpenWorkDir)
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
    .height(Length::Fixed(STATUSBAR_H))
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

    // One entry, not two: pause and resume are the same switch, and the
    // download window's footer has always shown it that way. Queued
    // counts as resumable — the job is holding bytes and waiting for a
    // slot, and "start it now" is exactly what the user means. Only a
    // finished download has nothing to resume; `Restart Download`
    // below is the entry for that.
    let running = phase.is_running();
    let done = phase == Phase::Completed;
    // Assembly writes the final file; interrupting it leaves a file
    // that looks finished and is not. It ends on its own.
    let assembling = phase == Phase::Assembling;
    // A failed integrity check has no missing bytes to go back for.
    // Restart Download, below, is the only way forward.
    let integrity_failed = m
        .snap
        .jobs
        .iter()
        .any(|j| j.id == id && j.integrity_failed());

    // Destructive row morphs with live modifiers (design: Finder-like):
    // default neutral "Remove from list" → ⇧ ochre "Move to Trash" →
    // ⇧⌥ rust "Delete permanently". Every kind routes through the Remove
    // confirm (B4) — the modifier only PRE-SELECTS the option.
    //
    // Trash/Permanent act on the FINISHED file, so they're only offered
    // when EVERY selected job has one. A job still missing bytes has no
    // final file and removal would purge its `.part` (irrecoverable) —
    // which would betray the "recoverable" promise — so those
    // selections get entry-only removal regardless of modifiers.
    // Completion is not the test: a download that failed its integrity
    // check has the whole file, and is the one most likely to be
    // thrown away.
    let all_done = !m.selection.is_empty()
        && m.selection.iter().all(|sid| {
            m.snap
                .jobs
                .iter()
                .any(|j| j.id == *sid && j.has_saved_file())
        });
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
            // The action most often wanted sits under the cursor.
            item(
                if running { "pause" } else { "play" },
                if running { "Pause" } else { "Resume" },
                None,
                !done && !assembling && !(integrity_failed && !running),
                Msg::Context(if running {
                    ContextAction::Pause
                } else {
                    ContextAction::Resume
                })
            ),
            separator(),
            // Design puts "Show progress…" first among the rest and
            // offers it in every state — the same window carries the
            // completion view, so a finished download can be reopened
            // after its dialog was dismissed. The label follows what
            // the window will show.
            item(
                "activity",
                if done {
                    "Show Completion Dialog"
                } else {
                    "Show Progress\u{2026}"
                },
                None,
                true,
                Msg::Context(ContextAction::ShowProgress)
            ),
            item(
                "file",
                "Open",
                None,
                done,
                Msg::Context(ContextAction::Open)
            ),
            item(
                "folder",
                "Open Containing Folder",
                None,
                true,
                Msg::Context(ContextAction::OpenFolder)
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
    let cy = cy - titlebar::chrome_h(); // overlay stack starts below the bar
    let (mw, mh) = (268.0, 290.0);
    let (ww, wh) = if m.win_size.0 > 0.0 {
        (m.win_size.0, m.win_size.1 - titlebar::chrome_h())
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
    for col in m.columns.order.map(|i| COL_BY_INDEX[i]) {
        let label = COL_LABELS[col as usize];
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
    let cy = cy - titlebar::chrome_h();
    let (mw, mh) = (188.0, 200.0);
    let (ww, wh) = if m.win_size.0 > 0.0 {
        (m.win_size.0, m.win_size.1 - titlebar::chrome_h())
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
