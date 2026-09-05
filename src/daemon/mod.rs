//! Headless daemon entry.
//!
//! Owns the `AppState`, runs all background workers (queue scheduler,
//! hook executor, browser-extension IPC, tray, notifications,
//! completion actions, daemon-↔-GUI IPC server). No window, no eframe.
//!
//! GUI subprocesses connect over `ipc_local`; the tray spawns them on
//! demand. Process exits via `Request::DaemonQuit` from a GUI or by a
//! tray "Quit" click.

use std::sync::Arc;

use tokio::runtime::Runtime;

use crate::data::AppState;
use crate::ipc_local;
use crate::single_instance::InstanceGuard;

pub mod completion_actions;
pub mod environment_guard;
pub mod notifications;
pub mod tray;
pub mod update_alerts;

pub fn run() {
    run_inner(None, false);
}

pub fn run_with_instance(guard: InstanceGuard) {
    run_inner(Some(guard), false);
}

pub fn run_with_instance_tray(guard: InstanceGuard) {
    run_inner(Some(guard), true);
}

pub fn run_tray() {
    run_inner(None, true);
}

fn run_inner(guard: Option<InstanceGuard>, force_tray: bool) {
    install_panic_hook();
    let rt = Runtime::new().expect("tokio runtime");
    let state = rt.block_on(AppState::load());
    spawn_workers(&rt, state.clone(), guard, force_tray);

    // Block forever — the per-task spawns above keep the runtime busy.
    // `Request::DaemonQuit` calls `process::exit` from inside the IPC
    // handler so we don't need a graceful shutdown channel here.
    rt.block_on(std::future::pending::<()>());
}

fn spawn_workers(
    rt: &Runtime,
    state: Arc<AppState>,
    guard: Option<InstanceGuard>,
    force_tray: bool,
) {
    let _g = rt.enter();
    // Cookies a previous run staged for a batch dialog that never
    // consumed them. Before anything can stage a new one.
    crate::ipc::staged::sweep_stale();
    // Programs an update had to rename aside because they were running
    // at the time. Nothing holds them now.
    if let Ok(exe) = crate::platform::current_exe() {
        crate::data::update_bundle::sweep_displaced(&exe);
    }
    {
        // A hash check the last run did not finish. Nothing waits on
        // it, and there is normally none — the marker only survives a
        // daemon that exited mid-check.
        let s = state.clone();
        tokio::spawn(async move {
            s.resume_pending_verifications().await;
        });
    }
    {
        let s = state.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::ipc::serve(s).await {
                tracing::error!(error = %e, "ipc server stopped");
            }
        });
    }
    {
        let s = state.clone();
        tokio::spawn(async move {
            if let Err(e) = ipc_local::serve(s).await {
                tracing::error!(error = %e, "ipc_local server stopped");
            }
        });
    }
    crate::ipc::manifest_check::spawn();
    spawn_autostart_check(state.clone());
    notifications::spawn(state.clone());
    environment_guard::spawn(state.clone());
    spawn_power_prompt(state.clone());
    completion_actions::spawn(state.clone());
    crate::data::spawn_hook_executor(state.clone());
    // One idle reading for the whole daemon: the queue scheduler and
    // the update checker both act on whether the user is here, and two
    // probes could answer differently in the same second.
    // Awaited: whether this host can report idleness at all decides
    // whether the queue builder offers the condition, and the first
    // window can open before a background probe would have answered.
    let idle = rt.block_on(crate::data::idle::spawn());
    state.attach_idle_watch(idle.clone());
    // One capability probe for the whole daemon: what the scheduler
    // decides on and what the queue builder offers have to be the same
    // list, or the UI offers a condition nothing can evaluate.
    let support = rt.block_on(crate::data::conditions::detect_support(idle.supported()));
    state.attach_cond_support(support);
    crate::data::spawn_queue_scheduler(state.clone(), idle.clone());
    // A build a package manager owns never looks for a new version:
    // there is nothing it could do with one, and the check would be a
    // weekly request on behalf of a feature that is not there.
    if crate::domain::SELF_UPDATE {
        crate::data::spawn_update_watch(state.clone(), idle);
        update_alerts::spawn(state.clone());
    }
    crate::data::spawn_file_watch(state.clone());
    tray::install(rt.handle().clone(), state.clone());

    // Graceful Ctrl-C / SIGTERM: route through the same Quit path the
    // tray uses so in-flight jobs hit `pause_all` (and thereby
    // `persist_job`) before the process exits. Without this, killing
    // the daemon from the terminal drops any unpersisted progress.
    {
        let s = state.clone();
        tokio::spawn(async move {
            // Looped, not awaited once: a handler that finishes after
            // the first signal hands the second back to the default
            // disposition, which kills the process outright — the one
            // moment that must not happen is while a final file is
            // being assembled. Repeats are dropped by `begin_exit`.
            while tokio::signal::ctrl_c().await.is_ok() {
                tray::quit_daemon(&tokio::runtime::Handle::current(), &s);
            }
        });
    }
    #[cfg(unix)]
    {
        let s = state.clone();
        tokio::spawn(async move {
            use tokio::signal::unix::{SignalKind, signal};
            if let Ok(mut sig) = signal(SignalKind::terminate()) {
                while sig.recv().await.is_some() {
                    tray::quit_daemon(&tokio::runtime::Handle::current(), &s);
                }
            }
        });
    }

    // Surface the main window on first launch unless the user opted
    // into start-to-tray. Subsequent launches go through
    // `ask_daemon_to_open_main` in `main.rs`.
    let settings = rt.block_on(state.settings());
    if !force_tray && !settings.start_to_tray {
        tray::spawn_main_gui();
    }

    if let Some(guard) = guard
        && let Err(e) = guard.spawn_listener(state.clone())
    {
        tracing::warn!(error = %e, "single-instance listener");
    }
}

/// Log panics through `tracing` instead of only to stderr.
///
/// A panic in a spawned task takes that task down and nothing else —
/// the daemon carries on with one worker silently missing. Whatever
/// else that costs, the user's log should at least say it happened, and
/// where.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic>".to_owned());
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown location>".to_owned());
        let thread = std::thread::current()
            .name()
            .unwrap_or("<unnamed>")
            .to_owned();
        tracing::error!(%payload, %location, %thread, "panic");
        previous(info);
    }));
}

/// Surface the grace-countdown window whenever a destructive power
/// action arms. The window itself handles Cancel / Confirm-now and
/// closes on `ShutdownCancelled` or when the deadline passes.
/// Keep the login entry pointing at this binary, the way
/// [`crate::ipc::manifest_check`] keeps the browser manifests pointing
/// at it. Moving oxdm elsewhere otherwise leaves the setting reading
/// "on" while nothing starts at login.
fn spawn_autostart_check(state: std::sync::Arc<crate::data::AppState>) {
    // A sandboxed or development copy shares the user's login session
    // but is not the app they asked to start with it.
    if std::env::var_os("OXDM_INSTANCE_SUFFIX").is_some_and(|v| !v.is_empty()) {
        return;
    }
    tokio::spawn(async move {
        let want = state.settings().await.start_at_login;
        let _ = tokio::task::spawn_blocking(move || crate::platform::refresh_autostart(want)).await;
    });
}

fn spawn_power_prompt(state: std::sync::Arc<crate::data::AppState>) {
    tokio::spawn(async move {
        let mut rx = state.subscribe();
        while let Some(ev) = crate::data::next_event(&mut rx, "power prompt").await {
            if matches!(ev, crate::data::DomainEvent::ShutdownPending { .. }) {
                tray::spawn_power_gui();
            }
        }
    });
}
