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
pub mod notifications;
pub mod tray;

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
    notifications::spawn(state.clone());
    completion_actions::spawn(state.clone());
    crate::data::spawn_hook_executor(state.clone());
    crate::data::spawn_queue_scheduler(state.clone());
    tray::install(rt.handle().clone(), state.clone());

    // Graceful Ctrl-C / SIGTERM: route through the same Quit path the
    // tray uses so in-flight jobs hit `pause_all` (and thereby
    // `persist_job`) before the process exits. Without this, killing
    // the daemon from the terminal drops any unpersisted progress.
    {
        let s = state.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                tray::quit_daemon(&tokio::runtime::Handle::current(), &s);
            }
        });
    }
    #[cfg(unix)]
    {
        let s = state.clone();
        tokio::spawn(async move {
            use tokio::signal::unix::{SignalKind, signal};
            if let Ok(mut sig) = signal(SignalKind::terminate())
                && sig.recv().await.is_some()
            {
                tray::quit_daemon(&tokio::runtime::Handle::current(), &s);
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
