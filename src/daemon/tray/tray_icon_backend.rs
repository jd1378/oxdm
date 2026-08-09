//! Tray for Windows / macOS via tray-icon + muda.
//!
//! `TrayIcon` is `!Send` on Windows (wraps `Rc<RefCell<…>>`), so it
//! cannot live behind `Arc<Mutex<…>>` shared across tokio tasks. We
//! pin it to a dedicated owner thread that drains menu/tray events
//! and applies rebuilds. A small tokio task watches `AppState` and
//! forwards the latest job list over an `std::sync::mpsc` channel.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc;

use muda::{Menu, MenuId, MenuItem, PredefinedMenuItem};
use tokio::runtime::Handle;
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use super::{label_for, quit_daemon, spawn_download_gui, spawn_main_gui, spawn_settings_gui};
use crate::data::AppState;
use crate::domain::{Job, JobId, Phase};

#[derive(Default, Clone)]
struct ActionIds {
    open: Option<MenuId>,
    settings: Option<MenuId>,
    pause_all: Option<MenuId>,
    resume_all: Option<MenuId>,
    quit: Option<MenuId>,
}

pub fn install(rt: Handle, state: Arc<AppState>) {
    let (jobs_tx, jobs_rx) = mpsc::channel::<Vec<Job>>();

    // State watcher: forwards job snapshots to the owner thread.
    {
        let _g = rt.enter();
        let state_loop = state.clone();
        let tx = jobs_tx.clone();
        tokio::spawn(async move {
            let mut rx = state_loop.subscribe();
            loop {
                let jobs = state_loop.list_jobs().await;
                if tx.send(jobs).is_err() {
                    break;
                }
                if rx.recv().await.is_err() {
                    break;
                }
            }
        });
    }

    // Owner thread: builds the tray, rebuilds menu, drains events.
    let rt_handle = rt.clone();
    let state_owner = state.clone();
    std::thread::Builder::new()
        .name("tray-icon".into())
        .spawn(move || run_owner(rt_handle, state_owner, jobs_rx))
        .expect("spawn tray thread");
}

fn run_owner(rt: Handle, state: Arc<AppState>, jobs_rx: mpsc::Receiver<Vec<Job>>) {
    let icon = crate::gui::app_icon::tray_icon_normal(crate::gui::theme::system_theme());
    let menu = Menu::new();
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        // Left click should open the main window, not the menu. Menu
        // stays accessible via right click. tray-icon defaults to
        // showing the menu on both buttons.
        .with_menu_on_left_click(false)
        .with_icon(
            icon.unwrap_or_else(|| tray_icon::Icon::from_rgba(vec![0, 0, 0, 0], 1, 1).unwrap()),
        )
        .build();
    let tray = match tray {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(error = %e, "tray-icon build failed");
            return;
        }
    };
    let mut actions = ActionIds::default();
    let mut dyn_map: HashMap<MenuId, JobId> = HashMap::new();

    let mut last_jobs: Vec<Job> = Vec::new();
    let mut last_theme = crate::gui::theme::system_theme();
    rebuild_now(
        &tray,
        &mut actions,
        &mut dyn_map,
        &last_jobs,
        last_theme,
        state.is_exiting(),
    );

    let menu_chan = muda::MenuEvent::receiver();
    let tray_chan = TrayIconEvent::receiver();
    loop {
        // Coalesce rebuilds: only apply the most recent snapshot.
        let mut latest: Option<Vec<Job>> = None;
        loop {
            match jobs_rx.try_recv() {
                Ok(j) => latest = Some(j),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        }
        if let Some(jobs) = latest {
            last_jobs = jobs;
            rebuild_now(
                &tray,
                &mut actions,
                &mut dyn_map,
                &last_jobs,
                last_theme,
                state.is_exiting(),
            );
        }
        let cur_theme = crate::gui::theme::system_theme();
        if cur_theme != last_theme {
            last_theme = cur_theme;
            rebuild_now(
                &tray,
                &mut actions,
                &mut dyn_map,
                &last_jobs,
                last_theme,
                state.is_exiting(),
            );
        }

        while let Ok(ev) = menu_chan.try_recv() {
            let id = ev.id();
            if actions.pause_all.as_ref() == Some(id) {
                let s = state.clone();
                rt.spawn(async move { s.pause_all().await });
            } else if actions.resume_all.as_ref() == Some(id) {
                let s = state.clone();
                rt.spawn(async move { s.resume_all().await });
            } else if actions.quit.as_ref() == Some(id) {
                quit_daemon(&rt, &state);
            } else if actions.open.as_ref() == Some(id) {
                spawn_main_gui();
            } else if actions.settings.as_ref() == Some(id) {
                spawn_settings_gui(None, false);
            } else if let Some(jid) = dyn_map.get(id).copied() {
                spawn_download_gui(jid);
            }
        }
        while let Ok(ev) = tray_chan.try_recv() {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                spawn_main_gui();
            }
        }

        #[cfg(target_os = "windows")]
        pump_win32_messages();

        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Drain pending Win32 messages on the tray-owner thread.
///
/// `tray-icon` registers a hidden window during `TrayIconBuilder::build`
/// and the shell delivers click / context-menu notifications to that
/// window via `WM_USER`. Without a message pump on the owning thread
/// the messages sit in the queue forever and `TrayIconEvent::receiver`
/// reports nothing. We can't use a blocking `GetMessage` loop because
/// the same thread also coalesces job-list snapshots and rebuilds the
/// menu, so each tick drains whatever is queued and returns
/// immediately.
#[cfg(target_os = "windows")]
fn pump_win32_messages() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
    };
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn rebuild_now(
    tray: &TrayIcon,
    actions: &mut ActionIds,
    dyn_map: &mut HashMap<MenuId, JobId>,
    jobs: &[Job],
    theme: crate::gui::theme::ResolvedTheme,
    exiting: bool,
) {
    let menu = Menu::new();
    let open = MenuItem::new("Open", true, None);
    let settings = MenuItem::new("Settings", true, None);
    let pause_all = MenuItem::new("Pause all", true, None);
    let resume_all = MenuItem::new("Resume all", true, None);
    // Once the exit is scheduled it cannot be asked for again — and
    // saying so beats a menu item that looks live and does nothing
    // while the app waits on an assembly.
    let quit = if exiting {
        MenuItem::new("Exiting\u{2026}", false, None)
    } else {
        MenuItem::new("Quit", true, None)
    };
    let new_actions = ActionIds {
        open: Some(open.id().clone()),
        settings: Some(settings.id().clone()),
        pause_all: Some(pause_all.id().clone()),
        resume_all: Some(resume_all.id().clone()),
        quit: Some(quit.id().clone()),
    };
    let _ = menu.append_items(&[&open, &settings, &pause_all, &resume_all]);

    let active: Vec<&Job> = jobs
        .iter()
        .filter(|j| j.status.phase.is_running() || j.status.phase == Phase::Paused)
        .collect();
    if !active.is_empty() {
        let _ = menu.append_items(&[&PredefinedMenuItem::separator()]);
    }
    let mut new_map: HashMap<MenuId, JobId> = HashMap::new();
    let dyn_items: Vec<MenuItem> = active
        .iter()
        .map(|j| MenuItem::new(label_for(j), true, None))
        .collect();
    for (item, job) in dyn_items.iter().zip(active.iter()) {
        new_map.insert(item.id().clone(), job.id);
        let _ = menu.append_items(&[item]);
    }
    let _ = menu.append_items(&[&PredefinedMenuItem::separator(), &quit]);

    let _ = tray.set_menu(Some(Box::new(menu)));
    let any_downloading = jobs.iter().any(|j| j.status.phase == Phase::Downloading);
    let icon = if any_downloading {
        crate::gui::app_icon::tray_icon_downloading(theme)
    } else {
        crate::gui::app_icon::tray_icon_normal(theme)
    };
    if let Some(i) = icon {
        let _ = tray.set_icon(Some(i));
    }
    *actions = new_actions;
    *dyn_map = new_map;
}
