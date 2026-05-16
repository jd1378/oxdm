//! Standalone Properties window subprocess (`oxdm gui properties <id>`).
//!
//! Per-download configuration & inspection. Runs as its own process —
//! same lifecycle model as the download/settings/queues windows — so
//! re-triggering "Show Properties" focuses or re-spawns a single window
//! per job (see `daemon::tray::spawn_properties_gui`, evict + spawn),
//! rather than the old in-process child viewport that lived inside the
//! main window. The actual layout/state lives in
//! `dialogs::properties` (`PropertiesHost` + `body`).

use std::sync::Arc;

use eframe::egui;
use tokio::runtime::Runtime;

use crate::domain::JobId;
use crate::ipc_local::Client;
use crate::ipc_local::protocol::{GuiKind, SubFilter};
use crate::ui::dialogs::properties::{PropertiesHost, PropertiesState, body};
use crate::ui::gui_state::Cache;
use crate::ui::theme;
use crate::ui::utils::icons;

pub fn launch(id: JobId) {
    let rt = Runtime::new().expect("tokio runtime");
    let (client, cache) = match rt.block_on(connect(id)) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "could not reach oxdm daemon");
            return;
        }
    };

    let title = format!("oxdm — Properties {id}");
    let viewport =
        crate::ui::utils::chrome::viewport_builder(&title, (650.0, 720.0), Some((650.0, 480.0)));
    let opts = eframe::NativeOptions {
        viewport,
        vsync: false,
        ..Default::default()
    };

    let _ = eframe::run_native(
        "oxdm-properties",
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
            Ok(Box::new(PropertiesShell::new(rt, client, cache, id)))
        }),
    );
}

async fn connect(id: JobId) -> Result<(Arc<Client>, Arc<Cache>), String> {
    let client = crate::ui::connect_or_spawn_daemon().await?;
    client.hello(GuiKind::Properties(id)).await?;
    let snap = client.snapshot().await?;
    let cache = Arc::new(Cache::from_snapshot(snap));
    Ok((client, cache))
}

struct PropertiesShell {
    // Owned to keep the tokio runtime alive for the process lifetime;
    // `host.rt` is a `Handle` into it (background Apply RPCs / picker).
    _rt: Runtime,
    host: PropertiesHost,
    theme_applied_for: Option<theme::ResolvedTheme>,
    #[cfg(target_os = "windows")]
    surfaced: bool,
}

impl PropertiesShell {
    fn new(rt: Runtime, client: Arc<Client>, cache: Arc<Cache>, id: JobId) -> Self {
        let handle = rt.handle().clone();
        Self {
            _rt: rt,
            host: PropertiesHost {
                state: PropertiesState::new(id),
                cache,
                client,
                rt: handle,
                want_close: false,
            },
            theme_applied_for: None,
            #[cfg(target_os = "windows")]
            surfaced: false,
        }
    }
}

impl eframe::App for PropertiesShell {
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

        if self.host.want_close
            || crate::ui::gui_state::daemon_lost()
            || crate::ui::gui_state::close_requested()
        {
            std::process::exit(0);
        }
        if crate::ui::gui_state::take_focus_request() {
            crate::ui::gui_state::surface_window(ctx);
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            std::process::exit(0);
        }

        let resolved_now = theme::resolve(self.host.cache.settings().theme);
        if self.theme_applied_for != Some(resolved_now) {
            theme::apply(ctx, &self.host.cache.settings());
            self.theme_applied_for = Some(resolved_now);
        }

        crate::ui::utils::resize::show_styled(
            ctx,
            crate::ui::utils::resize::ChromeStyle {
                dark_border: true,
                resizable: true,
            },
        );

        body(&mut self.host, root_ui);
    }
}
