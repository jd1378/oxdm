//! Standalone Settings window subprocess (`oxdm gui settings`).
//!
//! Mirrors the shape of `add_window` / `download_window`: connects to
//! the daemon over `ipc_local`, holds an `Arc<Cache>` mirror of the
//! current `Settings`, and renders the existing `dialogs::settings`
//! body via a `Ctx` that owns the form / tab / highlight scalar UI
//! state.
//!
//! Argv hints (set by callers like the statusbar Direct/Proxied pill):
//!   --tab <general|downloads|network|appearance|advanced>
//!   --highlight-proxy

use std::sync::Arc;

use eframe::egui;
use tokio::runtime::Runtime;

use crate::ipc_local::Client;
use crate::ipc_local::protocol::{GuiKind, SubFilter};
use crate::ui::dialogs::settings::{Ctx, FormState, SettingsTab};
use crate::ui::gui_state::Cache;
use crate::ui::theme;
use crate::ui::utils::icons;

pub fn launch(tab: Option<SettingsTab>, highlight_proxy: bool) {
    let rt = Runtime::new().expect("tokio runtime");
    let (client, cache) = match rt.block_on(connect()) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "could not reach oxdm daemon");
            return;
        }
    };

    let viewport = crate::ui::utils::chrome::viewport_builder(
        "oxdm — Settings",
        (820.0, 660.0),
        Some((640.0, 480.0)),
    );
    let opts = eframe::NativeOptions {
        viewport,
        vsync: false,
        ..Default::default()
    };

    let _ = eframe::run_native(
        "oxdm-settings",
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
            Ok(Box::new(SettingsShell::new(
                rt,
                client,
                cache,
                tab,
                highlight_proxy,
            )))
        }),
    );
}

async fn connect() -> Result<(Arc<Client>, Arc<Cache>), String> {
    let client = crate::ui::connect_or_spawn_daemon().await?;
    client.hello(GuiKind::Settings).await?;
    let snap = client.snapshot().await?;
    let cache = Arc::new(Cache::from_snapshot(snap));
    Ok((client, cache))
}

struct SettingsShell {
    rt: Runtime,
    client: Arc<Client>,
    cache: Arc<Cache>,
    form: Option<FormState>,
    tab: SettingsTab,
    highlight_proxy: bool,
    theme_applied_for: Option<crate::ui::theme::ResolvedTheme>,
    #[cfg(target_os = "windows")]
    surfaced: bool,
}

impl SettingsShell {
    fn new(
        rt: Runtime,
        client: Arc<Client>,
        cache: Arc<Cache>,
        tab: Option<SettingsTab>,
        highlight_proxy: bool,
    ) -> Self {
        Self {
            rt,
            client,
            cache,
            form: None,
            tab: tab.unwrap_or_default(),
            highlight_proxy,
            theme_applied_for: None,
            #[cfg(target_os = "windows")]
            surfaced: false,
        }
    }
}

impl eframe::App for SettingsShell {
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

        // Live theme follow-on: when the daemon broadcasts a settings
        // change, re-apply tokens so the open Settings window matches.
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
            form: &mut self.form,
            tab: &mut self.tab,
            highlight_proxy: &mut self.highlight_proxy,
            client: self.client.clone(),
            cache: self.cache.clone(),
            rt: self.rt.handle().clone(),
        };
        crate::ui::dialogs::settings::body(&mut c, root_ui);
    }
}
