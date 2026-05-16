//! About dialog: app identity card + update flow status card.

use std::sync::Arc;

use eframe::egui::{self, Align, Layout, Pos2, RichText, Sense, Stroke, Vec2};

use crate::data::{UpdateInfo, UpdaterEvent};
use crate::domain::JobId;
use crate::ui::AppShell;
use crate::ui::components::primitives::{Btn, BtnSize, eyebrow};
use crate::ui::components::titlebar;
use crate::ui::theme::{self, radius, space};
use crate::ui::updater::{self, UpdaterHandle};

#[derive(Default, Clone)]
pub struct AboutState {
    pub status: UpdateUi,
    pub updater_rx: Option<Arc<std::sync::Mutex<tokio::sync::mpsc::Receiver<UpdaterEvent>>>>,
    pub updater_handle: Option<Arc<UpdaterHandle>>,
    pub auto_check: bool,
}

#[derive(Default, Clone)]
pub enum UpdateUi {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(UpdateInfo),
    Downloading {
        info: UpdateInfo,
        job_id: JobId,
    },
    Verifying,
    AwaitingConfirm {
        info: UpdateInfo,
    },
    Installing,
    Error(String),
}

pub fn show(app: &mut AppShell, ctx: &egui::Context) {
    let closed = super::child_viewport(ctx, "oxdm-about", "oxdm — About", (520.0, 520.0), |ctx| {
        body(app, ctx)
    });
    if closed {
        app.about_open = false;
    }
}

fn body(app: &mut AppShell, root_ui: &mut egui::Ui) {
    let ctx = &root_ui.ctx().clone();
    let t = theme::tokens(ctx);

    // Drain updater events.
    if let Some(rx_arc) = app.about_state.updater_rx.clone()
        && let Ok(mut rx) = rx_arc.lock()
    {
        while let Ok(ev) = rx.try_recv() {
            match ev {
                UpdaterEvent::Started | UpdaterEvent::Verified => {}
                UpdaterEvent::Ready => {
                    if let UpdateUi::Downloading { info, .. } = &app.about_state.status {
                        app.about_state.status = UpdateUi::AwaitingConfirm { info: info.clone() };
                    }
                }
                UpdaterEvent::Installing | UpdaterEvent::Done => {
                    app.about_state.status = UpdateUi::Installing;
                }
                UpdaterEvent::Error { message } => {
                    app.about_state.status = UpdateUi::Error(message);
                }
            }
        }
    }

    egui::Panel::top("about_titlebar")
        .frame(egui::Frame::NONE.fill(t.bg_titlebar))
        .show_separator_line(true)
        .show_inside(root_ui, |ui| {
            titlebar::show(ui, ctx, "About");
        });

    egui::CentralPanel::default()
        .frame(
            egui::Frame::NONE
                .fill(t.bg_page)
                .inner_margin(egui::Margin::symmetric(space::S4, space::S4)),
        )
        .show_inside(root_ui, |ui| {
            ui.spacing_mut().item_spacing.y = space::S3 as f32;

            // Identity card.
            egui::Frame::NONE
                .fill(t.bg_surface)
                .stroke(Stroke::new(t.border_width, t.border_subtle))
                .corner_radius(theme::surface::RADIUS)
                .inner_margin(space::S3 as f32)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let tile = 56.0;
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(tile), Sense::hover());
                        let p = ui.painter().clone();
                        p.rect_filled(
                            rect,
                            radius::SM as f32,
                            crate::ui::dialogs::soft_tint(t.action_primary, t.bg_surface, 0.20),
                        );
                        let g =
                            p.layout_no_wrap("OX".into(), theme::display(20.0), t.action_primary);
                        p.galley(
                            Pos2::new(
                                rect.center().x - g.size().x / 2.0,
                                rect.center().y - g.size().y / 2.0,
                            ),
                            g,
                            t.action_primary,
                        );
                        ui.add_space(space::S3 as f32);
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new("oxdm")
                                    .font(theme::display(28.0))
                                    .color(t.fg_1),
                            );
                            ui.label(
                                RichText::new("Open-source cross-platform download manager.")
                                    .color(t.fg_2)
                                    .font(theme::body(13.0)),
                            );
                            ui.add_space(space::S0 as f32);
                            ui.label(
                                RichText::new(format!(
                                    "Built on the odl crate · v{}",
                                    env!("CARGO_PKG_VERSION")
                                ))
                                .color(t.fg_3)
                                .font(theme::mono(11.0)),
                            );
                        });
                    });
                });

            // Updates card.
            egui::Frame::NONE
                .fill(t.bg_surface)
                .stroke(Stroke::new(t.border_width, t.border_subtle))
                .corner_radius(theme::surface::RADIUS)
                .inner_margin(space::S3 as f32)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        crate::ui::utils::icons::show(ui, "cloud-upload", 17.0, t.fg_2);
                        ui.label(
                            RichText::new("Updates")
                                .color(t.fg_1)
                                .font(theme::body_bold(13.0)),
                        );
                    });
                    ui.add_space(space::S2 as f32);

                    let trigger_check = Btn::new("Check for updates")
                        .icon("refresh-cw")
                        .size(BtnSize::Sm)
                        .show(ui)
                        .clicked()
                        || std::mem::take(&mut app.about_state.auto_check);
                    if trigger_check {
                        app.about_state.status = UpdateUi::Checking;
                        let s = app.client.clone();
                        let res = app.block_on(async move { s.update_check().await });
                        app.about_state.status = match res {
                            Ok(None) => UpdateUi::UpToDate,
                            Ok(Some(info)) => UpdateUi::Available(info),
                            Err(e) => UpdateUi::Error(e),
                        };
                    }

                    ui.add_space(space::S2 as f32);
                    render_status(app, ui, &t);
                });

            ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
                ui.horizontal(|ui| {
                    if Btn::new("Repository")
                        .toolbar()
                        .icon("globe")
                        .size(BtnSize::Sm)
                        .show(ui)
                        .clicked()
                    {
                        crate::ui::platform::open_url("https://github.com/jd1378/oxdm");
                    }
                    if Btn::new("Donate")
                        .toolbar()
                        .icon("zap")
                        .size(BtnSize::Sm)
                        .show(ui)
                        .clicked()
                    {
                        crate::ui::platform::open_url("https://github.com/sponsors/jd1378");
                    }
                });
            });
        });

    if let UpdateUi::Downloading { info, job_id } = app.about_state.status.clone()
        && let Some(entry) = app.cache.job_entry_cached(job_id)
        && entry.counters.phase == crate::domain::Phase::Completed
        && let Some(path) = entry.job.status.final_path.clone()
    {
        app.about_state.status = UpdateUi::Verifying;
        match updater::spawn(path, info.sha256.clone()) {
            Ok((handle, rx)) => {
                app.about_state.updater_handle = Some(handle);
                app.about_state.updater_rx = Some(Arc::new(std::sync::Mutex::new(rx)));
            }
            Err(e) => app.about_state.status = UpdateUi::Error(e),
        }
    }
}

fn render_status(app: &mut AppShell, ui: &mut egui::Ui, t: &theme::Tokens) {
    match &app.about_state.status.clone() {
        UpdateUi::Idle => {
            ui.label(
                RichText::new("Click \"Check for updates\" to look for a new release.")
                    .color(t.fg_3)
                    .font(theme::body(12.0)),
            );
        }
        UpdateUi::Checking => {
            ui.label(
                RichText::new("Checking…")
                    .color(t.fg_2)
                    .font(theme::body_bold(12.0)),
            );
        }
        UpdateUi::UpToDate => {
            ui.horizontal(|ui| {
                crate::ui::utils::icons::show(ui, "circle-check", 17.0, t.status_success);
                ui.label(
                    RichText::new("You're up to date.")
                        .color(t.status_success)
                        .font(theme::body_bold(12.0)),
                );
            });
        }
        UpdateUi::Available(info) => {
            eyebrow(ui, "update available");
            ui.label(
                RichText::new(format!("v{}", info.version))
                    .font(theme::display(20.0))
                    .color(t.fg_1),
            );
            if let Some(notes) = &info.notes {
                ui.label(RichText::new(notes).color(t.fg_2).font(theme::body(12.0)));
            }
            ui.add_space(space::S2 as f32);
            if Btn::new("Download update")
                .primary()
                .icon("download")
                .show(ui)
                .clicked()
            {
                let s = app.client.clone();
                let info_owned = info.clone();
                let url = info_owned.url.clone();
                let filename = url
                    .path_segments()
                    .and_then(|mut sg| sg.next_back())
                    .map(|s| s.to_string());
                let res = app.block_on(async move { s.add_update_job(url, filename).await });
                match res {
                    Ok(job_id) => {
                        crate::ui::windows::download::state::spawn(app, job_id);
                        app.about_state.status = UpdateUi::Downloading {
                            info: info_owned,
                            job_id,
                        };
                    }
                    Err(e) => app.about_state.status = UpdateUi::Error(e),
                }
            }
        }
        UpdateUi::Downloading { info, .. } => {
            ui.label(
                RichText::new(format!("Downloading v{}…", info.version))
                    .color(t.fg_2)
                    .font(theme::body_bold(12.0)),
            );
        }
        UpdateUi::Verifying => {
            ui.label(
                RichText::new("Verifying…")
                    .color(t.fg_2)
                    .font(theme::body_bold(12.0)),
            );
        }
        UpdateUi::AwaitingConfirm { info } => {
            eyebrow(ui, "ready to install");
            ui.label(
                RichText::new(format!("v{} is verified.", info.version))
                    .font(theme::display(18.0))
                    .color(t.fg_1),
            );
            ui.label(
                RichText::new("Installing will close oxdm and relaunch.")
                    .color(t.fg_3)
                    .font(theme::body(12.0)),
            );
            ui.add_space(space::S2 as f32);
            ui.horizontal(|ui| {
                if Btn::new("Cancel").ghost().show(ui).clicked() {
                    if let Some(h) = app.about_state.updater_handle.clone() {
                        app.spawn(async move { h.abort().await });
                    }
                    app.about_state = AboutState::default();
                }
                if Btn::new("Install and restart")
                    .primary()
                    .icon("rotate-cw")
                    .show(ui)
                    .clicked()
                    && let Some(h) = app.about_state.updater_handle.clone()
                {
                    let h2 = h.clone();
                    let res = app.block_on(async move { h2.confirm().await });
                    if let Err(e) = res {
                        app.about_state.status = UpdateUi::Error(e);
                        return;
                    }
                    app.about_state.status = UpdateUi::Installing;
                    std::thread::sleep(std::time::Duration::from_millis(150));
                    let c = app.client.clone();
                    let _ = app.block_on(async move { c.daemon_quit().await });
                    std::process::exit(0);
                }
            });
        }
        UpdateUi::Installing => {
            ui.label(
                RichText::new("Installing…")
                    .color(t.fg_2)
                    .font(theme::body_bold(12.0)),
            );
        }
        UpdateUi::Error(e) => {
            ui.horizontal(|ui| {
                crate::ui::utils::icons::show(ui, "triangle-alert", 17.0, t.status_danger);
                ui.label(
                    RichText::new(e)
                        .color(t.status_danger)
                        .font(theme::body_bold(12.0)),
                );
            });
        }
    }
}
