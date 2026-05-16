//! Confirm-remove dialog. Custom titlebar, soft warning card with the
//! filename, options, and a Cancel / Remove footer (Remove styled as
//! the danger variant).

use eframe::egui::{self, Align, Layout, RichText, Stroke};

use crate::data::RemoveOpts;
use crate::domain::{JobId, Phase};
use crate::ui::AppShell;
use crate::ui::components::primitives::{Btn, eyebrow};
use crate::ui::components::titlebar;
use crate::ui::theme::{self, space};

#[derive(Debug, Clone, PartialEq)]
pub struct RemoveRequest {
    pub id: JobId,
    pub filename: String,
    pub phase: Phase,
}

#[derive(Default, Debug, Clone, Copy)]
pub struct RemoveState {
    pub delete_on_disk: bool,
    pub dont_ask_again: bool,
}

pub fn show(app: &mut AppShell, ctx: &egui::Context) {
    let closed = super::child_viewport(
        ctx,
        "oxdm-remove",
        "oxdm — Remove download",
        (480.0, 320.0),
        |ui| body(app, ui),
    );
    if closed {
        app.remove = None;
    }
}

fn body(app: &mut AppShell, root_ui: &mut egui::Ui) {
    let ctx = &root_ui.ctx().clone();
    let Some(req) = app.remove.clone() else {
        return;
    };
    let t = theme::tokens(ctx);
    let is_complete = req.phase == Phase::Completed;
    let title = if is_complete {
        "Remove from list?"
    } else {
        "Remove unfinished download?"
    };

    egui::Panel::top("rm_titlebar")
        .frame(egui::Frame::NONE.fill(t.bg_titlebar))
        .show_separator_line(true)
        .show_inside(root_ui, |ui| {
            titlebar::show(ui, ctx, title);
        });

    let mut close = false;
    let mut confirm = false;

    egui::Panel::bottom("rm_footer")
        .frame(
            egui::Frame::NONE
                .fill(t.bg_sunken)
                .inner_margin(egui::Margin::symmetric(space::S4, space::S2)),
        )
        .show_separator_line(true)
        .show_inside(root_ui, |ui| {
            ui.horizontal(|ui| {
                if Btn::new("Cancel").ghost().icon("x").show(ui).clicked() {
                    close = true;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if Btn::new("Remove")
                        .danger_filled()
                        .icon("trash-2")
                        .show(ui)
                        .clicked()
                    {
                        confirm = true;
                    }
                });
            });
        });

    egui::CentralPanel::default()
        .frame(
            egui::Frame::NONE
                .fill(t.bg_page)
                .inner_margin(egui::Margin::symmetric(space::S4, space::S4)),
        )
        .show_inside(root_ui, |ui| {
            ui.spacing_mut().item_spacing.y = space::S3 as f32;
            // Filename card.
            egui::Frame::NONE
                .fill(t.status_danger_bg)
                .stroke(Stroke::new(t.border_width, t.border_subtle))
                .corner_radius(theme::surface::RADIUS)
                .inner_margin(space::S3 as f32)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        crate::ui::utils::icons::show(ui, "triangle-alert", 22.0, t.status_danger);
                        ui.add_space(space::S2 as f32);
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new(&req.filename)
                                    .font(theme::body_bold(13.0))
                                    .color(t.fg_1),
                            );
                            if !is_complete {
                                ui.label(
                                    RichText::new(
                                        "Partial (.part) files will be deleted from disk.",
                                    )
                                    .color(t.fg_2)
                                    .font(theme::body(12.0)),
                                );
                            } else {
                                ui.label(
                                    RichText::new("This only removes the entry from oxdm.")
                                        .color(t.fg_2)
                                        .font(theme::body(12.0)),
                                );
                            }
                        });
                    });
                });

            ui.add_space(space::S2 as f32);
            eyebrow(ui, "options");
            if is_complete {
                ui.checkbox(
                    &mut app.remove_state.delete_on_disk,
                    "Also delete file on disk",
                );
            }
            let label = if is_complete {
                "Don't ask again for completed downloads"
            } else {
                "Don't ask again for incomplete downloads"
            };
            ui.checkbox(&mut app.remove_state.dont_ask_again, label);
        });

    if confirm {
        let id = req.id;
        let phase = req.phase;
        let dod = app.remove_state.delete_on_disk;
        let dont = app.remove_state.dont_ask_again;
        let s = app.client.clone();
        let mut current_settings = app.cache.settings();
        app.spawn(async move {
            let opts = match phase {
                Phase::Completed => RemoveOpts {
                    purge_partial: false,
                    delete_final_file: dod,
                },
                _ => RemoveOpts {
                    purge_partial: true,
                    delete_final_file: false,
                },
            };
            let _ = s.remove(id, opts).await;
            if dont {
                match phase {
                    Phase::Completed => current_settings.remove_confirm_completed = false,
                    _ => current_settings.remove_confirm_incomplete = false,
                }
                let _ = s.update_settings(current_settings).await;
            }
        });
        app.remove = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
    if close {
        app.remove = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}
