//! Standalone Queues & scheduling window subprocess (`oxdm gui queues`).
//!
//! Connects to the daemon, mirrors the queue list via `Cache`, renders
//! the existing `dialogs::queues` body via a `Ctx`. The body's editor
//! Delete button writes a queue id into `queue_delete_confirm`; this
//! shell renders an inline Area confirm overlay because the subprocess
//! does not own a sidebar to surface the prompt elsewhere.

use std::sync::Arc;

use eframe::egui::{self, Align, Color32, Layout, RichText, Stroke};
use tokio::runtime::Runtime;

use crate::domain::QueueId;
use crate::ipc_local::Client;
use crate::ipc_local::protocol::{GuiKind, SubFilter};
use crate::ui::components::primitives::Btn;
use crate::ui::dialogs::queues::{Ctx, QueuesState};
use crate::ui::gui_state::Cache;
use crate::ui::theme::{self, space};
use crate::ui::utils::icons;

pub fn launch() {
    let rt = Runtime::new().expect("tokio runtime");
    let (client, cache) = match rt.block_on(connect()) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "could not reach oxdm daemon");
            return;
        }
    };

    let viewport = crate::ui::utils::chrome::viewport_builder(
        "oxdm — Queues & scheduling",
        (820.0, 620.0),
        Some((640.0, 480.0)),
    );
    let opts = eframe::NativeOptions {
        viewport,
        vsync: false,
        ..Default::default()
    };

    let _ = eframe::run_native(
        "oxdm-queues",
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
            Ok(Box::new(QueuesShell::new(rt, client, cache)))
        }),
    );
}

async fn connect() -> Result<(Arc<Client>, Arc<Cache>), String> {
    let client = crate::ui::connect_or_spawn_daemon().await?;
    client.hello(GuiKind::Queues).await?;
    let snap = client.snapshot().await?;
    let cache = Arc::new(Cache::from_snapshot(snap));
    Ok((client, cache))
}

struct QueuesShell {
    rt: Runtime,
    client: Arc<Client>,
    cache: Arc<Cache>,
    state: QueuesState,
    queue_delete_confirm: Option<QueueId>,
    theme_applied_for: Option<crate::ui::theme::ResolvedTheme>,
    #[cfg(target_os = "windows")]
    surfaced: bool,
}

impl QueuesShell {
    fn new(rt: Runtime, client: Arc<Client>, cache: Arc<Cache>) -> Self {
        Self {
            rt,
            client,
            cache,
            state: QueuesState::default(),
            queue_delete_confirm: None,
            theme_applied_for: None,
            #[cfg(target_os = "windows")]
            surfaced: false,
        }
    }
}

impl eframe::App for QueuesShell {
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
        if ctx.input(|i| i.viewport().close_requested()) {
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

        let mut c = Ctx {
            state: &mut self.state,
            queue_delete_confirm: &mut self.queue_delete_confirm,
            client: self.client.clone(),
            cache: self.cache.clone(),
            rt: self.rt.handle().clone(),
        };
        crate::ui::dialogs::queues::body(&mut c, root_ui);

        if let Some(qid) = self.queue_delete_confirm {
            let queues = self.cache.queues();
            let Some(q) = queues.iter().find(|q| q.id == qid).cloned() else {
                self.queue_delete_confirm = None;
                return;
            };
            let mut decision: Option<bool> = None;
            let t = theme::tokens(ctx);
            let modal_frame = egui::Frame::NONE
                .fill(t.bg_surface)
                .stroke(Stroke::new(t.border_width, t.border_default))
                .corner_radius(theme::surface::RADIUS)
                .inner_margin(space::S4)
                .shadow(egui::epaint::Shadow {
                    offset: [0, 4],
                    blur: 16,
                    spread: 0,
                    color: Color32::from_black_alpha(80),
                });
            crate::ui::utils::modal::show(
                ctx,
                egui::Id::new("queues-delete-confirm"),
                modal_frame,
                |ui| {
                    ui.set_max_width(420.0);
                    ui.spacing_mut().item_spacing.y = space::S2 as f32;
                    ui.label(
                        RichText::new(format!("Delete queue \"{}\"?", q.name))
                            .font(theme::body_bold(14.0))
                            .color(t.fg_1),
                    );
                    let n = q.job_ids.len();
                    let plural = if n == 1 { "job" } else { "jobs" };
                    ui.label(
                        RichText::new(format!(
                            "{n} {plural} will become queueless. \
                             Files on disk are not touched."
                        ))
                        .color(t.fg_3)
                        .font(theme::body(12.0)),
                    );
                    ui.add_space(space::S1 as f32);
                    ui.horizontal(|ui| {
                        if Btn::new("Cancel").ghost().show(ui).clicked() {
                            decision = Some(false);
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if Btn::new("Delete")
                                .danger_filled()
                                .icon("trash-2")
                                .show(ui)
                                .clicked()
                            {
                                decision = Some(true);
                            }
                        });
                    });
                },
            );
            match decision {
                Some(true) => {
                    let s = self.client.clone();
                    let _g = self.rt.handle().enter();
                    tokio::spawn(async move {
                        let _ = s.delete_queue(qid).await;
                    });
                    self.queue_delete_confirm = None;
                    self.state.selected = None;
                    self.state.editor = None;
                }
                Some(false) => {
                    self.queue_delete_confirm = None;
                }
                None => {}
            }
        }
    }
}
