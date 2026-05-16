//! Standalone per-download GUI subprocess (`oxdm gui download <id>`).
//!
//! Layout (top → bottom):
//!   - Custom titlebar with filename
//!   - Header card: extension tile, filename + meta, big % readout, pill
//!     progress bar
//!   - Stat strip: SPEED · TIME LEFT · DOWNLOADED · TOTAL
//!   - Collapsible "Transfer rate" card with a live polyline+area chart
//!   - Collapsible "Segments" card with a per-part progress sub-table
//!   - Optional "Speed limiter" card
//!   - Optional "On completion" card
//!   - Footer: "Minimize to tray" left; Pause + Cancel right
//!
//! The chart and segment table are hidden by default to save compute;
//! the user expands them on demand via the card chevron.

pub mod state;

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Align, Color32, Layout, Pos2, RichText, Sense, Stroke, Vec2};
use tokio::runtime::Runtime;

use self::state::{DownloadState, Tab};
use crate::domain::{Category, JobId, OnCompletion, Phase, ShutdownAction};
use crate::ipc_local::Client;
use crate::ipc_local::protocol::{PartView, SubFilter};
use crate::ui::components::icon_row::icon_row;
use crate::ui::components::primitives::{
    Btn, BtnSize, Combo, TabBtn, TextInput, collapsible_card, pill_progress, status_dot,
};
use crate::ui::components::titlebar;
use crate::ui::gui_state::Cache;
use crate::ui::theme::{self, radius, space};
use crate::ui::utils::format::{format_bytes, format_eta, format_speed};
use crate::ui::utils::icons;

const CHART_SAMPLES: usize = 120;
const CHART_INTERVAL: Duration = Duration::from_millis(500);

/// Cap on the inner scroll viewport so the window can't grow beyond a
/// usable size when both collapsibles are expanded.
const SCROLL_MAX_H: f32 = 520.0;
/// Hard cap on the window inner height.
const MAX_WINDOW_H: f32 = 700.0;
/// Max height the segments table scrolls within.
const SEGMENTS_MAX_H: f32 = 220.0;

pub fn launch(id: JobId) {
    let rt = Runtime::new().expect("tokio runtime");
    let (client, cache) = match rt.block_on(connect(id)) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "could not reach oxdm daemon");
            return;
        }
    };

    let title = format!("oxdm — download {id}");
    let viewport =
        crate::ui::utils::chrome::viewport_builder(&title, (540.0, 460.0), Some((530.0, 320.0)));
    let opts = eframe::NativeOptions {
        viewport,
        vsync: false,
        ..Default::default()
    };

    let _ = eframe::run_native(
        "oxdm-download",
        opts,
        Box::new(move |cc| {
            let ctx = cc.egui_ctx.clone();
            theme::install_fonts(&cc.egui_ctx);
            icons::install_loaders(&cc.egui_ctx);
            crate::ui::gui_state::spawn_event_loop(
                rt.handle(),
                client.clone(),
                cache.clone(),
                SubFilter::Job(id),
                move || ctx.request_repaint(),
            );
            theme::apply(&cc.egui_ctx, &cache.settings());
            let ctx_for_theme = cc.egui_ctx.clone();
            theme::on_system_theme_change(move |_| ctx_for_theme.request_repaint());
            Ok(Box::new(DownloadShell::new(rt, client, cache, id)))
        }),
    );
}

async fn connect(id: JobId) -> Result<(Arc<Client>, Arc<Cache>), String> {
    let client = crate::ui::connect_or_spawn_daemon().await?;
    client
        .hello(crate::ipc_local::protocol::GuiKind::Download(id))
        .await?;
    let snap = client.snapshot().await?;
    let cache = Arc::new(Cache::from_snapshot(snap));
    Ok((client, cache))
}

struct DownloadShell {
    rt: Runtime,
    client: Arc<Client>,
    cache: Arc<Cache>,
    id: JobId,
    dlw: DownloadState,
    want_close: bool,
    chart: ChartBuf,
    /// Last pct shown to the user. Used to suppress brief flashes back to
    /// 0% during phase transitions (pause/resume) when the daemon may
    /// emit a transient counters frame with `downloaded == 0` or
    /// `total == None` before settling.
    displayed_pct: f64,
    auto_resize: crate::ui::utils::chrome::AutoResize,
    theme_applied_for: Option<theme::ResolvedTheme>,
    #[cfg(target_os = "windows")]
    surfaced: bool,
}

#[derive(Default)]
struct ChartBuf {
    samples: VecDeque<f32>,
    last_push: Option<Instant>,
    peak: f32,
    avg: f32,
}

impl ChartBuf {
    fn reset(&mut self) {
        self.samples.clear();
        self.last_push = None;
        self.peak = 0.0;
        self.avg = 0.0;
    }

    fn maybe_push(&mut self, value_bps: f32) {
        let now = Instant::now();
        let due = self
            .last_push
            .map(|t| now.duration_since(t) >= CHART_INTERVAL)
            .unwrap_or(true);
        if !due {
            return;
        }
        self.push_now(value_bps);
    }

    fn push_now(&mut self, value_bps: f32) {
        self.last_push = Some(Instant::now());
        if self.samples.len() == CHART_SAMPLES {
            self.samples.pop_front();
        }
        self.samples.push_back(value_bps);
        let n = self.samples.len() as f32;
        let sum: f32 = self.samples.iter().sum();
        self.avg = sum / n.max(1.0);
        self.peak = self.samples.iter().copied().fold(0.0_f32, f32::max);
    }

    fn is_flat_zero(&self) -> bool {
        self.samples.iter().all(|v| *v == 0.0)
    }
}

impl DownloadShell {
    fn new(rt: Runtime, client: Arc<Client>, cache: Arc<Cache>, id: JobId) -> Self {
        Self {
            rt,
            client,
            cache,
            id,
            dlw: DownloadState::default(),
            want_close: false,
            chart: ChartBuf::default(),
            displayed_pct: 0.0,
            auto_resize: crate::ui::utils::chrome::AutoResize::new(MAX_WINDOW_H, true, 530.0),
            theme_applied_for: None,
            #[cfg(target_os = "windows")]
            surfaced: false,
        }
    }
    fn block_on<F: std::future::Future>(&self, f: F) -> F::Output {
        self.rt.handle().block_on(f)
    }
    fn spawn<F>(&self, f: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.rt.spawn(f);
    }
}

impl eframe::App for DownloadShell {
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
        if self.want_close
            || crate::ui::gui_state::daemon_lost()
            || crate::ui::gui_state::close_requested()
        {
            std::process::exit(0);
        }
        if crate::ui::gui_state::take_focus_request() {
            crate::ui::gui_state::surface_window(ctx);
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            self.want_close = true;
            return;
        }

        crate::ui::utils::resize::show_styled(
            ctx,
            crate::ui::utils::resize::ChromeStyle {
                dark_border: true,
                resizable: true,
            },
        );

        let resolved_now = theme::resolve(self.cache.settings().theme);
        if self.theme_applied_for != Some(resolved_now) {
            theme::apply(ctx, &self.cache.settings());
            self.theme_applied_for = Some(resolved_now);
        }

        let id = self.id;
        let t = theme::tokens(ctx);

        let Some(entry) = self.cache.job_entry_cached(id) else {
            egui::CentralPanel::default().show_inside(root_ui, |ui| {
                ui.label("Job not found.");
            });
            return;
        };
        let counters = entry.counters.clone();
        let downloaded = counters.downloaded;
        let total = counters.total;
        let phase = counters.phase;
        let speed = if phase.is_running() {
            counters.speed_bps
        } else {
            0.0
        };
        let raw_pct = match total {
            Some(t) if t > 0 => (downloaded as f64 / t as f64) * 100.0,
            _ => 0.0,
        };
        // Smooth out transient counters frames during phase transitions
        // (e.g. pause→resume can briefly emit downloaded=0). Only update
        // displayed_pct when the frame looks valid: known total and any
        // progress, OR explicit terminal phases. Reset to 0 when the job
        // genuinely starts over (Queued/Discovering before any bytes).
        let frame_valid = matches!(phase, Phase::Completed | Phase::Failed | Phase::Cancelled)
            || (total.is_some() && downloaded > 0);
        if matches!(phase, Phase::Queued) && downloaded == 0 {
            self.displayed_pct = 0.0;
        } else if frame_valid {
            self.displayed_pct = raw_pct;
        }
        let pct = self.displayed_pct;
        let eta_text = match (total, speed) {
            (Some(t), s) if s > 1.0 && t > downloaded => {
                format_eta(((t - downloaded) as f64 / s) as u64)
            }
            _ => "—".into(),
        };
        let filename = entry.job.filename.clone().unwrap_or_else(|| {
            entry
                .job
                .url
                .path()
                .rsplit('/')
                .next()
                .unwrap_or("download")
                .into()
        });
        let host = entry.job.url.host_str().unwrap_or("").to_string();

        if !self.dlw.fetched_extras {
            let s = self.client.clone();
            let view = self
                .block_on(async move { s.job_entry(id).await })
                .ok()
                .flatten();
            if let Some(v) = view {
                self.dlw.on_completion_draft = Some(v.on_completion);
                let session = v.session_speed_override;
                let persisted = entry.job.speed_limit_override;
                let effective = if session != 0 {
                    Some(session)
                } else {
                    persisted
                };
                self.dlw.speed_enabled_draft = Some(effective.is_some());
                self.dlw.speed_kbs_draft = effective
                    .map(|b| ((b as f64) / 1024.0).round() as u64)
                    .unwrap_or(100)
                    .to_string();
                self.dlw.remember_speed = persisted.is_some();
                self.dlw.max_conn_draft = entry
                    .job
                    .max_connections
                    .map(|n| n.to_string())
                    .unwrap_or_default();
            }
            self.dlw.fetched_extras = true;
        }

        // Chart sampling: only when the Transfer rate card is expanded.
        // Running: push live speed. Stopped (paused/complete/failed):
        // keep pushing 0 at the same cadence so the trace drains to a
        // flat line, then stop sampling once every sample is 0.
        let chart_id = egui::Id::new(("dlw-chart-open", id));
        let chart_open: bool = ctx.data_mut(|d| *d.get_persisted_mut_or(chart_id, false));
        let is_running = phase.is_running();
        if chart_open {
            if is_running {
                self.chart.maybe_push(counters.speed_bps as f32);
            } else if !self.chart.samples.is_empty() && !self.chart.is_flat_zero() {
                self.chart.maybe_push(0.0);
            }
        }
        let minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));

        // Custom titlebar.
        let title_resp = egui::Panel::top("dlw_titlebar")
            .frame(egui::Frame::NONE.fill(t.bg_titlebar))
            .show_separator_line(true)
            .show_inside(root_ui, |ui| {
                titlebar::show(ui, ctx, &filename);
            });
        let title_h = title_resp.response.rect.height();

        // Completed jobs swap the entire body (and footer) for the
        // "Download complete" view — IDM-style summary card with Open /
        // Open folder / Close + a "don't show again" checkbox bound to
        // the global `show_complete_dialog` setting.
        if phase == Phase::Completed {
            // Honour the global "Don't show this dialog again" toggle:
            // if the user opted out, an already-open per-job window
            // should just close on completion rather than swap to the
            // summary view.
            if !self.cache.settings().show_complete_dialog {
                self.want_close = true;
                return;
            }
            self.render_complete(root_ui, &t, title_h, &filename, &entry, downloaded);
            return;
        }

        // Footer.
        let mut want_pause = false;
        let mut want_resume = false;
        let mut want_cancel = false;
        let mut want_minimize = false;
        let mut want_apply_speed: Option<(bool, Option<u64>, bool)> = None;
        let mut want_apply_on_completion: Option<OnCompletion> = None;
        let mut want_apply_max_conn: Option<Option<u64>> = None;
        let pause_label = if phase == Phase::Paused || !counters.running {
            "Resume"
        } else {
            "Pause"
        };
        let pause_icon = if pause_label == "Resume" {
            "play"
        } else {
            "pause"
        };

        let footer_resp = egui::Panel::bottom("dlw_footer")
            .frame(
                egui::Frame::NONE
                    .fill(t.bg_sunken)
                    .inner_margin(egui::Margin::symmetric(space::S4, space::S2)),
            )
            .show_separator_line(true)
            .show_inside(root_ui, |ui| {
                ui.horizontal(|ui| {
                    if Btn::new("Minimize to tray")
                        .toolbar()
                        .icon("minimize-2")
                        .show(ui)
                        .clicked()
                    {
                        want_minimize = true;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if Btn::new("Cancel").ghost().icon("x").show(ui).clicked() {
                            want_cancel = true;
                        }
                        if Btn::new(pause_label)
                            .primary()
                            .icon(pause_icon)
                            .show(ui)
                            .clicked()
                        {
                            if pause_label == "Resume" {
                                want_resume = true;
                            } else {
                                want_pause = true;
                            }
                        }
                    });
                });
            });
        let footer_h = footer_resp.response.rect.height();

        let mut content_h = 0.0_f32;
        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(t.bg_page)
                    .inner_margin(egui::Margin::symmetric(space::S3, space::S3)),
            )
            .show_inside(root_ui, |ui| {
                let scope = ui.scope(|ui| {
                    ui.spacing_mut().item_spacing.y = space::S2 as f32;

                    // Header card stays above the tabs (dialog identity).
                    header_card(
                        ui,
                        &t,
                        &filename,
                        &host,
                        &category_meta(&filename, &t),
                        counters.is_resumable,
                        pct,
                        downloaded,
                        total,
                        phase,
                    );

                    // Tab strip.
                    let tabs = ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = space::S4 as f32;
                        let mut sep_y = ui.cursor().top();
                        for (which, label, icon) in [
                            (Tab::Info, "Info", "info"),
                            (Tab::Speed, "Speed", "gauge"),
                            (Tab::OnCompletion, "On Completion", "circle-check-big"),
                        ] {
                            let resp = TabBtn::new(label)
                                .icon(icon)
                                .icon_size(13.0)
                                .pad_x(0.0)
                                .height(28.0)
                                .active(self.dlw.tab == which)
                                .show(ui);
                            // Take the hairline y from a tab's OWN rect bottom,
                            // not the surrounding `ui.horizontal` rect (which
                            // drifts by the row's interact_size/padding and
                            // leaves a gap above the active underline).
                            sep_y = resp.rect.bottom();
                            if resp.clicked() {
                                self.dlw.tab = which;
                                ui.request_repaint();
                            }
                        }
                        sep_y
                    });
                    // Hairline under tab strip — active tab underline sits flush at rect.bottom().
                    // Drawn full window width by widening the painter's clip rect.
                    let sep_y = tabs.inner;
                    let full = ctx.content_rect();
                    ui.painter().clone().with_clip_rect(full).line_segment(
                        [
                            Pos2::new(full.left(), sep_y),
                            Pos2::new(full.right(), sep_y),
                        ],
                        Stroke::new(1.0, t.border_subtle),
                    );
                    ui.add_space(space::S3 as f32);

                    egui::ScrollArea::vertical()
                        .auto_shrink([false, true])
                        .max_height(SCROLL_MAX_H)
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = space::S3 as f32;
                            match self.dlw.tab {
                                Tab::Info => {
                                    let speed_color = if speed > 0.0 {
                                        t.action_primary
                                    } else {
                                        t.fg_3
                                    };
                                    stat_strip(
                                        ui,
                                        &t,
                                        speed,
                                        speed_color,
                                        &eta_text,
                                        downloaded,
                                        total,
                                    );

                                    let _ = collapsible_card(
                                        ui,
                                        chart_id,
                                        "Transfer rate",
                                        None,
                                        false,
                                        |ui| {
                                            if !minimized {
                                                draw_chart(ui, &t, &mut self.chart);
                                            }
                                        },
                                    );

                                    let seg_count = counters.parts.len();
                                    let seg_right = Some(
                                        RichText::new(format!("{seg_count} parallel connections"))
                                            .color(t.fg_3)
                                            .font(theme::body(11.0))
                                            .into(),
                                    );
                                    let seg_id = egui::Id::new(("dlw-segments-open", id));
                                    let _ = collapsible_card(
                                        ui,
                                        seg_id,
                                        "Segments",
                                        seg_right,
                                        false,
                                        |ui| {
                                            segments_table(ui, &t, &counters.parts);
                                        },
                                    );
                                }
                                Tab::Speed => {
                                    egui::Frame::NONE
                                        .fill(t.bg_surface)
                                        .stroke(Stroke::new(t.border_width, t.border_subtle))
                                        .corner_radius(theme::surface::RADIUS)
                                        .inner_margin(space::S3 as f32)
                                        .show(ui, |ui| {
                                            ui.spacing_mut().item_spacing.y = space::S3 as f32;
                                            let mut enabled =
                                                self.dlw.speed_enabled_draft.unwrap_or(false);
                                            ui.checkbox(&mut enabled, "Use speed limiter");
                                            self.dlw.speed_enabled_draft = Some(enabled);
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    RichText::new("Maximum speed (KB/s)")
                                                        .color(t.fg_2),
                                                );
                                                TextInput::new(&mut self.dlw.speed_kbs_draft)
                                                    .width(120.0)
                                                    .enabled(enabled)
                                                    .show(ui);
                                            });
                                            ui.checkbox(
                                                &mut self.dlw.remember_speed,
                                                "Remember for this file",
                                            );
                                            ui.add_space(space::S3 as f32);
                                            ui.separator();
                                            ui.add_space(space::S3 as f32);
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    RichText::new("Max parallel connections")
                                                        .color(t.fg_2),
                                                );
                                                let resp =
                                                    TextInput::new(&mut self.dlw.max_conn_draft)
                                                        .width(60.0)
                                                        .hint("auto")
                                                        .show(ui);
                                                if resp.changed() {
                                                    let trimmed =
                                                        self.dlw.max_conn_draft.trim().to_string();
                                                    if let Ok(v) = trimmed.parse::<u64>()
                                                        && v > 16
                                                    {
                                                        self.dlw.max_conn_draft = "16".into();
                                                    }
                                                }
                                                ui.label(
                                                    RichText::new("(1–16, blank = auto)")
                                                        .color(t.fg_3)
                                                        .font(theme::body(11.0)),
                                                );
                                            });
                                            ui.add_space(space::S3 as f32);
                                            if Btn::new("Apply")
                                                .primary()
                                                .size(BtnSize::Sm)
                                                .show(ui)
                                                .clicked()
                                            {
                                                let kbs: Option<u64> =
                                                    self.dlw.speed_kbs_draft.trim().parse().ok();
                                                let bps = if enabled {
                                                    kbs.map(|k| k.saturating_mul(1024))
                                                } else {
                                                    None
                                                };
                                                want_apply_speed =
                                                    Some((true, bps, self.dlw.remember_speed));
                                                let raw = self.dlw.max_conn_draft.trim();
                                                let mc: Option<u64> = if raw.is_empty() {
                                                    None
                                                } else {
                                                    raw.parse::<u64>().ok().map(|v| v.clamp(1, 16))
                                                };
                                                want_apply_max_conn = Some(mc);
                                            }
                                        });
                                }
                                Tab::OnCompletion => {
                                    if self.dlw.on_completion_draft.is_none() {
                                        self.dlw.on_completion_draft =
                                            Some(OnCompletion::default());
                                    }
                                    let prefs = self.dlw.on_completion_draft.as_mut().unwrap();
                                    egui::Frame::NONE
                                        .fill(t.bg_surface)
                                        .stroke(Stroke::new(t.border_width, t.border_subtle))
                                        .corner_radius(theme::surface::RADIUS)
                                        .inner_margin(space::S3 as f32)
                                        .show(ui, |ui| {
                                            ui.spacing_mut().item_spacing.y = space::S3 as f32;
                                            ui.checkbox(
                                                &mut prefs.show_dialog,
                                                "Show notification when done",
                                            );
                                            ui.add_enabled_ui(!prefs.show_dialog, |ui| {
                                                ui.checkbox(
                                                    &mut prefs.exit_app,
                                                    "Exit oxdm when done",
                                                );
                                                let mut shutdown_on = prefs.shutdown.is_some();
                                                let mut shutdown_kind = prefs
                                                    .shutdown
                                                    .unwrap_or(ShutdownAction::ShutDown);
                                                ui.horizontal(|ui| {
                                                    ui.checkbox(&mut shutdown_on, "Power action");
                                                    ui.add_enabled_ui(shutdown_on, |ui| {
                                                        Combo::new(
                                                            "oc_shutdown",
                                                            format!("{:?}", shutdown_kind),
                                                        )
                                                        .width(140.0)
                                                        .show(ui, |ui| {
                                                            let options: &[(
                                                                ShutdownAction,
                                                                &str,
                                                            )] = &[
                                                                (
                                                                    ShutdownAction::ShutDown,
                                                                    "Shut down",
                                                                ),
                                                                (
                                                                    ShutdownAction::Restart,
                                                                    "Restart",
                                                                ),
                                                                (ShutdownAction::Sleep, "Sleep"),
                                                            ];
                                                            for (val, label) in options {
                                                                if Combo::item(ui, label, true)
                                                                    .clicked()
                                                                {
                                                                    shutdown_kind = *val;
                                                                    ui.close();
                                                                }
                                                            }
                                                        });
                                                    });
                                                });
                                                prefs.shutdown = if shutdown_on {
                                                    Some(shutdown_kind)
                                                } else {
                                                    None
                                                };
                                                ui.checkbox(
                                                    &mut prefs.force_terminate,
                                                    "Force terminate",
                                                );
                                            });
                                            ui.add_space(space::S3 as f32);
                                            if Btn::new("Apply")
                                                .primary()
                                                .size(BtnSize::Sm)
                                                .show(ui)
                                                .clicked()
                                            {
                                                want_apply_on_completion = Some(prefs.clone());
                                            }
                                        });
                                }
                            }
                        });
                });
                content_h = scope.response.rect.height();
            });

        let margin_v = (space::S3 as f32 * 2.0) + 4.0;
        let target_h = title_h + footer_h + content_h + margin_v;
        self.auto_resize.apply(ctx, target_h);

        if let Some((_, bps, remember)) = want_apply_speed {
            let st = self.client.clone();
            self.spawn(async move {
                let _ = st.set_session_speed_limit(id, bps).await;
                if remember {
                    let _ = st.set_persistent_speed_limit(id, bps).await;
                }
            });
        }
        if let Some(prefs) = want_apply_on_completion {
            let st = self.client.clone();
            self.spawn(async move {
                let _ = st.set_on_completion(id, prefs).await;
            });
        }
        if let Some(n) = want_apply_max_conn {
            let st = self.client.clone();
            self.spawn(async move {
                let _ = st.set_max_connections(id, n).await;
            });
        }
        if want_pause {
            let st = self.client.clone();
            self.spawn(async move {
                let _ = st.pause(id).await;
            });
        }
        if want_resume {
            let st = self.client.clone();
            self.spawn(async move {
                let _ = st.resume(id).await;
            });
        }
        if want_cancel {
            let st = self.client.clone();
            self.spawn(async move {
                let _ = st.cancel_to_queued(id).await;
            });
            self.want_close = true;
        }
        if want_minimize {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }

        // Continuous repaint while downloading. Skip while minimized —
        // nothing visible, no need to wake up. Also keep waking while the
        // chart is still draining zeros after a stop, so the flat-line
        // animation completes without user interaction.
        let draining_chart = chart_open
            && !phase.is_running()
            && !self.chart.samples.is_empty()
            && !self.chart.is_flat_zero();
        if !minimized && (phase.is_running() || draining_chart) {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }
}

impl DownloadShell {
    fn render_complete(
        &mut self,
        root_ui: &mut egui::Ui,
        t: &theme::Tokens,
        title_h: f32,
        filename: &str,
        entry: &crate::ipc_local::protocol::JobEntryView,
        downloaded: u64,
    ) {
        let ctx = &root_ui.ctx().clone();
        let url = entry.job.url.to_string();
        let saved_as = entry
            .job
            .status
            .final_path
            .clone()
            .unwrap_or_else(|| std::path::PathBuf::from(filename));
        let cat = category_meta(filename, t);

        let mut want_open = false;
        let mut want_open_folder = false;
        let mut want_close = false;
        let mut want_toggle: Option<bool> = None;

        let mut content_h = 0.0_f32;
        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(t.bg_page)
                    .inner_margin(egui::Margin::symmetric(space::S4, space::S3)),
            )
            .show_inside(root_ui, |ui| {
                let scope = ui.scope(|ui| {
                    ui.spacing_mut().item_spacing.y = space::S3 as f32;

                    // Header card: icon tile + "Download complete" + size.
                    egui::Frame::NONE
                        .fill(t.bg_surface)
                        .stroke(Stroke::new(t.border_width, t.border_subtle))
                        .corner_radius(theme::surface::RADIUS)
                        .inner_margin(space::S3 as f32)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let tile = 56.0;
                                let (rect, _) =
                                    ui.allocate_exact_size(Vec2::splat(tile), Sense::hover());
                                let painter = ui.painter().clone();
                                let bg = soft_tint(cat.color, t.bg_surface, 0.20);
                                painter.rect_filled(rect, radius::SM as f32, bg);
                                let label = if cat.ext.is_empty() {
                                    cat.icon.to_uppercase()
                                } else {
                                    cat.ext.clone()
                                };
                                let g = painter.layout_no_wrap(
                                    label,
                                    theme::body_bold(13.0),
                                    cat.color,
                                );
                                painter.galley(
                                    Pos2::new(
                                        rect.center().x - g.size().x / 2.0,
                                        rect.center().y - g.size().y / 2.0,
                                    ),
                                    g,
                                    cat.color,
                                );
                                ui.add_space(space::S3 as f32);
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    ui.label(
                                        RichText::new("Download complete")
                                            .font(theme::display(20.0))
                                            .color(t.fg_1),
                                    );
                                    ui.label(
                                        RichText::new(format!(
                                            "Downloaded {} ({} bytes)",
                                            format_bytes(downloaded),
                                            downloaded,
                                        ))
                                        .color(t.fg_2)
                                        .font(theme::body(12.0)),
                                    );
                                });
                            });
                        });

                    // Address.
                    ui.label(
                        RichText::new("Address")
                            .color(t.fg_3)
                            .font(theme::body(11.0)),
                    );
                    let mut url_view = url.clone();
                    let w = ui.available_width();
                    TextInput::new(&mut url_view)
                        .width(w)
                        .font(theme::mono(11.0))
                        .show(ui);

                    // Saved as.
                    ui.label(
                        RichText::new("The file saved as")
                            .color(t.fg_3)
                            .font(theme::body(11.0)),
                    );
                    let mut path_view = saved_as.to_string_lossy().to_string();
                    let pw = ui.available_width();
                    TextInput::new(&mut path_view)
                        .width(pw)
                        .font(theme::mono(11.0))
                        .show(ui);

                    ui.add_space(space::S2 as f32);

                    // Action row.
                    ui.horizontal(|ui| {
                        if Btn::new("Open").primary().icon("play").show(ui).clicked() {
                            want_open = true;
                        }
                        if Btn::new(crate::ui::platform::reveal_label())
                            .toolbar()
                            .icon("folder")
                            .show(ui)
                            .clicked()
                        {
                            want_open_folder = true;
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if Btn::new("Close").toolbar().icon("x").show(ui).clicked() {
                                want_close = true;
                            }
                        });
                    });

                    ui.add_space(space::S2 as f32);

                    // Bound to the *global* setting: checking suppresses
                    // the dialog for future completions everywhere
                    // (IDM-style). Polarity is inverted vs the setting
                    // so the checkbox label can stay positive.
                    let show_global = self.cache.settings().show_complete_dialog;
                    let mut dont_show = !show_global;
                    if ui
                        .checkbox(&mut dont_show, "Don't show this dialog again")
                        .changed()
                    {
                        want_toggle = Some(!dont_show);
                    }
                });
                content_h = scope.response.rect.height();
            });

        let target_h = (title_h + content_h + space::S4 as f32 + 16.0).max(220.0);
        self.auto_resize.apply(ctx, target_h);

        if want_open {
            crate::ui::platform::open_path(&saved_as);
        }
        if want_open_folder {
            crate::ui::platform::reveal_in_folder(&saved_as);
        }
        if want_close {
            self.want_close = true;
        }
        if let Some(new_show) = want_toggle {
            let mut s = self.cache.settings();
            s.show_complete_dialog = new_show;
            let cl = self.client.clone();
            self.spawn(async move {
                let _ = cl.update_settings(s).await;
            });
        }
    }
}

// ---- Components ---------------------------------------------------

struct CategoryMeta {
    label: &'static str,
    color: Color32,
    ext: String,
    icon: &'static str,
}

fn category_meta(filename: &str, t: &theme::Tokens) -> CategoryMeta {
    let ext = filename
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_uppercase())
        .unwrap_or_default();
    let cat = crate::domain::classify(filename, &Default::default());
    let (label, color, icon): (&'static str, Color32, &'static str) = match cat {
        Category::Compressed => ("Compressed", t.cat_compressed, "archive"),
        Category::Programs => ("Programs", t.cat_programs, "package"),
        Category::Videos => ("Videos", t.cat_videos, "film"),
        Category::Music => ("Music", t.cat_music, "music"),
        Category::Pictures => ("Pictures", t.cat_pictures, "image"),
        Category::Documents => ("Documents", t.cat_documents, "file-text"),
        Category::Other => ("Other", t.fg_3, "file"),
    };
    CategoryMeta {
        label,
        color,
        ext,
        icon,
    }
}

#[allow(clippy::too_many_arguments)]
fn header_card(
    ui: &mut egui::Ui,
    t: &theme::Tokens,
    filename: &str,
    host: &str,
    cat: &CategoryMeta,
    is_resumable: i8,
    pct: f64,
    downloaded: u64,
    total: Option<u64>,
    phase: Phase,
) {
    let resumable = match is_resumable {
        1 => "resumable",
        -1 => "no resume",
        _ => "checking",
    };
    let _ = downloaded;

    icon_row(
        ui,
        56.0,
        |ui, rect| {
            let painter = ui.painter().clone();
            let bg = soft_tint(cat.color, t.bg_surface, 0.20);
            painter.rect_filled(rect, radius::SM as f32, bg);
            let label = if cat.ext.is_empty() {
                cat.icon.to_uppercase()
            } else {
                cat.ext.clone()
            };
            let g = painter.layout_no_wrap(label, theme::body_bold(13.0), cat.color);
            painter.galley(
                Pos2::new(
                    rect.center().x - g.size().x / 2.0,
                    rect.center().y - g.size().y / 2.0,
                ),
                g,
                cat.color,
            );
        },
        |ui| {
            ui.label(
                RichText::new(filename)
                    .font(theme::body_bold(14.0))
                    .color(t.fg_1),
            );
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(RichText::new(host).color(t.fg_3).font(theme::mono(11.0)));
                ui.label(RichText::new("·").color(t.fg_4));
                ui.label(
                    RichText::new(cat.label)
                        .color(t.fg_3)
                        .font(theme::body(11.0)),
                );
                ui.label(RichText::new("·").color(t.fg_4));
                ui.label(
                    RichText::new(resumable)
                        .color(t.fg_3)
                        .font(theme::body(11.0)),
                );
            });
        },
        |ui| {
            let pct_text = if total.is_some() {
                format!("{}%", pct.round() as i32)
            } else {
                "—".into()
            };
            ui.label(
                RichText::new(pct_text)
                    .font(theme::display(28.0))
                    .color(t.fg_1),
            );
        },
    );

    // Animated striped progress under header.
    let bar_w = ui.available_width();
    let frac = (pct / 100.0) as f32;
    let (track, fill) = match phase {
        Phase::Completed => (t.status_success_bg, t.status_success),
        Phase::Failed => (t.status_danger_bg, t.status_danger),
        // Idle / not-yet-started phases use the same muted grey as
        // Paused so the bar only goes clay once bytes are actually
        // moving.
        Phase::Paused | Phase::Queued | Phase::Cancelled => (t.progress_track, t.fg_4),
        _ => (t.progress_track, t.progress_fill),
    };
    let animate = matches!(
        phase,
        Phase::Downloading
            | Phase::Evaluating
            | Phase::Assembling
            | Phase::ResolvingConflicts
            | Phase::Flushing
            | Phase::Verifying
    );
    // Active-download fill is a horizontal gradient (clay-400 → clay-300).
    // Other phases keep their solid status color.
    let fill_gradient = if animate {
        Some((
            Color32::from_rgb(0xC9, 0x70, 0x3F),
            Color32::from_rgb(0xDA, 0x8E, 0x63),
        ))
    } else {
        None
    };
    crate::ui::components::primitives::striped_progress(
        ui,
        frac,
        bar_w,
        10.0,
        track,
        fill,
        fill_gradient,
        animate,
        // CentralPanel fill = `bg_page` — pass it so the corner-mask
        // outside-stroke blends with the surrounding bg.
        t.bg_page,
    );
}

fn stat_strip(
    ui: &mut egui::Ui,
    t: &theme::Tokens,
    speed: f64,
    speed_color: Color32,
    eta_text: &str,
    downloaded: u64,
    total: Option<u64>,
) {
    egui::Frame::NONE
        .fill(t.bg_sunken)
        .corner_radius(theme::surface::RADIUS)
        .inner_margin(egui::Margin::same(space::S1))
        .show(ui, |ui| {
            ui.columns(4, |cols| {
                stat(&mut cols[0], t, "speed", &format_speed(speed), speed_color);
                stat(&mut cols[1], t, "time left", eta_text, t.fg_1);
                stat(
                    &mut cols[2],
                    t,
                    "downloaded",
                    &format_bytes(downloaded),
                    t.fg_1,
                );
                stat(
                    &mut cols[3],
                    t,
                    "total",
                    &total.map(format_bytes).unwrap_or_else(|| "—".into()),
                    t.fg_1,
                );
            });
        });
}

fn stat(ui: &mut egui::Ui, t: &theme::Tokens, label: &str, value: &str, value_color: Color32) {
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(space::S2, space::S2))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = theme::space::S0 as f32;
                let upper = label.to_uppercase();
                let mut job = egui::text::LayoutJob::default();
                job.append(
                    &upper,
                    0.0,
                    egui::TextFormat {
                        font_id: theme::body(9.0),
                        color: t.fg_3,
                        extra_letter_spacing: 0.8,
                        ..Default::default()
                    },
                );
                ui.label(job);
                ui.label(
                    RichText::new(value)
                        .font(theme::mono(14.0))
                        .color(value_color),
                );
            });
        });
}

fn draw_chart(ui: &mut egui::Ui, t: &theme::Tokens, chart: &mut ChartBuf) {
    let w = ui.available_width();
    let h = 124.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    let painter = ui.painter().clone();

    // Rounded background. Inner rect insets so gridlines/data stay
    // clear of corners.
    painter.rect_filled(rect, theme::radius::XS as f32, t.bg_sunken);
    let pad_x = 12.0;
    let pad_top = 22.0;
    let pad_bot = 10.0;
    let inner = egui::Rect::from_min_max(
        Pos2::new(rect.left() + pad_x, rect.top() + pad_top),
        Pos2::new(rect.right() - pad_x, rect.bottom() - pad_bot),
    );

    let has_data = chart.peak > 0.0;
    let max = chart.peak.max(1.0);
    // 4 dotted gridlines + labels inside (top=max .. bottom=0). The
    // bottom label is always rendered as "0"; the upper labels only
    // appear once we have real samples (peak > 0), so an empty chart
    // doesn't display meaningless fractions of the fallback max.
    let grid_color = Color32::from_rgba_unmultiplied(t.fg_4.r(), t.fg_4.g(), t.fg_4.b(), 170);
    let lines = 4;
    for i in 0..lines {
        let y = inner.top() + (i as f32) * inner.height() / (lines as f32 - 1.0);
        let mut x = inner.left();
        while x < inner.right() {
            let x2 = (x + 1.8).min(inner.right());
            painter.line_segment(
                [Pos2::new(x, y), Pos2::new(x2, y)],
                Stroke::new(1.2, grid_color),
            );
            x += 4.5;
        }
        let is_bottom = i == lines - 1;
        if is_bottom || has_data {
            let value = max * (lines - 1 - i) as f32 / (lines as f32 - 1.0);
            let label = if is_bottom {
                "0 B/s".to_string()
            } else {
                format_speed(value as f64)
            };
            let g = painter.layout_no_wrap(label, theme::mono(10.0), t.fg_3);
            painter.galley(Pos2::new(inner.left(), y - g.size().y - 2.0), g, t.fg_3);
        }
    }

    // Avg dashed line.
    if chart.avg > 0.0 {
        let avg_y = inner.bottom() - (chart.avg / max).clamp(0.0, 1.0) * inner.height();
        let mut x = inner.left();
        while x < inner.right() {
            let x2 = (x + 6.0).min(inner.right());
            painter.line_segment(
                [Pos2::new(x, avg_y), Pos2::new(x2, avg_y)],
                Stroke::new(1.2, t.fg_2),
            );
            x += 12.0;
        }
    }

    // Polyline + area fill within inner rect.
    if chart.samples.len() >= 2 {
        let n = chart.samples.len();
        let dx = inner.width() / (CHART_SAMPLES as f32 - 1.0);
        let start_x = inner.right() - dx * (n as f32 - 1.0);
        let mut points: Vec<Pos2> = Vec::with_capacity(n);
        for (i, v) in chart.samples.iter().enumerate() {
            let x = start_x + dx * (i as f32);
            let y = inner.bottom() - (*v / max).clamp(0.0, 1.0) * inner.height();
            points.push(Pos2::new(x, y));
        }
        let fill = Color32::from_rgba_unmultiplied(
            t.action_primary.r(),
            t.action_primary.g(),
            t.action_primary.b(),
            36,
        );
        for w in points.windows(2) {
            let a = w[0];
            let b = w[1];
            let quad = vec![
                a,
                b,
                Pos2::new(b.x, inner.bottom()),
                Pos2::new(a.x, inner.bottom()),
            ];
            painter.add(egui::Shape::convex_polygon(quad, fill, Stroke::NONE));
        }
        painter.add(egui::Shape::line(
            points,
            Stroke::new(2.0, t.action_primary),
        ));
    }

    // Legend below chart.
    let current = chart.samples.back().copied().unwrap_or(0.0);
    let avg = chart.avg;
    let peak = chart.peak;
    let mut want_reset = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::S4 as f32;
        legend_chip(ui, t, t.action_primary, "Current", current);
        legend_chip(ui, t, t.fg_4, "Avg", avg);
        legend_chip(ui, t, t.action_primary_press, "Peak", peak);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if Btn::new("")
                .toolbar()
                .size(BtnSize::Sm)
                .icon_only("rotate-cw")
                .tooltip("Reset chart")
                .show(ui)
                .clicked()
            {
                want_reset = true;
            }
        });
    });
    if want_reset {
        chart.reset();
    }
}

fn legend_chip(ui: &mut egui::Ui, t: &theme::Tokens, color: Color32, label: &str, value: f32) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let (r, _) = ui.allocate_exact_size(Vec2::new(8.0, 8.0), Sense::hover());
        ui.painter().rect_filled(r, 2.0, color);
        ui.label(RichText::new(label).color(t.fg_3).font(theme::body(11.0)));
        ui.label(
            RichText::new(format_speed(value as f64))
                .color(t.fg_1)
                .font(theme::body_bold(11.0)),
        );
    });
}

fn segments_table(ui: &mut egui::Ui, t: &theme::Tokens, parts: &[PartView]) {
    use egui_extras::{Column, TableBuilder};
    if parts.is_empty() {
        ui.label(RichText::new("No active segments yet.").color(t.fg_3));
        return;
    }
    TableBuilder::new(ui)
        .id_salt("dlw_segments")
        .max_scroll_height(SEGMENTS_MAX_H)
        .vscroll(true)
        .resizable(false)
        .column(Column::initial(28.0))
        .column(Column::initial(80.0))
        .column(Column::initial(100.0))
        .column(Column::initial(90.0))
        .column(Column::remainder())
        .column(Column::initial(48.0))
        .header(22.0, |mut h| {
            for label in ["#", "STATUS", "DOWNLOADED", "TOTAL", "PROGRESS", ""] {
                h.col(|ui| {
                    ui.label(
                        RichText::new(label)
                            .color(t.fg_3)
                            .font(theme::body_bold(10.0)),
                    );
                });
            }
        })
        .body(|body| {
            body.rows(28.0, parts.len(), |mut row| {
                let i = row.index();
                let p = &parts[i];
                let pct = if p.size == 0 {
                    0.0
                } else {
                    p.downloaded as f64 / p.size as f64
                };
                row.col(|ui| {
                    ui.label(
                        RichText::new(format!("{:02}", i + 1))
                            .font(theme::mono(11.0))
                            .color(t.fg_2),
                    );
                });
                row.col(|ui| {
                    let (color, text) = if p.finished {
                        (t.status_success, "Done")
                    } else if p.speed_bps > 0.0 {
                        (t.action_primary, "Active")
                    } else {
                        (t.fg_3, "Idle")
                    };
                    status_dot(ui, color, text, 11.0);
                });
                row.col(|ui| {
                    ui.label(
                        RichText::new(format_bytes(p.downloaded))
                            .font(theme::mono(11.0))
                            .color(t.fg_2),
                    );
                });
                row.col(|ui| {
                    ui.label(
                        RichText::new(format_bytes(p.size))
                            .font(theme::mono(11.0))
                            .color(t.fg_2),
                    );
                });
                row.col(|ui| {
                    let avail = ui.available_width().min(180.0);
                    pill_progress(
                        ui,
                        pct as f32,
                        avail,
                        6.0,
                        t.progress_track,
                        t.progress_fill,
                    );
                });
                row.col(|ui| {
                    ui.label(
                        RichText::new(format!("{}%", (pct * 100.0).round() as i32))
                            .font(theme::mono(11.0))
                            .color(t.fg_2),
                    );
                });
            });
        });
}

fn soft_tint(accent: Color32, base: Color32, t: f32) -> Color32 {
    let lerp = |a: u8, b: u8| (a as f32 * (1.0 - t) + b as f32 * t) as u8;
    Color32::from_rgb(
        lerp(base.r(), accent.r()),
        lerp(base.g(), accent.g()),
        lerp(base.b(), accent.b()),
    )
}
