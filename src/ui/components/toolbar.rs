//! Top toolbar.
//!
//! Layout (left → right): primary "Add URL", separator, ghost actions
//! (Start all / Pause all / Clean / Schedule), spacer, search field
//! aligned to the right.

use eframe::egui;

use super::primitives::{Btn, BtnSize, search_field};
use crate::data::RemoveOpts;
use crate::domain::{JobId, Phase, QueueId};
use crate::ui::AppShell;
use crate::ui::components::sidebar_tree::SidebarFilter;
use crate::ui::theme;

pub fn ui(app: &mut AppShell, ui: &mut egui::Ui) {
    let t = theme::tokens(ui.ctx());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = theme::space::S2 as f32;

        // Primary "Add URL".
        if Btn::new("Add URL")
            .primary()
            .icon("plus")
            .size(BtnSize::Md)
            .show(ui)
            .clicked()
        {
            crate::ui::ask_open_add(app);
        }

        vertical_divider(ui, t.border_subtle);

        // Start / Pause queue.
        let qid = filter_queue_id(app);
        let queue_active = qid
            .map(|q| app.snap.active_queues.contains(&q))
            .unwrap_or(false);

        if Btn::new("Pause queue")
            .toolbar()
            .icon("pause")
            .enabled(queue_active)
            .show(ui)
            .clicked()
            && qid.is_some()
        {
            let s = app.client.clone();
            app.spawn(async move {
                let _ = s.pause_all().await;
            });
        }
        if Btn::new("Stop all")
            .toolbar()
            .icon("square")
            .show(ui)
            .clicked()
        {
            let s = app.client.clone();
            app.spawn(async move {
                let _ = s.pause_all().await;
            });
        }
        if Btn::new("Clean")
            .toolbar()
            .icon("trash-2")
            .show(ui)
            .clicked()
        {
            // Clean = remove finished from current view.
            let ids: Vec<JobId> = app
                .snap
                .jobs
                .iter()
                .filter(|j| j.status.phase == Phase::Completed)
                .map(|j| j.id)
                .collect();
            let s = app.client.clone();
            app.spawn(async move {
                for id in ids {
                    let _ = s
                        .remove(
                            id,
                            RemoveOpts {
                                purge_partial: false,
                                delete_final_file: false,
                            },
                        )
                        .await;
                }
            });
        }
        vertical_divider(ui, t.border_subtle);
        if Btn::new("Schedule")
            .toolbar()
            .icon("calendar")
            .show(ui)
            .clicked()
        {
            crate::ui::ask_open_queues(app);
        }

        // Right-aligned search.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let w = 200.0_f32.min(ui.available_width());
            search_field(ui, &mut app.search, "Search...", w);
        });
    });
}

fn vertical_divider(ui: &mut egui::Ui, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, 24.0), egui::Sense::hover());
    ui.painter().line_segment(
        [
            egui::pos2(rect.center().x, rect.top()),
            egui::pos2(rect.center().x, rect.bottom()),
        ],
        egui::Stroke::new(1.0, color),
    );
}

pub fn trigger_delete(app: &mut AppShell) {
    let ids: Vec<JobId> = app.selection.iter().copied().collect();
    let settings = app.snap.settings.clone();
    let mut found_confirm: Option<crate::ui::dialogs::remove::RemoveRequest> = None;
    for id in ids.clone() {
        let Some(entry) = app.cache.job_entry_cached(id) else {
            continue;
        };
        let phase = entry.counters.phase;
        let filename = entry
            .job
            .filename
            .clone()
            .unwrap_or_else(|| entry.job.url.to_string());
        let must_confirm = match phase {
            Phase::Completed => settings.remove_confirm_completed,
            _ => settings.remove_confirm_incomplete,
        };
        if must_confirm {
            found_confirm = Some(crate::ui::dialogs::remove::RemoveRequest {
                id,
                filename,
                phase,
            });
            break;
        } else {
            let opts = match phase {
                Phase::Completed => RemoveOpts {
                    purge_partial: false,
                    delete_final_file: false,
                },
                _ => RemoveOpts {
                    purge_partial: true,
                    delete_final_file: false,
                },
            };
            let s = app.client.clone();
            app.spawn(async move {
                let _ = s.remove(id, opts).await;
            });
        }
    }
    if let Some(req) = found_confirm {
        app.remove = Some(req);
        app.remove_state = crate::ui::dialogs::remove::RemoveState::default();
    }
}

pub fn filter_queue_id(app: &AppShell) -> Option<QueueId> {
    match app.filter {
        SidebarFilter::Queue(id) => Some(id),
        _ => Some(app.cache.main_queue_id()),
    }
}
