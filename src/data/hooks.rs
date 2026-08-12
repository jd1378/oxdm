//! Queue hook executor.
//!
//! Subscribes to `DomainEvent::QueueStarted` / `QueueFinished` and runs
//! the corresponding `QueueHook` actions. Destructive power actions
//! (shutdown / restart / sleep / hibernate) don't fire immediately —
//! they arm the shared `AppState` grace timer (feature #9), giving the
//! user a cancellable countdown.

use std::sync::Arc;

use crate::data::events::DomainEvent;
use crate::data::state::AppState;
use crate::domain::{PowerAction, QueueHook, QueueId};

pub fn spawn(state: Arc<AppState>) {
    let mut rx = state.subscribe();
    tokio::spawn(async move {
        while let Some(event) = crate::data::next_event(&mut rx, "queue hooks").await {
            match event {
                DomainEvent::QueueStarted { id } => run_hooks(&state, id, HookPhase::Start).await,
                DomainEvent::QueueFinished {
                    id,
                    completed,
                    failed,
                    needs_answer,
                } => {
                    run_hooks(
                        &state,
                        id,
                        HookPhase::Finish {
                            completed,
                            failed,
                            needs_answer,
                        },
                    )
                    .await
                }
                _ => {}
            }
        }
    });
}

#[derive(Copy, Clone)]
enum HookPhase {
    Start,
    /// Carries the run's outcome so a Notify hook can say what
    /// happened rather than repeat its own title.
    Finish {
        completed: u32,
        failed: u32,
        needs_answer: u32,
    },
}

async fn run_hooks(state: &AppState, id: QueueId, when: HookPhase) {
    let queue = match state.queue(id).await {
        Some(q) => q,
        None => return,
    };
    let hooks = match when {
        HookPhase::Start => &queue.on_start,
        HookPhase::Finish { .. } => &queue.on_finish,
    };
    for hook in hooks {
        if let Err(e) = execute(state, hook, &queue, when).await {
            tracing::warn!(queue = %queue.name, error = %e, "queue hook failed");
        }
    }
}

async fn execute(
    state: &AppState,
    hook: &QueueHook,
    queue: &crate::domain::Queue,
    when: HookPhase,
) -> Result<(), String> {
    match hook {
        QueueHook::Notify { title, body } => {
            // The finish notification describes the run, so neither
            // stored string survives: they were written when the hook
            // was created and cannot know how the run went. The title
            // names the queue because that is what a collapsed or
            // truncated notification keeps.
            let (title, body) = match when {
                HookPhase::Finish {
                    completed,
                    failed,
                    needs_answer,
                } => (
                    crate::domain::finish_title(&queue.name),
                    crate::domain::finish_summary(completed, failed, needs_answer),
                ),
                HookPhase::Start => (title.clone(), body.clone()),
            };
            // Logged like the per-download notifier's: delivery is
            // fire-and-forget, and this is the only report a queue run
            // gets, so "the desktop dropped it" has to be tellable
            // from "it never said anything".
            tracing::debug!(%title, %body, "queue notification");
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            {
                crate::platform::show_notification(title, body);
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
        QueueHook::Shutdown(action) => {
            let action = *action;
            state.arm_power_action(action.into(), move || power_action(action));
            Ok(())
        }
        QueueHook::Sleep => {
            state.arm_power_action(PowerAction::Sleep, sleep_action);
            Ok(())
        }
        QueueHook::Hibernate => {
            state.arm_power_action(PowerAction::Hibernate, hibernate_action);
            Ok(())
        }
        QueueHook::ExitOxdm => {
            // Exiting would kill the daemon-side grace task and
            // silently drop a promised power action — skip, like the
            // per-job completion path does.
            if state.pending_shutdown().is_some() {
                tracing::warn!("skipping exit-oxdm hook while a power action is pending");
                return Ok(());
            }
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
    return Err("hibernate not supported on macOS; use sleep".into());
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
