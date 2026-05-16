use eframe::egui;

use crate::data::RemoveOpts;
use crate::domain::Phase;
use crate::ui::AppShell;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuId {
    File,
    Tasks,
    Tools,
    Help,
}

const REPO_URL: &str = "https://github.com/jd1378/oxdm";
const SOURCE_URL: &str = "https://github.com/jd1378/oxdm";
const LICENSE_URL: &str = "https://github.com/jd1378/oxdm/blob/main/LICENSE";
const DONATE_URL: &str = "https://github.com/sponsors/jd1378";
const TELEGRAM_CHANNEL_URL: &str = "https://t.me/oxdm_channel";
const TELEGRAM_GROUP_URL: &str = "https://t.me/oxdm_group";
const CHROME_EXT_URL: &str = "https://chromewebstore.google.com/";
const FIREFOX_EXT_URL: &str = "https://addons.mozilla.org/en-US/firefox/";

pub fn ui(app: &mut AppShell, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.menu_button("File", |ui| file_menu(app, ui));
        ui.menu_button("Tasks", |ui| tasks_menu(app, ui));
        ui.menu_button("Tools", |ui| tools_menu(app, ui));
        ui.menu_button("Help", |ui| help_menu(app, ui));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            super::primitives::search_field(ui, &mut app.search, "Search", 200.0);
        });
    });
    let _ = app.menu_open;
}

fn file_menu(app: &mut AppShell, ui: &mut egui::Ui) {
    if ui.button("New Download").clicked() {
        crate::ui::ask_open_add(app);
        ui.close();
    }
    if ui.button("Import From Clipboard").clicked() {
        // Subprocess auto-reads clipboard on launch.
        crate::ui::ask_open_add(app);
        ui.close();
    }
    ui.separator();
    if ui.button("Exit").clicked() {
        app.want_quit = true;
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        ui.close();
    }
}

fn tasks_menu(app: &mut AppShell, ui: &mut egui::Ui) {
    let queues = app.snap.queues.clone();
    let active = app.snap.active_queues.clone();
    ui.menu_button("Start Queue", |ui| {
        for q in &queues {
            let running = active.contains(&q.id);
            let qid = q.id;
            let s = app.client.clone();
            if ui
                .add_enabled(!running, egui::Button::new(&q.name))
                .clicked()
            {
                app.spawn(async move {
                    let _ = s.start_queue(qid).await;
                });
                ui.close();
            }
        }
    });
    ui.menu_button("Stop Queue", |ui| {
        for q in &queues {
            let running = active.contains(&q.id);
            let qid = q.id;
            let s = app.client.clone();
            if ui
                .add_enabled(running, egui::Button::new(&q.name))
                .clicked()
            {
                app.spawn(async move {
                    let _ = s.stop_queue(qid).await;
                });
                ui.close();
            }
        }
    });
    if ui.button("Stop All").clicked() {
        let s = app.client.clone();
        app.spawn(async move {
            let _ = s.pause_all().await;
        });
        ui.close();
    }
    ui.separator();
    ui.menu_button("Delete", |ui| {
        for (kind, label) in [
            ("missing", "All Missing Files"),
            ("finished", "All Finished"),
            ("unfinished", "All Unfinished"),
            ("all", "Entire List"),
        ] {
            if ui.button(label).clicked() {
                bulk_delete(app, kind);
                ui.close();
            }
        }
    });
}

fn bulk_delete(app: &mut AppShell, kind: &'static str) {
    let s = app.client.clone();
    let jobs = app.cache.jobs();
    let ids: Vec<_> = jobs
        .iter()
        .filter(|j| match kind {
            "missing" => j.status.phase == Phase::Failed,
            "finished" => j.status.phase == Phase::Completed,
            "unfinished" => !matches!(j.status.phase, Phase::Completed | Phase::Failed),
            "all" => true,
            _ => false,
        })
        .map(|j| j.id)
        .collect();
    app.spawn(async move {
        for id in ids {
            let _ = s
                .remove(
                    id,
                    RemoveOpts {
                        purge_partial: kind != "finished",
                        delete_final_file: false,
                    },
                )
                .await;
        }
    });
}

fn tools_menu(app: &mut AppShell, ui: &mut egui::Ui) {
    ui.menu_button("Download Browser Integration", |ui| {
        if ui.button("Google Chrome").clicked() {
            crate::ui::platform::open_url(CHROME_EXT_URL);
            ui.close();
        }
        if ui.button("Mozilla Firefox").clicked() {
            crate::ui::platform::open_url(FIREFOX_EXT_URL);
            ui.close();
        }
    });
    #[cfg(target_os = "linux")]
    {
        if ui.button("Create Desktop Entry").clicked() {
            match crate::ui::platform::install_desktop_entry() {
                Ok(p) => tracing::info!(path = %p.display(), "desktop entry written"),
                Err(e) => tracing::warn!(error = %e, "desktop entry failed"),
            }
            ui.close();
        }
    }
    if ui.button("Per Host Settings").clicked() {
        app.host_open = true;
        ui.close();
    }
    if ui.button("Settings").clicked() {
        crate::ui::ask_open_settings(app, None, false);
        ui.close();
    }
}

fn help_menu(app: &mut AppShell, ui: &mut egui::Ui) {
    ui.menu_button("Support & Community", |ui| {
        if ui.button("Website").clicked() {
            crate::ui::platform::open_url(REPO_URL);
            ui.close();
        }
        if ui.button("Source Code").clicked() {
            crate::ui::platform::open_url(SOURCE_URL);
            ui.close();
        }
        ui.menu_button("Telegram", |ui| {
            if ui.button("Channel").clicked() {
                crate::ui::platform::open_url(TELEGRAM_CHANNEL_URL);
                ui.close();
            }
            if ui.button("Group").clicked() {
                crate::ui::platform::open_url(TELEGRAM_GROUP_URL);
                ui.close();
            }
        });
    });
    if ui.button("View the Open-Source licenses").clicked() {
        crate::ui::platform::open_url(LICENSE_URL);
        ui.close();
    }
    if ui.button("Donate").clicked() {
        crate::ui::platform::open_url(DONATE_URL);
        ui.close();
    }
    if ui.button("Check for Update").clicked() {
        app.about_open = true;
        app.about_state.auto_check = true;
        ui.close();
    }
    ui.separator();
    if ui.button("About").clicked() {
        app.about_open = true;
        ui.close();
    }
}
