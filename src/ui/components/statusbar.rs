//! Bottom status bar. Mirrors the design's left-aggregate / right-status
//! split: download count, total speed, current queue and concurrency on
//! the left; free disk and connection mode on the right.

use eframe::egui::{self, Align, Color32, Layout, Sense, Vec2};

use super::primitives::{Btn, BtnSize, status_dot};
use crate::domain::{Job, Phase, Settings};
use crate::ui::AppShell;
use crate::ui::components::sidebar_tree::SidebarFilter;
use crate::ui::theme::{self, space};
use crate::ui::utils::format::format_speed;

pub fn ui(app: &mut AppShell, ui: &mut egui::Ui) {
    let t = theme::tokens(ui.ctx());

    let total = app
        .snap
        .jobs
        .iter()
        .filter(|j| matches_filter(j, app.filter, &app.snap.settings))
        .count();
    let active = app
        .snap
        .jobs
        .iter()
        .filter(|j| j.status.phase.is_running())
        .count();
    let total_speed: f64 = app
        .snap
        .jobs
        .iter()
        .filter(|j| j.status.phase.is_running())
        .map(|j| j.status.speed_bps)
        .sum();
    let sel = app.selection.len();

    let qid = match app.filter {
        SidebarFilter::Queue(id) => Some(id),
        _ => Some(app.cache.main_queue_id()),
    };
    let queue_name = qid
        .and_then(|id| {
            app.snap
                .queues
                .iter()
                .find(|q| q.id == id)
                .map(|q| q.name.clone())
        })
        .unwrap_or_else(|| "—".into());
    let queue_max = qid
        .and_then(|id| {
            app.snap
                .queues
                .iter()
                .find(|q| q.id == id)
                .and_then(|q| q.max_concurrent)
        })
        .unwrap_or(app.snap.settings.max_concurrent_downloads);

    let font = theme::body(11.0);
    let font_b = theme::body_bold(11.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::S2 as f32;
        // Active dot + count, or "Idle" when nothing is running.
        if active == 0 {
            status_dot(ui, t.fg_4, "Idle".to_string(), 11.0);
        } else {
            status_dot(ui, t.action_primary, format!("{active} downloading"), 11.0);
        }
        sep(ui, &t);
        crate::ui::utils::icons::show(ui, "activity", 14.0, t.fg_3);
        ui.label(
            egui::RichText::new(format_speed(total_speed))
                .color(t.fg_2)
                .font(font_b.clone()),
        );
        sep(ui, &t);
        ui.label(
            egui::RichText::new(format!("Queue: {queue_name}"))
                .color(t.fg_3)
                .font(font.clone()),
        );
        sep(ui, &t);
        ui.label(
            egui::RichText::new(format!("max {queue_max}×"))
                .color(t.fg_3)
                .font(font.clone()),
        );

        if sel > 0 && total > 0 {
            sep(ui, &t);
            ui.label(
                egui::RichText::new(format!("{sel}/{total} selected"))
                    .color(t.fg_3)
                    .font(font.clone()),
            );
        }

        ui.scope_builder(
            egui::UiBuilder::new()
                .id_salt(egui::Id::new("oxdm-statusbar-right"))
                .global_scope(true)
                .layout(Layout::right_to_left(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = space::S2 as f32;
                let proxy_url = app
                    .snap
                    .settings
                    .proxy
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                let (icon, label) = if proxy_url.is_some() {
                    ("shield", "Proxied")
                } else {
                    ("globe", "Direct")
                };
                let hint = match proxy_url {
                    Some(url) => format!("Proxy: {url}"),
                    None => "No proxy configured".to_string(),
                };
                let btn_resp = Btn::new(label)
                    .toolbar()
                    .icon(icon)
                    .size(BtnSize::Sm)
                    .show(ui)
                    .on_hover_text(hint);
                if btn_resp.clicked() {
                    crate::ui::ask_open_settings(app, Some("network".into()), true);
                }
                sep(ui, &t);
                let temp_root = app.snap.settings.download_dir.clone();
                let disk_resp = Btn::new(free_disk_str(&temp_root))
                    .toolbar()
                    .icon("hard-drive")
                    .size(BtnSize::Sm)
                    .show(ui)
                    .on_hover_text(format!("Open {}", temp_root.display()));
                if disk_resp.clicked() {
                    let probe = nearest_existing(&temp_root);
                    crate::ui::platform::open_path(&probe);
                }
            },
        );
    });
}

fn sep(ui: &mut egui::Ui, t: &theme::Tokens) {
    let (_, rect) = ui.allocate_space(Vec2::new(2.0, 8.0));
    ui.painter().circle_filled(rect.center(), 1.5, t.fg_4);
    let _ = (Color32::WHITE, Sense::hover());
}

fn nearest_existing(path: &std::path::Path) -> std::path::PathBuf {
    let mut probe = path;
    loop {
        if probe.exists() {
            return probe.to_path_buf();
        }
        match probe.parent() {
            Some(p) => probe = p,
            None => return path.to_path_buf(),
        }
    }
}

fn free_disk_str(path: &std::path::Path) -> String {
    // Walk up to nearest existing ancestor — `available_space` errors
    // if path doesn't exist (e.g. download_dir not yet created).
    let mut probe = path;
    loop {
        if probe.exists() {
            break;
        }
        match probe.parent() {
            Some(p) => probe = p,
            None => return "—".into(),
        }
    }
    match fs4::available_space(probe) {
        Ok(free) => format!("{} free", crate::ui::utils::format::format_bytes(free)),
        Err(_) => "—".into(),
    }
}

pub fn matches_filter(job: &Job, f: SidebarFilter, _settings: &Settings) -> bool {
    let phase = job.status.phase;
    let cat = job.category;
    match f {
        SidebarFilter::All { category } => category.map(|c| c == cat).unwrap_or(true),
        SidebarFilter::Finished { category } => {
            phase == Phase::Completed && category.map(|c| c == cat).unwrap_or(true)
        }
        SidebarFilter::Unfinished { category } => {
            !matches!(phase, Phase::Completed | Phase::Failed)
                && category.map(|c| c == cat).unwrap_or(true)
        }
        SidebarFilter::Queue(qid) => job.queue_id == qid,
    }
}
