//! Tray facade. ksni on Linux, tray-icon on win/mac.

use std::sync::{Arc, Mutex, OnceLock};

use crate::platform::attach_close_high_fds;

use tokio::runtime::Handle;

use crate::data::AppState;
use crate::domain::JobId;

#[cfg(target_os = "linux")]
mod ksni_backend;
#[cfg(not(target_os = "linux"))]
mod tray_icon_backend;

pub fn install(rt: Handle, state: Arc<AppState>) {
    #[cfg(target_os = "linux")]
    ksni_backend::install(rt, state);
    #[cfg(not(target_os = "linux"))]
    tray_icon_backend::install(rt, state);
}

pub(super) fn label_for(job: &crate::domain::Job) -> String {
    let name = job
        .filename
        .clone()
        .unwrap_or_else(|| job.url.path().rsplit('/').next().unwrap_or("").to_string());
    let phase = match job.status.phase {
        crate::domain::Phase::Downloading => "downloading",
        crate::domain::Phase::Evaluating => "evaluating",
        crate::domain::Phase::Paused => "paused",
        _ => "active",
    };
    let trimmed = if name.chars().count() > 32 {
        let cut: String = name.chars().take(31).collect();
        format!("{cut}…")
    } else {
        name
    };
    format!("{trimmed}  ·  {phase}")
}

/// Per-kind spawn lifecycle. Driven entirely by observable child
/// events — no timer fallback, no debounce window.
///
/// - `Spawning` — a subprocess of this kind has been forked but has
///   not yet sent `Hello`. Re-triggers are blocked while in this state.
/// - `Registered` — the child has identified itself via `Hello`.
///   Re-triggers proceed (evict+spawn for every kind).
///
/// `pending` and `children` live under a single `Mutex<State>` so that
/// "decide whether to spawn" + "fork the child" + "record the child"
/// is one atomic critical section. Splitting them across two locks
/// (the previous design) opened a race where two parallel triggers
/// could both decide to spawn: thread A set `Spawning` and released
/// the pending lock; thread B then ran the reap path and saw no
/// alive child of A's kind (A had not yet pushed) → wiped A's
/// `Spawning` → both threads forked.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SpawnState {
    Spawning,
    Registered,
}

struct ChildEntry {
    kind: crate::ipc_local::protocol::GuiKind,
    child: std::process::Child,
}

struct State {
    pending: std::collections::HashMap<crate::ipc_local::protocol::GuiKind, SpawnState>,
    children: Vec<ChildEntry>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(State {
            pending: std::collections::HashMap::new(),
            children: Vec::new(),
        })
    })
}

/// Reap any exited children inside an already-held lock, then wipe
/// `pending` entries whose kind has no alive child. Auto-recovers
/// crashed children: a child of kind K dies pre-`Hello`, the next
/// click reaps it, sees no alive K, removes the stale `Spawning`,
/// fresh spawn proceeds. During an eviction the predecessor and
/// replacement coexist briefly; the replacement keeps the kind in
/// `alive_kinds`, so the predecessor's exit can't wipe the
/// in-flight `Spawning`.
fn reap_locked(s: &mut State) {
    s.children
        .retain_mut(|entry| !matches!(entry.child.try_wait(), Ok(Some(_))));
    let alive: std::collections::HashSet<crate::ipc_local::protocol::GuiKind> =
        s.children.iter().map(|e| e.kind).collect();
    s.pending.retain(|kind, _| alive.contains(kind));
}

/// Atomic "claim Spawning + fork + record". Returns `true` if a child
/// was actually spawned. Holds the global state lock across the whole
/// operation so no parallel trigger can race the gap between marking
/// `Spawning` and pushing the `Child`.
fn try_spawn(kind: crate::ipc_local::protocol::GuiKind, args: &[&str]) -> bool {
    let Ok(mut s) = state().lock() else {
        return false;
    };
    reap_locked(&mut s);
    if matches!(s.pending.get(&kind), Some(SpawnState::Spawning)) {
        return false;
    }
    let exe = match crate::platform::current_exe() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "current_exe");
            return false;
        }
    };
    s.pending.insert(kind, SpawnState::Spawning);
    let mut cmd = std::process::Command::new(&exe);
    cmd.args(args);
    attach_close_high_fds(&mut cmd);
    match cmd.spawn() {
        Ok(child) => {
            #[cfg(target_os = "windows")]
            allow_child_foreground(child.id());
            s.children.push(ChildEntry { kind, child });
            true
        }
        Err(e) => {
            tracing::warn!(error = %e, exe = %exe.display(), args = ?args, "spawn gui failed");
            s.pending.remove(&kind);
            false
        }
    }
}

/// Promote the entry to `Registered`. Called from the IPC server when
/// a connection sends `Hello(kind)`.
pub fn mark_registered(kind: crate::ipc_local::protocol::GuiKind) {
    if let Ok(mut s) = state().lock() {
        s.pending.insert(kind, SpawnState::Registered);
    }
}

/// Drop the entry on a clean disconnect — but only when the current
/// state is `Registered`. Called from the IPC server's connection
/// `Drop`. Skipping when state is `Spawning` is critical: during an
/// `evict_and_spawn` the predecessor's `Drop` races the replacement's
/// fork; if it cleared the in-flight `Spawning`, a rapid follow-up
/// click would slip past the guard and spawn a third child.
pub fn clear_pending(kind: crate::ipc_local::protocol::GuiKind) {
    if let Ok(mut s) = state().lock()
        && matches!(s.pending.get(&kind), Some(SpawnState::Registered))
    {
        s.pending.remove(&kind);
    }
}

/// Surface the main GUI: like every other window, evict an existing
/// process and spawn a fresh top-level so the new window reliably lands
/// on top.
///
/// `try_focus` (surface_window's `AlwaysOnTop` pulse) proved unreliable
/// on Linux/XDG for a re-triggered main window — the pulse toggles the
/// stacking order but the WM's focus-stealing prevention keeps the
/// window from actually grabbing focus. A fresh window spawned in
/// response to the direct tray click carries the user activation the WM
/// honours, so close+reopen is the only path that consistently focuses.
/// Trade-off: transient UI state (sidebar selection, scroll) resets;
/// window size/position survive via `save_window_size` on close.
pub fn spawn_main_gui() {
    evict_and_spawn(crate::ipc_local::protocol::GuiKind::Main, &["gui", "main"]);
}

/// Centralised "evict-and-respawn" single-instance helper for every
/// non-main standalone window.
///
/// `ViewportCommand::Focus` is unreliable across Linux window managers
/// (Wayland focus-stealing prevention, X11 stacking quirks), so the
/// only reliable way to surface a re-triggered window is to evict
/// the existing process and spawn a brand-new top-level — fresh
/// windows naturally land on top. `try_close` queues `Event::Close`
/// on the existing connection; the shell observes the flag and
/// `process::exit`s. Trade-off: in-progress UI state in the existing
/// window (download chart, settings form, queues editor) is lost on
/// re-trigger.
///
/// The `Spawning` flag stops rapid double-clicks from piling up
/// processes in the gap between fork and the child's `Hello`. It
/// auto-clears when the child registers (`mark_registered`), on a
/// clean disconnect (`clear_pending`), or when the watchdog reaping
/// inside `try_spawn` notices the child crashed pre-`Hello`.
/// No timer involved — the state machine is driven by observable
/// child events only.
fn evict_and_spawn(kind: crate::ipc_local::protocol::GuiKind, args: &[&str]) {
    // `try_close` lives in the IPC server (different lock); calling it
    // before `try_spawn` is safe because `try_spawn` will refuse if a
    // spawn is already in flight, so even if this thread loses the
    // race to a parallel trigger we don't fork twice.
    crate::ipc_local::server::try_close(kind);
    try_spawn(kind, args);
}

/// Per-job download window. Always evict + spawn (see `evict_and_spawn`).
pub fn spawn_download_gui(id: JobId) {
    let id_str = id.to_string();
    evict_and_spawn(
        crate::ipc_local::protocol::GuiKind::Download(id),
        &["gui", "download", &id_str],
    );
}

/// Per-job Properties window. Always evict + spawn, like the download
/// window — re-triggering "Show Properties" closes any existing window
/// for the job and opens a fresh one (single instance per job).
pub fn spawn_properties_gui(id: JobId) {
    let id_str = id.to_string();
    evict_and_spawn(
        crate::ipc_local::protocol::GuiKind::Properties(id),
        &["gui", "properties", &id_str],
    );
}

/// Add Download window. `prefill_url` is the clipboard URL the caller
/// already resolved (daemon has no clipboard); `edit_id` carries the
/// capture-review path. Argv hints only apply to the fresh spawn.
pub fn spawn_add_gui(edit_id: Option<JobId>, prefill_url: Option<&str>) {
    let edit = edit_id.map(|id| id.to_string());
    let mut args: Vec<&str> = vec!["gui", "add"];
    if let Some(s) = edit.as_deref() {
        args.push(s);
    }
    if let Some(u) = prefill_url {
        args.push("--url");
        args.push(u);
    }
    evict_and_spawn(crate::ipc_local::protocol::GuiKind::Add, &args);
}

/// Batch-capture triage window. `staged_path` points at a JSON file
/// the WS bridge wrote in the user's temp dir; the dialog subprocess
/// reads + deletes it on launch (see `ipc::batch`).
pub fn spawn_batch_gui(staged_path: &std::path::Path) {
    let p = staged_path.to_string_lossy().to_string();
    evict_and_spawn(
        crate::ipc_local::protocol::GuiKind::Batch,
        &["gui", "batch", &p],
    );
}

/// Settings window.
pub fn spawn_settings_gui(tab: Option<&str>, highlight_proxy: bool) {
    let mut args: Vec<&str> = vec!["gui", "settings"];
    if let Some(t) = tab {
        args.push("--tab");
        args.push(t);
    }
    if highlight_proxy {
        args.push("--highlight-proxy");
    }
    evict_and_spawn(crate::ipc_local::protocol::GuiKind::Settings, &args);
}

/// Queues & scheduling window.
pub fn spawn_queues_gui() {
    evict_and_spawn(
        crate::ipc_local::protocol::GuiKind::Queues,
        &["gui", "queues"],
    );
}

/// Shutdown/sleep grace-countdown window. Spawned by the daemon's
/// power-prompt listener when a destructive power action arms.
pub fn spawn_power_gui() {
    evict_and_spawn(
        crate::ipc_local::protocol::GuiKind::Power,
        &["gui", "power"],
    );
}

/// Grant a freshly spawned GUI subprocess permission to call
/// `SetForegroundWindow`. Without this, Windows' focus-stealing
/// prevention forces the new window to the background and only flashes
/// the taskbar button — the daemon process is itself non-foreground, so
/// its children inherit no foreground rights.
#[cfg(target_os = "windows")]
fn allow_child_foreground(pid: u32) {
    // SAFETY: FFI call with no preconditions; failures (e.g. the child
    // already exited, or this process lacks foreground rights) are
    // harmless and the user can still click the taskbar entry.
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(pid);
    }
}

/// Reap any GUI children that already exited; SIGKILL the rest.
fn kill_living_gui_children() {
    let Ok(mut s) = state().lock() else {
        return;
    };
    for entry in s.children.iter_mut() {
        match entry.child.try_wait() {
            Ok(Some(_)) => {} // already exited
            _ => {
                if let Err(e) = entry.child.kill() {
                    tracing::debug!(pid = entry.child.id(), error = %e, "kill gui child");
                }
                let _ = entry.child.wait();
            }
        }
    }
    s.children.clear();
    s.pending.clear();
}

/// Orderly shutdown of the daemon.
///
/// Sequence:
///   1. `pause_all` — gracefully halts every active runner so partial
///      files + metadata are flushed and resumable next launch.
///   2. Drop IPC connections so every live GUI subprocess detects
///      `daemon_lost` and exits.
///   3. `process::exit(0)`.
///
/// Steps 1-2 happen on the tokio runtime so we don't block the tray
/// callback thread; the brief sleep before exit gives the GUIs a
/// chance to wind down on their own.
pub fn quit_daemon(rt: &tokio::runtime::Handle, state: &Arc<AppState>) {
    let s = state.clone();
    // Tray callbacks fire on the tray-owner thread, which is *not* a
    // tokio runtime context, so bare `tokio::spawn` would panic and
    // muda silently swallows the panic — leaving Quit a no-op. Use the
    // explicit handle the daemon passed into the tray.
    rt.spawn(async move {
        s.pause_all().await;
        // pause_all returns once each runner has acknowledged the
        // pause; LiveCounters + .part files are now in a consistent
        // on-disk state. cancel_all_runners is a belt-and-braces
        // catch for runners that didn't observe the pause in time.
        s.cancel_all_runners();
        // Brief grace so GUIs see the IPC stream close and exit on
        // their own (`gui_state::daemon_lost` poll).
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        // Anything still alive gets SIGKILL'd so the user never ends
        // up with orphan oxdm windows after picking Quit.
        kill_living_gui_children();
        std::process::exit(0);
    });
}
