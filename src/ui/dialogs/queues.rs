//! Queues & scheduling dialog.
//!
//! Left pane: queue list (colour dot, name, job count, "+ Add queue").
//! Right pane: editor for the selected queue — name field + Delete,
//! Concurrency segmented buttons (1×–8×), Schedule segmented (Manual /
//! Recurring / One-off / Condition), "When the queue finishes" grid.
//! Footer: Cancel / Save.

use std::sync::Arc;

use chrono::NaiveTime;
use eframe::egui::{self, Align, Color32, CornerRadius, Layout, RichText, Sense, Stroke, Vec2};
use tokio::runtime::Handle;

use crate::domain::{
    Queue, QueueHook, QueueId, QueueSchedule, ShutdownAction, WeekDayMask, random_vivid_color,
};
use crate::ipc_local::Client;
use crate::ui::AppShell;
use crate::ui::components::primitives::{Btn, BtnSize, TextInput, segmented};
use crate::ui::components::sidebar_tree::queue_dot_color;
use crate::ui::gui_state::Cache;
use crate::ui::theme::{self, radius, space};

/// Inputs the body needs from its host shell.
pub struct Ctx<'a> {
    pub state: &'a mut QueuesState,
    pub queue_delete_confirm: &'a mut Option<QueueId>,
    pub client: Arc<Client>,
    pub cache: Arc<Cache>,
    pub rt: Handle,
}

#[derive(Default)]
pub struct QueuesState {
    pub selected: Option<QueueId>,
    pub editor: Option<EditorState>,
    pub hook_kind_start: String,
    pub hook_cmd_start: String,
    pub hook_kind_finish: String,
    pub hook_cmd_finish: String,
    pub color_dialog: Option<ColorDialog>,
}

#[derive(Clone)]
pub struct ColorDialog {
    pub queue_id: QueueId,
    pub draft: [u8; 3],
    /// Color shown when the dialog opened — Reset target.
    pub initial: [u8; 3],
    /// Hue 0..1 — kept independent of `draft` so dragging through
    /// black/grey doesn't reset the hue marker.
    pub hue: f32,
    pub hex_text: String,
}

impl ColorDialog {
    pub fn new(queue_id: QueueId, draft: [u8; 3]) -> Self {
        let hue =
            eframe::egui::ecolor::Hsva::from(Color32::from_rgb(draft[0], draft[1], draft[2])).h;
        let hex_text = format!("#{:02X}{:02X}{:02X}", draft[0], draft[1], draft[2]);
        Self {
            queue_id,
            draft,
            initial: draft,
            hue,
            hex_text,
        }
    }
}

#[derive(Clone)]
pub struct EditorState {
    pub id: QueueId,
    pub name: String,
    pub max_concurrent: usize, // 0 = inherit
    pub stop_on_error: bool,
    pub schedule_kind: ScheduleKind,
    pub daily_start: String,
    pub daily_stop: String,
    pub days_mask: WeekDayMask,
    pub on_finish_kind: FinishKind,
    pub on_finish_cmd: String,
    pub builtin: bool,
    pub color: Option<[u8; 3]>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ScheduleKind {
    Manual,
    Recurring,
    OneOff,
    Condition,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FinishKind {
    Nothing,
    Notify,
    Sleep,
    Shutdown,
    Disconnect,
    RunCommand,
}

impl EditorState {
    fn from_queue(q: &Queue) -> Self {
        let (init_start, init_stop, init_days, kind) = match q.schedule {
            QueueSchedule::Manual => (
                "00:00".into(),
                String::new(),
                WeekDayMask::ALL,
                ScheduleKind::Manual,
            ),
            QueueSchedule::Daily { start, stop, days } => (
                start.format("%H:%M").to_string(),
                stop.map(|t| t.format("%H:%M").to_string())
                    .unwrap_or_default(),
                days,
                ScheduleKind::Recurring,
            ),
            QueueSchedule::Once { .. } => (
                "00:00".into(),
                String::new(),
                WeekDayMask::ALL,
                ScheduleKind::OneOff,
            ),
        };
        let (finish_kind, finish_cmd) = derive_finish(&q.on_finish);
        Self {
            id: q.id,
            name: q.name.clone(),
            max_concurrent: q.max_concurrent.unwrap_or(0),
            stop_on_error: q.stop_on_error,
            schedule_kind: kind,
            daily_start: init_start,
            daily_stop: init_stop,
            days_mask: init_days,
            on_finish_kind: finish_kind,
            on_finish_cmd: finish_cmd,
            builtin: q.builtin,
            color: q.color,
        }
    }
}

fn derive_finish(hooks: &[QueueHook]) -> (FinishKind, String) {
    use QueueHook::*;
    if hooks.is_empty() {
        return (FinishKind::Nothing, String::new());
    }
    match &hooks[0] {
        Notify { .. } => (FinishKind::Notify, String::new()),
        Sleep => (FinishKind::Sleep, String::new()),
        Shutdown(ShutdownAction::ShutDown) => (FinishKind::Shutdown, String::new()),
        ExitOxdm => (FinishKind::Disconnect, String::new()),
        RunCommand { cmd, args } => {
            let mut s = cmd.clone();
            if !args.is_empty() {
                s.push(' ');
                s.push_str(&args.join(" "));
            }
            (FinishKind::RunCommand, s)
        }
        _ => (FinishKind::Nothing, String::new()),
    }
}

/// Delete-queue confirmation viewport. Surfaces only when the target
/// queue has at least one job; empty queues are deleted directly by
/// `AppShell::request_delete_queue`. Spawned from any context: the
/// schedule editor, the sidebar context menu, or the Delete shortcut.
pub fn show_delete_confirm(app: &mut AppShell, ctx: &egui::Context) {
    let Some(qid) = app.queue_delete_confirm else {
        return;
    };
    let Some(q) = app.snap.queues.iter().find(|q| q.id == qid).cloned() else {
        app.queue_delete_confirm = None;
        return;
    };
    let title = format!("oxdm — Delete \"{}\"?", q.name);
    let mut want_close = false;
    let mut confirm = false;
    let closed = super::child_viewport(ctx, "oxdm-queue-del", &title, (440.0, 220.0), |root_ui| {
        let ctx = &root_ui.ctx().clone();
        let t = theme::tokens(ctx);
        egui::Panel::top("qdel_titlebar")
            .frame(egui::Frame::NONE.fill(t.bg_titlebar))
            .show_separator_line(true)
            .show_inside(root_ui, |ui| {
                crate::ui::components::titlebar::show(ui, ctx, &title);
            });
        egui::Panel::bottom("qdel_actions")
            .frame(
                egui::Frame::NONE
                    .fill(t.bg_sunken)
                    .inner_margin(egui::Margin::symmetric(space::S4, space::S2)),
            )
            .show_separator_line(true)
            .show_inside(root_ui, |ui| {
                ui.horizontal(|ui| {
                    if Btn::new("Cancel").ghost().show(ui).clicked() {
                        want_close = true;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if Btn::new("Delete")
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
                let n = q.job_ids.len();
                let line1 = format!("Delete queue \"{}\"?", q.name);
                ui.label(
                    RichText::new(line1)
                        .color(t.fg_1)
                        .font(theme::body_bold(14.0)),
                );
                ui.add_space(space::S2 as f32);
                let plural = if n == 1 { "job" } else { "jobs" };
                let line2 = format!(
                    "{n} {plural} currently in this queue will become queueless. \
                         Files on disk are not touched."
                );
                ui.label(RichText::new(line2).color(t.fg_3).font(theme::body(12.0)));
            });
    });
    if confirm {
        let s = app.client.clone();
        app.spawn(async move {
            let _ = s.delete_queue(qid).await;
        });
        if matches!(app.filter, crate::ui::SidebarFilter::Queue(id) if id == qid) {
            app.filter = app.focus_after_queue_delete(qid);
        }
        app.queue_delete_confirm = None;
    } else if want_close || closed {
        app.queue_delete_confirm = None;
    }
}

pub fn body(c: &mut Ctx<'_>, root_ui: &mut egui::Ui) {
    let ctx = &root_ui.ctx().clone();
    let t = theme::tokens(ctx);

    egui::Panel::top("queues_titlebar")
        .frame(egui::Frame::NONE.fill(t.bg_titlebar))
        .show_separator_line(true)
        .show_inside(root_ui, |ui| {
            crate::ui::components::titlebar::show(ui, ctx, "oxdm — Queues & scheduling");
        });

    let queues = c.cache.queues();
    if c.state.selected.is_none()
        && let Some(q) = queues.first()
    {
        c.state.selected = Some(q.id);
        c.state.editor = Some(EditorState::from_queue(q));
    }

    // Left list.
    egui::Panel::left("queues_left")
        .default_size(240.0)
        .resizable(false)
        .frame(
            egui::Frame::NONE
                .fill(t.bg_sidebar)
                .inner_margin(egui::Margin::symmetric(space::S3, space::S4)),
        )
        .show_separator_line(false)
        .show_inside(root_ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for q in &queues {
                let active = c.state.selected == Some(q.id);
                let dot = queue_dot_color(q, &t);
                if queue_row(ui, &t, dot, &q.name, q.job_ids.len(), active).clicked() {
                    c.state.selected = Some(q.id);
                    c.state.editor = Some(EditorState::from_queue(q));
                }
            }
            ui.add_space(space::S3 as f32);
            if Btn::new("Add queue")
                .toolbar()
                .icon("plus")
                .show(ui)
                .clicked()
            {
                let q = Queue {
                    id: QueueId::new(),
                    name: "New queue".into(),
                    builtin: false,
                    job_ids: Vec::new(),
                    schedule: QueueSchedule::Manual,
                    on_start: Vec::new(),
                    on_finish: Vec::new(),
                    max_concurrent: None,
                    stop_on_error: false,
                    color: Some(random_vivid_color()),
                };
                let id = q.id;
                let s = c.client.clone();
                c.rt.block_on(async move {
                    let _ = s.upsert_queue(q).await;
                });
                c.state.selected = Some(id);
                c.state.editor = None;
            }
        });

    // Bottom action bar.
    let mut want_save = false;
    let mut want_close = false;
    egui::Panel::bottom("queues_actions")
        .frame(
            egui::Frame::NONE
                .fill(t.bg_sunken)
                .inner_margin(egui::Margin::symmetric(space::S4, space::S2)),
        )
        .show_separator_line(true)
        .show_inside(root_ui, |ui| {
            ui.horizontal(|ui| {
                if Btn::new("Cancel").ghost().show(ui).clicked() {
                    want_close = true;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if Btn::new("Save").primary().icon("check").show(ui).clicked() {
                        want_save = true;
                    }
                });
            });
        });

    // Editor.
    let mut delete_id: Option<QueueId> = None;
    let mut color_dialog_to_open: Option<ColorDialog> = None;
    egui::CentralPanel::default()
        .frame(
            egui::Frame::NONE
                .fill(t.bg_page)
                .inner_margin(egui::Margin::symmetric(space::S4, space::S4)),
        )
        .show_inside(root_ui, |ui| {
            let Some(ed) = c.state.editor.as_mut() else {
                ui.label("Select a queue to edit.");
                return;
            };
            // Name + Delete row.
            let mut open_color_for: Option<(QueueId, [u8; 3])> = None;
            ui.horizontal(|ui| {
                let rgb = ed.color.unwrap_or_else(|| {
                    let probe = Queue {
                        id: ed.id,
                        name: ed.name.clone(),
                        builtin: ed.builtin,
                        job_ids: Vec::new(),
                        schedule: QueueSchedule::Manual,
                        on_start: Vec::new(),
                        on_finish: Vec::new(),
                        max_concurrent: None,
                        stop_on_error: false,
                        color: None,
                    };
                    let dc = queue_dot_color(&probe, &t);
                    [dc.r(), dc.g(), dc.b()]
                });
                if color_swatch_button(ui, &t, rgb).clicked() {
                    open_color_for = Some((ed.id, rgb));
                }
                let w = ui.available_width() - 110.0;
                TextInput::new(&mut ed.name)
                    .width(w)
                    .font(theme::body_bold(14.0))
                    .show(ui);
                if Btn::new("Delete")
                    .danger_filled()
                    .icon("trash-2")
                    .enabled(!ed.builtin)
                    .show(ui)
                    .clicked()
                {
                    delete_id = Some(ed.id);
                }
            });
            if let Some((qid, draft)) = open_color_for {
                color_dialog_to_open = Some(ColorDialog::new(qid, draft));
            }
            ui.add_space(space::S4 as f32);

            // Concurrency card.
            card(
                ui,
                &t,
                "layers",
                "Concurrency",
                "How many downloads from this queue can run in parallel.",
                |ui| {
                    let opts = [(1usize, "1×"), (2, "2×"), (3, "3×"), (5, "5×"), (8, "8×")];
                    let preset = opts.iter().position(|(v, _)| *v == ed.max_concurrent);
                    let labels: Vec<(&'static str, Option<&'static str>)> =
                        opts.iter().map(|(_, l)| (*l, None)).collect();
                    ui.horizontal(|ui| {
                        if let Some(i) = segmented(ui, &labels, preset.unwrap_or(usize::MAX)) {
                            ed.max_concurrent = opts[i].0;
                        }
                        let custom_active = preset.is_none();
                        if Btn::new("Custom")
                            .toolbar()
                            .selected(custom_active)
                            .show(ui)
                            .clicked()
                            && !custom_active
                        {
                            ed.max_concurrent = 4;
                        }
                        if custom_active {
                            let mut val = ed.max_concurrent as i32;
                            if ui
                                .add(
                                    egui::DragValue::new(&mut val)
                                        .range(1..=64)
                                        .speed(1.0)
                                        .suffix("×"),
                                )
                                .changed()
                            {
                                ed.max_concurrent = val.clamp(1, 64) as usize;
                            }
                        }
                    });
                },
            );

            ui.add_space(space::S3 as f32);

            // Schedule card.
            card(ui, &t, "calendar", "Schedule", "", |ui| {
                let opts = [
                    ("Manual", Some("calendar")),
                    ("Recurring", Some("refresh-cw")),
                    ("One-off", Some("zap")),
                    ("Condition", Some("wifi")),
                ];
                let sel = match ed.schedule_kind {
                    ScheduleKind::Manual => 0,
                    ScheduleKind::Recurring => 1,
                    ScheduleKind::OneOff => 2,
                    ScheduleKind::Condition => 3,
                };
                if let Some(i) = segmented(ui, &opts, sel) {
                    ed.schedule_kind = match i {
                        0 => ScheduleKind::Manual,
                        1 => ScheduleKind::Recurring,
                        2 => ScheduleKind::OneOff,
                        _ => ScheduleKind::Condition,
                    };
                }
                if ed.schedule_kind == ScheduleKind::Recurring {
                    ui.add_space(space::S3 as f32);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Start").color(t.fg_3).font(theme::body(12.0)));
                        TextInput::new(&mut ed.daily_start).width(80.0).show(ui);
                        ui.label(
                            RichText::new("Stop (optional)")
                                .color(t.fg_3)
                                .font(theme::body(12.0)),
                        );
                        TextInput::new(&mut ed.daily_stop).width(80.0).show(ui);
                    });
                    ui.add_space(space::S2 as f32);
                    ui.horizontal(|ui| {
                        for (i, lbl) in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
                            .iter()
                            .enumerate()
                        {
                            let bit = i as u8;
                            let on = (ed.days_mask.0 >> bit) & 1 == 1;
                            let resp = Btn::new(*lbl)
                                .size(BtnSize::Sm)
                                .selected(on)
                                .toolbar()
                                .show(ui);
                            if resp.clicked() {
                                if on {
                                    ed.days_mask.0 &= !(1 << bit);
                                } else {
                                    ed.days_mask.0 |= 1 << bit;
                                }
                            }
                        }
                    });
                }
            });

            ui.add_space(space::S3 as f32);

            // When the queue finishes card.
            card(ui, &t, "clock", "When the queue finishes", "", |ui| {
                let opts = [
                    ("Nothing", None),
                    ("Notify", Some("bell")),
                    ("Sleep", Some("moon")),
                    ("Shutdown", Some("power")),
                ];
                let sel = match ed.on_finish_kind {
                    FinishKind::Nothing => 0,
                    FinishKind::Notify => 1,
                    FinishKind::Sleep => 2,
                    FinishKind::Shutdown => 3,
                    _ => 0,
                };
                if let Some(i) = segmented(ui, &opts, sel) {
                    ed.on_finish_kind = match i {
                        0 => FinishKind::Nothing,
                        1 => FinishKind::Notify,
                        2 => FinishKind::Sleep,
                        _ => FinishKind::Shutdown,
                    };
                }
                ui.add_space(space::S2 as f32);
                let row2 = [
                    ("Disconnect", Some("unplug")),
                    ("Run command", Some("terminal")),
                ];
                let sel2 = match ed.on_finish_kind {
                    FinishKind::Disconnect => 0,
                    FinishKind::RunCommand => 1,
                    _ => usize::MAX,
                };
                if let Some(i) = segmented(ui, &row2, sel2) {
                    ed.on_finish_kind = match i {
                        0 => FinishKind::Disconnect,
                        _ => FinishKind::RunCommand,
                    };
                }
                if ed.on_finish_kind == FinishKind::RunCommand {
                    ui.add_space(space::S2 as f32);
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Command")
                                .color(t.fg_3)
                                .font(theme::body(12.0)),
                        );
                        let w = ui.available_width();
                        TextInput::new(&mut ed.on_finish_cmd)
                            .width(w)
                            .hint("cmd arg1 arg2")
                            .show(ui);
                    });
                }
            });
        });

    if let Some(id) = delete_id {
        let has_jobs = queues
            .iter()
            .find(|q| q.id == id)
            .map(|q| !q.job_ids.is_empty())
            .unwrap_or(false);
        if has_jobs {
            *c.queue_delete_confirm = Some(id);
        } else {
            let s = c.client.clone();
            let _g = c.rt.enter();
            tokio::spawn(async move {
                let _ = s.delete_queue(id).await;
            });
            c.state.selected = None;
            c.state.editor = None;
        }
    }
    if let Some(d) = color_dialog_to_open {
        c.state.color_dialog = Some(d);
    }
    if c.state.color_dialog.is_some() {
        run_color_dialog(c, ctx);
    }
    if want_close {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
    if want_save && let Some(ed) = c.state.editor.clone() {
        persist(c, ed);
    }
}

fn persist(c: &mut Ctx<'_>, ed: EditorState) {
    let new_name = ed.name.trim().to_string();
    let max_c = if ed.max_concurrent == 0 {
        None
    } else {
        Some(ed.max_concurrent)
    };
    let schedule = match ed.schedule_kind {
        ScheduleKind::Manual => QueueSchedule::Manual,
        ScheduleKind::Recurring => {
            let start = NaiveTime::parse_from_str(&ed.daily_start, "%H:%M")
                .unwrap_or_else(|_| NaiveTime::from_hms_opt(0, 0, 0).unwrap());
            let stop = if ed.daily_stop.trim().is_empty() {
                None
            } else {
                NaiveTime::parse_from_str(&ed.daily_stop, "%H:%M").ok()
            };
            QueueSchedule::Daily {
                start,
                stop,
                days: ed.days_mask,
            }
        }
        ScheduleKind::OneOff => QueueSchedule::Manual, // placeholder
        ScheduleKind::Condition => QueueSchedule::Manual, // placeholder
    };
    let on_finish = match ed.on_finish_kind {
        FinishKind::Nothing => Vec::new(),
        FinishKind::Notify => vec![QueueHook::Notify {
            title: "Queue finished".into(),
            body: String::new(),
        }],
        FinishKind::Sleep => vec![QueueHook::Sleep],
        FinishKind::Shutdown => vec![QueueHook::Shutdown(ShutdownAction::ShutDown)],
        FinishKind::Disconnect => vec![QueueHook::ExitOxdm],
        FinishKind::RunCommand => {
            let raw = ed.on_finish_cmd.trim().to_string();
            if raw.is_empty() {
                Vec::new()
            } else {
                let mut p = raw.split_whitespace();
                let cmd = p.next().unwrap_or("").to_string();
                let args: Vec<String> = p.map(String::from).collect();
                vec![QueueHook::RunCommand { cmd, args }]
            }
        }
    };
    let id = ed.id;
    let builtin = ed.builtin;
    let stop_on_error = ed.stop_on_error;
    let color = ed.color;
    let s = c.client.clone();
    let existing = c.cache.queues().into_iter().find(|q| q.id == id);
    let _g = c.rt.enter();
    tokio::spawn(async move {
        let mut q = existing.unwrap_or_else(|| Queue {
            id,
            name: new_name.clone(),
            builtin,
            job_ids: Vec::new(),
            schedule: QueueSchedule::Manual,
            on_start: Vec::new(),
            on_finish: Vec::new(),
            max_concurrent: None,
            stop_on_error: false,
            color: None,
        });
        q.name = new_name;
        q.schedule = schedule;
        q.max_concurrent = max_c;
        q.stop_on_error = stop_on_error;
        q.on_finish = on_finish;
        q.color = color;
        let _ = s.upsert_queue(q).await;
    });
}

fn card(
    ui: &mut egui::Ui,
    t: &theme::Tokens,
    icon: &'static str,
    title: &str,
    subtitle: &str,
    body: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::NONE
        .fill(t.bg_surface)
        .stroke(Stroke::new(t.border_width, t.border_subtle))
        .corner_radius(theme::surface::RADIUS)
        .inner_margin(space::S3 as f32)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                crate::ui::utils::icons::show(ui, icon, 17.0, t.fg_2);
                ui.label(
                    RichText::new(title)
                        .color(t.fg_1)
                        .font(theme::body_bold(13.0)),
                );
            });
            if !subtitle.is_empty() {
                ui.label(
                    RichText::new(subtitle)
                        .color(t.fg_3)
                        .font(theme::body(12.0)),
                );
            }
            ui.add_space(space::S2 as f32);
            body(ui);
        });
}

fn queue_row(
    ui: &mut egui::Ui,
    t: &theme::Tokens,
    dot: Color32,
    name: &str,
    count: usize,
    active: bool,
) -> egui::Response {
    let h = 36.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::click());
    let painter = ui.painter().clone();
    if active {
        painter.rect_filled(rect, radius::SM as f32, t.bg_surface);
        painter.rect_stroke(
            rect,
            radius::SM as f32,
            Stroke::new(t.border_width, t.border_brand),
            egui::StrokeKind::Inside,
        );
    } else if resp.hovered() {
        painter.rect_filled(rect, radius::SM as f32, t.bg_sunken);
    }
    painter.circle_filled(egui::pos2(rect.left() + 14.0, rect.center().y), 5.0, dot);
    let g = painter.layout_no_wrap(name.to_owned(), theme::body_bold(13.0), t.fg_1);
    painter.galley(
        egui::pos2(rect.left() + 28.0, rect.center().y - g.size().y / 2.0),
        g,
        t.fg_1,
    );

    let cnt = format!("{count}×");
    let cg = painter.layout_no_wrap(cnt, theme::mono(11.0), t.fg_3);
    painter.galley(
        egui::pos2(
            rect.right() - 12.0 - cg.size().x,
            rect.center().y - cg.size().y / 2.0,
        ),
        cg,
        t.fg_3,
    );

    let _ = CornerRadius::ZERO;
    resp
}

/// Square swatch button. Hover cursor flips to crosshair to read as a
/// color picker affordance. Click opens the standalone color dialog.
fn color_swatch_button(ui: &mut egui::Ui, t: &theme::Tokens, rgb: [u8; 3]) -> egui::Response {
    let size = Vec2::new(28.0, 22.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let swatch = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
    let painter = ui.painter().clone();
    painter.rect_filled(rect, radius::SM as f32, swatch);
    painter.rect_stroke(
        rect,
        radius::SM as f32,
        Stroke::new(t.border_width, t.border_subtle),
        egui::StrokeKind::Inside,
    );
    if resp.hovered() {
        ui.set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    resp.on_hover_text("Pick color")
}

/// Stand-alone, transactional color picker. Renders as a child
/// viewport with Cancel / Reset / Save. Save commits the draft to
/// the editor and persists it; Cancel and the window-X discard.
fn run_color_dialog(c: &mut Ctx<'_>, ctx: &egui::Context) {
    let mut want_close = false;
    let mut want_save: Option<[u8; 3]> = None;
    let closed = super::child_viewport_fit(
        ctx,
        "oxdm-queue-color",
        "oxdm — Pick queue color",
        360.0,
        |root_ui| {
            let ctx = &root_ui.ctx().clone();
            let t = theme::tokens(ctx);
            let Some(dlg) = c.state.color_dialog.as_mut() else {
                return;
            };
            // Stacked frames (no Panel) so the root ui's min_rect
            // tracks actual content — `child_viewport_fit` uses it to
            // size the viewport.
            root_ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                egui::Frame::NONE
                    .fill(t.bg_titlebar)
                    .stroke(egui::Stroke::NONE)
                    .show(ui, |ui| {
                        crate::ui::components::titlebar::show(ui, ctx, "oxdm — Pick queue color");
                    });
                let sep_y = ui.cursor().min.y;
                ui.painter().hline(
                    ui.max_rect().x_range(),
                    sep_y,
                    egui::Stroke::new(1.0, t.border_subtle),
                );
                egui::Frame::NONE
                    .fill(t.bg_page)
                    .inner_margin(egui::Margin::symmetric(space::S4, space::S3))
                    .show(ui, |ui| color_picker_body(ui, &t, dlg));
                let footer_sep_y = ui.cursor().min.y;
                ui.painter().hline(
                    ui.max_rect().x_range(),
                    footer_sep_y,
                    egui::Stroke::new(1.0, t.border_subtle),
                );
                egui::Frame::NONE
                    .fill(t.bg_titlebar)
                    .inner_margin(egui::Margin::symmetric(space::S4, space::S2))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if Btn::new("Cancel").ghost().show(ui).clicked() {
                                want_close = true;
                            }
                            if Btn::new("Reset")
                                .toolbar()
                                .icon("rotate-cw")
                                .show(ui)
                                .clicked()
                            {
                                let o = dlg.initial;
                                dlg.draft = o;
                                let hsv =
                                    egui::ecolor::Hsva::from(Color32::from_rgb(o[0], o[1], o[2]));
                                if hsv.s > 0.0 && hsv.v > 0.0 {
                                    dlg.hue = hsv.h;
                                }
                                dlg.hex_text = hex_string(o);
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if Btn::new("Save").primary().icon("check").show(ui).clicked() {
                                    want_save = Some(dlg.draft);
                                }
                            });
                        });
                    });
            });
        },
    );
    if let Some(rgb) = want_save {
        if let Some(ed) = c.state.editor.as_mut() {
            ed.color = Some(rgb);
        }
        c.state.color_dialog = None;
    } else if want_close || closed {
        c.state.color_dialog = None;
    }
}

fn color_picker_body(ui: &mut egui::Ui, t: &theme::Tokens, dlg: &mut ColorDialog) {
    ui.spacing_mut().item_spacing.y = space::S3 as f32;

    // Current HSV from stored hue + sat/val derived from draft.
    let mut hsv =
        egui::ecolor::Hsva::from(Color32::from_rgb(dlg.draft[0], dlg.draft[1], dlg.draft[2]));
    hsv.h = dlg.hue;
    let mut s = hsv.s;
    let mut v = hsv.v;

    // 2D saturation / value box. Anchored to both sides — fills the
    // available width and grows on resize.
    if sv_box(ui, dlg.hue, &mut s, &mut v).changed() {
        let new = Color32::from(egui::ecolor::Hsva::new(dlg.hue, s, v, 1.0));
        dlg.draft = [new.r(), new.g(), new.b()];
        dlg.hex_text = hex_string(dlg.draft);
    }

    // Hue strip.
    let mut h = dlg.hue;
    if hue_strip(ui, &mut h).changed() {
        dlg.hue = h;
        let new = Color32::from(egui::ecolor::Hsva::new(h, s, v, 1.0));
        dlg.draft = [new.r(), new.g(), new.b()];
        dlg.hex_text = hex_string(dlg.draft);
    }

    // Preview swatch + hex + paste row.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::S2 as f32;
        let (sw_rect, _) = ui.allocate_exact_size(Vec2::new(36.0, 28.0), Sense::hover());
        ui.painter().rect_filled(
            sw_rect,
            radius::SM as f32,
            Color32::from_rgb(dlg.draft[0], dlg.draft[1], dlg.draft[2]),
        );
        ui.painter().rect_stroke(
            sw_rect,
            radius::SM as f32,
            Stroke::new(1.0, t.border_subtle),
            egui::StrokeKind::Inside,
        );

        ui.label(
            RichText::new("HEX")
                .color(t.fg_3)
                .font(theme::body_bold(11.0)),
        );
        let resp = TextInput::new(&mut dlg.hex_text)
            .width(96.0)
            .font(theme::mono(12.0))
            .show(ui);
        if resp.changed()
            && let Some(rgb) = parse_hex(&dlg.hex_text)
        {
            dlg.draft = rgb;
            let nh = egui::ecolor::Hsva::from(Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
            if nh.s > 0.0 && nh.v > 0.0 {
                dlg.hue = nh.h;
            }
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if Btn::new("Paste")
                .toolbar()
                .icon("clipboard")
                .size(BtnSize::Sm)
                .show(ui)
                .clicked()
                && let Some(rgb) = read_clipboard_color()
            {
                dlg.draft = rgb;
                dlg.hex_text = hex_string(rgb);
                let nh = egui::ecolor::Hsva::from(Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
                if nh.s > 0.0 && nh.v > 0.0 {
                    dlg.hue = nh.h;
                }
            }
        });
    });

    // R / G / B numeric inputs. Always visible alongside hex. Mono
    // font matches the hex field above. Fixed-width so 3-digit values
    // (e.g. "255") fit without the field re-flowing the row.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::S2 as f32;
        ui.style_mut().override_font_id = Some(theme::mono(12.0));
        let field_size = egui::vec2(56.0, theme::control::H_MD);
        for (i, label) in ["R", "G", "B"].iter().enumerate() {
            ui.label(
                RichText::new(*label)
                    .color(t.fg_3)
                    .font(theme::body_bold(11.0)),
            );
            let mut val = dlg.draft[i] as i32;
            let resp = ui.add_sized(
                field_size,
                egui::DragValue::new(&mut val).range(0..=255).speed(1.0),
            );
            if resp.changed() {
                dlg.draft[i] = val.clamp(0, 255) as u8;
                dlg.hex_text = hex_string(dlg.draft);
                let nh = egui::ecolor::Hsva::from(Color32::from_rgb(
                    dlg.draft[0],
                    dlg.draft[1],
                    dlg.draft[2],
                ));
                if nh.s > 0.0 && nh.v > 0.0 {
                    dlg.hue = nh.h;
                }
            }
        }
    });
}

fn sv_box(ui: &mut egui::Ui, hue: f32, s: &mut f32, v: &mut f32) -> egui::Response {
    // Sane bounds so the SV box never collapses (unreadable) or
    // pushes the rest of the form off-screen on a wide window.
    let w = ui.available_width().clamp(240.0, 520.0);
    let h = (w * 0.6).clamp(140.0, 320.0);
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click_and_drag());
    let painter = ui.painter().clone();

    // Bilinear gradient: TL=white, TR=hue-pure, BL=BR=black. egui's
    // mesh tessellator linearly interpolates the per-vertex colours
    // across the two triangles, giving a standard SV box.
    let hue_color = Color32::from(egui::ecolor::Hsva::new(hue, 1.0, 1.0, 1.0));
    let mut mesh = egui::Mesh::default();
    let i0 = mesh.vertices.len() as u32;
    mesh.colored_vertex(rect.left_top(), Color32::WHITE);
    mesh.colored_vertex(rect.right_top(), hue_color);
    mesh.colored_vertex(rect.left_bottom(), Color32::BLACK);
    mesh.colored_vertex(rect.right_bottom(), Color32::BLACK);
    mesh.add_triangle(i0, i0 + 1, i0 + 3);
    mesh.add_triangle(i0, i0 + 3, i0 + 2);
    painter.add(egui::Shape::mesh(mesh));

    if (resp.dragged() || resp.clicked())
        && let Some(p) = resp.interact_pointer_pos()
    {
        *s = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        *v = 1.0 - ((p.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
        resp.mark_changed();
    }

    let cx = rect.left() + *s * rect.width();
    let cy = rect.top() + (1.0 - *v) * rect.height();
    painter.circle_stroke(egui::pos2(cx, cy), 6.0, Stroke::new(2.0, Color32::WHITE));
    painter.circle_stroke(egui::pos2(cx, cy), 6.0, Stroke::new(1.0, Color32::BLACK));

    if resp.hovered() {
        ui.set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    resp
}

fn hue_strip(ui: &mut egui::Ui, h: &mut f32) -> egui::Response {
    let w = ui.available_width().clamp(240.0, 520.0);
    let height = 18.0;
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(w, height), Sense::click_and_drag());
    let painter = ui.painter().clone();

    let stops = [
        Color32::from_rgb(255, 0, 0),
        Color32::from_rgb(255, 255, 0),
        Color32::from_rgb(0, 255, 0),
        Color32::from_rgb(0, 255, 255),
        Color32::from_rgb(0, 0, 255),
        Color32::from_rgb(255, 0, 255),
        Color32::from_rgb(255, 0, 0),
    ];
    let n = stops.len();
    let mut mesh = egui::Mesh::default();
    for (i, color) in stops.iter().enumerate() {
        let x = rect.left() + (i as f32) / (n as f32 - 1.0) * rect.width();
        mesh.colored_vertex(egui::pos2(x, rect.top()), *color);
        mesh.colored_vertex(egui::pos2(x, rect.bottom()), *color);
    }
    for i in 0..(n - 1) {
        let a = (i * 2) as u32;
        mesh.add_triangle(a, a + 1, a + 2);
        mesh.add_triangle(a + 1, a + 3, a + 2);
    }
    painter.add(egui::Shape::mesh(mesh));

    if (resp.dragged() || resp.clicked())
        && let Some(p) = resp.interact_pointer_pos()
    {
        *h = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        resp.mark_changed();
    }

    let cx = rect.left() + *h * rect.width();
    painter.line_segment(
        [
            egui::pos2(cx, rect.top() - 2.0),
            egui::pos2(cx, rect.bottom() + 2.0),
        ],
        Stroke::new(3.0, Color32::WHITE),
    );
    painter.line_segment(
        [
            egui::pos2(cx, rect.top() - 2.0),
            egui::pos2(cx, rect.bottom() + 2.0),
        ],
        Stroke::new(1.0, Color32::BLACK),
    );

    if resp.hovered() {
        ui.set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    resp
}

fn hex_string(rgb: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
}

fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    match s.len() {
        6 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            Some([r, g, b])
        }
        3 => {
            let mut chars = s.chars();
            let to = |c: char| u8::from_str_radix(&c.to_string(), 16).ok().map(|x| x * 17);
            Some([to(chars.next()?)?, to(chars.next()?)?, to(chars.next()?)?])
        }
        _ => None,
    }
}

fn parse_rgb(s: &str) -> Option<[u8; 3]> {
    let s = s.trim();
    let inner = s
        .strip_prefix("rgb(")
        .and_then(|r| r.strip_suffix(')'))
        .or_else(|| s.strip_prefix("rgba(").and_then(|r| r.strip_suffix(')')))
        .unwrap_or(s);
    let parts: Vec<&str> = inner
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .collect();
    if parts.len() < 3 {
        return None;
    }
    let r: u8 = parts[0].parse().ok()?;
    let g: u8 = parts[1].parse().ok()?;
    let b: u8 = parts[2].parse().ok()?;
    Some([r, g, b])
}

fn read_clipboard_color() -> Option<[u8; 3]> {
    let text = crate::ui::clipboard::read_text()?;
    parse_hex(text.trim()).or_else(|| parse_rgb(text.trim()))
}
