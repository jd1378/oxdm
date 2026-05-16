//! egui/eframe GUI subprocess.
//!
//! This module is loaded by the `oxdm` binary when it runs as a GUI
//! window (`oxdm gui main` or `oxdm gui download <id>`). It connects
//! to the running daemon over `ipc_local`, mirrors the daemon state
//! into a local `Cache`, and renders the existing UI components
//! against that cache.
//!
//! Process lifecycle:
//!   - GUI process owns one or more eframe viewports.
//!   - Closing the main window shuts the GUI process down (`Quit`
//!     here means *this window*, not the daemon). The daemon survives
//!     and a tray click can spawn a fresh GUI process.
//!   - Daemon-wide quit goes through `client.daemon_quit()` and is
//!     reachable from the File menu, the tray menu, and `Ctrl+Q`.

pub(crate) mod clipboard;
pub mod color;
pub mod components;
pub mod dialogs;
pub(crate) mod gui_state;
pub(crate) mod icon;
pub(crate) mod platform;
pub mod theme;
pub(crate) mod ui_prefs;
pub mod updater;
pub mod utils;
pub mod windows;

use std::sync::Arc;

pub use clipboard::read_url_from_clipboard;

use eframe::egui;
use indexmap::IndexSet;
use tokio::runtime::Runtime;

use crate::domain::{Job, JobId, Queue, Settings};
use crate::ipc_local::Client;
use crate::ipc_local::protocol::SubFilter;
use crate::ui::theme::ResolvedTheme;
use gui_state::Cache;

pub use components::sidebar_tree::SidebarFilter;
pub use components::table::{ColumnsState, Tab, TableSort};

const APP_TITLE: &str = "oxdm";

/// Run the per-job download window subprocess.
pub fn launch_download(id: JobId) {
    windows::download::launch(id);
}

/// Run the per-job Properties window subprocess.
pub fn launch_properties(id: JobId) {
    windows::properties::launch(id);
}

/// Ask the daemon to surface (evict + re-spawn) the per-job Properties
/// window. Mirrors the download window: one window per job, re-trigger
/// closes the old one and opens fresh.
pub fn ask_open_properties(app: &AppShell, id: JobId) {
    let s = app.client.clone();
    app.spawn(async move {
        let _ = s.open_properties_window(id).await;
    });
}

/// Run the standalone Add Download dialog subprocess. `edit_id` is
/// `Some` for the capture-review path (job already in store) and
/// `None` for a fresh manual add.
pub fn launch_add(edit_id: Option<JobId>, prefill_url: Option<String>) {
    windows::add::launch(edit_id, prefill_url);
}

/// Run the Settings window subprocess.
pub fn launch_settings(
    tab: Option<crate::ui::dialogs::settings::SettingsTab>,
    highlight_proxy: bool,
) {
    windows::settings::launch(tab, highlight_proxy);
}

/// Run the Queues & scheduling window subprocess.
pub fn launch_queues() {
    windows::queues::launch();
}

/// Run the batch-capture triage window subprocess. `staged_path` is
/// the temp JSON file the WS bridge wrote to hand off the item list.
pub fn launch_batch(staged_path: std::path::PathBuf) {
    windows::batch::launch(staged_path);
}

/// Ask the daemon to surface (or spawn) the Settings window. Goes
/// through `ipc_local` so the daemon's per-`GuiKind` registry de-dups
/// to a single live process; a re-trigger focuses the existing window
/// instead of forking a duplicate. Argv hints (`tab` / proxy
/// highlight) only apply on a fresh spawn.
pub fn ask_open_settings(app: &AppShell, tab: Option<String>, highlight_proxy: bool) {
    let s = app.client.clone();
    app.spawn(async move {
        let _ = s.open_settings_window(tab, highlight_proxy).await;
    });
}

/// Ask the daemon to surface (or spawn) the Queues window.
pub fn ask_open_queues(app: &AppShell) {
    let s = app.client.clone();
    app.spawn(async move {
        let _ = s.open_queues_window().await;
    });
}

/// Ask the daemon to surface (or spawn) the Add Download window.
/// Used by the main window's "New download" / `Ctrl+N` / `Ctrl+V`
/// paths so the dialog runs independently of the main shell — same
/// UI as the capture flow.
///
/// Reads the clipboard *here* in the parent because the daemon
/// process has no clipboard; the URL travels over IPC and lands as
/// argv on a fresh spawn (subprocess-side clipboard reads proved
/// unreliable — GTK init races with winit).
pub fn ask_open_add(app: &AppShell) {
    let prefill = clipboard::read_url_from_clipboard().map(|u| u.to_string());
    let s = app.client.clone();
    app.spawn(async move {
        let _ = s.open_add_window(None, prefill).await;
    });
}

/// Run the main GUI subprocess. Connects to the daemon, subscribes,
/// then opens the main window. Process exits when the window closes.
pub fn launch_main() {
    let rt = Runtime::new().expect("tokio runtime");
    let (client, cache) = match rt.block_on(connect_and_seed()) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "could not reach oxdm daemon");
            return;
        }
    };
    run_egui(rt, client, cache);
}

async fn connect_and_seed() -> Result<(Arc<Client>, Arc<Cache>), String> {
    let client = connect_or_spawn_daemon().await?;
    client
        .hello(crate::ipc_local::protocol::GuiKind::Main)
        .await?;
    let snap = client.snapshot().await?;
    let cache = Arc::new(Cache::from_snapshot(snap));
    Ok((client, cache))
}

/// Try to connect; if the daemon socket is absent, fork the daemon
/// process and retry. Lets `oxdm gui …` work directly from the shell
/// even when no daemon is running yet.
pub(crate) async fn connect_or_spawn_daemon() -> Result<Arc<Client>, String> {
    if let Ok(c) = Client::connect_retry(std::time::Duration::from_millis(200)).await {
        return Ok(c);
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = std::process::Command::new(&exe);
    attach_close_high_fds(&mut cmd);
    if let Err(e) = cmd.spawn() {
        return Err(format!("spawn daemon: {e}"));
    }
    Client::connect_retry(std::time::Duration::from_secs(5))
        .await
        .map_err(|e| e.to_string())
}

fn run_egui(rt: Runtime, client: Arc<Client>, cache: Arc<Cache>) {
    let saved = ui_prefs::load().window;
    let size = saved
        .map(|w| (w.width.max(820.0), w.height.max(520.0)))
        .unwrap_or((1240.0, 760.0));
    let viewport =
        crate::ui::utils::chrome::viewport_builder(APP_TITLE, size, Some((870.0, 520.0)));
    let opts = eframe::NativeOptions {
        viewport,
        // glow backend with vsync off: no first-vblank wait => faster cold start,
        // non-blocking present. egui still repaints on demand, so idle = no work.
        vsync: false,
        ..Default::default()
    };

    let client_app = client.clone();
    let cache_app = cache.clone();
    let _ = eframe::run_native(
        APP_TITLE,
        opts,
        Box::new(move |cc| {
            let ctx = cc.egui_ctx.clone();

            theme::install_fonts(&cc.egui_ctx);
            utils::icons::install_loaders(&cc.egui_ctx);

            gui_state::spawn_event_loop(
                rt.handle(),
                client_app.clone(),
                cache_app.clone(),
                SubFilter::All,
                move || ctx.request_repaint(),
            );
            theme::apply(&cc.egui_ctx, &cache_app.settings());
            let ctx_for_theme = cc.egui_ctx.clone();
            theme::on_system_theme_change(move |_| ctx_for_theme.request_repaint());
            Ok(Box::new(AppShell::new(rt, client_app, cache_app)))
        }),
    );
}

enum DbChoice {
    Exit,
    Reset,
}

/// Attach a `pre_exec` hook (Unix) that closes every fd >= 3 between
/// fork and exec. Critical for the relaunch / daemon-spawn paths: the
/// `single_instance` crate (and any other transitively-opened socket)
/// keeps abstract-UDS bindings alive as long as **any** process holds
/// a fd referencing the underlying socket struct. Inheriting those
/// fds across exec means a freshly-spawned daemon hits `AlreadyRunning`
/// even after the original owner has died, because we ourselves are
/// pinning the binding. Closing the high fds before exec breaks the
/// chain.
///
/// No-op on non-Unix platforms — Windows handles use explicit
/// `bInheritHandle = FALSE` by default.
#[allow(unused_variables)]
pub fn attach_close_high_fds(cmd: &mut std::process::Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                let _ = libc::close_range(3, libc::c_uint::MAX, libc::CLOSE_RANGE_UNSHARE as i32);
                Ok(())
            });
        }
    }
}

pub struct AppShell {
    rt: Runtime,
    pub client: Arc<Client>,
    pub cache: Arc<Cache>,
    pub selection: IndexSet<JobId>,
    pub last_clicked: Option<JobId>,
    pub filter: SidebarFilter,
    pub tab: Tab,
    pub search: String,
    pub sort: TableSort,
    pub columns: ColumnsState,
    pub menu_open: Option<components::menubar::MenuId>,
    pub sidebar_expanded: std::collections::HashSet<components::sidebar_tree::Group>,
    pub context_menu: Option<components::table::ContextMenuState>,
    pub remove: Option<dialogs::remove::RemoveRequest>,
    pub remove_state: dialogs::remove::RemoveState,
    pub host_state: dialogs::host_settings::HostState,
    pub about_state: dialogs::about::AboutState,
    pub conflict_open: bool,
    pub host_open: bool,
    pub about_open: bool,
    /// If `Some`, a queue-delete confirmation viewport is open for that
    /// queue. Confirmations only surface for queues with one or more
    /// jobs; empty queues are deleted directly.
    pub queue_delete_confirm: Option<crate::domain::QueueId>,
    pub theme_applied_for: Option<ResolvedTheme>,
    pub want_quit: bool,
    pub want_quit_daemon: bool,
    pub snap: FrameSnapshot,
    /// Borderless-resize subclass install attempted? Windows-only;
    /// stays `false` on other platforms and on install failure so we
    /// don't spam logs by retrying every frame.
    pub native_chrome_init: bool,
    /// Secrets-locked dialog state. `None` on first paint — we probe
    /// the daemon once via `Client::secrets_status` and store the
    /// resulting flag. The modal stays up until the user clicks
    /// "Wipe and continue", which calls `Client::wipe_job_secrets` to
    /// drop every encrypted column and bootstrap a fresh master key.
    pub secrets_locked: Option<bool>,
    /// DB recovery dialog state. `None` on first paint — probed once
    /// via `Client::db_status`; `Some(None)` means the daemon's store
    /// is healthy; `Some(Some(msg))` triggers the Exit / Reset modal.
    pub db_error: Option<Option<String>>,
}

#[derive(Default, Clone)]
pub struct FrameSnapshot {
    pub jobs: Vec<Job>,
    pub queues: Vec<Queue>,
    pub settings: Settings,
    pub active_queues: std::collections::HashSet<crate::domain::QueueId>,
    pub conflict_head: Option<(JobId, crate::data::ConflictKind, u64)>,
    pub conflict_len: usize,
}

impl AppShell {
    pub fn new(rt: Runtime, client: Arc<Client>, cache: Arc<Cache>) -> Self {
        // Sections default to open; presence in the set marks collapsed.
        let sidebar_expanded = std::collections::HashSet::new();
        let initial_filter = SidebarFilter::Queue(cache.main_queue_id());
        Self {
            rt,
            client,
            cache,
            selection: IndexSet::new(),
            last_clicked: None,
            filter: initial_filter,
            tab: Tab::default(),
            search: String::new(),
            sort: TableSort::default(),
            columns: ColumnsState::load(),
            menu_open: None,
            sidebar_expanded,
            context_menu: None,
            remove: None,
            remove_state: dialogs::remove::RemoveState::default(),
            host_state: dialogs::host_settings::HostState::default(),
            about_state: dialogs::about::AboutState::default(),
            conflict_open: false,
            host_open: false,
            about_open: false,
            queue_delete_confirm: None,
            theme_applied_for: None,
            want_quit: false,
            want_quit_daemon: false,
            snap: FrameSnapshot::default(),
            native_chrome_init: false,
            secrets_locked: None,
            db_error: None,
        }
    }

    pub fn rt(&self) -> &Runtime {
        &self.rt
    }

    /// Request deletion of `qid`. If the queue has any jobs, opens a
    /// confirmation dialog; otherwise deletes immediately. Built-in
    /// "Main" is silently rejected.
    pub fn request_delete_queue(&mut self, qid: crate::domain::QueueId) {
        if qid == self.cache.main_queue_id() {
            return;
        }
        let has_jobs = self
            .snap
            .queues
            .iter()
            .find(|q| q.id == qid)
            .map(|q| !q.job_ids.is_empty())
            .unwrap_or(false);
        if has_jobs {
            self.queue_delete_confirm = Some(qid);
        } else {
            let s = self.client.clone();
            self.spawn(async move {
                let _ = s.delete_queue(qid).await;
            });
            if matches!(self.filter, SidebarFilter::Queue(id) if id == qid) {
                self.filter = self.focus_after_queue_delete(qid);
            }
        }
    }

    /// Pick sidebar focus to land on after `qid` is deleted: next queue
    /// in the list, else previous, else default filter.
    pub fn focus_after_queue_delete(&self, qid: crate::domain::QueueId) -> SidebarFilter {
        let qs = &self.snap.queues;
        if let Some(idx) = qs.iter().position(|q| q.id == qid) {
            if let Some(next) = qs.get(idx + 1) {
                return SidebarFilter::Queue(next.id);
            }
            if idx > 0 {
                return SidebarFilter::Queue(qs[idx - 1].id);
            }
        }
        SidebarFilter::Queue(self.cache.main_queue_id())
    }

    /// Spawn an async task on the runtime. Fire-and-forget.
    pub fn spawn<F>(&self, f: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.rt.spawn(f);
    }

    /// Block on a short async read. Use only for cheap snapshots.
    pub fn block_on<F: std::future::Future>(&self, f: F) -> F::Output {
        self.rt.handle().block_on(f)
    }

    /// One-shot probe + dialog for the "DB broken" boot mode. Same
    /// shape as `secrets_lock_overlay`: on first paint we ask the
    /// daemon `db_status`; on a non-empty error we raise a modal with
    /// two terminal actions — Exit (quit daemon + GUI) and Reset
    /// (rename the corrupt DB aside and restart the daemon).
    fn db_error_overlay(&mut self, ctx: &egui::Context, t: &theme::Tokens) {
        if self.db_error.is_none() {
            let s = self.client.clone();
            let v = self
                .block_on(async move { s.db_status().await })
                .unwrap_or(None);
            self.db_error = Some(v);
        }
        let msg = match &self.db_error {
            Some(Some(m)) => m.clone(),
            _ => return,
        };
        let modal_frame = egui::Frame::NONE
            .fill(t.bg_surface)
            .stroke(egui::Stroke::new(t.border_width, t.border_default))
            .corner_radius(theme::surface::RADIUS)
            .inner_margin(theme::space::S4)
            .shadow(egui::epaint::Shadow {
                offset: [0, 4],
                blur: 16,
                spread: 0,
                color: egui::Color32::from_black_alpha(80),
            });
        let mut choice: Option<DbChoice> = None;
        utils::modal::show(ctx, egui::Id::new("db-broken"), modal_frame, |ui| {
            ui.set_max_width(480.0);
            ui.spacing_mut().item_spacing.y = theme::space::S2 as f32;
            ui.horizontal(|ui| {
                crate::ui::utils::icons::show(ui, "circle-alert", 22.0, t.status_danger);
                ui.add_space(theme::space::S1 as f32);
                ui.label(
                    egui::RichText::new("Database unavailable")
                        .font(theme::body_bold(14.0))
                        .color(t.fg_1),
                );
            });
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!(
                        "oxdm could not open its on-disk database, so it is running \
                             in an in-memory fallback. Nothing typed in this session will \
                             be saved.\n\n\
                             • Exit: close oxdm without touching the database file.\n\
                             • Reset: rename the broken file to a `.bak-<timestamp>` \
                             sibling, then restart so oxdm creates a fresh database. \
                             Existing downloads will not appear in the new one.\n\n\
                             Error: {msg}"
                    ))
                    .color(t.fg_2)
                    .font(theme::body(12.0)),
                )
                .wrap(),
            );
            ui.add_space(theme::space::S1 as f32);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if components::primitives::Btn::new("Reset")
                    .danger_filled()
                    .icon("trash-2")
                    .show(ui)
                    .clicked()
                {
                    choice = Some(DbChoice::Reset);
                }
                if components::primitives::Btn::new("Exit")
                    .ghost()
                    .show(ui)
                    .clicked()
                {
                    choice = Some(DbChoice::Exit);
                }
            });
        });
        match choice {
            Some(DbChoice::Exit) => {
                self.want_quit = true;
                self.want_quit_daemon = true;
            }
            Some(DbChoice::Reset) => {
                let s = self.client.clone();
                match self.block_on(async move { s.reset_database().await }) {
                    Ok(()) => {
                        // Daemon takes care of spawning a replacement
                        // daemon (which will spawn a fresh GUI) before
                        // it exits. We just close this window — the
                        // user sees the new window come up shortly
                        // after.
                        self.want_quit = true;
                        self.want_quit_daemon = false;
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "reset_database failed");
                    }
                }
            }
            None => {}
        }
    }

    /// One-shot probe + dialog for the "master key missing" boot mode.
    /// First paint: ask the daemon `secrets_status`. If `locked`, show
    /// a modal that explains the situation and offers one button —
    /// "Wipe and continue" — which calls `wipe_job_secrets` and flips
    /// the daemon back to a healthy state.
    fn secrets_lock_overlay(&mut self, ctx: &egui::Context, t: &theme::Tokens) {
        if self.secrets_locked.is_none() {
            let s = self.client.clone();
            let locked = self
                .block_on(async move { s.secrets_status().await })
                .unwrap_or(false);
            self.secrets_locked = Some(locked);
        }
        if !matches!(self.secrets_locked, Some(true)) {
            return;
        }
        let modal_frame = egui::Frame::NONE
            .fill(t.bg_surface)
            .stroke(egui::Stroke::new(t.border_width, t.border_default))
            .corner_radius(theme::surface::RADIUS)
            .inner_margin(theme::space::S4)
            .shadow(egui::epaint::Shadow {
                offset: [0, 4],
                blur: 16,
                spread: 0,
                color: egui::Color32::from_black_alpha(80),
            });
        let mut acknowledged = false;
        utils::modal::show(ctx, egui::Id::new("secrets-locked"), modal_frame, |ui| {
            ui.set_max_width(460.0);
            ui.spacing_mut().item_spacing.y = theme::space::S2 as f32;
            ui.horizontal(|ui| {
                crate::ui::utils::icons::show(ui, "triangle-alert", 22.0, t.status_warning);
                ui.add_space(theme::space::S1 as f32);
                ui.label(
                    egui::RichText::new("Encryption key missing")
                        .font(theme::body_bold(14.0))
                        .color(t.fg_1),
                );
            });
            ui.add(
                egui::Label::new(
                    egui::RichText::new(
                        "oxdm cannot find the master key for this database in the OS \
                             keyring. Without it, the encrypted password and cookie data \
                             attached to existing downloads is unreadable.\n\n\
                             Acknowledging will clear every stored Basic-auth password, \
                             proxy password, and cookie jar from the database, then \
                             generate a fresh key. Affected downloads can still run, \
                             but the server may now respond with 401 / 403 until you \
                             re-enter credentials in the Edit dialog.",
                    )
                    .color(t.fg_2)
                    .font(theme::body(12.0)),
                )
                .wrap(),
            );
            ui.add_space(theme::space::S1 as f32);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if components::primitives::Btn::new("Wipe and continue")
                    .danger_filled()
                    .icon("trash-2")
                    .show(ui)
                    .clicked()
                {
                    acknowledged = true;
                }
            });
        });
        if acknowledged {
            let s = self.client.clone();
            match self.block_on(async move { s.wipe_job_secrets().await }) {
                Ok(()) => {
                    self.secrets_locked = Some(false);
                }
                Err(e) => {
                    tracing::error!(error = %e, "wipe_job_secrets failed");
                    // Leave the modal up; user can retry.
                }
            }
        }
    }

    fn refresh_snapshot(&mut self) {
        self.snap = FrameSnapshot {
            jobs: self.cache.jobs(),
            queues: self.cache.queues(),
            settings: self.cache.settings(),
            active_queues: self.cache.active_queues(),
            conflict_head: self.cache.conflict_head(),
            conflict_len: self.cache.conflict_len(),
        };
        // The Queues window can delete a queue while we're filtered to
        // it — fall back to the default filter rather than rendering an
        // empty table tied to a missing id.
        if let SidebarFilter::Queue(qid) = self.filter
            && !self.snap.queues.iter().any(|q| q.id == qid)
        {
            self.filter = SidebarFilter::Queue(self.cache.main_queue_id());
        }
    }

    fn maybe_apply_theme(&mut self, ctx: &egui::Context) {
        let resolved = theme::resolve(self.snap.settings.theme);
        if self.theme_applied_for != Some(resolved) {
            theme::apply(ctx, &self.snap.settings);
            self.theme_applied_for = Some(resolved);
        }
    }
}

impl eframe::App for AppShell {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        utils::chrome::raw_input_hook(ctx, raw_input);
    }

    fn ui(&mut self, root_ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = &root_ui.ctx().clone();
        #[cfg(target_os = "windows")]
        if !self.native_chrome_init {
            self.native_chrome_init = true;
            // `Frame` implements `HasWindowHandle` in eframe 0.29.
            // Failure inside `install_resize_subclass` is logged and
            // swallowed; the egui overlay remains a working fallback.
            crate::ui::platform::windows::install_resize_subclass(frame);
            // Surface above the daemon / taskbar on first paint — the
            // daemon already called AllowSetForegroundWindow on our pid.
            crate::ui::platform::windows::bring_to_foreground(frame);
        }
        #[cfg(not(target_os = "windows"))]
        let _ = frame;
        if gui_state::daemon_lost() {
            tracing::info!("daemon disconnected; closing main GUI");
            std::process::exit(0);
        }
        // The daemon evicts this process (queues `Event::Close`) before
        // spawning a fresh main window on a tray re-trigger — the
        // close+reopen focus trick. Without observing it here the old
        // window would linger and a second main would appear, breaking
        // the single-instance guarantee. Save size first so the
        // replacement restores it.
        if gui_state::close_requested() {
            save_window_size(ctx);
            std::process::exit(0);
        }
        self.refresh_snapshot();
        self.maybe_apply_theme(ctx);

        if gui_state::take_focus_request() {
            gui_state::surface_window(ctx);
        }

        if self.snap.conflict_len > 0 {
            self.conflict_open = true;
        }

        if self.want_quit {
            save_window_size(ctx);
            if self.want_quit_daemon {
                let c = self.client.clone();
                let _ = self.block_on(async move { c.daemon_quit().await });
            }
            std::process::exit(0);
        }

        handle_shortcuts(self, ctx);

        crate::ui::utils::resize::show(ctx);

        let t = theme::tokens(ctx);

        // Custom titlebar (Linux/Windows borderless; macOS native bar
        // covered with same fill so the centred title still renders).
        let title_text = APP_TITLE.to_string();
        egui::Panel::top("titlebar")
            .frame(egui::Frame::NONE.fill(t.bg_titlebar))
            .show_separator_line(true)
            .show_inside(root_ui, |ui| {
                components::titlebar::show(ui, ctx, &title_text);
            });

        egui::Panel::bottom("statusbar")
            .frame(
                egui::Frame::NONE
                    .fill(t.bg_sidebar)
                    .inner_margin(egui::Margin {
                        left: theme::space::S3,
                        right: theme::space::S3,
                        top: theme::space::S1,
                        bottom: theme::space::S1,
                    }),
            )
            .show_separator_line(true)
            .show_inside(root_ui, |ui| {
                components::statusbar::ui(self, ui);
            });
        egui::Panel::left("sidebar")
            .default_size(220.0)
            .resizable(false)
            .frame(
                egui::Frame::NONE
                    .fill(t.bg_sidebar)
                    .inner_margin(egui::Margin::symmetric(theme::space::S2, theme::space::S2)),
            )
            .show_separator_line(true)
            .show_inside(root_ui, |ui| {
                components::sidebar_tree::ui(self, ui);
            });
        egui::Panel::top("toolbar")
            .frame(
                egui::Frame::NONE
                    .fill(t.bg_page)
                    .inner_margin(egui::Margin::symmetric(theme::space::S4, theme::space::S2)),
            )
            .show_separator_line(true)
            .show_inside(root_ui, |ui| {
                components::toolbar::ui(self, ui);
            });
        let central = egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(t.bg_page)
                    .inner_margin(egui::Margin::symmetric(0, theme::space::S1)),
            )
            .show_inside(root_ui, |ui| {
                components::table::ui(self, ui);
            });
        // Clicking anywhere outside the table (toolbar, sidebar, status bar)
        // also clears the selection — not just the empty space inside it.
        // The in-table empty-area click is handled by `table::ui`. Skip while
        // a context menu / popup is open so menu actions aren't disrupted.
        if !self.selection.is_empty() && !egui::Popup::is_any_open(ctx) {
            let table_rect = central.response.rect;
            let clicked_outside = ctx.input(|i| {
                i.pointer.primary_clicked()
                    && i.pointer
                        .interact_pos()
                        .is_some_and(|p| !table_rect.contains(p))
            });
            if clicked_outside {
                self.selection.clear();
                self.last_clicked = None;
            }
        }

        if self.remove.is_some() {
            dialogs::remove::show(self, ctx);
        }
        if self.host_open {
            dialogs::host_settings::show(self, ctx);
        }
        if self.about_open {
            dialogs::about::show(self, ctx);
        }
        if self.conflict_open {
            dialogs::conflict::show(self, ctx);
        }
        if self.queue_delete_confirm.is_some() {
            dialogs::queues::show_delete_confirm(self, ctx);
        }
        self.db_error_overlay(ctx, &t);
        // Skip the secrets dialog while the DB recovery modal is up —
        // the daemon is in ephemeral mode and any acknowledgement
        // would be wasted work.
        if !matches!(self.db_error, Some(Some(_))) {
            self.secrets_lock_overlay(ctx, &t);
        }

        // Closing the main window terminates the GUI process. The
        // daemon stays alive and can spawn a fresh GUI on demand.
        if ctx.input(|i| i.viewport().close_requested()) {
            save_window_size(ctx);
            self.want_quit = true;
        }
    }
}

/// Read current window size in logical px, preferring `inner_rect`
/// (real OS-reported size) and falling back to egui's screen_rect.
fn current_window_size(ctx: &egui::Context) -> Option<(f32, f32)> {
    let from_viewport = ctx.input(|i| i.viewport().inner_rect.map(|r| (r.width(), r.height())));
    if let Some(s) = from_viewport
        && s.0 > 0.0
        && s.1 > 0.0
    {
        return Some(s);
    }
    let r = ctx.content_rect();
    if r.width() > 0.0 && r.height() > 0.0 {
        Some((r.width(), r.height()))
    } else {
        None
    }
}

/// Persist current main window size. Falls back to `screen_rect` when
/// the platform doesn't report `inner_rect` (e.g. Wayland). Called
/// both on close-request and just before `process::exit` so Ctrl+Q
/// and the close button both record the size.
fn save_window_size(ctx: &egui::Context) {
    let Some((w, h)) = current_window_size(ctx) else {
        return;
    };
    ui_prefs::save_window(ui_prefs::WindowPrefs {
        width: w,
        height: h,
    });
}

/// Global keyboard shortcuts (PLAN §10.3).
fn handle_shortcuts(app: &mut AppShell, ctx: &egui::Context) {
    let editing = ctx.memory(|m| m.focused().is_some());
    let (ctrl_n, ctrl_v, ctrl_q, ctrl_alt_s) = ctx.input_mut(|i| {
        (
            i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::N,
            )),
            i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::V,
            )),
            i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::Q,
            )),
            i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND | egui::Modifiers::ALT,
                egui::Key::S,
            )),
        )
    });
    if (ctrl_n || ctrl_v) && !editing {
        ask_open_add(app);
    }
    if ctrl_alt_s {
        ask_open_settings(app, None, false);
    }
    if ctrl_q {
        app.want_quit = true;
        app.want_quit_daemon = true;
    }

    // Delete: when the sidebar is filtered to a (non-Main) queue and
    // no text widget owns focus, route the keypress to the queue
    // delete-confirm flow.
    if !editing
        && let SidebarFilter::Queue(qid) = app.filter
        && qid != app.cache.main_queue_id()
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete))
    {
        app.request_delete_queue(qid);
    }
}

// (Step 5: ui_signals replaced by direct calls through
// `client.open_download_window` / `Event::Focus`.)
#[allow(dead_code)]
mod ui_signals {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;

    use crate::domain::JobId;
    use crate::ipc_local::Client;
    use crate::ipc_local::protocol::{Event, SubFilter};
    use crate::ui::AppShell;

    pub enum UiSignal {
        OpenDownloadDialog(JobId),
        SpawnDownloadProcess(JobId),
    }

    static BUS: OnceLock<(Sender<UiSignal>, Mutex<Receiver<UiSignal>>)> = OnceLock::new();
    static INSTALLED: AtomicBool = AtomicBool::new(false);

    fn bus() -> &'static (Sender<UiSignal>, Mutex<Receiver<UiSignal>>) {
        BUS.get_or_init(|| {
            let (tx, rx) = channel();
            (tx, Mutex::new(rx))
        })
    }

    /// Open a *separate* IPC connection scoped to `Lifecycle` events
    /// and forward the ones the UI shell needs to act on directly
    /// (currently `OpenDownloadDialog`). Run once per process.
    pub fn install(rt: tokio::runtime::Handle, ctx: eframe::egui::Context) {
        if INSTALLED.swap(true, Ordering::SeqCst) {
            return;
        }
        let _g = rt.enter();
        tokio::spawn(async move {
            let client = match Client::connect_retry(Duration::from_secs(3)).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(error = %e, "ui_signals connect failed");
                    return;
                }
            };
            if let Err(e) = client.subscribe(SubFilter::Lifecycle).await {
                tracing::warn!(error = %e, "ui_signals subscribe failed");
                return;
            }
            let Some(mut rx) = client.take_events().await else {
                return;
            };
            let tx = bus().0.clone();
            while let Some(ev) = rx.recv().await {
                match ev {
                    Event::OpenDownloadDialog(id) => {
                        let _ = tx.send(UiSignal::OpenDownloadDialog(id));
                        ctx.request_repaint();
                    }
                    Event::ShowMainWindow => {
                        // The daemon spawned us already if we're
                        // running; nothing else to do. (Future:
                        // raise the existing window.)
                    }
                    _ => {}
                }
            }
        });
    }

    pub fn drain(_app: &mut AppShell) {
        if let Ok(rx) = bus().1.lock() {
            while let Ok(sig) = rx.try_recv() {
                match sig {
                    UiSignal::OpenDownloadDialog(id) | UiSignal::SpawnDownloadProcess(id) => {
                        // Per the design, every download window is its
                        // own process — independently lives even after
                        // the main window closes.
                        spawn_download_subprocess(id);
                    }
                }
            }
        }
    }

    fn spawn_download_subprocess(id: JobId) {
        let exe = match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "current_exe");
                return;
            }
        };
        if let Err(e) = std::process::Command::new(&exe)
            .args(["gui", "download", &id.to_string()])
            .spawn()
        {
            tracing::warn!(error = %e, "spawn download gui failed");
        }
    }

    pub fn enqueue(sig: UiSignal) {
        let _ = bus().0.send(sig);
    }
}
