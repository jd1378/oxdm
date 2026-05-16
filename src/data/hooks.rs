//! Queue hook executor.
//!
//! Subscribes to `DomainEvent::QueueStarted` / `QueueFinished` and runs
//! the corresponding `QueueHook` actions. Real platform integrations
//! (shutdown, sleep, hibernate) are stubbed for now and just log; they
//! land in step 7 of PLAN §10.12.

use std::sync::Arc;

use crate::data::events::DomainEvent;
use crate::data::state::AppState;
use crate::domain::{QueueHook, QueueId};

pub fn spawn(state: Arc<AppState>) {
    let mut rx = state.subscribe();
    tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            match event {
                DomainEvent::QueueStarted { id } => run_hooks(&state, id, HookPhase::Start).await,
                DomainEvent::QueueFinished { id } => run_hooks(&state, id, HookPhase::Finish).await,
                _ => {}
            }
        }
    });
}

#[derive(Copy, Clone)]
enum HookPhase {
    Start,
    Finish,
}

async fn run_hooks(state: &AppState, id: QueueId, when: HookPhase) {
    let queue = match state.queue(id).await {
        Some(q) => q,
        None => return,
    };
    let hooks = match when {
        HookPhase::Start => &queue.on_start,
        HookPhase::Finish => &queue.on_finish,
    };
    for hook in hooks {
        if let Err(e) = execute(hook).await {
            tracing::warn!(queue = %queue.name, error = %e, "queue hook failed");
        }
    }
}

async fn execute(hook: &QueueHook) -> Result<(), String> {
    match hook {
        QueueHook::Notify { title, body } => {
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            {
                let _ = notify_rust::Notification::new()
                    .summary(title)
                    .body(body)
                    .show();
            }
            #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
            {
                let _ = (title, body);
            }
            Ok(())
        }
        QueueHook::RunCommand { cmd, args } => {
            tokio::process::Command::new(cmd)
                .args(args)
                .spawn()
                .map_err(|e| e.to_string())?;
            Ok(())
        }
        QueueHook::Shutdown(action) => power_action(*action),
        QueueHook::Sleep => sleep_action(),
        QueueHook::Hibernate => hibernate_action(),
        QueueHook::ExitOxdm => {
            // Best-effort: signal the process to terminate. Tray Quit
            // path triggers the orderly cleanup; here we just exit
            // since hooks fire from a tokio task without main-window
            // access. Per-job cancel tokens get tripped by the OS
            // process exit; downloaded `.part` files remain so a
            // restart can resume.
            std::process::exit(0);
        }
    }
}

fn power_action(action: crate::domain::ShutdownAction) -> Result<(), String> {
    use crate::domain::ShutdownAction::*;
    #[cfg(target_os = "linux")]
    let cmd_args: (&str, &[&str]) = match action {
        ShutDown => ("systemctl", &["poweroff"]),
        Restart => ("systemctl", &["reboot"]),
        Sleep => ("systemctl", &["suspend"]),
    };
    #[cfg(target_os = "macos")]
    let cmd_args: (&str, &[&str]) = match action {
        ShutDown => (
            "osascript",
            &["-e", "tell app \"System Events\" to shut down"],
        ),
        Restart => (
            "osascript",
            &["-e", "tell app \"System Events\" to restart"],
        ),
        Sleep => ("pmset", &["sleepnow"]),
    };
    #[cfg(target_os = "windows")]
    let cmd_args: (&str, &[&str]) = match action {
        ShutDown => ("shutdown", &["/s", "/t", "0"]),
        Restart => ("shutdown", &["/r", "/t", "0"]),
        Sleep => ("rundll32.exe", &["powrprof.dll,SetSuspendState", "0,1,0"]),
    };
    spawn_detached(cmd_args.0, cmd_args.1)
}

fn sleep_action() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    return spawn_detached("systemctl", &["suspend"]);
    #[cfg(target_os = "macos")]
    return spawn_detached("pmset", &["sleepnow"]);
    #[cfg(target_os = "windows")]
    return spawn_detached("rundll32.exe", &["powrprof.dll,SetSuspendState", "0,1,0"]);
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    return Err("sleep not supported on this platform".into());
}

fn hibernate_action() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    return spawn_detached("systemctl", &["hibernate"]);
    #[cfg(target_os = "macos")]
    return Err("hibernate not supported on macOS — use sleep".into());
    #[cfg(target_os = "windows")]
    return spawn_detached("shutdown", &["/h"]);
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    return Err("hibernate not supported on this platform".into());
}

fn spawn_detached(cmd: &str, args: &[&str]) -> Result<(), String> {
    std::process::Command::new(cmd)
        .args(args)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("{}: {}", cmd, e))
}
