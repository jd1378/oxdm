use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{self, Align, Layout, RichText, Stroke};
use indexmap::IndexMap;
use tokio::runtime::Handle;

use crate::domain::{Category, ConflictWhileHidden, Settings, Theme};
use crate::ipc_local::Client;
use crate::ui::components::primitives::{
    Btn, BtnSize, Combo, FileInput, TextInput, copy_feedback, eyebrow, search_field, text_input,
};
use crate::ui::components::titlebar;
use crate::ui::gui_state::Cache;
use crate::ui::theme::{self, radius, space};

/// Inputs the body needs from its host (subprocess shell). The body
/// owns no AppShell reference; it reads / writes scalar UI state via
/// these references and uses `client + cache + rt` for I/O.
pub struct Ctx<'a> {
    pub form: &'a mut Option<FormState>,
    pub tab: &'a mut SettingsTab,
    pub highlight_proxy: &'a mut bool,
    pub client: Arc<Client>,
    pub cache: Arc<Cache>,
    pub rt: Handle,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    #[default]
    General,
    Downloads,
    Categories,
    Network,
    Browser,
    Notifications,
    Advanced,
    About,
}

impl SettingsTab {
    fn label(self) -> &'static str {
        match self {
            SettingsTab::General => "General",
            SettingsTab::Downloads => "Downloads",
            SettingsTab::Categories => "Categories",
            SettingsTab::Network => "Network",
            SettingsTab::Browser => "Browser",
            SettingsTab::Notifications => "Notifications",
            SettingsTab::Advanced => "Advanced",
            SettingsTab::About => "About",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            SettingsTab::General => "sliders-horizontal",
            SettingsTab::Downloads => "download",
            SettingsTab::Categories => "folder",
            SettingsTab::Network => "globe",
            SettingsTab::Browser => "puzzle",
            SettingsTab::Notifications => "bell",
            SettingsTab::Advanced => "terminal",
            SettingsTab::About => "info",
        }
    }

    fn all() -> &'static [SettingsTab] {
        &[
            SettingsTab::General,
            SettingsTab::Downloads,
            SettingsTab::Categories,
            SettingsTab::Network,
            SettingsTab::Browser,
            SettingsTab::Notifications,
            SettingsTab::Advanced,
            SettingsTab::About,
        ]
    }

    fn has_reset(self) -> bool {
        !matches!(self, SettingsTab::Advanced | SettingsTab::About)
    }
}

fn cat_text_for(cat: Category, overrides: &IndexMap<Category, Vec<String>>) -> String {
    overrides
        .get(&cat)
        .map(|v| v.join(", "))
        .unwrap_or_else(|| cat.default_extensions().join(", "))
}

fn parse_cat_text(s: &str) -> Vec<String> {
    s.split([',', ' ', '\n', '\t', ';'])
        .map(|t| t.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|t| !t.is_empty())
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct FormState {
    pub download_dir: String,
    pub work_dir: String,
    pub max_connections: String,
    /// When true, `max_connections` is ignored at save time and saved
    /// as `None` (size-based heuristic at job creation).
    pub max_connections_auto: bool,
    pub max_concurrent_downloads: String,
    pub max_retries: String,
    pub wait_between_retries: String,
    pub n_fixed_retries: String,
    pub user_agent: String,
    pub randomize_user_agent: bool,
    pub proxy: String,
    pub use_server_time: bool,
    pub accept_invalid_certs: bool,
    pub speed_limit: String,
    pub connect_timeout: String,
    pub headers_text: String,
    pub ipc_port: String,
    pub start_at_login: bool,
    pub start_to_tray: bool,
    pub show_complete_dialog: bool,
    pub theme: String,
    pub reduce_motion: bool,
    pub theme_overrides_text: String,
    pub ext_token_view: String,
    pub conflict_while_hidden: String,
    pub remove_confirm_incomplete: bool,
    pub remove_confirm_completed: bool,
    pub update_feed_url: String,
    pub category_extensions: IndexMap<Category, Vec<String>>,
    pub category_drafts: IndexMap<Category, String>,
    // Carried through untouched (no UI yet). Owned by oxdm; exposed to
    // the browser extension via the `get_capture_rules` IPC.
    pub capture_min_size: u64,
    pub capture_skip_domains: Vec<String>,
    pub capture_skip_extensions: Vec<String>,
    pub capture_skip_mime_prefixes: Vec<String>,
    pub capture_allow_extensions: Vec<String>,
    pub capture_allow_mime_prefixes: Vec<String>,
    pub toast: Option<(bool, String)>,
}

impl From<Settings> for FormState {
    fn from(s: Settings) -> Self {
        Self {
            download_dir: s.download_dir.to_string_lossy().into_owned(),
            work_dir: s.work_dir.to_string_lossy().into_owned(),
            max_connections: s
                .max_connections
                .map(|n| n.to_string())
                .unwrap_or_else(|| "8".into()),
            max_connections_auto: s.max_connections.is_none(),
            max_concurrent_downloads: s.max_concurrent_downloads.to_string(),
            max_retries: s.max_retries.to_string(),
            wait_between_retries: humantime::format_duration(s.wait_between_retries).to_string(),
            n_fixed_retries: s.n_fixed_retries.to_string(),
            user_agent: s.user_agent.unwrap_or_default(),
            randomize_user_agent: s.randomize_user_agent,
            proxy: s.proxy.unwrap_or_default(),
            use_server_time: s.use_server_time,
            accept_invalid_certs: s.accept_invalid_certs,
            speed_limit: s.speed_limit.map(|v| v.to_string()).unwrap_or_default(),
            connect_timeout: s
                .connect_timeout
                .map(|d| humantime::format_duration(d).to_string())
                .unwrap_or_default(),
            headers_text: s
                .headers
                .iter()
                .map(|(k, v)| format!("{k}: {v}"))
                .collect::<Vec<_>>()
                .join("\n"),
            ipc_port: s.ipc_port.to_string(),
            start_at_login: s.start_at_login,
            start_to_tray: s.start_to_tray,
            show_complete_dialog: s.show_complete_dialog,
            theme: match s.theme {
                Theme::System => "system",
                Theme::Light => "light",
                Theme::Dark => "dark",
                Theme::Warm => "warm",
            }
            .into(),
            reduce_motion: s.reduce_motion,
            theme_overrides_text: s
                .theme_overrides
                .iter()
                .map(|(k, v)| format!("{k}: {v}"))
                .collect::<Vec<_>>()
                .join("\n"),
            ext_token_view: s.ext_token,
            conflict_while_hidden: match s.conflict_while_hidden {
                ConflictWhileHidden::AutoPopup => "auto_popup",
                ConflictWhileHidden::NotifyAndPark => "notify_and_park",
            }
            .into(),
            remove_confirm_incomplete: s.remove_confirm_incomplete,
            remove_confirm_completed: s.remove_confirm_completed,
            update_feed_url: s.update_feed_url,
            category_drafts: {
                let mut m: IndexMap<Category, String> = IndexMap::new();
                for &cat in Category::ALL_VISIBLE {
                    m.insert(cat, cat_text_for(cat, &s.category_extensions));
                }
                m
            },
            category_extensions: s.category_extensions,
            capture_min_size: s.capture_min_size,
            capture_skip_domains: s.capture_skip_domains,
            capture_skip_extensions: s.capture_skip_extensions,
            capture_skip_mime_prefixes: s.capture_skip_mime_prefixes,
            capture_allow_extensions: s.capture_allow_extensions,
            capture_allow_mime_prefixes: s.capture_allow_mime_prefixes,
            toast: None,
        }
    }
}

impl FormState {
    fn flush_category_drafts(&mut self) {
        let mut out: IndexMap<Category, Vec<String>> = IndexMap::new();
        for &cat in Category::ALL_VISIBLE {
            let Some(text) = self.category_drafts.get(&cat) else {
                continue;
            };
            let parsed = parse_cat_text(text);
            // Only persist when overriding the built-in defaults; this
            // keeps `Settings::category_extensions` aligned with the
            // "absent ⇒ default" convention noted in the field doc.
            let defaults: Vec<String> = cat
                .default_extensions()
                .iter()
                .map(|s| (*s).to_string())
                .collect();
            if parsed != defaults {
                out.insert(cat, parsed);
            }
        }
        self.category_extensions = out;
    }

    pub fn into_settings(mut self) -> Result<Settings, String> {
        self.flush_category_drafts();
        let download_dir = std::path::PathBuf::from(self.download_dir.trim());
        let work_dir = match self.work_dir.trim() {
            "" => crate::domain::settings::default_work_dir(),
            s => std::path::PathBuf::from(s),
        };
        let max_connections = if self.max_connections_auto {
            None
        } else {
            let n = parse_u64(&self.max_connections, "connections per file")?;
            if n == 0 {
                return Err("connections per file must be ≥ 1".into());
            }
            Some(n)
        };
        let max_concurrent_downloads =
            parse_usize(&self.max_concurrent_downloads, "concurrent downloads")?;
        if max_concurrent_downloads == 0 {
            return Err("concurrent downloads must be ≥ 1".into());
        }
        let max_retries = parse_u32(&self.max_retries, "max retries")?;
        let wait_between_retries = humantime::parse_duration(self.wait_between_retries.trim())
            .map_err(|e| format!("wait between retries: {e}"))?;
        if wait_between_retries == Duration::ZERO {
            return Err("wait between retries must be > 0".into());
        }
        let n_fixed_retries = parse_u32(&self.n_fixed_retries, "fixed retries")?;
        let user_agent = blank_to_none(&self.user_agent);
        let proxy = blank_to_none(&self.proxy);
        let speed_limit = match self.speed_limit.trim() {
            "" => None,
            s => Some(parse_u64(s, "speed limit")?),
        };
        let connect_timeout = match self.connect_timeout.trim() {
            "" => None,
            s => Some(humantime::parse_duration(s).map_err(|e| format!("connect timeout: {e}"))?),
        };
        let headers = parse_kv_lines(&self.headers_text, "header")?;
        let ipc_port = parse_u16(&self.ipc_port, "IPC port")?;
        let theme = match self.theme.as_str() {
            "system" => Theme::System,
            "light" => Theme::Light,
            "dark" => Theme::Dark,
            "warm" => Theme::Warm,
            other => return Err(format!("invalid theme: {other}")),
        };
        let theme_overrides = parse_kv_lines(&self.theme_overrides_text, "theme override")?;
        let conflict_while_hidden = match self.conflict_while_hidden.as_str() {
            "auto_popup" => ConflictWhileHidden::AutoPopup,
            "notify_and_park" => ConflictWhileHidden::NotifyAndPark,
            other => return Err(format!("invalid conflict-while-hidden: {other}")),
        };
        Ok(Settings {
            download_dir,
            work_dir,
            max_connections,
            max_concurrent_downloads,
            max_retries,
            wait_between_retries,
            n_fixed_retries,
            user_agent,
            randomize_user_agent: self.randomize_user_agent,
            proxy,
            use_server_time: self.use_server_time,
            accept_invalid_certs: self.accept_invalid_certs,
            speed_limit,
            connect_timeout,
            headers,
            ipc_port,
            ext_token: self.ext_token_view,
            conflict_while_hidden,
            remove_confirm_incomplete: self.remove_confirm_incomplete,
            remove_confirm_completed: self.remove_confirm_completed,
            start_at_login: self.start_at_login,
            start_to_tray: self.start_to_tray,
            show_complete_dialog: self.show_complete_dialog,
            update_feed_url: self.update_feed_url.trim().to_string(),
            theme,
            reduce_motion: self.reduce_motion,
            theme_overrides,
            category_extensions: self.category_extensions,
            capture_min_size: self.capture_min_size,
            capture_skip_domains: self.capture_skip_domains,
            capture_skip_extensions: self.capture_skip_extensions,
            capture_skip_mime_prefixes: self.capture_skip_mime_prefixes,
            capture_allow_extensions: self.capture_allow_extensions,
            capture_allow_mime_prefixes: self.capture_allow_mime_prefixes,
        })
    }
}

pub fn body(c: &mut Ctx<'_>, root_ui: &mut egui::Ui) {
    let ctx = &root_ui.ctx().clone();
    if c.form.is_none() {
        *c.form = Some(FormState::from(c.cache.settings()));
    }
    let t = theme::tokens(ctx);
    let rt_handle = c.rt.clone();
    let mut form = c.form.take().unwrap();
    let mut to_save: Option<FormState> = None;
    let mut to_revert = false;
    let mut regenerate = false;

    egui::Panel::top("set_titlebar")
        .frame(egui::Frame::NONE.fill(t.bg_titlebar))
        .show_separator_line(true)
        .show_inside(root_ui, |ui| {
            titlebar::show(ui, ctx, "Settings");
        });

    let mut reset_pane = false;
    egui::Panel::bottom("set_footer")
        .frame(
            egui::Frame::NONE
                .fill(t.bg_sunken)
                .inner_margin(egui::Margin::symmetric(space::S4, space::S2)),
        )
        .show_separator_line(true)
        .show_inside(root_ui, |ui| {
            ui.horizontal(|ui| {
                if Btn::new("Cancel").ghost().show(ui).clicked() {
                    to_revert = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if let Some((ok, msg)) = &form.toast {
                    let color = if *ok {
                        t.status_success
                    } else {
                        t.status_danger
                    };
                    ui.label(RichText::new(msg).color(color).font(theme::body_bold(12.0)));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if Btn::new("Save").primary().icon("save").show(ui).clicked() {
                        to_save = Some(form.clone());
                    }
                    if c.tab.has_reset() {
                        let label = format!("Reset {}", c.tab.label());
                        if Btn::new(label)
                            .toolbar()
                            .icon("rotate-cw")
                            .show(ui)
                            .clicked()
                        {
                            reset_pane = true;
                        }
                    }
                });
            });
        });

    egui::Panel::left("set_nav")
        .exact_size(200.0)
        .resizable(false)
        .frame(
            egui::Frame::NONE
                .fill(t.bg_sunken)
                .inner_margin(egui::Margin::symmetric(space::S2, space::S3)),
        )
        .show_separator_line(true)
        .show_inside(root_ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for tab in SettingsTab::all().iter().copied() {
                let active = *c.tab == tab;
                if nav_row(ui, &t, tab.icon(), tab.label(), active).clicked() {
                    *c.tab = tab;
                    ui.request_repaint();
                }
            }
        });

    let active = *c.tab;
    let highlight_proxy = *c.highlight_proxy;
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(t.bg_page))
        .show_inside(root_ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .content_margin(egui::Margin::symmetric(space::S4, space::S4))
                .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = space::S3 as f32;
                match active {
                    SettingsTab::General => {
                        card(ui, &t, "moon", "Appearance", |ui| {
                            eyebrow(ui, "theme");
                            Combo::new("theme", form.theme.clone()).show(ui, |ui| {
                                let options: &[(&str, &str)] = &[
                                    ("system", "Follow system"),
                                    ("light", "Utility (light)"),
                                    ("warm", "Warm (cream)"),
                                    ("dark", "Dark"),
                                ];
                                for (val, label) in options {
                                    if Combo::item(ui, label, true).clicked() {
                                        form.theme = (*val).into();
                                        ui.close();
                                    }
                                }
                            });
                            ui.add_space(space::S2 as f32);
                            ui.checkbox(&mut form.reduce_motion, "Reduce motion (skip animations)");
                        });
                        card(ui, &t, "hard-drive", "Storage", |ui| {
                            ui.horizontal(|ui| {
                                eyebrow(ui, "default download folder");
                            });
                            ui.push_id("download_dir", |ui| {
                                let resp = FileInput::new(&mut form.download_dir)
                                    .hint("~/Downloads")
                                    .tooltip("Browse for download folder")
                                    .show(ui);
                                if resp.browse.clicked() {
                                    let _g = rt_handle.enter();
                                    if let Some(p) = rfd::FileDialog::new()
                                        .set_directory(form.download_dir.trim())
                                        .pick_folder()
                                    {
                                        form.download_dir = p.to_string_lossy().to_string();
                                    }
                                }
                            });
                            ui.add_space(space::S2 as f32);
                            ui.horizontal(|ui| {
                                eyebrow(ui, "in-flight cache folder (per-job .part + metadata)");
                            });
                            ui.push_id("work_dir", |ui| {
                                let resp = FileInput::new(&mut form.work_dir)
                                    .hint("<app data>/oxdm/work")
                                    .tooltip("Browse for cache folder")
                                    .show(ui);
                                if resp.browse.clicked() {
                                    let _g = rt_handle.enter();
                                    if let Some(p) = rfd::FileDialog::new()
                                        .set_directory(form.work_dir.trim())
                                        .pick_folder()
                                    {
                                        form.work_dir = p.to_string_lossy().to_string();
                                    }
                                }
                            });
                        });

                        card(ui, &t, "settings", "Misc", |ui| {
                            ui.checkbox(&mut form.start_at_login, "Start oxdm on system login");
                            ui.checkbox(&mut form.start_to_tray, "Start in tray (no main window on launch)");
                            ui.checkbox(&mut form.show_complete_dialog, "Show download-complete dialog when a download finishes");
                        });

                    }
                    SettingsTab::Categories => {
                        card(ui, &t, "folder", "Categories", |ui| {
                            ui.label(RichText::new("Override file extensions per category. Comma-separated, no dots.")
                                .color(t.fg_3).font(theme::body(11.0)));
                            for &cat in Category::ALL_VISIBLE {
                                ui.push_id(cat, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new(format!("{}:", cat.label()))
                                            .color(t.fg_2).font(theme::body_bold(12.0)));
                                        let entry = form
                                            .category_drafts
                                            .entry(cat)
                                            .or_insert_with(|| cat_text_for(cat, &form.category_extensions));
                                        let w = ui.available_width() - 80.0;
                                        TextInput::new(entry)
                                            .width(w)
                                            .font(theme::mono(11.0))
                                            .show(ui);
                                        if Btn::new("Reset").toolbar().size(BtnSize::Sm).show(ui).clicked() {
                                            *entry = cat.default_extensions().join(", ");
                                        }
                                    });
                                });
                            }
                        });
                    }
                    SettingsTab::Downloads => {
                        card(ui, &t, "rotate-cw", "Behavior", |ui| {
                            kv_row(ui, &t, "Max retries", &mut form.max_retries, 80.0);
                            kv_row(ui, &t, "Fixed retries before backoff", &mut form.n_fixed_retries, 80.0);
                            kv_row(ui, &t, "Wait between retries", &mut form.wait_between_retries, 120.0);
                            ui.checkbox(&mut form.use_server_time, "Use server-provided last-modified time");
                        });

                        card(ui, &t, "trash-2", "Remove behavior", |ui| {
                            ui.checkbox(&mut form.remove_confirm_incomplete, "Confirm before removing incomplete downloads");
                            ui.checkbox(&mut form.remove_confirm_completed, "Confirm before removing completed downloads");
                        });
                    }
                    SettingsTab::Network => {
                        card(ui, &t, "activity", "Network", |ui| {
                            ui.checkbox(
                                &mut form.max_connections_auto,
                                "Determine connections per file automatically (by file size)",
                            );
                            if !form.max_connections_auto {
                                kv_row(ui, &t, "Connections per file", &mut form.max_connections, 80.0);
                            }
                            kv_row(ui, &t, "Concurrent downloads", &mut form.max_concurrent_downloads, 80.0);
                            kv_row(ui, &t, "Speed limit (B/s — blank for unlimited)", &mut form.speed_limit, 160.0);
                            proxy_row(ui, &t, &mut form.proxy, highlight_proxy);
                            kv_row(ui, &t, "Connect timeout", &mut form.connect_timeout, 120.0);
                            ui.checkbox(&mut form.accept_invalid_certs, "Accept invalid TLS certificates (dangerous)");
                        });

                        card(ui, &t, "user", "Identity", |ui| {
                            kv_row(ui, &t, "Custom User-Agent", &mut form.user_agent, 0.0);
                            ui.checkbox(&mut form.randomize_user_agent, "Randomize User-Agent per request");
                            eyebrow(ui, "custom headers");
                            ui.add(egui::TextEdit::multiline(&mut form.headers_text)
                                .desired_rows(3).desired_width(ui.available_width())
                                .font(theme::mono(12.0)));
                        });
                    }
                    SettingsTab::Browser => {
                        card(ui, &t, "puzzle", "Browser integration", |ui| {
                            kv_row(ui, &t, "IPC port", &mut form.ipc_port, 100.0);
                            eyebrow(ui, "pairing code");
                            let port_now: u16 = form.ipc_port.parse().unwrap_or(27812);
                            let mut pairing =
                                crate::data::encode_pairing_code(port_now, &form.ext_token_view);
                            ui.horizontal(|ui| {
                                ui.add(egui::TextEdit::singleline(&mut pairing)
                                    .interactive(false).desired_width(ui.available_width() - 220.0)
                                    .font(theme::mono(11.0)));
                                let copy_id = ui.id().with("settings-pairing-copy");
                                if Btn::new("Copy").toolbar().icon(copy_feedback::icon(ui.ctx(), copy_id)).size(BtnSize::Sm).show(ui).clicked() {
                                    copy_feedback::commit(ui.ctx(), copy_id, pairing.clone());
                                }
                                if Btn::new("Regenerate").toolbar().icon("rotate-cw").size(BtnSize::Sm).show(ui).clicked() {
                                    regenerate = true;
                                }
                            });
                            eyebrow(ui, "conflict while dialog hidden");
                            Combo::new("conflict_while_hidden", form.conflict_while_hidden.clone()).show(ui, |ui| {
                                let options: &[(&str, &str)] = &[
                                    ("auto_popup", "Re-open dialog automatically"),
                                    ("notify_and_park", "Notify, park at end of queue"),
                                ];
                                for (val, label) in options {
                                    if Combo::item(ui, label, true).clicked() {
                                        form.conflict_while_hidden = (*val).into();
                                        ui.close();
                                    }
                                }
                            });
                        });
                    }
                    SettingsTab::Notifications => {
                        card(ui, &t, "bell", "Notifications", |ui| {
                            ui.checkbox(&mut form.show_complete_dialog, "Show download-complete dialog when a download finishes");
                            ui.label(
                                RichText::new("System notifications follow your queue's on-finish hooks (see Queues & scheduling).")
                                    .color(t.fg_3)
                                    .font(theme::body(11.0)),
                            );
                        });
                    }
                    SettingsTab::Advanced => {
                        card(ui, &t, "palette", "Theme overrides", |ui| {
                            eyebrow(ui, "overrides — accent / bg / text (one per line)");
                            ui.add(egui::TextEdit::multiline(&mut form.theme_overrides_text)
                                .desired_rows(3).desired_width(ui.available_width())
                                .font(theme::mono(12.0)));
                        });
                        card(ui, &t, "cloud-upload", "Updates", |ui| {
                            kv_row(ui, &t, "Update feed URL", &mut form.update_feed_url, 0.0);
                        });
                    }
                    SettingsTab::About => {
                        card(ui, &t, "info", "About oxdm", |ui| {
                            ui.label(
                                RichText::new("oxdm")
                                    .color(t.fg_1)
                                    .font(theme::display(22.0)),
                            );
                            ui.label(
                                RichText::new(concat!("Version ", env!("CARGO_PKG_VERSION")))
                                    .color(t.fg_2)
                                    .font(theme::mono(12.0)),
                            );
                            ui.label(
                                RichText::new("A focused, native download manager.")
                                    .color(t.fg_3)
                                    .font(theme::body(12.0)),
                            );
                        });
                    }
                }
            });
        });

    if regenerate {
        let s = c.client.clone();
        let _g = c.rt.enter();
        tokio::spawn(async move {
            let _ = s.regenerate_ext_token().await;
        });
    }
    if to_revert {
        *c.form = Some(FormState::from(c.cache.settings()));
        return;
    }
    if reset_pane {
        let defaults = FormState::from(crate::domain::Settings::default());
        reset_pane_fields(*c.tab, &mut form, &defaults);
        form.toast = Some((true, format!("Reset {}.", c.tab.label())));
    }
    if let Some(f) = to_save {
        match f.clone().into_settings() {
            Ok(s) => {
                let prev_autostart = c.cache.settings().start_at_login;
                let next_autostart = s.start_at_login;
                let st = c.client.clone();
                let res = c.rt.block_on(async move { st.update_settings(s).await });
                form.toast = match res {
                    Ok(()) => {
                        if prev_autostart != next_autostart
                            && let Err(e) = crate::ui::platform::set_autostart(next_autostart)
                        {
                            tracing::warn!(error = %e, "set_autostart failed");
                        }
                        Some((true, "Settings saved.".into()))
                    }
                    Err(e) => Some((false, e)),
                };
            }
            Err(e) => form.toast = Some((false, e)),
        }
    }
    *c.form = Some(form);

    let _ = search_field; // keep import if unused later
}

fn reset_pane_fields(tab: SettingsTab, form: &mut FormState, d: &FormState) {
    match tab {
        SettingsTab::General => {
            form.download_dir = d.download_dir.clone();
            form.work_dir = d.work_dir.clone();
            form.start_at_login = d.start_at_login;
            form.start_to_tray = d.start_to_tray;
            form.theme = d.theme.clone();
        }
        SettingsTab::Downloads => {
            form.max_retries = d.max_retries.clone();
            form.n_fixed_retries = d.n_fixed_retries.clone();
            form.wait_between_retries = d.wait_between_retries.clone();
            form.use_server_time = d.use_server_time;
            form.remove_confirm_incomplete = d.remove_confirm_incomplete;
            form.remove_confirm_completed = d.remove_confirm_completed;
        }
        SettingsTab::Categories => {
            form.category_extensions.clear();
            form.category_drafts.clear();
        }
        SettingsTab::Network => {
            form.max_connections = d.max_connections.clone();
            form.max_connections_auto = d.max_connections_auto;
            form.max_concurrent_downloads = d.max_concurrent_downloads.clone();
            form.speed_limit = d.speed_limit.clone();
            form.proxy = d.proxy.clone();
            form.connect_timeout = d.connect_timeout.clone();
            form.accept_invalid_certs = d.accept_invalid_certs;
            form.user_agent = d.user_agent.clone();
            form.randomize_user_agent = d.randomize_user_agent;
            form.headers_text = d.headers_text.clone();
        }
        SettingsTab::Browser => {
            form.ipc_port = d.ipc_port.clone();
            form.conflict_while_hidden = d.conflict_while_hidden.clone();
        }
        SettingsTab::Notifications => {
            form.show_complete_dialog = d.show_complete_dialog;
        }
        SettingsTab::Advanced | SettingsTab::About => {}
    }
}

fn nav_row(
    ui: &mut egui::Ui,
    t: &theme::Tokens,
    icon: &'static str,
    label: &str,
    active: bool,
) -> egui::Response {
    let h = 32.0;
    let (rect, resp) = ui.allocate_exact_size(
        egui::Vec2::new(ui.available_width(), h),
        egui::Sense::click(),
    );
    let painter = ui.painter().clone();
    if active {
        painter.rect_filled(rect, radius::SM as f32, t.bg_raised);
        painter.line_segment(
            [
                egui::pos2(rect.left() + 2.0, rect.top() + 6.0),
                egui::pos2(rect.left() + 2.0, rect.bottom() - 6.0),
            ],
            Stroke::new(3.0, t.action_primary),
        );
    } else if resp.hovered() {
        painter.rect_filled(rect, radius::SM as f32, t.bg_raised);
    }
    let mut x = rect.left() + 14.0;
    let ic = crate::ui::utils::icons::icon(ui.ctx(), icon, 14.0, t.fg_2);
    let ic_rect = egui::Rect::from_min_size(
        egui::pos2(x, rect.center().y - 7.0),
        egui::Vec2::splat(14.0),
    );
    ic.paint_at(ui, ic_rect);
    x += 22.0;
    let font = if active {
        theme::body_bold(13.0)
    } else {
        theme::body(13.0)
    };
    let g = painter.layout_no_wrap(label.to_string(), font, t.fg_1);
    painter.galley(egui::pos2(x, rect.center().y - g.size().y / 2.0), g, t.fg_1);
    resp
}

fn card(
    ui: &mut egui::Ui,
    t: &theme::Tokens,
    icon: &'static str,
    title: &str,
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
            ui.add_space(space::S2 as f32);
            ui.spacing_mut().item_spacing.y = space::S2 as f32;
            body(ui);
        });
}

fn proxy_row(ui: &mut egui::Ui, t: &theme::Tokens, value: &mut String, highlight: bool) {
    let stroke = if highlight {
        Stroke::new(1.5, t.action_primary)
    } else {
        Stroke::NONE
    };
    let inner_id = egui::Id::new("settings.proxy.input");
    egui::Frame::NONE
        .stroke(stroke)
        .corner_radius(theme::surface::RADIUS)
        .inner_margin(if highlight {
            egui::Margin::same(space::S1)
        } else {
            egui::Margin::ZERO
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let lbl_w = 220.0_f32.min(ui.available_width() * 0.5);
                ui.allocate_ui(egui::vec2(lbl_w, 24.0), |ui| {
                    ui.label(
                        RichText::new("Proxy URL")
                            .color(t.fg_2)
                            .font(theme::body(12.0)),
                    );
                });
                let w = ui.available_width();
                egui::Frame::NONE
                    .fill(t.bg_raised)
                    .stroke(Stroke::new(t.border_width, t.border_subtle))
                    .corner_radius(theme::surface::RADIUS)
                    .inner_margin(egui::Margin::symmetric(space::S2, space::S1))
                    .show(ui, |ui| {
                        let resp = ui.add(
                            egui::TextEdit::singleline(value)
                                .id(inner_id)
                                .frame(egui::Frame::NONE)
                                .hint_text("")
                                .desired_width(w - 20.0)
                                .font(theme::body(13.0)),
                        );
                        if highlight {
                            resp.scroll_to_me(Some(Align::Center));
                            resp.request_focus();
                        }
                    });
            });
        });
}

fn kv_row(ui: &mut egui::Ui, _t: &theme::Tokens, label: &str, value: &mut String, edit_w: f32) {
    ui.horizontal(|ui| {
        let lbl_w = 220.0_f32.min(ui.available_width() * 0.5);
        ui.allocate_ui(egui::vec2(lbl_w, 24.0), |ui| {
            ui.label(
                RichText::new(label)
                    .color(theme::tokens(ui.ctx()).fg_2)
                    .font(theme::body(12.0)),
            );
        });
        let w = if edit_w > 0.0 {
            edit_w
        } else {
            ui.available_width()
        };
        text_input(ui, value, "", w);
    });
}

fn blank_to_none(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() { None } else { Some(s.into()) }
}
fn parse_u64(s: &str, label: &str) -> Result<u64, String> {
    s.trim()
        .parse()
        .map_err(|_| format!("{label} must be a non-negative integer"))
}
fn parse_u32(s: &str, label: &str) -> Result<u32, String> {
    s.trim()
        .parse()
        .map_err(|_| format!("{label} must be a non-negative integer"))
}
fn parse_u16(s: &str, label: &str) -> Result<u16, String> {
    s.trim()
        .parse()
        .map_err(|_| format!("{label} must be a port number"))
}
fn parse_usize(s: &str, label: &str) -> Result<usize, String> {
    s.trim()
        .parse()
        .map_err(|_| format!("{label} must be a non-negative integer"))
}
fn parse_kv_lines(text: &str, label: &str) -> Result<IndexMap<String, String>, String> {
    let mut out = IndexMap::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (k, v) = line
            .split_once(':')
            .ok_or_else(|| format!("{label} line {} missing `:`", i + 1))?;
        let k = k.trim();
        let v = v.trim();
        if k.is_empty() {
            return Err(format!("{label} line {}: empty name", i + 1));
        }
        out.insert(k.to_string(), v.to_string());
    }
    Ok(out)
}
