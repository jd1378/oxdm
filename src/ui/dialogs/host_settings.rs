//! Per-host settings dialog. Left list of hosts with search + add /
//! delete; right pane editor (token-styled cards).

use eframe::egui::{self, Align, Layout, RichText, Sense, Stroke, Vec2};

use crate::data::keyring;
use crate::domain::HostSetting;
use crate::ui::AppShell;
use crate::ui::components::primitives::{
    Btn, BtnSize, TextInput, eyebrow, search_field, text_input,
};
use crate::ui::components::titlebar;
use crate::ui::theme::{self, radius, space};

#[derive(Default)]
pub struct HostState {
    pub search: String,
    pub selected: Option<String>,
    pub editor: Option<HostEditor>,
}

#[derive(Default, Clone)]
pub struct HostEditor {
    pub original_host: String,
    pub host: String,
    pub speed_enabled: bool,
    pub speed_kbs: String,
    pub threads: String,
    pub username: String,
    pub password: String,
    pub user_agent: String,
    pub had_password: bool,
    pub password_loaded_for: Option<String>,
}

impl HostEditor {
    fn from_host(h: &HostSetting) -> Self {
        Self {
            original_host: h.host.clone(),
            host: h.host.clone(),
            speed_enabled: h.speed_limit.is_some(),
            speed_kbs: h
                .speed_limit
                .map(|b| (b / 1024).to_string())
                .unwrap_or_default(),
            threads: h.thread_count.map(|v| v.to_string()).unwrap_or_default(),
            username: h.username.clone().unwrap_or_default(),
            password: String::new(),
            user_agent: h.default_user_agent.clone().unwrap_or_default(),
            had_password: h.has_password,
            password_loaded_for: None,
        }
    }
}

pub fn show(app: &mut AppShell, ctx: &egui::Context) {
    let closed = super::child_viewport(
        ctx,
        "oxdm-host",
        "oxdm — Per host settings",
        (820.0, 580.0),
        |ui| body(app, ui),
    );
    if closed {
        app.host_open = false;
        app.host_state = HostState::default();
    }
}

fn body(app: &mut AppShell, root_ui: &mut egui::Ui) {
    let ctx = &root_ui.ctx().clone();
    let t = theme::tokens(ctx);
    let hosts: Vec<HostSetting> = {
        let s = app.client.clone();
        app.block_on(async move { s.host_list().await })
            .unwrap_or_default()
    };

    if app.host_state.selected.is_none()
        && let Some(h) = hosts.first()
    {
        app.host_state.selected = Some(h.host.clone());
        app.host_state.editor = Some(HostEditor::from_host(h));
    }

    egui::Panel::top("hs_titlebar")
        .frame(egui::Frame::NONE.fill(t.bg_titlebar))
        .show_separator_line(true)
        .show_inside(root_ui, |ui| {
            titlebar::show(ui, ctx, "Per host settings");
        });

    egui::Panel::left("hs_left")
        .default_size(260.0)
        .resizable(false)
        .frame(
            egui::Frame::NONE
                .fill(t.bg_sidebar)
                .inner_margin(egui::Margin::symmetric(space::S3, space::S4)),
        )
        .show_separator_line(false)
        .show_inside(root_ui, |ui| {
            search_field(
                ui,
                &mut app.host_state.search,
                "Search hosts…",
                ui.available_width(),
            );
            ui.add_space(space::S2 as f32);
            let q = app.host_state.search.to_lowercase();
            for h in hosts
                .iter()
                .filter(|h| q.is_empty() || h.host.to_lowercase().contains(&q))
            {
                let active = app.host_state.selected.as_deref() == Some(&h.host);
                if host_row(ui, &t, &h.host, active, h.has_password).clicked() {
                    app.host_state.selected = Some(h.host.clone());
                    app.host_state.editor = Some(HostEditor::from_host(h));
                }
            }
            ui.add_space(space::S3 as f32);
            ui.horizontal(|ui| {
                if Btn::new("Add host")
                    .toolbar()
                    .icon("plus")
                    .show(ui)
                    .clicked()
                {
                    app.host_state.selected = Some(String::new());
                    app.host_state.editor = Some(HostEditor::default());
                }
                if let Some(host) = app.host_state.selected.clone()
                    && !host.is_empty()
                    && Btn::new("Delete")
                        .toolbar()
                        .icon("trash-2")
                        .size(BtnSize::Sm)
                        .show(ui)
                        .clicked()
                {
                    let st = app.client.clone();
                    let host2 = host.clone();
                    app.spawn(async move {
                        let _ = st.delete_host(host2.clone()).await;
                        let _ = keyring::delete_password(&host2);
                    });
                    app.host_state.selected = None;
                    app.host_state.editor = None;
                }
            });
        });

    // Lazy-load keyring password.
    if let Some(ed) = app.host_state.editor.as_ref()
        && ed.had_password
        && !ed.original_host.is_empty()
        && ed.password_loaded_for.as_deref() != Some(ed.original_host.as_str())
    {
        let host = ed.original_host.clone();
        let s = app.client.clone();
        let pw = app
            .block_on(async move { s.host_password(host).await })
            .ok()
            .flatten();
        if let Some(ed) = app.host_state.editor.as_mut() {
            if let Some(p) = pw {
                ed.password = p;
            }
            ed.password_loaded_for = Some(ed.original_host.clone());
        }
    }

    let mut want_save = false;
    egui::Panel::bottom("hs_footer")
        .frame(
            egui::Frame::NONE
                .fill(t.bg_sunken)
                .inner_margin(egui::Margin::symmetric(space::S4, space::S2)),
        )
        .show_separator_line(true)
        .show_inside(root_ui, |ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if Btn::new("Save").primary().icon("save").show(ui).clicked() {
                    want_save = true;
                }
            });
        });

    egui::CentralPanel::default()
        .frame(
            egui::Frame::NONE
                .fill(t.bg_page)
                .inner_margin(egui::Margin::symmetric(space::S4, space::S4)),
        )
        .show_inside(root_ui, |ui| {
            let Some(ed) = app.host_state.editor.as_mut() else {
                ui.label(
                    RichText::new("Select a host to edit, or click + to add one.").color(t.fg_3),
                );
                return;
            };
            ui.spacing_mut().item_spacing.y = space::S3 as f32;

            card(ui, &t, "globe", "Identity", |ui| {
                eyebrow(ui, "host");
                text_input(ui, &mut ed.host, "example.com", ui.available_width());
            });

            card(ui, &t, "activity", "Connection", |ui| {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut ed.speed_enabled, "Speed limit (KB/s)");
                    TextInput::new(&mut ed.speed_kbs)
                        .width(120.0)
                        .font(theme::mono(12.0))
                        .enabled(ed.speed_enabled)
                        .show(ui);
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Threads").color(t.fg_2));
                    TextInput::new(&mut ed.threads)
                        .width(80.0)
                        .font(theme::mono(12.0))
                        .show(ui);
                });
            });

            card(ui, &t, "key", "Authentication", |ui| {
                eyebrow(ui, "username");
                text_input(ui, &mut ed.username, "anonymous", ui.available_width());
                eyebrow(ui, "password");
                let aw = ui.available_width();
                crate::ui::components::primitives::PasswordInput::new(
                    &mut ed.password,
                    ("host-pw", &ed.original_host),
                )
                .width(aw)
                .hint("••••••••")
                .show(ui);
                if ed.had_password {
                    ui.horizontal(|ui| {
                        crate::ui::utils::icons::show(ui, "lock", 15.0, t.status_success);
                        ui.label(
                            RichText::new("Stored in OS keyring")
                                .color(t.status_success)
                                .font(theme::body_bold(11.0)),
                        );
                    });
                }
            });

            card(ui, &t, "settings", "Custom user agent", |ui| {
                text_input(
                    ui,
                    &mut ed.user_agent,
                    "Mozilla/5.0 …",
                    ui.available_width(),
                );
            });
        });

    if want_save && let Some(ed) = app.host_state.editor.as_mut() {
        let h = ed.host.trim().to_string();
        if h.is_empty() {
            return;
        }
        let setting = HostSetting {
            host: h.clone(),
            speed_limit: if ed.speed_enabled {
                ed.speed_kbs.parse::<u64>().ok().map(|k| k * 1024)
            } else {
                None
            },
            thread_count: ed.threads.parse::<u64>().ok(),
            username: {
                let u = ed.username.trim().to_string();
                if u.is_empty() { None } else { Some(u) }
            },
            has_password: !ed.password.is_empty() || ed.had_password,
            default_user_agent: {
                let ua = ed.user_agent.trim().to_string();
                if ua.is_empty() { None } else { Some(ua) }
            },
        };
        let original = ed.original_host.clone();
        let pw = ed.password.clone();
        let st = app.client.clone();
        app.spawn(async move {
            if !original.is_empty() && original != h {
                let _ = st.delete_host(original.clone()).await;
                let _ = keyring::delete_password(&original);
            }
            if !pw.is_empty()
                && let Err(e) = keyring::set_password(&h, &pw)
            {
                tracing::warn!(host = %h, error = %e, "keyring write failed");
            }
            let _ = st.upsert_host(setting).await;
        });
    }
}

fn host_row(
    ui: &mut egui::Ui,
    t: &theme::Tokens,
    host: &str,
    active: bool,
    has_pw: bool,
) -> egui::Response {
    let h = 32.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::click());
    let p = ui.painter().clone();
    if active {
        p.rect_filled(rect, radius::SM as f32, t.action_primary);
    } else if resp.hovered() {
        p.rect_filled(rect, radius::SM as f32, t.bg_sunken);
    }

    let fg = if active { t.action_primary_fg } else { t.fg_1 };
    let g = p.layout_no_wrap(host.to_owned(), theme::body(13.0), fg);
    p.galley(
        egui::pos2(rect.left() + 12.0, rect.center().y - g.size().y / 2.0),
        g,
        fg,
    );

    if has_pw {
        let icon_color = if active {
            t.action_primary_fg
        } else {
            t.status_success
        };
        let img = crate::ui::utils::icons::icon(ui.ctx(), "lock", 15.0, icon_color);
        let r = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 18.0, rect.center().y),
            Vec2::splat(15.0),
        );
        img.paint_at(ui, r);
    }
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
            body(ui);
        });
}
