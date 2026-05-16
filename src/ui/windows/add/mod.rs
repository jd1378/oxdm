//! Standalone Add-download dialog subprocess (`oxdm gui add [<id>]`).
//!
//! Runs as its own eframe window so the capture flow can surface the
//! same UX without requiring the main GUI to be open. Two modes:
//!
//!   - **Fresh** (`oxdm gui add`): empty form, auto-prefilled with a
//!     URL from the OS clipboard if one is present. Submit creates a
//!     new job via `Client::add_job`.
//!   - **Edit** (`oxdm gui add <id>`): loads the target job, prefills
//!     all fields, and submits via `Client::update_job_location` (the
//!     daemon wipes the per-job working dir on filename changes so a
//!     fresh evaluate runs against a clean slate).
//!
//! Filename collision handling: a probe response carrying a filename
//! that matches another job in the daemon's store triggers an
//! overwrite-confirm overlay. Confirm removes the colliding job
//! (purge_partial + delete_final_file) and proceeds with the user's
//! original action.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Align, Color32, Layout, Pos2, Rect, RichText, Stroke, Vec2};
use indexmap::IndexMap;
use tokio::runtime::Runtime;

use crate::data::ProbeResult;
use crate::domain::{Category, Job, JobId, Queue, QueueId, Settings};
use crate::ipc_local::Client;
use crate::ipc_local::protocol::{AddJobReq, JobEdit};
use crate::ui::components::icon_row::icon_row;
use crate::ui::components::primitives::control::{CONTROL_H_MD, CONTROL_RADIUS};
use crate::ui::components::primitives::{
    Btn, BtnSize, Combo, FileInput, PasswordInput, TabBtn, TextArea, TextInput,
    collapsible_section, labeled,
};
use crate::ui::components::titlebar;
use crate::ui::theme::{self, radius, space};
use crate::ui::utils::format::format_bytes_opt;
use crate::ui::utils::icons;

type ProbeSlot = Arc<Mutex<Option<Result<ProbeResult, String>>>>;

/// Hard cap on the add-window inner height when auto-resizing to fit
/// content. Mirrors the download window's growth policy.
const MAX_WINDOW_H: f32 = 820.0;

pub fn launch(edit_id: Option<JobId>, prefill_url: Option<String>) {
    let rt = Runtime::new().expect("tokio runtime");
    let bootstrap = match rt.block_on(connect_and_seed(edit_id)) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "could not reach oxdm daemon");
            return;
        }
    };

    let title = if edit_id.is_some() {
        "oxdm — Edit Download".to_string()
    } else {
        "oxdm — Download File Info".to_string()
    };
    let viewport =
        crate::ui::utils::chrome::viewport_builder(&title, (520.0, 260.0), Some((520.0, 160.0)));
    let opts = eframe::NativeOptions {
        viewport,
        vsync: false,
        ..Default::default()
    };

    let _ = eframe::run_native(
        "oxdm-add",
        opts,
        Box::new(move |cc| {
            let ctx = cc.egui_ctx.clone();
            theme::install_fonts(&cc.egui_ctx);
            icons::install_loaders(&cc.egui_ctx);
            theme::apply(&cc.egui_ctx, &bootstrap.settings);
            let ctx_for_theme = cc.egui_ctx.clone();
            theme::on_system_theme_change(move |_| ctx_for_theme.request_repaint());
            // Subscribe to Lifecycle events so the daemon's Focus
            // pokes (single-instance re-trigger) actually surface this
            // window. spawn_event_loop wants a `Cache`, but the Add
            // window never reads from it — we just need the loop to
            // tick `gui_state` atomics on Focus / daemon_lost.
            let throwaway_snap = crate::ipc_local::protocol::SnapshotData {
                jobs: Vec::new(),
                queues: Vec::new(),
                settings: bootstrap.settings.clone(),
                active_queues: Default::default(),
                conflict_head: None,
                conflict_len: 0,
                counters: Vec::new(),
            };
            let cache =
                std::sync::Arc::new(crate::ui::gui_state::Cache::from_snapshot(throwaway_snap));
            crate::ui::gui_state::spawn_event_loop(
                rt.handle(),
                bootstrap.client.clone(),
                cache,
                crate::ipc_local::protocol::SubFilter::Lifecycle,
                move || ctx.request_repaint(),
            );
            Ok(Box::new(AddShell::new(
                rt,
                bootstrap,
                edit_id,
                prefill_url.clone(),
            )))
        }),
    );
}

struct Bootstrap {
    client: Arc<Client>,
    settings: Settings,
    queues: Vec<Queue>,
    job: Option<Job>,
    /// Decrypted secrets for the job under edit. Pulled in one IPC
    /// round-trip on dialog open so the password component can lazily
    /// reveal the real value when the user clicks the eye icon or
    /// focuses the field, without a second daemon hop.
    secrets: crate::ipc_local::client::JobSecretsPlaintext,
}

async fn connect_and_seed(edit_id: Option<JobId>) -> Result<Bootstrap, String> {
    let client = crate::ui::connect_or_spawn_daemon().await?;
    client
        .hello(crate::ipc_local::protocol::GuiKind::Add)
        .await?;
    let snap = client.snapshot().await?;
    let (job, secrets) = if let Some(id) = edit_id {
        let j = client.job_entry(id).await?.map(|v| v.job);
        let s = client.job_secrets_plaintext(id).await.unwrap_or_default();
        (j, s)
    } else {
        (None, Default::default())
    };
    Ok(Bootstrap {
        client,
        settings: snap.settings,
        queues: snap.queues,
        job,
        secrets,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum AdvTab {
    #[default]
    Proxy,
    Headers,
    Auth,
    UserAgent,
    Cookies,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ProxyKind {
    #[default]
    None,
    Http,
    Socks5,
}

impl ProxyKind {
    fn label(self) -> &'static str {
        match self {
            ProxyKind::None => "None (direct)",
            ProxyKind::Http => "HTTP",
            ProxyKind::Socks5 => "SOCKS5",
        }
    }
}

#[derive(Debug, Clone, Default)]
struct AddForm {
    url: String,
    /// Unified save target — full path. Probe completion fills this
    /// with `<download_dir>/<detected_filename>` on first success;
    /// edits split into directory + filename at submit time.
    save_path: String,
    referrer: String,
    headers: Vec<(String, String)>,
    proxy_kind: ProxyKind,
    proxy_host_port: String,
    proxy_user: String,
    proxy_pass: String,
    auth_user: String,
    auth_pass: String,
    /// `true` once the user has typed (or cleared) the auth password
    /// field in this session. Edit mode loads a blank field, so without
    /// this flag we cannot tell "user wants no password" from "user
    /// hasn't touched the field — keep whatever is in the keyring".
    user_agent: String,
    cookies: String,
    adv_tab: AdvTab,
    queue: Option<QueueId>,
    category_override: Option<Category>,
    segments: u32,
    /// User manually picked a segment count, so probe-based size
    /// suggestion should no longer override it.
    segments_user_edited: bool,
    /// User typed in save-to or picked via the file dialog, so probe
    /// completion should NOT replace `save_path` with `<dir>/<probed>`
    /// on a subsequent URL change.
    save_path_user_edited: bool,
    error: Option<String>,
    probe_result: Option<Result<ProbeResult, String>>,
    probing: bool,
    last_probed_url: String,
    /// Set by the URL row when egui reports `Response::changed()` and
    /// consumed by `probe_pipeline` on the same frame. Decoupling the
    /// edit event from the probe keeps `url_row` UI-only and lets the
    /// pipeline handle debouncing in one place.
    url_dirty: bool,
    /// Instant of the most recent URL field change, used by the probe
    /// debounce. `None` means "no pending probe to fire".
    pending_probe_since: Option<std::time::Instant>,
    /// Skip the debounce on the next probe trigger. Set when the URL
    /// arrives non-interactively (init prefill from argv / clipboard /
    /// edit job) so the user doesn't sit watching an empty card for
    /// 400ms before the probe even starts.
    skip_debounce: bool,
    probe_slot: Option<ProbeSlot>,
    /// Info-level message shown above the body. Currently used to
    /// surface the auto-rename that fires when the probed filename
    /// already exists in the daemon's store. Cleared whenever the
    /// user starts a new probe (URL changed).
    info: Option<String>,
}

struct AddShell {
    rt: Runtime,
    client: Arc<Client>,
    edit_id: Option<JobId>,
    settings: Settings,
    queues: Vec<Queue>,
    form: AddForm,
    overwrite_confirm: Option<JobId>,
    pending_action: Option<bool>, // start_now after overwrite resolution
    /// Async dup-check slot. Result is `Option<JobId>` of the matching
    /// job (or `None` if no match). Filled by a background task spawned
    /// once per new probed filename.
    dup_slot: Option<Arc<Mutex<Option<Option<JobId>>>>>,
    dup_for_name: Option<String>,
    want_close: bool,
    auto_resize: crate::ui::utils::chrome::AutoResize,
    theme_applied_for: Option<crate::ui::theme::ResolvedTheme>,
    #[cfg(target_os = "windows")]
    surfaced: bool,
}

impl AddShell {
    fn new(rt: Runtime, b: Bootstrap, edit_id: Option<JobId>, prefill_url: Option<String>) -> Self {
        let main_queue_id = b.queues.iter().find(|q| q.builtin).map(|q| q.id);
        let download_dir = b.settings.download_dir.to_string_lossy().into_owned();
        let mut form = AddForm {
            segments: b.settings.max_connections.unwrap_or(8) as u32,
            queue: main_queue_id,
            ..Default::default()
        };
        if let Some(j) = b.job.as_ref() {
            form.url = j.url.to_string();
            form.url_dirty = !form.url.is_empty();
            form.skip_debounce = form.url_dirty;
            form.save_path = match &j.filename {
                Some(name) => j.save_dir.join(name).to_string_lossy().into_owned(),
                None => j.save_dir.to_string_lossy().into_owned(),
            };
            // Editing an existing job: treat its save path as
            // user-chosen so a re-probe (on URL change) doesn't
            // overwrite the user's prior pick.
            form.save_path_user_edited = true;
            form.referrer = j
                .referrer
                .as_ref()
                .map(|u| u.to_string())
                .unwrap_or_default();
            if let Some(n) = j.max_connections {
                form.segments = n as u32;
                form.segments_user_edited = true;
            }
            if let Some(p) = j.proxy.as_deref()
                && let Some((kind, host_port, user, _)) = parse_proxy_url(p)
            {
                form.proxy_kind = kind;
                form.proxy_host_port = host_port;
                form.proxy_user = user;
            }
            // Basic-auth fields come from structured columns + keyring
            // sentinel — the password itself is never sent over the
            // wire, only a "stored?" flag.
            form.auth_user = j.auth_user.clone().unwrap_or_default();
            // Cookies render the decrypted value directly — user can
            // Plaintexts come from the one-shot `JobSecretsPlaintext`
            // fetch in `connect_and_seed`. `PasswordInput` masks
            // visually but the bound `String`s hold real plaintext so
            // any keystroke edits the actual current password.
            form.cookies = b.secrets.cookies.clone().unwrap_or_default();
            form.auth_pass = b.secrets.auth_password.clone().unwrap_or_default();
            form.proxy_pass = b.secrets.proxy_password.clone().unwrap_or_default();
            // Split known headers into structured tabs; the rest stay
            // in the Headers list. `Authorization` is *not* a managed
            // tab anymore — leave any bearer / captured token as a raw
            // header so the user can see and edit it.
            for (k, v) in j.headers.iter() {
                match k.to_ascii_lowercase().as_str() {
                    "user-agent" => form.user_agent = v.clone(),
                    _ => form.headers.push((k.clone(), v.clone())),
                }
            }
            form.queue = Some(j.queue_id);
        } else {
            form.save_path = download_dir;
            // Parent process passes a clipboard URL via `--url` so the
            // common "copy → Add Download" path needs zero clicks. If
            // the parent's read failed (e.g. clipboard busy at the
            // moment of spawn) try once more here as a fallback.
            if let Some(u) = prefill_url {
                form.url = u;
            } else if let Some(u) = crate::ui::clipboard::read_url_from_clipboard() {
                form.url = u.to_string();
            }
            form.url_dirty = !form.url.is_empty();
            form.skip_debounce = form.url_dirty;
        }
        Self {
            rt,
            client: b.client,
            edit_id,
            settings: b.settings,
            queues: b.queues,
            form,
            overwrite_confirm: None,
            pending_action: None,
            dup_slot: None,
            dup_for_name: None,
            want_close: false,
            auto_resize: crate::ui::utils::chrome::AutoResize::new(MAX_WINDOW_H, true, 520.0),
            theme_applied_for: None,
            #[cfg(target_os = "windows")]
            surfaced: false,
        }
    }
}

impl eframe::App for AddShell {
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
        if crate::ui::gui_state::take_focus_request() {
            crate::ui::gui_state::surface_window(ctx);
        }
        if self.want_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            std::process::exit(0);
        }
        if crate::ui::gui_state::daemon_lost() {
            std::process::exit(0);
        }
        crate::ui::utils::resize::show_styled(
            ctx,
            crate::ui::utils::chrome::ChromeStyle {
                dark_border: true,
                resizable: true,
            },
        );
        let resolved_now = theme::resolve(self.settings.theme);
        if self.theme_applied_for != Some(resolved_now) {
            theme::apply(ctx, &self.settings);
            self.theme_applied_for = Some(resolved_now);
        }
        self.body(root_ui);
        if ctx.input(|i| i.viewport().close_requested()) {
            std::process::exit(0);
        }
    }
}

impl AddShell {
    fn block_on<F: std::future::Future>(&self, f: F) -> F::Output {
        self.rt.handle().block_on(f)
    }

    fn spawn<Fut: std::future::Future<Output = ()> + Send + 'static>(&self, fut: Fut) {
        self.rt.spawn(fut);
    }

    fn body(&mut self, root_ui: &mut egui::Ui) {
        let ctx = &root_ui.ctx().clone();
        let t = theme::tokens(ctx);
        let title = if self.edit_id.is_some() {
            "Edit Download"
        } else {
            "Download File Info"
        };

        let title_resp = egui::Panel::top("add_titlebar")
            .frame(egui::Frame::NONE.fill(t.bg_titlebar))
            .show_separator_line(true)
            .show_inside(root_ui, |ui| {
                titlebar::show_with(
                    ui,
                    ctx,
                    title,
                    titlebar::Opts {
                        show_maximize: false,
                    },
                );
            });
        let title_h = title_resp.response.rect.height();

        let queue_name = self
            .form
            .queue
            .and_then(|qid| {
                self.queues
                    .iter()
                    .find(|q| q.id == qid)
                    .map(|q| q.name.clone())
            })
            .unwrap_or_else(|| "Main".into());
        let detected = matches!(self.form.probe_result, Some(Ok(_)));
        let editing = self.edit_id.is_some();
        let mut close = false;
        let mut submit_now: Option<bool> = None;

        let footer_resp = egui::Panel::bottom("add_footer")
            .frame(
                egui::Frame::NONE
                    .fill(t.bg_sunken)
                    .inner_margin(egui::Margin::symmetric(space::S4, space::S2)),
            )
            .show_separator_line(true)
            .show_inside(root_ui, |ui| {
                ui.horizontal(|ui| {
                    if Btn::new("Cancel").ghost().icon("x").show(ui).clicked() {
                        close = true;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if Btn::new("Download now")
                            .primary()
                            .icon("download")
                            .enabled(detected)
                            .show(ui)
                            .clicked()
                        {
                            submit_now = Some(true);
                        }
                        let later_label = if editing {
                            "Save".to_string()
                        } else {
                            format!("Add to {queue_name}")
                        };
                        if Btn::new(later_label)
                            .icon("clock")
                            .enabled(detected)
                            .show(ui)
                            .clicked()
                        {
                            submit_now = Some(false);
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
                    .inner_margin(egui::Margin::symmetric(space::S4, space::S4)),
            )
            .show_inside(root_ui, |ui| {
                let scope = ui.scope(|ui| {
                    ui.spacing_mut().item_spacing.y = space::S3 as f32;
                    self.url_row(ui, &t);
                    self.detected_card(ui, &t);
                    let non_resumable = self
                        .form
                        .probe_result
                        .as_ref()
                        .and_then(|r| r.as_ref().ok())
                        .map(|r| !r.is_resumable)
                        .unwrap_or(false);
                    if non_resumable {
                        warning_banner_locked_segments(ui, &t, 1);
                    }
                    if detected {
                        self.location_and_category_row(ui, &t);
                        if let Some(msg) = self.form.info.as_ref() {
                            info_banner(ui, &t, msg);
                        }
                        self.options_row(ui, &t);
                        self.advanced_section(ui, &t);
                    }
                    if let Some(err) = self.form.error.as_ref() {
                        ui.add(
                            egui::Label::new(
                                RichText::new(err)
                                    .color(t.status_danger)
                                    .font(theme::body_bold(12.0)),
                            )
                            .wrap(),
                        );
                    }
                });
                content_h = scope.response.rect.height();
            });

        let margin_v = (space::S4 as f32 * 2.0) + 4.0;
        let target_h = title_h + footer_h + content_h + margin_v;
        self.auto_resize.apply(ctx, target_h);

        // Probe pipeline.
        self.probe_pipeline(ctx);
        // Async dup-check pipeline.
        self.dup_pipeline();

        // Submit.
        if let Some(start_now) = submit_now {
            self.try_submit(ctx, start_now);
        }

        // Overwrite overlay.
        if let Some(other_id) = self.overwrite_confirm {
            self.overwrite_overlay(ctx, &t, other_id);
        }

        if close {
            // Edit-cancel for capture path: remove the half-baked
            // capture job so a dismissed prompt never leaves an orphan.
            if let Some(id) = self.edit_id {
                let s = self.client.clone();
                let _ = self.block_on(async move {
                    s.remove(
                        id,
                        crate::data::RemoveOpts {
                            purge_partial: true,
                            delete_final_file: false,
                        },
                    )
                    .await
                });
            }
            self.want_close = true;
        }
    }

    fn url_row(&mut self, ui: &mut egui::Ui, t: &theme::Tokens) {
        labeled(ui, "url", |ui| {
            ui.horizontal(|ui| {
                let avail = ui.available_width();
                let paste_w = 88.0;
                let gap = space::S2 as f32;
                let frame_pad_x = space::S3 as f32 * 2.0;
                let edit_w = avail - paste_w - gap;
                let stroke = if self.form.url.is_empty() {
                    t.border_subtle
                } else {
                    t.border_brand
                };
                egui::Frame::NONE
                    .fill(t.bg_raised)
                    .stroke(Stroke::new(t.border_width, stroke))
                    .corner_radius(CONTROL_RADIUS)
                    .inner_margin(egui::Margin::symmetric(space::S3, 0))
                    .show(ui, |ui| {
                        let inner_w = (edit_w - frame_pad_x).max(60.0);
                        ui.set_min_width(inner_w);
                        ui.set_max_width(inner_w);
                        ui.set_min_height(CONTROL_H_MD);
                        let hint = RichText::new("https://…").color(t.fg_4);
                        let edit = egui::TextEdit::singleline(&mut self.form.url)
                            .frame(egui::Frame::NONE)
                            .hint_text(hint)
                            .desired_width(inner_w)
                            .font(theme::mono(12.0))
                            .vertical_align(Align::Center);
                        let resp = ui.add(edit);
                        if resp.changed() {
                            self.form.url_dirty = true;
                            // Any paste (Ctrl+V, middle-click primary
                            // selection, context-menu Paste) lands as
                            // `Event::Paste` in the same frame the value
                            // changes. Skip the typing debounce so the
                            // probe fires immediately on paste.
                            if resp.has_focus() {
                                let pasted = ui.input(|i| {
                                    i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)))
                                });
                                if pasted {
                                    self.form.skip_debounce = true;
                                }
                            }
                        }
                    });
                if Btn::new("Paste").icon("clipboard").show(ui).clicked()
                    && let Some(u) = crate::ui::clipboard::read_url_from_clipboard()
                    && matches!(u.scheme(), "http" | "https")
                {
                    self.form.url = u.to_string();
                    self.form.url_dirty = true;
                    self.form.skip_debounce = true;
                }
            });
        });
    }

    fn detected_card(&self, ui: &mut egui::Ui, t: &theme::Tokens) {
        let detected = self
            .form
            .probe_result
            .as_ref()
            .and_then(|r| r.as_ref().ok());
        let has_link = self.form.probing || self.form.probe_result.is_some();
        let non_resumable = detected.map(|r| !r.is_resumable).unwrap_or(false);
        // Non-resumable detected: tint the card with the same warning
        // colours used by `warning_banner` so the chain of warnings reads
        // as one visual unit.
        let fill = if non_resumable {
            t.status_warning_bg
        } else if has_link {
            t.bg_raised
        } else {
            t.bg_surface
        };
        let stroke_solid = Stroke::new(
            t.border_width,
            if non_resumable {
                t.status_warning
            } else if has_link {
                t.border_subtle
            } else {
                Color32::TRANSPARENT
            },
        );
        let radius = theme::surface::RADIUS as f32;
        let resp = egui::Frame::NONE
            .fill(fill)
            .stroke(stroke_solid)
            .corner_radius(theme::surface::RADIUS)
            .inner_margin(space::S3 as f32)
            .show(ui, |ui| {
                if self.form.probing {
                    ui.multiply_opacity(0.6);
                }
                icon_row(
                    ui,
                    48.0,
                    |ui, rect| {
                        let painter = ui.painter().clone();
                        let tile_r = radius::SM as f32;
                        let clay_100 = Color32::from_rgb(0xF4, 0xD9, 0xC6);
                        let clay_200 = Color32::from_rgb(0xE9, 0xB5, 0x95);
                        let clay_700 = Color32::from_rgb(0x6B, 0x34, 0x17);
                        if let Some(r) = detected {
                            let cat = crate::domain::classify(&r.filename, &Default::default());
                            let (_color, _icon, label) = cat_visual(cat, t);
                            let ext = r
                                .filename
                                .rsplit_once('.')
                                .map(|(_, e)| e.to_ascii_uppercase())
                                .filter(|s| !s.is_empty())
                                .unwrap_or_else(|| label.to_uppercase());
                            // Non-resumable: ochre-100 bg + ochre-500 text,
                            // no border. Matches the warning visual chain.
                            let (tile_bg, text_color) = if !r.is_resumable {
                                (crate::ui::color::ochre::O100, crate::ui::color::ochre::O500)
                            } else {
                                (clay_100, clay_700)
                            };
                            painter.rect_filled(rect, tile_r, tile_bg);
                            let g = painter.layout_no_wrap(ext, theme::mono_bold(11.0), text_color);
                            painter.galley(
                                Pos2::new(
                                    rect.center().x - g.size().x / 2.0,
                                    rect.center().y - g.size().y / 2.0,
                                ),
                                g,
                                text_color,
                            );
                        } else {
                            let bg = if self.form.probing {
                                t.bg_surface
                            } else {
                                t.bg_sunken
                            };
                            painter.rect_filled(rect, tile_r, bg);
                            let glyph = if self.form.probing {
                                "ellipsis"
                            } else {
                                "link"
                            };
                            let icon = icons::icon(ui.ctx(), glyph, 22.0, t.fg_3);
                            let ic_rect = Rect::from_center_size(rect.center(), Vec2::splat(22.0));
                            icon.paint_at(ui, ic_rect);
                        }
                        painter.rect_stroke(
                            rect,
                            tile_r,
                            Stroke::new(1.0, clay_200),
                            egui::StrokeKind::Inside,
                        );
                    },
                    |ui| {
                        if let Some(r) = detected {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&r.filename)
                                        .font(theme::body_bold(14.0))
                                        .color(t.fg_1),
                                )
                                .truncate(),
                            );
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                let host = url::Url::parse(&self.form.url)
                                    .ok()
                                    .and_then(|u| u.host_str().map(str::to_owned))
                                    .unwrap_or_default();
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(host).color(t.fg_3).font(theme::mono(11.0)),
                                    )
                                    .truncate(),
                                );
                            });
                        } else if self.form.probing {
                            ui.label(
                                RichText::new("Detecting file information…")
                                    .color(t.fg_2)
                                    .font(theme::body_bold(13.0)),
                            );
                            ui.label(
                                RichText::new("Probing the link for filename and size.")
                                    .color(t.fg_3)
                                    .font(theme::body(12.0)),
                            );
                        } else if matches!(self.form.probe_result, Some(Err(_))) {
                            let msg = if let Some(Err(e)) = &self.form.probe_result {
                                e.clone()
                            } else {
                                String::new()
                            };
                            ui.label(
                                RichText::new("Could not detect file")
                                    .color(t.status_danger)
                                    .font(theme::body_bold(13.0)),
                            );
                            ui.label(RichText::new(msg).color(t.fg_3).font(theme::body(12.0)));
                        } else {
                            ui.label(
                                RichText::new("Paste a URL link")
                                    .color(t.fg_2)
                                    .font(theme::body_bold(13.0)),
                            );
                            ui.label(
                                RichText::new("We'll detect filename, size, and resumability.")
                                    .color(t.fg_3)
                                    .font(theme::body(12.0)),
                            );
                        }
                    },
                    |ui| {
                        let (value, color) = match detected {
                            Some(r) => (format_bytes_opt(r.size), t.fg_1),
                            None => ("—".to_string(), t.fg_3),
                        };
                        stat_column(ui, t, "SIZE", &value, color);
                    },
                );
            });
        if !has_link {
            paint_dashed_rect(
                ui.painter(),
                resp.response.rect,
                radius,
                Stroke::new(t.border_width, t.border_default),
            );
        }
    }

    fn location_and_category_row(&mut self, ui: &mut egui::Ui, _t: &theme::Tokens) {
        ui.spacing_mut().item_spacing.x = space::S3 as f32;
        ui.columns(2, |cols| {
            labeled(&mut cols[0], "save to", |ui| {
                let resp = FileInput::new(&mut self.form.save_path)
                    .hint("Pick a URL to detect filename")
                    .tooltip("Browse for save location")
                    .show(ui);
                if resp.text.changed() {
                    self.form.save_path_user_edited = true;
                }
                if resp.browse.clicked() {
                    let _g = self.rt.handle().enter();
                    let starting_dir = current_dir(&self.form.save_path)
                        .unwrap_or_else(|| self.settings.download_dir.clone());
                    let starting_name = current_filename(&self.form.save_path)
                        .or_else(|| {
                            self.form
                                .probe_result
                                .as_ref()
                                .and_then(|r| r.as_ref().ok())
                                .map(|r| r.filename.clone())
                        })
                        .unwrap_or_else(|| "unknown".into());
                    if let Some(p) = rfd::FileDialog::new()
                        .set_directory(&starting_dir)
                        .set_file_name(&starting_name)
                        .save_file()
                    {
                        self.form.save_path = p.to_string_lossy().into_owned();
                        self.form.save_path_user_edited = true;
                    }
                }
            });

            labeled(&mut cols[1], "category", |ui| {
                let detected_cat = self
                    .form
                    .probe_result
                    .as_ref()
                    .and_then(|r| r.as_ref().ok())
                    .map(|r| crate::domain::classify(&r.filename, &Default::default()));
                let current = self.form.category_override.or(detected_cat);
                let label = current.map(|c| c.label()).unwrap_or("—");
                Combo::new("add_category", label).show(ui, |ui| {
                    for cat in Category::ALL_ASSIGNABLE.iter().copied() {
                        if Combo::item(ui, cat.label(), true).clicked() {
                            self.form.category_override = Some(cat);
                            ui.close();
                        }
                    }
                });
            });
        });
    }

    fn options_row(&mut self, ui: &mut egui::Ui, _t: &theme::Tokens) {
        ui.spacing_mut().item_spacing.x = space::S3 as f32;
        ui.columns(2, |cols| {
            labeled(&mut cols[0], "queue", |ui| {
                let queues = &self.queues;
                let cur = self
                    .form
                    .queue
                    .and_then(|qid| queues.iter().find(|q| q.id == qid).map(|q| q.name.clone()))
                    .unwrap_or_else(|| "Main".into());
                Combo::new("add_queue", cur).show(ui, |ui| {
                    for q in queues {
                        if Combo::item(ui, &q.name, true).clicked() {
                            self.form.queue = Some(q.id);
                            ui.close();
                        }
                    }
                });
            });

            labeled(&mut cols[1], "segments", |ui| {
                let current = self.form.segments;
                Combo::new("add_segments", format!("{current} connections")).show(ui, |ui| {
                    for n in [1u32, 2, 4, 8, 16, 32] {
                        if Combo::item(ui, &format!("{n} connections"), true).clicked() {
                            self.form.segments = n;
                            self.form.segments_user_edited = true;
                            ui.close();
                        }
                    }
                });
            });
        });
    }

    fn advanced_section(&mut self, ui: &mut egui::Ui, t: &theme::Tokens) {
        let _ = collapsible_section(
            ui,
            egui::Id::new("add-advanced-open"),
            "Advanced",
            false,
            |ui| {
                egui::Frame::NONE
                    .fill(t.bg_sunken)
                    .corner_radius(theme::surface::RADIUS)
                    .inner_margin(egui::Margin {
                        left: space::S3,
                        right: space::S3,
                        top: 0,
                        bottom: space::S3,
                    })
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = space::S3 as f32;
                        self.adv_tab_strip(ui, t);
                        match self.form.adv_tab {
                            AdvTab::Proxy => self.adv_proxy(ui, t),
                            AdvTab::Headers => self.adv_headers(ui, t),
                            AdvTab::Auth => self.adv_auth(ui, t),
                            AdvTab::UserAgent => self.adv_user_agent(ui, t),
                            AdvTab::Cookies => self.adv_cookies(ui, t),
                        }
                    });
            },
        );
    }

    fn adv_tab_strip(&mut self, ui: &mut egui::Ui, t: &theme::Tokens) {
        let tabs = [
            ("Proxy", AdvTab::Proxy),
            ("Headers", AdvTab::Headers),
            ("Auth", AdvTab::Auth),
            ("User agent", AdvTab::UserAgent),
            ("Cookies", AdvTab::Cookies),
        ];
        let row = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (label, kind) in tabs {
                let active = self.form.adv_tab == kind;
                if TabBtn::new(label)
                    .active(active)
                    .font_size(11.0)
                    .show(ui)
                    .clicked()
                {
                    self.form.adv_tab = kind;
                }
            }
        });
        // Hairline below the tab strip — full width, matching the table header.
        let y = row.response.rect.bottom();
        let mr = ui.max_rect();
        ui.painter().line_segment(
            [Pos2::new(mr.left(), y), Pos2::new(mr.right(), y)],
            Stroke::new(1.0, t.border_subtle),
        );
    }

    fn adv_proxy(&mut self, ui: &mut egui::Ui, _t: &theme::Tokens) {
        ui.spacing_mut().item_spacing.y = space::S3 as f32;
        ui.spacing_mut().item_spacing.x = space::S3 as f32;
        let enabled = self.form.proxy_kind != ProxyKind::None;
        ui.columns(2, |cols| {
            labeled(&mut cols[0], "type", |ui| {
                let cur = self.form.proxy_kind;
                Combo::new("add_proxy_kind", cur.label()).show(ui, |ui| {
                    for k in [ProxyKind::None, ProxyKind::Http, ProxyKind::Socks5] {
                        if Combo::item(ui, k.label(), true).clicked() && self.form.proxy_kind != k {
                            self.form.proxy_kind = k;
                            if k == ProxyKind::None {
                                self.form.proxy_host_port.clear();
                                self.form.proxy_user.clear();
                                self.form.proxy_pass.clear();
                            }
                            ui.close();
                        }
                    }
                });
            });
            labeled(&mut cols[1], "host : port", |ui| {
                TextInput::new(&mut self.form.proxy_host_port)
                    .hint("127.0.0.1:1080")
                    .font(theme::mono(12.0))
                    .enabled(enabled)
                    .show(ui);
            });
        });
        ui.columns(2, |cols| {
            labeled(&mut cols[0], "username", |ui| {
                TextInput::new(&mut self.form.proxy_user)
                    .hint("optional")
                    .font(theme::body(12.0))
                    .enabled(enabled)
                    .show(ui);
            });
            labeled(&mut cols[1], "password", |ui| {
                PasswordInput::new(&mut self.form.proxy_pass, "add-proxy-pass")
                    .hint("optional")
                    .enabled(enabled)
                    .show(ui);
            });
        });
    }

    fn adv_headers(&mut self, ui: &mut egui::Ui, _t: &theme::Tokens) {
        ui.spacing_mut().item_spacing.y = space::S3 as f32;
        let scrollbar_pad = ui.spacing().scroll.bar_width + ui.spacing().scroll.bar_inner_margin;
        let row_w = ui.available_width() - scrollbar_pad;
        let row_h = crate::ui::theme::control::H_MD;
        let gap_y = space::S3 as f32;
        let max_rows_visible = 3;
        let max_h =
            row_h * max_rows_visible as f32 + gap_y * (max_rows_visible as f32 - 1.0).max(0.0);

        let mut remove_idx: Option<usize> = None;
        egui::ScrollArea::vertical()
            .id_salt("adv-headers-scroll")
            .max_height(max_h)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = space::S3 as f32;
                for i in 0..self.form.headers.len() {
                    ui.push_id(("adv-header-row", i), |ui| {
                        ui.horizontal(|ui| {
                            ui.set_max_width(row_w);
                            ui.spacing_mut().item_spacing.x = space::S3 as f32;
                            let x_w = crate::ui::theme::control::H_MD;
                            let gap = space::S3 as f32;
                            let each = (row_w - x_w - gap * 2.0) * 0.5;
                            let (k, v) = self.form.headers.get_mut(i).unwrap();
                            TextInput::new(k)
                                .hint("Header")
                                .width(each)
                                .font(theme::body(12.0))
                                .show(ui);
                            TextInput::new(v)
                                .hint("value")
                                .width(each)
                                .font(theme::body(12.0))
                                .show(ui);
                            if Btn::new("")
                                .icon_only("x")
                                .size(BtnSize::Md)
                                .show(ui)
                                .clicked()
                            {
                                remove_idx = Some(i);
                            }
                        });
                    });
                }
            });
        if let Some(i) = remove_idx {
            self.form.headers.remove(i);
        }
        let full_w = ui.available_width();
        if Btn::new("Add header")
            .ghost()
            .icon("plus")
            .font_size(11.0)
            .icon_size(14.0)
            .min_width(full_w)
            .show(ui)
            .clicked()
        {
            self.form.headers.push((String::new(), String::new()));
        }
    }

    fn adv_auth(&mut self, ui: &mut egui::Ui, _t: &theme::Tokens) {
        ui.spacing_mut().item_spacing.y = space::S3 as f32;
        ui.spacing_mut().item_spacing.x = space::S3 as f32;
        ui.columns(2, |cols| {
            labeled(&mut cols[0], "username", |ui| {
                TextInput::new(&mut self.form.auth_user)
                    .font(theme::body(12.0))
                    .show(ui);
            });
            labeled(&mut cols[1], "password", |ui| {
                PasswordInput::new(&mut self.form.auth_pass, "add-auth-pass").show(ui);
            });
        });
    }

    fn adv_user_agent(&mut self, ui: &mut egui::Ui, _t: &theme::Tokens) {
        labeled(ui, "user agent", |ui| {
            TextInput::new(&mut self.form.user_agent)
                .hint("oxdm/1.0")
                .font(theme::body(12.0))
                .show(ui);
        });
    }

    fn adv_cookies(&mut self, ui: &mut egui::Ui, _t: &theme::Tokens) {
        labeled(ui, "cookies", |ui| {
            TextArea::new(&mut self.form.cookies, "add-adv-cookies")
                .hint("session_id=…; csrf=…")
                .font(theme::mono(12.0))
                .initial_height(96.0)
                .min_height(60.0)
                .max_height(320.0)
                .show(ui);
        });
    }

    fn probe_pipeline(&mut self, ctx: &egui::Context) {
        use std::time::{Duration, Instant};
        const DEBOUNCE: Duration = Duration::from_millis(400);

        let trimmed = self.form.url.trim().to_string();

        // Edit event from the URL row (egui `Response::changed()`).
        // Clear stale detected info so the card flips to "not detected"
        // immediately, then arm / reset the debounce timer. The actual
        // probe fires only once the URL has been quiet for `DEBOUNCE`.
        if std::mem::take(&mut self.form.url_dirty) {
            self.form.probe_result = None;
            self.form.probing = false;
            self.form.probe_slot = None;
            self.form.info = None;
            self.dup_slot = None;
            self.dup_for_name = None;
            self.form.last_probed_url.clear();
            if trimmed.is_empty() {
                self.form.pending_probe_since = None;
            } else {
                self.form.pending_probe_since = Some(Instant::now());
                ctx.request_repaint_after(DEBOUNCE);
            }
        }

        let due = std::mem::take(&mut self.form.skip_debounce)
            || self
                .form
                .pending_probe_since
                .map(|t| t.elapsed() >= DEBOUNCE)
                .unwrap_or(false);
        if due
            && let Ok(url) = url::Url::parse(&trimmed)
            && matches!(url.scheme(), "http" | "https")
        {
            self.form.pending_probe_since = None;
            self.form.last_probed_url = trimmed;
            self.form.probing = true;
            let slot: ProbeSlot = Arc::new(Mutex::new(None));
            self.form.probe_slot = Some(slot.clone());
            let s = self.client.clone();
            let cctx = ctx.clone();
            self.spawn(async move {
                let r = tokio::time::timeout(Duration::from_millis(8000), s.probe(url)).await;
                let mapped: Result<ProbeResult, String> = match r {
                    Ok(Ok(Ok(r))) => Ok(r),
                    Ok(Ok(Err(e))) => Err(e),
                    Ok(Err(e)) => Err(e),
                    Err(_) => Err("timed out".into()),
                };
                if let Ok(mut g) = slot.lock() {
                    *g = Some(mapped);
                }
                cctx.request_repaint();
            });
        } else if due {
            // Debounce elapsed but URL doesn't parse — drop the timer
            // so we don't busy-loop checking again every frame.
            self.form.pending_probe_since = None;
        }
        if self.form.probing
            && let Some(slot) = self.form.probe_slot.clone()
            && let Some(res) = slot.lock().ok().and_then(|mut g| g.take())
        {
            self.form.probing = false;
            self.form.probe_result = Some(res);
            self.form.probe_slot = None;
            if !self.form.segments_user_edited
                && self.settings.max_connections.is_none()
                && let Some(Ok(r)) = &self.form.probe_result
            {
                self.form.segments = suggest_segments(r.size);
            }
            // Re-derive save_path on every probe completion as long as
            // the user hasn't manually edited it. The directory part
            // comes from the existing path when present (preserves a
            // pre-edited folder) or from the default download dir
            // otherwise; filename comes from the freshly-detected one.
            let probed_name = match &self.form.probe_result {
                Some(Ok(r)) => Some(r.filename.clone()),
                _ => None,
            };
            if let Some(name) = probed_name {
                if !self.form.save_path_user_edited {
                    let raw = self.form.save_path.trim();
                    let path = Path::new(raw);
                    // If the existing path is a real directory (or
                    // empty / trailing-slash), keep that directory;
                    // otherwise fall back to the user's default
                    // download dir.
                    let dir = if raw.is_empty() {
                        self.settings.download_dir.clone()
                    } else if raw.ends_with(std::path::MAIN_SEPARATOR) || path.is_dir() {
                        path.to_path_buf()
                    } else {
                        current_dir(raw).unwrap_or_else(|| self.settings.download_dir.clone())
                    };
                    self.form.save_path = dir.join(&name).to_string_lossy().into_owned();
                }
                self.kick_dup_check(&name);
            }
        }
    }

    fn kick_dup_check(&mut self, name: &str) {
        if self.dup_for_name.as_deref() == Some(name) {
            return;
        }
        self.dup_for_name = Some(name.to_string());
        let slot: Arc<Mutex<Option<Option<JobId>>>> = Arc::new(Mutex::new(None));
        self.dup_slot = Some(slot.clone());
        let s = self.client.clone();
        let owned = name.to_string();
        let edit = self.edit_id;
        self.spawn(async move {
            let r = s.find_job_by_filename(owned).await.ok().flatten();
            // When editing, ignore self-match.
            let r = match (r, edit) {
                (Some(id), Some(e)) if id == e => None,
                (r, _) => r,
            };
            if let Ok(mut g) = slot.lock() {
                *g = Some(r);
            }
        });
    }

    fn dup_pipeline(&mut self) {
        if let Some(slot) = self.dup_slot.clone()
            && let Some(res) = slot.lock().ok().and_then(|mut g| g.take())
        {
            self.dup_slot = None;
            // Auto-rename the probed filename portion of save_path so
            // the user's next click is unambiguous; collision is still
            // flagged via overwrite overlay if the user keeps the
            // matching name and submits.
            if res.is_some()
                && let Some(Ok(r)) = self.form.probe_result.clone()
            {
                let dir = current_dir(&self.form.save_path)
                    .unwrap_or_else(|| self.settings.download_dir.clone());
                let cur_name = current_filename(&self.form.save_path);
                // Only rename if the path's filename still matches the
                // detected name (user hasn't typed an override).
                if cur_name.as_deref() == Some(&r.filename) {
                    // Suggest "(1)" and let the user confirm at submit
                    // — async chain to keep UI responsive.
                    self.suggest_unique_name(&dir, &r.filename);
                }
            }
        }
    }

    fn suggest_unique_name(&mut self, dir: &Path, name: &str) {
        let candidate = next_numbered_name(name, 1);
        // Block briefly to look up the candidate. Acceptable: small
        // sqlite reads, called once per probe completion.
        let s = self.client.clone();
        let original = name.to_string();
        let chosen = self.block_on(async move {
            for n in 1..=99u32 {
                let cand = next_numbered_name(&original, n);
                if s.find_job_by_filename(cand.clone())
                    .await
                    .ok()
                    .flatten()
                    .is_none()
                {
                    return cand;
                }
            }
            candidate
        });
        self.form.info = Some(format!(
            "A download named `{name}` already exists. Renamed to `{chosen}` to avoid the conflict."
        ));
        self.form.save_path = dir.join(chosen).to_string_lossy().into_owned();
    }

    fn try_submit(&mut self, ctx: &egui::Context, start_now: bool) {
        let parsed = match parse_form(&self.form) {
            Ok(p) => p,
            Err(e) => {
                self.form.error = Some(e);
                return;
            }
        };
        // Dup query against the chosen filename — sqlite, single hop.
        if let Some(name) = &parsed.filename {
            let s = self.client.clone();
            let owned = name.clone();
            let edit_id = self.edit_id;
            let res = self.block_on(async move { s.find_job_by_filename(owned).await });
            let dup = match res {
                Ok(Some(id)) if Some(id) != edit_id => Some(id),
                _ => None,
            };
            if let Some(other) = dup {
                self.overwrite_confirm = Some(other);
                self.pending_action = Some(start_now);
                return;
            }
        }
        self.commit(ctx, parsed, start_now);
    }

    fn commit(&mut self, ctx: &egui::Context, p: ParsedForm, start_now: bool) {
        let s = self.client.clone();
        let queue = self.form.queue;
        if let Some(id) = self.edit_id {
            // Edit mode: update URL + final destination. Per-job
            // working dir lives under `Settings::work_dir` keyed by
            // job id, so existing `.part` data is preserved across
            // edits. URL change is forwarded to odl on the next
            // start; any size / Last-Modified mismatch surfaces via
            // the conflict resolver.
            let edit = JobEdit {
                url: p.url.clone(),
                save_dir: p.save_dir.clone(),
                filename: p.filename.clone(),
                referrer: p.referrer.clone(),
                headers: p.headers.clone(),
                max_connections: p.max_connections,
                proxy: p.proxy.clone(),
                auth_user: p.auth_user.clone(),
                auth_password: p.auth_password.clone(),
                proxy_password: p.proxy_password.clone(),
                cookies: p.cookies.clone(),
            };
            let res = self.block_on(async move { s.update_job_location(id, edit).await });
            if let Err(e) = res {
                self.form.error = Some(e);
                return;
            }
            let s = self.client.clone();
            self.block_on(async move {
                if let Some(qid) = queue {
                    let _ = s.set_job_queue(id, qid).await;
                }
                if start_now {
                    let _ = s.start_job(id).await;
                    let _ = s.open_download_window(id).await;
                }
            });
        } else {
            let req = AddJobReq {
                url: p.url,
                save_dir: p.save_dir,
                filename: p.filename,
                referrer: p.referrer,
                headers: p.headers,
                max_connections: p.max_connections,
                proxy: p.proxy,
                auth_user: p.auth_user,
                auth_password: p.auth_password,
                proxy_password: p.proxy_password,
                cookies: p.cookies,
                category: self.form.category_override,
            };
            self.block_on(async move {
                match s.add_job(req).await {
                    Ok(id) => {
                        if let Some(qid) = queue {
                            let _ = s.set_job_queue(id, qid).await;
                        }
                        if start_now {
                            let _ = s.start_job(id).await;
                            let _ = s.open_download_window(id).await;
                        }
                    }
                    Err(m) => tracing::warn!(error = %m, "add_job failed"),
                }
            });
        }
        let _ = ctx;
        self.want_close = true;
    }

    fn overwrite_overlay(&mut self, ctx: &egui::Context, t: &theme::Tokens, other_id: JobId) {
        let label = self
            .block_on(self.client.job_entry(other_id))
            .ok()
            .flatten()
            .and_then(|v| v.job.filename)
            .unwrap_or_else(|| "this download".into());
        let mut decision: Option<bool> = None;
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
            egui::Id::new("add-overwrite-confirm"),
            modal_frame,
            |ui| {
                ui.set_max_width(420.0);
                ui.spacing_mut().item_spacing.y = space::S2 as f32;
                ui.horizontal(|ui| {
                    icons::show(ui, "triangle-alert", 22.0, t.status_warning);
                    ui.add_space(space::S1 as f32);
                    ui.label(
                        RichText::new("Overwrite existing download?")
                            .font(theme::body_bold(14.0))
                            .color(t.fg_1),
                    );
                });
                ui.add(egui::Label::new(
                    RichText::new(format!(
                        "`{label}` is already in the list. Continuing will delete the existing entry along with any partial or completed file on disk and replace it with this new download."
                    )).color(t.fg_2).font(theme::body(12.0)),
                ).wrap());
                ui.add_space(space::S1 as f32);
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if Btn::new("Overwrite")
                            .danger_filled()
                            .icon("trash-2")
                            .show(ui)
                            .clicked()
                        {
                            decision = Some(true);
                        }
                        if Btn::new("Cancel").ghost().show(ui).clicked() {
                            decision = Some(false);
                        }
                    });
                });
            },
        );
        match decision {
            Some(true) => {
                let s = self.client.clone();
                let _ = self.block_on(async move {
                    s.remove(
                        other_id,
                        crate::data::RemoveOpts {
                            purge_partial: true,
                            delete_final_file: true,
                        },
                    )
                    .await
                });
                self.overwrite_confirm = None;
                let start_now = self.pending_action.take().unwrap_or(false);
                if let Ok(p) = parse_form(&self.form) {
                    self.commit(ctx, p, start_now);
                }
            }
            Some(false) => {
                self.overwrite_confirm = None;
                self.pending_action = None;
            }
            None => {}
        }
    }
}

// ---- helpers ----------------------------------------------------------

struct ParsedForm {
    url: url::Url,
    save_dir: PathBuf,
    filename: Option<String>,
    referrer: Option<url::Url>,
    max_connections: Option<u64>,
    headers: IndexMap<String, String>,
    /// Proxy URL **without** an embedded password (the password lives in
    /// the OS keyring, set separately via `proxy_password`).
    proxy: Option<String>,
    auth_user: Option<String>,
    /// Plaintext password at submit. `None` when the user left the
    /// field blank — daemon clears the encrypted column.
    auth_password: Option<String>,
    proxy_password: Option<String>,
    cookies: Option<String>,
}

fn parse_form(form: &AddForm) -> Result<ParsedForm, String> {
    let url = url::Url::parse(form.url.trim()).map_err(|e| format!("invalid URL: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("only http(s) URLs supported".into());
    }
    let raw = form.save_path.trim();
    if raw.is_empty() {
        return Err("save destination is required".into());
    }
    let path = Path::new(raw);
    let (save_dir, filename) = match path.file_name() {
        Some(name) => {
            let parent = path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            let n = name.to_string_lossy().to_string();
            // Treat a trailing slash / no filename as directory-only.
            if raw.ends_with(std::path::MAIN_SEPARATOR) || n.is_empty() {
                (path.to_path_buf(), None)
            } else {
                (parent, Some(n))
            }
        }
        None => (path.to_path_buf(), None),
    };
    let referrer = match form.referrer.trim() {
        "" => None,
        s => Some(url::Url::parse(s).map_err(|e| format!("invalid referrer: {e}"))?),
    };
    let max_connections = Some(form.segments as u64);
    if form.auth_user.contains(':') {
        return Err("auth username must not contain ':'".into());
    }
    let headers = collect_headers(form);
    let proxy = build_proxy_url(form)?;
    let auth_user = match form.auth_user.as_str() {
        "" => None,
        u => Some(u.to_string()),
    };
    let auth_password = if form.auth_pass.is_empty() {
        None
    } else {
        Some(form.auth_pass.clone())
    };
    let proxy_password = if form.proxy_pass.is_empty() {
        None
    } else {
        Some(form.proxy_pass.clone())
    };
    let cookies = if form.cookies.is_empty() {
        None
    } else {
        Some(form.cookies.clone())
    };
    Ok(ParsedForm {
        url,
        save_dir,
        filename,
        referrer,
        max_connections,
        headers,
        proxy,
        auth_user,
        auth_password,
        proxy_password,
        cookies,
    })
}

/// Build the persistable proxy URL — `scheme://[user@]host:port`. The
/// password is **never** included here; it lives in the OS keyring and
/// the runner merges it back in at job-start time.
fn build_proxy_url(form: &AddForm) -> Result<Option<String>, String> {
    let scheme = match form.proxy_kind {
        ProxyKind::None => return Ok(None),
        ProxyKind::Http => "http",
        ProxyKind::Socks5 => "socks5",
    };
    let host_port = form.proxy_host_port.trim();
    if host_port.is_empty() {
        return Err("proxy host:port required".into());
    }
    let mut url = url::Url::parse(&format!("{scheme}://{host_port}"))
        .map_err(|e| format!("invalid proxy host:port: {e}"))?;
    if url.host_str().map(str::is_empty).unwrap_or(true) {
        return Err("proxy host:port required".into());
    }
    let user = form.proxy_user.as_str();
    if !user.is_empty() {
        url.set_username(user)
            .map_err(|_| "invalid proxy username".to_string())?;
    }
    Ok(Some(url.into()))
}

fn parse_proxy_url(s: &str) -> Option<(ProxyKind, String, String, String)> {
    use percent_encoding::percent_decode_str;
    let u = url::Url::parse(s).ok()?;
    let kind = match u.scheme() {
        "http" => ProxyKind::Http,
        "socks5" => ProxyKind::Socks5,
        _ => return None,
    };
    let host = u.host_str()?;
    let host_port = match u.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_string(),
    };
    let user = percent_decode_str(u.username())
        .decode_utf8_lossy()
        .into_owned();
    let pass = u
        .password()
        .map(|p| percent_decode_str(p).decode_utf8_lossy().into_owned())
        .unwrap_or_default();
    Some((kind, host_port, user, pass))
}

fn collect_headers(form: &AddForm) -> IndexMap<String, String> {
    let mut out = IndexMap::new();
    for (k, v) in &form.headers {
        let k = k.trim();
        let v = v.trim();
        if k.is_empty() {
            continue;
        }
        out.insert(k.to_string(), v.to_string());
    }
    let ua = form.user_agent.trim();
    if !ua.is_empty() {
        out.insert("User-Agent".into(), ua.to_string());
    }
    // Cookies live in the encrypted-secret column, not in `headers`.
    // Basic auth no longer rides as an `Authorization` header — the
    // username sits on `Job.auth_user` and the password lives in the
    // OS keyring. Any captured `Authorization: Bearer …` already in
    // `form.headers` flows through as a regular header.
    out
}

fn suggest_segments(size: Option<u64>) -> u32 {
    const MB: u64 = 1024 * 1024;
    match size {
        Some(s) if s < MB => 1,
        Some(s) if s < 10 * MB => 4,
        _ => 8,
    }
}

fn current_dir(path: &str) -> Option<PathBuf> {
    let p = Path::new(path.trim());
    if p.as_os_str().is_empty() {
        return None;
    }
    if path.trim().ends_with(std::path::MAIN_SEPARATOR) || p.file_name().is_none() {
        return Some(p.to_path_buf());
    }
    p.parent().map(Path::to_path_buf)
}

fn current_filename(path: &str) -> Option<String> {
    let trimmed = path.trim();
    if trimmed.ends_with(std::path::MAIN_SEPARATOR) {
        return None;
    }
    Path::new(trimmed)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
}

fn next_numbered_name(name: &str, n: u32) -> String {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, e),
        _ => (name, ""),
    };
    if ext.is_empty() {
        format!("{stem} ({n})")
    } else {
        format!("{stem} ({n}).{ext}")
    }
}

/// Dashed stroke around a rounded rect.
///
/// Builds the perimeter as a closed polyline (straight edges + sampled
/// corner arcs), then walks arc-length emitting `dash` / `gap` runs.
/// Mirrors SVG `stroke-dasharray` semantics so corners curve with the
/// dashes instead of getting cut off.
fn paint_dashed_rect(painter: &egui::Painter, rect: Rect, radius: f32, stroke: Stroke) {
    use std::f32::consts::PI;
    const ARC_STEPS: usize = 24;
    let dash = 3.0_f32;
    let gap = 3.0_f32;
    // Inset by half-stroke so dashes sit inside the frame outline
    // instead of straddling it — keeps corners crisp at pixel
    // boundaries instead of AA-blurry.
    let inset = stroke.width * 0.5;
    let rect = rect.shrink(inset);
    let r = (radius - inset)
        .min(rect.width() * 0.5)
        .min(rect.height() * 0.5)
        .max(0.0);

    let mut path: Vec<Pos2> = Vec::with_capacity(4 + 4 * (ARC_STEPS + 1));
    let arc = |center: Pos2, start: f32, end: f32, out: &mut Vec<Pos2>| {
        for i in 0..=ARC_STEPS {
            let t = i as f32 / ARC_STEPS as f32;
            let a = start + (end - start) * t;
            out.push(Pos2::new(center.x + r * a.cos(), center.y + r * a.sin()));
        }
    };
    path.push(Pos2::new(rect.left() + r, rect.top()));
    path.push(Pos2::new(rect.right() - r, rect.top()));
    arc(
        Pos2::new(rect.right() - r, rect.top() + r),
        -PI * 0.5,
        0.0,
        &mut path,
    );
    path.push(Pos2::new(rect.right(), rect.bottom() - r));
    arc(
        Pos2::new(rect.right() - r, rect.bottom() - r),
        0.0,
        PI * 0.5,
        &mut path,
    );
    path.push(Pos2::new(rect.left() + r, rect.bottom()));
    arc(
        Pos2::new(rect.left() + r, rect.bottom() - r),
        PI * 0.5,
        PI,
        &mut path,
    );
    path.push(Pos2::new(rect.left(), rect.top() + r));
    arc(
        Pos2::new(rect.left() + r, rect.top() + r),
        PI,
        PI * 1.5,
        &mut path,
    );
    if let Some(&first) = path.first() {
        path.push(first);
    }

    // Walk perimeter accumulating one polyline per dash so each dash
    // renders as a single anti-aliased path (instead of N stacked
    // segments per arc step, which produces blurry overlaps).
    let mut remaining = dash;
    let mut drawing = true;
    let mut buf: Vec<Pos2> = Vec::new();
    if drawing {
        buf.push(path[0]);
    }
    let flush = |buf: &mut Vec<Pos2>| {
        if buf.len() >= 2 {
            painter.add(egui::Shape::line(std::mem::take(buf), stroke));
        } else {
            buf.clear();
        }
    };
    for w in path.windows(2) {
        let mut a = w[0];
        let b = w[1];
        let mut seg_len = (b - a).length();
        if seg_len <= f32::EPSILON {
            continue;
        }
        let dir = (b - a) / seg_len;
        while seg_len > 0.0 {
            let take = seg_len.min(remaining);
            let end = a + dir * take;
            if drawing {
                buf.push(end);
            }
            a = end;
            seg_len -= take;
            remaining -= take;
            if remaining <= f32::EPSILON {
                if drawing {
                    flush(&mut buf);
                }
                drawing = !drawing;
                remaining = if drawing { dash } else { gap };
                if drawing {
                    buf.push(a);
                }
            }
        }
    }
    if drawing {
        flush(&mut buf);
    }
}

fn stat_column(
    ui: &mut egui::Ui,
    t: &theme::Tokens,
    label: &str,
    value: &str,
    value_color: Color32,
) {
    let label_w = ui.fonts_mut(|f| {
        f.layout_no_wrap(label.to_string(), theme::body_medium(9.0), t.fg_3)
            .size()
            .x
    }) + 0.76 * label.chars().count() as f32;
    let value_w = ui.fonts_mut(|f| {
        f.layout_no_wrap(value.to_string(), theme::mono(12.0), value_color)
            .size()
            .x
    });
    let col_w = label_w.max(value_w).ceil();
    ui.allocate_ui_with_layout(Vec2::new(col_w, 0.0), Layout::top_down(Align::Max), |ui| {
        ui.spacing_mut().interact_size.y = 0.0;
        ui.spacing_mut().item_spacing.y = space::S0 as f32;
        let mut lbl = egui::text::LayoutJob::default();
        lbl.append(
            label,
            0.0,
            egui::TextFormat {
                font_id: theme::body_medium(9.0),
                color: t.fg_3,
                extra_letter_spacing: 0.76,
                ..Default::default()
            },
        );
        ui.label(lbl);
        ui.label(
            RichText::new(value)
                .font(theme::mono(12.0))
                .color(value_color),
        );
    });
}

fn warning_banner(ui: &mut egui::Ui, t: &theme::Tokens, title: &str, body: egui::text::LayoutJob) {
    egui::Frame::NONE
        .fill(t.status_warning_bg)
        .stroke(Stroke::new(t.border_width, t.status_warning))
        .corner_radius(theme::surface::RADIUS)
        .inner_margin(space::S3 as f32)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal_top(|ui| {
                // Circular ochre-100 bg with ochre-500 icon.
                let dot = 24.0;
                let icon = 18.0;
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(dot), egui::Sense::hover());
                ui.painter()
                    .circle_filled(rect.center(), dot * 0.5, crate::ui::color::ochre::O100);
                let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(icon));
                icons::icon(
                    ui.ctx(),
                    "triangle-alert",
                    icon,
                    crate::ui::color::ochre::O500,
                )
                .paint_at(ui, icon_rect);
                ui.add_space(space::S2 as f32);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = space::S1 as f32;
                    ui.add(
                        egui::Label::new(
                            RichText::new(title)
                                .color(t.fg_1)
                                .font(theme::body_bold(12.0)),
                        )
                        .wrap(),
                    );
                    ui.add(egui::Label::new(body).wrap());
                });
            });
        });
}

/// Build the non-resumable warning body with the segment count rendered
/// bold via a mixed-weight `LayoutJob`. Subtitle: 400 11.5px / 1.5
/// Jakarta, `fg_2`; the `{n}` substring uses 600 weight.
fn build_non_resumable_body(t: &theme::Tokens, segments: u32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let line_height = Some(11.0 * 1.5);
    let regular = egui::TextFormat {
        font_id: theme::body(11.0),
        color: t.fg_2,
        line_height,
        ..Default::default()
    };
    let bold = egui::TextFormat {
        font_id: theme::body_bold(11.0),
        color: t.fg_2,
        line_height,
        ..Default::default()
    };
    job.append(
        "If your connection drops or you pause, the download restarts from the beginning. Parallel connections are unavailable — segments are locked to ",
        0.0,
        regular.clone(),
    );
    job.append(&segments.to_string(), 0.0, bold);
    job.append(".", 0.0, regular);
    job
}

fn warning_banner_locked_segments(ui: &mut egui::Ui, t: &theme::Tokens, segments: u32) {
    let body = build_non_resumable_body(t, segments);
    warning_banner(ui, t, "This server doesn't support resuming.", body);
}

fn info_banner(ui: &mut egui::Ui, t: &theme::Tokens, msg: &str) {
    egui::Frame::NONE
        .fill(soft_tint(t.status_info, t.bg_surface, 0.18))
        .stroke(Stroke::new(t.border_width, t.status_info))
        .corner_radius(theme::surface::RADIUS)
        .inner_margin(space::S3 as f32)
        .show(ui, |ui| {
            // Fill the full available width without forcing a height.
            ui.set_min_width(ui.available_width());
            // Centre the icon + label group horizontally; vertical centering
            // is implicit because the row hugs its content height.
            ui.with_layout(Layout::top_down(Align::Center), |ui| {
                ui.horizontal(|ui| {
                    icons::show(ui, "info", 24.0, t.status_info);
                    ui.add_space(space::S1 as f32);
                    ui.add(
                        egui::Label::new(RichText::new(msg).color(t.fg_1).font(theme::body(12.0)))
                            .wrap(),
                    );
                });
            });
        });
}

fn cat_visual(cat: Category, t: &theme::Tokens) -> (Color32, &'static str, &'static str) {
    match cat {
        Category::Compressed => (t.cat_compressed, "archive", "Compressed"),
        Category::Programs => (t.cat_programs, "package", "Programs"),
        Category::Videos => (t.cat_videos, "film", "Videos"),
        Category::Music => (t.cat_music, "music", "Music"),
        Category::Pictures => (t.cat_pictures, "image", "Pictures"),
        Category::Documents => (t.cat_documents, "file-text", "Documents"),
        Category::Other => (t.fg_3, "file", "Other"),
    }
}

fn soft_tint(accent: Color32, base: Color32, t: f32) -> Color32 {
    let lerp = |a: u8, b: u8| (a as f32 * (1.0 - t) + b as f32 * t) as u8;
    Color32::from_rgb(
        lerp(base.r(), accent.r()),
        lerp(base.g(), accent.g()),
        lerp(base.b(), accent.b()),
    )
}
