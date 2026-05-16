//! Batch-capture triage window (`oxdm gui batch <staged-path>`).
//!
//! Spawned by the WS bridge when an extension sends
//! `batch_capture { interactive: true }`. The bridge stages the list as
//! JSON in a temp file (see `crate::ipc::batch::stage_for_dialog`); this
//! subprocess loads + deletes the file, kicks off one HEAD probe per
//! row through the daemon's `Probe` IPC, and renders a table where the
//! user can deselect rows, pick a target queue, and submit.
//!
//! On submit, each selected row goes through `add_job` + `set_job_queue`
//! over `ipc_local`. The window then closes; per-job download windows
//! are *not* surfaced automatically (the queue's start policy decides
//! when each job runs).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use eframe::egui::{self, Align, Color32, Layout, RichText, ScrollArea, Stroke};
use tokio::runtime::Runtime;

use crate::domain::{CaptureRequest, QueueId};
use crate::ipc_local::Client;
use crate::ipc_local::protocol::{AddJobReq, GuiKind, SubFilter};
use crate::ui::components::primitives::{Btn, Combo};
use crate::ui::gui_state::Cache;
use crate::ui::theme::{self, space};
use crate::ui::utils::icons;

/// One row in the table. `selected` defaults to `true` so a careless
/// user pressing Send still gets everything; the staging step already
/// went through filters once.
#[derive(Clone)]
struct Row {
    req: CaptureRequest,
    selected: bool,
    /// Server-supplied filename + size + resume status, filled in
    /// asynchronously by the probe pool. `None` while probing,
    /// `Some(Err(_))` on failure.
    probe: Option<Result<Probed, String>>,
}

#[derive(Clone)]
struct Probed {
    filename: String,
    size: Option<u64>,
    is_resumable: bool,
}

pub fn launch(path: PathBuf) {
    let items = match crate::ipc::batch::load_and_consume(&path) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(?path, error = %e, "batch: could not load staged file");
            return;
        }
    };
    if items.is_empty() {
        return;
    }

    let rt = Runtime::new().expect("tokio runtime");
    let (client, cache) = match rt.block_on(connect()) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "batch: could not reach oxdm daemon");
            return;
        }
    };

    let viewport = crate::ui::utils::chrome::viewport_builder(
        "oxdm — Send to oxdm",
        (760.0, 520.0),
        Some((520.0, 360.0)),
    );
    let opts = eframe::NativeOptions {
        viewport,
        vsync: false,
        ..Default::default()
    };

    let _ = eframe::run_native(
        "oxdm-batch",
        opts,
        Box::new(move |cc| {
            let ctx = cc.egui_ctx.clone();
            theme::install_fonts(&cc.egui_ctx);
            icons::install_loaders(&cc.egui_ctx);
            crate::ui::gui_state::spawn_event_loop(
                rt.handle(),
                client.clone(),
                cache.clone(),
                SubFilter::Lifecycle,
                move || ctx.request_repaint(),
            );
            theme::apply(&cc.egui_ctx, &cache.settings());
            let ctx_for_theme = cc.egui_ctx.clone();
            theme::on_system_theme_change(move |_| ctx_for_theme.request_repaint());

            let main_qid = cache
                .queues()
                .iter()
                .find(|q| q.builtin)
                .map(|q| q.id)
                .or_else(|| cache.queues().first().map(|q| q.id));
            let shell = BatchShell::new(rt, client.clone(), cache, items, main_qid);
            shell.spawn_probes();
            Ok(Box::new(shell))
        }),
    );
}

async fn connect() -> Result<(Arc<Client>, Arc<Cache>), String> {
    let client = crate::ui::connect_or_spawn_daemon().await?;
    client.hello(GuiKind::Batch).await?;
    let snap = client.snapshot().await?;
    let cache = Arc::new(Cache::from_snapshot(snap));
    Ok((client, cache))
}

struct BatchShell {
    rt: Runtime,
    client: Arc<Client>,
    cache: Arc<Cache>,
    rows: Arc<Mutex<Vec<Row>>>,
    queue: Option<QueueId>,
    submitting: bool,
    want_close: bool,
    start_now: bool,
    theme_applied_for: Option<theme::ResolvedTheme>,
    #[cfg(target_os = "windows")]
    surfaced: bool,
}

impl BatchShell {
    fn new(
        rt: Runtime,
        client: Arc<Client>,
        cache: Arc<Cache>,
        items: Vec<CaptureRequest>,
        queue: Option<QueueId>,
    ) -> Self {
        let rows = items
            .into_iter()
            .map(|req| Row {
                req,
                selected: true,
                probe: None,
            })
            .collect();
        Self {
            rt,
            client,
            cache,
            rows: Arc::new(Mutex::new(rows)),
            queue,
            submitting: false,
            want_close: false,
            start_now: false,
            theme_applied_for: None,
            #[cfg(target_os = "windows")]
            surfaced: false,
        }
    }

    fn spawn_probes(&self) {
        let snapshot: Vec<(usize, url::Url)> = {
            let rows = self.rows.lock().expect("rows mutex");
            rows.iter()
                .enumerate()
                .map(|(i, r)| (i, r.req.url.clone()))
                .collect()
        };
        for (idx, url) in snapshot {
            let client = self.client.clone();
            let rows = self.rows.clone();
            let _g = self.rt.handle().enter();
            tokio::spawn(async move {
                let outcome = match client.probe(url).await {
                    Ok(Ok(p)) => Ok(Probed {
                        filename: p.filename,
                        size: p.size,
                        is_resumable: p.is_resumable,
                    }),
                    Ok(Err(reason)) => Err(reason),
                    Err(e) => Err(e),
                };
                let mut guard = rows.lock().expect("rows mutex");
                if let Some(row) = guard.get_mut(idx) {
                    row.probe = Some(outcome);
                }
            });
        }
    }

    fn submit(&mut self) {
        if self.submitting {
            return;
        }
        let target = self.queue;
        let rows_snapshot: Vec<Row> = {
            let g = self.rows.lock().expect("rows mutex");
            g.iter().filter(|r| r.selected).cloned().collect()
        };
        if rows_snapshot.is_empty() {
            return;
        }
        let save_dir = self.cache.settings().download_dir.clone();
        let client = self.client.clone();
        let start_now = self.start_now;
        self.submitting = true;
        self.rt.block_on(async move {
            for row in rows_snapshot {
                let mut headers = row.req.headers.clone();
                if let Some(ua) = row.req.user_agent.as_deref()
                    && !headers.contains_key("User-Agent")
                {
                    headers.insert("User-Agent".into(), ua.into());
                }
                if let Some(c) = row.req.cookies.as_deref()
                    && !headers.contains_key("Cookie")
                {
                    headers.insert("Cookie".into(), c.into());
                }
                if let Some(ref r) = row.req.referrer
                    && !headers.contains_key("Referer")
                {
                    headers.insert("Referer".into(), r.to_string());
                }
                let filename = row.req.filename.clone().or_else(|| {
                    row.probe
                        .as_ref()
                        .and_then(|p| p.as_ref().ok().map(|x| x.filename.clone()))
                });
                let req = AddJobReq {
                    url: row.req.url.clone(),
                    save_dir: save_dir.clone(),
                    filename,
                    referrer: row.req.referrer.clone(),
                    headers,
                    max_connections: None,
                    proxy: None,
                    auth_user: None,
                    auth_password: None,
                    proxy_password: None,
                    cookies: None,
                    category: None,
                };
                match client.add_job(req).await {
                    Ok(id) => {
                        if let Some(qid) = target {
                            let _ = client.set_job_queue(id, qid).await;
                        }
                        if start_now {
                            let _ = client.start_job(id).await;
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "batch: add_job failed"),
                }
            }
        });
        self.want_close = true;
    }
}

impl eframe::App for BatchShell {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        crate::ui::utils::chrome::raw_input_hook(ctx, raw_input);
    }

    fn ui(&mut self, root_ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = &root_ui.ctx().clone();
        #[cfg(target_os = "windows")]
        if !self.surfaced {
            self.surfaced = true;
            crate::ui::platform::windows::bring_to_foreground(frame);
        }
        #[cfg(not(target_os = "windows"))]
        let _ = frame;

        if crate::ui::gui_state::daemon_lost() || crate::ui::gui_state::close_requested() {
            std::process::exit(0);
        }
        if crate::ui::gui_state::take_focus_request() {
            crate::ui::gui_state::surface_window(ctx);
        }
        if ctx.input(|i| i.viewport().close_requested()) || self.want_close {
            std::process::exit(0);
        }

        let resolved_now = theme::resolve(self.cache.settings().theme);
        if self.theme_applied_for != Some(resolved_now) {
            theme::apply(ctx, &self.cache.settings());
            self.theme_applied_for = Some(resolved_now);
        }

        crate::ui::utils::resize::show_styled(
            ctx,
            crate::ui::utils::resize::ChromeStyle {
                dark_border: true,
                resizable: true,
            },
        );

        let t = theme::tokens(ctx);
        let queues = self.cache.queues();

        egui::Panel::bottom("batch-footer")
            .frame(
                egui::Frame::NONE
                    .fill(t.bg_surface)
                    .stroke(Stroke::new(t.border_width, t.border_default))
                    .inner_margin(space::S3),
            )
            .show_inside(root_ui, |ui| {
                ui.horizontal(|ui| {
                    if Btn::new("Cancel").ghost().show(ui).clicked() {
                        std::process::exit(0);
                    }
                    ui.checkbox(&mut self.start_now, "Start now");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let n_selected = self
                            .rows
                            .lock()
                            .expect("rows")
                            .iter()
                            .filter(|r| r.selected)
                            .count();
                        let enabled = n_selected > 0 && !self.submitting;
                        let clicked = ui
                            .add_enabled_ui(enabled, |ui| {
                                Btn::new(format!("Send {n_selected} to oxdm"))
                                    .primary()
                                    .icon("download")
                                    .show(ui)
                                    .clicked()
                            })
                            .inner;
                        if clicked {
                            self.submit();
                        }
                    });
                });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(t.bg_page).inner_margin(space::S4))
            .show_inside(root_ui, |ui| {
                ui.spacing_mut().item_spacing.y = space::S2 as f32;

                let n_total = self.rows.lock().expect("rows").len();
                let n_selected = self
                    .rows
                    .lock()
                    .expect("rows")
                    .iter()
                    .filter(|r| r.selected)
                    .count();
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "Send {n_selected} of {n_total} link{} to oxdm",
                            if n_total == 1 { "" } else { "s" }
                        ))
                        .font(theme::body_bold(14.0))
                        .color(t.fg_1),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if !queues.is_empty() {
                            let label = self
                                .queue
                                .and_then(|qid| queues.iter().find(|q| q.id == qid))
                                .map(|q| q.name.clone())
                                .unwrap_or_else(|| "(no queue)".into());
                            Combo::new("batch-queue", label)
                                .width(160.0)
                                .show(ui, |ui| {
                                    for q in &queues {
                                        if Combo::item(ui, &q.name, true).clicked() {
                                            self.queue = Some(q.id);
                                            ui.close();
                                        }
                                    }
                                });
                            ui.label(eframe::egui::RichText::new("Queue").color(t.fg_2));
                        }
                    });
                });

                ui.separator();

                ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        let mut rows = self.rows.lock().expect("rows mutex");
                        let mut all_selected = rows.iter().all(|r| r.selected);
                        let any = !rows.is_empty();
                        if any && ui.checkbox(&mut all_selected, "Select all").changed() {
                            for r in rows.iter_mut() {
                                r.selected = all_selected;
                            }
                        }
                        ui.add_space(space::S1 as f32);
                        let total_rows = rows.len();
                        for (i, row) in rows.iter_mut().enumerate() {
                            let mut sel = row.selected;
                            let label = match &row.probe {
                                None => format!("{}  …", row.req.url),
                                Some(Ok(p)) => {
                                    let sz = p.size.map(fmt_bytes).unwrap_or_else(|| "—".into());
                                    let res = if p.is_resumable {
                                        "resumable"
                                    } else {
                                        "no resume"
                                    };
                                    format!("{}  ·  {}  ·  {sz}  ·  {res}", row.req.url, p.filename)
                                }
                                Some(Err(e)) => format!("{}  —  probe failed: {e}", row.req.url),
                            };
                            if ui.checkbox(&mut sel, label).changed() {
                                row.selected = sel;
                            }
                            if i + 1 < total_rows {
                                ui.separator();
                            }
                        }
                    });
            });

        let _ = Color32::WHITE; // silence unused-import lints when chrome flag toggles
    }
}

fn fmt_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{:.1} {}", v, UNITS[i])
    }
}
