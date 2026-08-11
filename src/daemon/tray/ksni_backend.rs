//! Linux tray via ksni. Spawned on the tokio runtime; rebuilds on
//! domain events.

use std::sync::Arc;

use ksni::{Tray, TrayMethods};
use tokio::runtime::Handle;
use tokio::sync::OnceCell;

use super::{label_for, quit_daemon, spawn_download_gui, spawn_main_gui, spawn_settings_gui};
use crate::data::{AppState, DomainEvent};
use crate::domain::{JobId, Phase};

pub fn install(rt: Handle, state: Arc<AppState>) {
    let _g = rt.enter();
    let cell: Arc<OnceCell<ksni::Handle<OxdmTray>>> = Arc::new(OnceCell::new());
    let tray = OxdmTray {
        rt: rt.clone(),
        state: state.clone(),
        active: Vec::new(),
        any_downloading: false,
        theme: crate::gui::theme::system_theme(),
    };
    let cell_spawn = cell.clone();
    tokio::spawn(async move {
        match tray.spawn().await {
            Ok(h) => {
                let _ = cell_spawn.set(h);
            }
            Err(e) => tracing::warn!(error = %e, "ksni tray spawn failed"),
        }
    });

    let cell_loop = cell.clone();
    let state_loop = state.clone();
    tokio::spawn(async move {
        let mut rx = state_loop.subscribe();
        loop {
            rebuild(&cell_loop, &state_loop).await;
            if crate::data::next_event(&mut rx, "ksni tray")
                .await
                .is_none()
            {
                break;
            }
        }
    });

    // OS theme follow-on: when the system light/dark preference flips,
    // notify the ksni tray so it re-emits its pixmap.
    let cell_theme = cell.clone();
    crate::gui::theme::on_system_theme_change(move |theme| {
        let cell_theme = cell_theme.clone();
        tokio::spawn(async move {
            if let Some(h) = cell_theme.get() {
                let _ = h
                    .update(move |t: &mut OxdmTray| {
                        t.theme = theme;
                    })
                    .await;
            }
        });
    });
    let _ = DomainEvent::SettingsChanged;
}

async fn rebuild(cell: &Arc<OnceCell<ksni::Handle<OxdmTray>>>, state: &Arc<AppState>) {
    let jobs = state.list_jobs().await;
    let any_downloading = jobs.iter().any(|j| j.status.phase == Phase::Downloading);
    let active: Vec<ActiveJob> = jobs
        .iter()
        .filter(|j| j.status.phase.is_running() || j.status.phase == Phase::Paused)
        .map(|j| ActiveJob {
            id: j.id,
            label: label_for(j),
        })
        .collect();
    if let Some(h) = cell.get() {
        let _ = h
            .update(move |t: &mut OxdmTray| {
                t.active = active;
                t.any_downloading = any_downloading;
            })
            .await;
    }
}

#[derive(Clone)]
struct ActiveJob {
    id: JobId,
    label: String,
}

struct OxdmTray {
    rt: Handle,
    state: Arc<AppState>,
    active: Vec<ActiveJob>,
    any_downloading: bool,
    theme: crate::gui::theme::ResolvedTheme,
}

impl Tray for OxdmTray {
    fn id(&self) -> String {
        "oxdm".into()
    }
    fn title(&self) -> String {
        "oxdm".into()
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        if self.any_downloading {
            crate::gui::app_icon::ksni_icon_downloading(self.theme)
        } else {
            crate::gui::app_icon::ksni_icon_normal(self.theme)
        }
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        spawn_main_gui();
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{MenuItem as M, StandardItem};
        let mut items: Vec<ksni::MenuItem<Self>> = Vec::new();
        items.push(
            StandardItem {
                label: "Open".into(),
                activate: Box::new(|_t: &mut Self| spawn_main_gui()),
                ..Default::default()
            }
            .into(),
        );
        items.push(
            StandardItem {
                label: "Settings".into(),
                activate: Box::new(|_t: &mut Self| spawn_settings_gui(None, false)),
                ..Default::default()
            }
            .into(),
        );
        items.push(
            StandardItem {
                label: "Pause all".into(),
                activate: Box::new(|t: &mut Self| {
                    let s = t.state.clone();
                    tokio::spawn(async move { s.pause_all().await });
                }),
                ..Default::default()
            }
            .into(),
        );
        items.push(
            StandardItem {
                label: "Resume all".into(),
                activate: Box::new(|t: &mut Self| {
                    let s = t.state.clone();
                    tokio::spawn(async move { s.resume_all().await });
                }),
                ..Default::default()
            }
            .into(),
        );
        if !self.active.is_empty() {
            items.push(M::Separator);
            for j in &self.active {
                let id = j.id;
                items.push(
                    StandardItem {
                        label: j.label.clone(),
                        activate: Box::new(move |_t: &mut Self| spawn_download_gui(id)),
                        ..Default::default()
                    }
                    .into(),
                );
            }
        }
        items.push(M::Separator);
        // Once the exit is scheduled it cannot be asked for again —
        // and saying so beats a menu item that looks live and does
        // nothing while the app waits on an assembly.
        let exiting = self.state.is_exiting();
        items.push(
            StandardItem {
                label: if exiting {
                    "Exiting\u{2026}".into()
                } else {
                    "Quit".into()
                },
                enabled: !exiting,
                activate: Box::new(|t: &mut Self| quit_daemon(&t.rt, &t.state)),
                ..Default::default()
            }
            .into(),
        );
        items
    }
}
