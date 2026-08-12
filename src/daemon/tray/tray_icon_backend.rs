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

/// What the menu would say, if it were rebuilt right now.
///
/// Compared against the last one applied so a rebuild only happens when
/// the result would differ. The watcher wakes on every domain event —
/// which during a download means several a second — and each rebuild
/// drops the live `Menu` and attaches a new one. That is the path muda
/// 0.18 fixed a Windows subclass leak in and 0.19.3 a dangling-pointer
/// crash in; doing it only when the text actually changes is both
/// cheaper and less exposed.
#[derive(PartialEq, Eq, Default)]
struct MenuShape {
    entries: Vec<(JobId, String)>,
    exiting: bool,
}

impl MenuShape {
    fn of(jobs: &[Job], exiting: bool) -> Self {
        Self {
            entries: jobs
                .iter()
                .filter(|j| j.status.phase.is_running() || j.status.phase == Phase::Paused)
                .map(|j| (j.id, label_for(j)))
                .collect(),
            exiting,
        }
    }
}

/// Which of the two tray icons is showing, and in which theme.
#[derive(PartialEq, Eq, Clone, Copy)]
struct IconState {
    downloading: bool,
    theme: crate::gui::theme::ResolvedTheme,
}

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
                if crate::data::next_event(&mut rx, "tray icon")
                    .await
                    .is_none()
                {
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
    let mut shape = MenuShape::of(&last_jobs, state.is_exiting());
    rebuild_menu(&tray, &mut actions, &mut dyn_map, &shape);
    let mut icon_state = IconState {
        downloading: false,
        theme: crate::gui::theme::system_theme(),
    };
    apply_icon(&tray, icon_state);

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
        }
        // Both are gated on their own state: a job that ticked forward
        // by a few kilobytes changes neither the menu text nor which
        // icon is showing, and re-applying either would be churn the
        // shell has to redraw.
        let next_shape = MenuShape::of(&last_jobs, state.is_exiting());
        if next_shape != shape {
            shape = next_shape;
            rebuild_menu(&tray, &mut actions, &mut dyn_map, &shape);
        }
        let next_icon = IconState {
            downloading: last_jobs
                .iter()
                .any(|j| j.status.phase == Phase::Downloading),
            theme: crate::gui::theme::system_theme(),
        };
        if next_icon != icon_state {
            icon_state = next_icon;
            apply_icon(&tray, icon_state);
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

fn rebuild_menu(
    tray: &TrayIcon,
    actions: &mut ActionIds,
    dyn_map: &mut HashMap<MenuId, JobId>,
    shape: &MenuShape,
) {
    let exiting = shape.exiting;
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

    if !shape.entries.is_empty() {
        let _ = menu.append_items(&[&PredefinedMenuItem::separator()]);
    }
    let mut new_map: HashMap<MenuId, JobId> = HashMap::new();
    let dyn_items: Vec<MenuItem> = shape
        .entries
        .iter()
        .map(|(_, label)| MenuItem::new(label, true, None))
        .collect();
    for (item, (id, _)) in dyn_items.iter().zip(shape.entries.iter()) {
        new_map.insert(item.id().clone(), *id);
        let _ = menu.append_items(&[item]);
    }
    let _ = menu.append_items(&[&PredefinedMenuItem::separator(), &quit]);

    let _ = tray.set_menu(Some(Box::new(menu)));
    *actions = new_actions;
    *dyn_map = new_map;
}

fn apply_icon(tray: &TrayIcon, state: IconState) {
    let icon = if state.downloading {
        crate::gui::app_icon::tray_icon_downloading(state.theme)
    } else {
        crate::gui::app_icon::tray_icon_normal(state.theme)
    };
    if let Some(i) = icon {
        let _ = tray.set_icon(Some(i));
    }
}
