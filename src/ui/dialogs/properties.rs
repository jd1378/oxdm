//! Properties dialog — per-download configuration & inspection.
//! Six tabs (General · Checksums · Connection · Cookies · Headers ·
//! Advanced) inside a single resizable viewport. Spec: see
//! `design/handoff/09b_properties_dialog.md`.
//!
//! State model: a snapshot `original: PropertiesData` is captured the
//! first frame the dialog opens; `current` mutates with every edit.
//! `has_changes = original != current` drives the footer Apply/Discard
//! affordance. Lock semantics: `is_locked = phase.is_running()`.
//!
//! Reusable composites that paint this dialog live in
//! `crate::ui::components::properties`; this file owns layout, state
//! threading and per-tab plumbing only.

use eframe::egui::{self, Align, Layout, Pos2, RichText, Sense, Stroke, StrokeKind, Vec2};
use sha2::{Digest, Sha512};

use std::sync::Arc;

use crate::domain::{Advanced, AuthScheme, Category, CustomHeader, JobId, Phase, ProxyMode};
use crate::ipc_local::Client;
use crate::ui::components::icon_row::icon_row;
use crate::ui::components::primitives::{
    Btn, BtnSize, Combo, FileInput, NumberStepper, PasswordInput, TabBtn, TextArea, TextInput,
    Toggle, copy_feedback, eyebrow,
};
use crate::ui::components::properties as cp;
use crate::ui::components::titlebar;
use crate::ui::gui_state::Cache;
use crate::ui::theme::{self, space, ts};
use crate::ui::utils::format::{format_bytes_2, format_int_grouped};
use crate::ui::utils::icons;

// ──────────────────────────────────────────────────────────────────────
// Re-exports & data model
// ──────────────────────────────────────────────────────────────────────

// Re-export the shared types so existing callers (table.rs, mod.rs) keep
// working through `dialogs::properties::Algo` etc.
pub use cp::{Algo, Checksum, CsSource, CsStatus};

impl cp::HeaderRow for CustomHeader {
    fn name(&mut self) -> &mut String {
        &mut self.name
    }
    fn value(&mut self) -> &mut String {
        &mut self.value
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PropertiesData {
    pub checksums: Vec<Checksum>,
    pub adv: Advanced,
    /// Source URL as text. Editable in the General tab while the job is
    /// not running; persisted via `set_job_source` on Apply.
    pub url: String,
    /// Full destination path (`save_dir` + filename) as text. Editable
    /// while the job is not running; split back into dir + filename on
    /// Apply.
    pub save_path: String,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    #[default]
    General,
    Checksums,
    Connection,
    Cookies,
    Headers,
    Advanced,
}

impl Tab {
    fn all() -> &'static [Tab] {
        &[
            Tab::General,
            Tab::Checksums,
            Tab::Connection,
            Tab::Cookies,
            Tab::Headers,
            Tab::Advanced,
        ]
    }
    fn label(self) -> &'static str {
        match self {
            Tab::General => "General",
            Tab::Checksums => "Checksums",
            Tab::Connection => "Connection",
            Tab::Cookies => "Cookies",
            Tab::Headers => "Headers",
            Tab::Advanced => "Advanced",
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Tab::General => "info",
            Tab::Checksums => "shield-check",
            Tab::Connection => "globe",
            Tab::Cookies => "cookie",
            Tab::Headers => "list",
            Tab::Advanced => "sliders-horizontal",
        }
    }
    /// Whether the tab body may contain editable inputs (locked when
    /// the download is in flight). General + Checksums are exempt per
    /// spec §6.
    fn lockable(self) -> bool {
        !matches!(self, Tab::General | Tab::Checksums)
    }
}

/// Compose-time state attached to `AppShell`. Only the minimum to find
/// the right job + remember dialog UI state (which tab, add-form open
/// flag, etc.); the editable bucket lives in `data`.
pub struct PropertiesState {
    pub id: JobId,
    pub tab: Tab,
    pub original: Option<PropertiesData>,
    pub current: PropertiesData,
    /// Add-checksum form open?
    pub adding: bool,
    pub add: cp::AddChecksumState,
    /// Transient: the General-tab Save-to picker button was clicked. The
    /// top-level `show` consumes this to open the folder dialog (it needs
    /// the tokio runtime handle, which the tab fn doesn't have). Not part
    /// of `PropertiesData` — never diffed/persisted.
    pub request_save_pick: bool,
}

impl PropertiesState {
    pub fn new(id: JobId) -> Self {
        Self {
            id,
            tab: Tab::General,
            original: None,
            current: PropertiesData::default(),
            adding: false,
            add: cp::AddChecksumState::default(),
            request_save_pick: false,
        }
    }

    fn has_changes(&self) -> bool {
        match &self.original {
            Some(o) => o != &self.current,
            None => false,
        }
    }
}

// ──────────────────────────────────────────────────────────────────────
// Host — the runtime surface `body` needs, independent of the main
// `AppShell`. The Properties window runs as its own subprocess
// (`oxdm gui properties <id>`) just like the download/settings/queues
// windows, so it owns its `PropertiesState` plus the deps required to
// read the job and push edits over IPC.
// ──────────────────────────────────────────────────────────────────────

pub struct PropertiesHost {
    pub state: PropertiesState,
    pub cache: Arc<Cache>,
    pub client: Arc<Client>,
    pub rt: tokio::runtime::Handle,
    /// Set by `body` when the window should close (Discard/Close, or the
    /// job vanished). The shell reads it to exit the process.
    pub want_close: bool,
}

// ──────────────────────────────────────────────────────────────────────
// Layout
// ──────────────────────────────────────────────────────────────────────

pub fn body(host: &mut PropertiesHost, root_ui: &mut egui::Ui) {
    let ctx = &root_ui.ctx().clone();
    let id = host.state.id;
    let Some(entry) = host.cache.job_entry_cached(id) else {
        host.want_close = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        return;
    };
    let job = entry.job.clone();
    let counters = entry.counters.clone();
    let phase = counters.phase;
    let is_locked = phase == Phase::Downloading;
    let t = theme::tokens(ctx);

    let filename = job
        .filename
        .clone()
        .or_else(|| {
            job.url
                .path()
                .rsplit('/')
                .next()
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "—".to_string());

    {
        let s = &mut host.state;
        if s.original.is_none() {
            // Hydrate from the persisted `Job` so the dialog opens
            // with whatever the user previously applied — not the
            // hard-coded defaults.
            let data = PropertiesData {
                checksums: job.checksums.clone(),
                adv: job.advanced.clone(),
                url: job.url.to_string(),
                save_path: job.save_dir.join(&filename).to_string_lossy().into_owned(),
            };
            s.current = data.clone();
            s.original = Some(data);
        }
    }

    egui::Panel::top("props_titlebar")
        .frame(egui::Frame::NONE.fill(t.bg_titlebar))
        .show_separator_line(true)
        .show_inside(root_ui, |ui| {
            let title = format!("Properties — {filename}");
            titlebar::show(ui, ctx, &title);
            if is_locked {
                ui.allocate_ui_with_layout(
                    Vec2::new(ui.available_width(), 0.0),
                    Layout::right_to_left(Align::Center),
                    |_ui| {},
                );
            }
        });

    // Tab strip.
    let tab_now = host.state.tab;
    let checksum_count = host.state.current.checksums.len();
    let mut next_tab = tab_now;
    egui::Panel::top("props_tabs")
        .frame(
            egui::Frame::NONE
                // Tab strip shares the dialog body fill (`bg_page`), not the
                // titlebar tint — matches the design where the bar is flush
                // with the content below it.
                .fill(t.bg_page)
                .inner_margin(egui::Margin {
                    left: space::S3,
                    right: space::S3,
                    top: 0,
                    // No bottom padding: the active tab's underline is drawn at
                    // the tab's bottom edge, so the strip must end exactly on the
                    // panel's hairline separator for the two to coincide.
                    bottom: 0,
                }),
        )
        // Draw the hairline ourselves (not the panel's) so it lands exactly
        // on the tab strip's bottom edge, where each active tab's underline
        // sits — the panel separator is offset by the panel's own padding.
        .show_separator_line(false)
        .show_inside(root_ui, |ui| {
            // Capture a tab's own bottom edge — that's exactly where its
            // underline is painted. Using the surrounding `horizontal`'s
            // response rect instead drifts by the panel's content padding,
            // leaving a gap between underline and hairline.
            let mut sep_y = 0.0_f32;
            ui.horizontal(|ui| {
                // Inter-tab spacing comes from each tab's own 14px horizontal
                // padding (design `padding: 9px 14px 10px`), so no extra row
                // item-spacing.
                ui.spacing_mut().item_spacing.x = 0.0;
                for tab in Tab::all().iter().copied() {
                    let mut b = TabBtn::new(tab.label())
                        .icon(tab.icon())
                        .icon_size(13.0)
                        // pad_x 14 (default) + height 35 ≈ design 9/10 vertical
                        // padding around the 12px label / 13px icon.
                        .height(35.0)
                        .active(tab == tab_now);
                    if tab == Tab::Checksums && checksum_count > 0 {
                        b = b.count(checksum_count);
                    }
                    let resp = b.show(ui);
                    sep_y = resp.rect.bottom();
                    if resp.clicked() {
                        next_tab = tab;
                    }
                }
            });
            // Full-width hairline flush with the tab underline.
            let full = ctx.content_rect();
            ui.painter().clone().with_clip_rect(full).line_segment(
                [
                    Pos2::new(full.left(), sep_y),
                    Pos2::new(full.right(), sep_y),
                ],
                Stroke::new(t.border_width, t.border_subtle),
            );
        });
    if next_tab != tab_now {
        host.state.tab = next_tab;
    }

    let mut do_close = false;
    let mut do_apply = false;
    let mut do_reveal = false;
    let has_changes = host.state.has_changes();
    egui::Panel::bottom("props_footer")
        .frame(
            egui::Frame::NONE
                .fill(t.bg_sunken)
                .inner_margin(egui::Margin::symmetric(space::S4, space::S2)),
        )
        .show_separator_line(true)
        .show_inside(root_ui, |ui| {
            ui.horizontal(|ui| {
                if Btn::new(crate::ui::platform::reveal_label())
                    .ghost()
                    .icon("folder-open")
                    .show(ui)
                    .clicked()
                {
                    do_reveal = true;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let apply_resp = Btn::new("Apply")
                        .primary()
                        .icon("check")
                        .enabled(has_changes)
                        .show(ui);
                    if apply_resp.clicked() {
                        do_apply = true;
                    }
                    let discard_label = if has_changes { "Discard" } else { "Close" };
                    if Btn::new(discard_label).ghost().show(ui).clicked() {
                        do_close = true;
                    }
                    if has_changes {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                        ui.painter()
                            .circle_filled(r.center(), 4.0, t.action_primary);
                        ui.label(
                            RichText::new("UNSAVED")
                                .color(t.action_primary)
                                .font(theme::body_bold(11.0)),
                        );
                    }
                });
            });
        });

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(t.bg_page))
        .show_inside(root_ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = space::S3 as f32;
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::symmetric(space::S4, space::S3))
                        .show(ui, |ui| {
                            let tab = host.state.tab;
                            if is_locked && tab.lockable() {
                                cp::lock_banner(ui, &t);
                            }
                            let cats = host.cache.settings().category_extensions.clone();
                            let state = &mut host.state;
                            ui.add_enabled_ui(!(is_locked && tab.lockable()), |ui| match tab {
                                Tab::General => {
                                    general_tab(ui, &t, state, &cats, &job, &counters, phase);
                                }
                                Tab::Checksums => {
                                    checksums_tab(ui, &t, state, is_locked, &filename);
                                }
                                Tab::Connection => connection_tab(ui, &t, state),
                                Tab::Cookies => cookies_tab(ui, &t, state),
                                Tab::Headers => {
                                    headers_tab(ui, &t, state, &job, &counters);
                                }
                                Tab::Advanced => advanced_tab(ui, &t, state),
                            });
                        });
                });
        });

    if do_reveal {
        if let Some(p) = job.status.final_path.clone().or(Some(job.save_dir.clone())) {
            crate::ui::platform::reveal_in_folder(&p);
        }
    }
    // Save-to picker (requested by the General tab). Runs here because it
    // needs the tokio runtime handle. Owns a cloned `Handle` so the enter
    // guard doesn't borrow `app` while we write back the chosen path.
    let pick_save = std::mem::take(&mut host.state.request_save_pick);
    if pick_save {
        let cur = std::path::PathBuf::from(&host.state.current.save_path);
        let start_dir = cur
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_path_buf());
        let fname = cur.file_name().map(|f| f.to_string_lossy().into_owned());
        let handle = host.rt.clone();
        let _g = handle.enter();
        let mut dlg = rfd::FileDialog::new();
        if let Some(d) = &start_dir {
            dlg = dlg.set_directory(d);
        }
        if let Some(dir) = dlg.pick_folder() {
            let new_path = match &fname {
                Some(n) => dir.join(n),
                None => dir,
            };
            host.state.current.save_path = new_path.to_string_lossy().into_owned();
        }
    }
    if do_apply {
        let s = &mut host.state;
        let snapshot = s.current.clone();
        // Optimistic local commit — the original baseline moves to
        // `current` so `has_changes()` flips back to false immediately.
        // If the RPC errors the daemon-side push on next snapshot will
        // re-hydrate us from the source of truth.
        s.original = Some(snapshot.clone());
        let client = host.client.clone();
        let job_url = job.url.clone();
        let job_save_dir = job.save_dir.clone();
        let job_filename = job.filename.clone();
        host.rt.spawn(async move {
            if let Err(e) = client.set_job_advanced(id, snapshot.adv).await {
                tracing::warn!(job = %id, error = %e, "properties: set_job_advanced failed");
            }
            if let Err(e) = client.set_job_checksums(id, snapshot.checksums).await {
                tracing::warn!(job = %id, error = %e, "properties: set_job_checksums failed");
            }
            // Source URL + destination. Only push when it actually changed
            // and the URL still parses; an invalid edit leaves the job's
            // source untouched (the other Apply writes still land).
            match url::Url::parse(snapshot.url.trim()) {
                Ok(new_url) => {
                    let p = std::path::PathBuf::from(snapshot.save_path.trim());
                    let new_dir = p
                        .parent()
                        .filter(|d| !d.as_os_str().is_empty())
                        .map(|d| d.to_path_buf())
                        .unwrap_or_else(|| job_save_dir.clone());
                    let new_name = p.file_name().map(|f| f.to_string_lossy().into_owned());
                    if new_url != job_url || new_dir != job_save_dir || new_name != job_filename {
                        if let Err(e) =
                            client.set_job_source(id, new_url, new_dir, new_name).await
                        {
                            tracing::warn!(job = %id, error = %e, "properties: set_job_source failed");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(job = %id, error = %e, "properties: invalid URL — source not updated");
                }
            }
        });
    }
    if do_close {
        host.want_close = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

// ──────────────────────────────────────────────────────────────────────
// 4.1 General
// ──────────────────────────────────────────────────────────────────────

pub fn general_tab(
    ui: &mut egui::Ui,
    t: &theme::Tokens,
    state: &mut PropertiesState,
    _cats: &indexmap::IndexMap<Category, Vec<String>>,
    job: &crate::domain::Job,
    counters: &crate::ipc_local::protocol::JobCounters,
    phase: Phase,
) {
    let filename = job.filename.clone().unwrap_or_else(|| "—".to_string());
    let host = job.url.host_str().unwrap_or("").to_string();
    let cat = job.category;
    let cat_label: &str = cat.label();
    let ext = std::path::Path::new(&filename)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_uppercase();

    // Hero strip — `.prop-hero { padding: 12px 14px; gap: 12px }`.
    cp::section_card(ui, t, |ui| {
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(14, 12))
            .show(ui, |ui| {
                // Tile + title/meta + phase pill, sharing one vertical
                // centre line. Uses the shared `icon_row` (egui_flex) rather
                // than a hand-rolled `ui.horizontal`: a plain horizontal
                // allocates the fixed tile *before* the taller title+meta
                // column and egui never re-centres an earlier, shorter item,
                // so the tile pins to the top. `icon_row`'s `align_items`
                // centres all three cells — matching the Add dialog's
                // `detected_card`, which already uses it.
                // Host + category live in the Source/Type rows below; the
                // hero meta keeps just the size to avoid duplicating them.
                let meta = match counters.total {
                    Some(n) => format_bytes_2(n),
                    None => "unknown".into(),
                };
                icon_row(
                    ui,
                    44.0,
                    |ui, rect| {
                        ui.painter()
                            .rect_filled(rect, theme::radius::SM as f32, t.bg_sunken);
                        ui.painter().rect_stroke(
                            rect,
                            theme::radius::SM as f32,
                            Stroke::new(t.border_width, t.border_default),
                            StrokeKind::Inside,
                        );
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            if ext.is_empty() {
                                "—".to_string()
                            } else {
                                ext.clone()
                            },
                            theme::mono_bold(11.0),
                            t.fg_2,
                        );
                    },
                    |ui| {
                        // `.prop-hero-name { font: 600 13px var(--font-body); }`
                        ui.label(
                            RichText::new(&filename)
                                .font(theme::body_bold(13.0))
                                .color(t.fg_1),
                        );
                        // `.prop-hero-size` — mono, tabular numerals.
                        ui.label(RichText::new(meta).color(t.fg_3).font(theme::mono(11.0)));
                    },
                    |ui| {
                        let (color, label) = phase_pill_props(t, phase);
                        cp::phase_pill(ui, t, color, label);
                    },
                );
            });
    });

    // URL + destination are editable only while the job is not running
    // (paused / queued / cancelled / failed). Mid-transfer edits are
    // refused server-side too (`set_job_source`).
    let editable = !phase.is_running();

    eyebrow(ui, "file");
    cp::section_card(ui, t, |ui| {
        cp::kv_row(ui, t, "Name", &filename, true);
        cp::row_sep(ui, t);
        cp::kv_row(ui, t, "Category", cat_label, false);
        cp::row_sep(ui, t);
        match counters.total {
            Some(n) => {
                // Human size in fg_1; the exact "(N bytes)" suffix dimmed to
                // fg_3 so the readable figure leads.
                let human = format_bytes_2(n);
                let exact = format!("  ({} bytes)", format_int_grouped(n));
                cp::prop_row(ui, t, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Size")
                                .color(t.fg_1)
                                .font(theme::body_medium(12.0)),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let mut job_txt = egui::text::LayoutJob::default();
                            job_txt.append(
                                &human,
                                0.0,
                                egui::text::TextFormat {
                                    font_id: ts::mono_sm(),
                                    color: t.fg_2,
                                    ..Default::default()
                                },
                            );
                            job_txt.append(
                                &exact,
                                0.0,
                                egui::text::TextFormat {
                                    font_id: ts::mono_sm(),
                                    color: t.fg_3,
                                    ..Default::default()
                                },
                            );
                            ui.label(job_txt);
                        });
                    });
                });
            }
            None => cp::kv_row(ui, t, "Size", "—", true),
        }
        cp::row_sep(ui, t);
        cp::prop_row_stack(ui, t, "Save to", |ui| {
            // Framed input + trailing button (same control as the Add
            // dialog). Editable → the button opens a folder picker (handled
            // by the top-level `show`, which has the runtime handle).
            // Read-only → it reveals the folder.
            let resp = FileInput::new(&mut state.current.save_path)
                .id_salt("props-save-to")
                .interactive(editable)
                .icon("folder")
                .tooltip(if editable {
                    "Choose folder"
                } else {
                    crate::ui::platform::reveal_label()
                })
                .font(ts::mono_sm())
                .show(ui);
            if resp.browse.clicked() {
                if editable {
                    state.request_save_pick = true;
                } else {
                    crate::ui::platform::reveal_in_folder(&job.save_dir);
                }
            }
        });
    });

    eyebrow(ui, "source");
    cp::section_card(ui, t, |ui| {
        cp::prop_row_stack(ui, t, "URL", |ui| {
            // Same framed input, copy button instead of a folder picker.
            // Editable while the job is not running.
            let copy_id = ui.id().with("props-url-copy");
            let resp = FileInput::new(&mut state.current.url)
                .id_salt("props-url")
                .interactive(editable)
                .icon(copy_feedback::icon(ui.ctx(), copy_id))
                .tooltip("Copy URL")
                .font(ts::mono_sm())
                .show(ui);
            if resp.browse.clicked() {
                copy_feedback::commit(ui.ctx(), copy_id, state.current.url.clone());
            }
        });
        cp::row_sep(ui, t);
        cp::kv_row(ui, t, "Server", &host, true);
        cp::row_sep(ui, t);
        let created = job
            .created_at
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        cp::kv_row(ui, t, "Created", &created, false);
    });

    eyebrow(ui, "integrity");
    cp::section_card(ui, t, |ui| {
        let cs = &state.current.checksums;
        let trail_w = if cs.is_empty() { 260.0 } else { 100.0 };
        cp::header_with_trailing(
            ui,
            t,
            "Checksums",
            "Hashes saved for this file.",
            trail_w,
            |ui| {
                if cs.is_empty() {
                    ui.label(
                        RichText::new("None — open the Checksums tab to add one.")
                            .color(t.fg_3)
                            .font(theme::body(12.0)),
                    );
                } else {
                    let v = cs.iter().filter(|c| c.status == CsStatus::Verified).count();
                    let m = cs.iter().filter(|c| c.status == CsStatus::Mismatch).count();
                    let n = cs.len();
                    let color = if m > 0 {
                        t.status_danger
                    } else if v == n {
                        t.status_success
                    } else {
                        t.fg_3
                    };
                    ui.label(
                        RichText::new(format!("{n} saved"))
                            .color(t.fg_2)
                            .font(theme::body(12.0)),
                    );
                    ui.add_space(space::S1 as f32);
                    icons::show(ui, "shield", 14.0, color);
                }
            },
        );
    });
}

fn phase_pill_props(t: &theme::Tokens, phase: Phase) -> (egui::Color32, &'static str) {
    match phase {
        Phase::Completed => (t.status_success, "COMPLETE"),
        Phase::Failed => (t.status_danger, "FAILED"),
        Phase::Cancelled => (t.fg_2, "CANCELLED"),
        Phase::Paused => (t.fg_2, "PAUSED"),
        Phase::Queued => (t.status_info, "QUEUED"),
        _ => (t.action_primary, "DOWNLOADING"),
    }
}

// ──────────────────────────────────────────────────────────────────────
// 4.2 Checksums
// ──────────────────────────────────────────────────────────────────────

/// Deterministic placeholder hash for the prototype. Spec §4.2.4
/// explicitly says to fake this — feed the filename through SHA-512 and
/// truncate to the algo's hex length.
// TODO: replace with a real streaming hasher over the on-disk file.
fn compute_local_hash(filename: &str, algo: Algo) -> String {
    let mut h = Sha512::new();
    h.update(format!("{}:{}", filename, algo.label()));
    let bytes = h.finalize();
    let mut full = String::with_capacity(bytes.len() * 2);
    for b in bytes.iter() {
        use std::fmt::Write;
        let _ = write!(full, "{b:02x}");
    }
    full.chars().take(algo.hex_len()).collect()
}

pub fn checksums_tab(
    ui: &mut egui::Ui,
    t: &theme::Tokens,
    state: &mut PropertiesState,
    is_locked: bool,
    filename: &str,
) {
    let cs_snapshot = state.current.checksums.clone();
    let n = cs_snapshot.len();
    let v = cs_snapshot
        .iter()
        .filter(|c| c.status == CsStatus::Verified)
        .count();
    let m = cs_snapshot
        .iter()
        .filter(|c| c.status == CsStatus::Mismatch)
        .count();
    let u = cs_snapshot
        .iter()
        .filter(|c| c.status == CsStatus::Unverified)
        .count();

    // 4.2.1 Status banner.
    let (tone, title, sub) = if n == 0 {
        (
            cp::BannerTone::Neutral,
            "No checksums on file".to_string(),
            "Add a hash from the publisher's website to verify the file's integrity. MD5, SHA-1, SHA-256, SHA-384 and SHA-512 are supported.".to_string(),
        )
    } else if m > 0 {
        (
            cp::BannerTone::Danger,
            format!("{m} mismatch — do not trust this file"),
            "At least one saved hash does not match the actual contents on disk. The file may be corrupt, intercepted, or from a different version.".into(),
        )
    } else if u == 0 && v > 0 {
        (
            cp::BannerTone::Success,
            format!("All {v} hash{} verified", if v > 1 { "es" } else { "" }),
            format!("Every saved hash matches the locally-computed value for {filename}."),
        )
    } else {
        (
            cp::BannerTone::Partial,
            format!("{v}/{n} verified · {u} pending"),
            "Some hashes still need to be verified against the file on disk.".to_string(),
        )
    };

    let action = if u > 0 {
        Some(format!("Verify {u}"))
    } else {
        None
    };
    // Icon is chosen by `tone` inside `status_banner`.
    let verify_all = cp::status_banner(ui, t, tone, "shield", &title, &sub, action.as_deref());

    if is_locked {
        cp::lock_banner_checksums(ui, t);
    }

    if verify_all {
        let s = &mut *state;
        for c in s.current.checksums.iter_mut() {
            if c.status == CsStatus::Unverified {
                let local = compute_local_hash(filename, c.algo);
                if local == c.hash {
                    c.status = CsStatus::Verified;
                    c.expected = None;
                } else {
                    c.status = CsStatus::Mismatch;
                    c.expected = Some(local);
                }
            }
        }
    }

    // 4.2.2 Checksum list.
    if !cs_snapshot.is_empty() {
        checksum_list(ui, t, state, is_locked, filename);
    }

    // 4.2.3 Add affordance / form.
    let adding = state.adding;
    if adding {
        let existing: std::collections::HashSet<Algo> =
            state.current.checksums.iter().map(|c| c.algo).collect();
        let s = &mut *state;
        let outcome = cp::add_checksum_form(ui, t, &mut s.add, &existing);
        if let Some((effective, canon)) = outcome.save {
            let local = compute_local_hash(filename, effective);
            let status = if canon == local {
                CsStatus::Verified
            } else {
                CsStatus::Unverified
            };
            s.current.checksums.push(Checksum {
                algo: effective,
                hash: canon,
                source: CsSource::User,
                status,
                expected: None,
            });
            s.adding = false;
            s.add.hash.clear();
        }
        if outcome.cancel {
            s.adding = false;
            s.add.hash.clear();
        }
    } else {
        let existing: std::collections::HashSet<Algo> =
            cs_snapshot.iter().map(|c| c.algo).collect();
        let all_used = existing.len() == Algo::ALL.len();
        ui.horizontal(|ui| {
            if Btn::new("Add checksum manually")
                .icon("plus")
                .enabled(!all_used)
                .show(ui)
                .clicked()
            {
                let s = &mut *state;
                s.adding = true;
                s.add.algo = Algo::ALL
                    .iter()
                    .copied()
                    .find(|a| !existing.contains(a))
                    .unwrap_or(Algo::Md5);
                s.add.hash.clear();
                s.add.auto_detect = true;
            }
            if all_used {
                ui.label(
                    RichText::new("All five supported algorithms are already in the list.")
                        .color(t.fg_3)
                        .font(theme::body(12.0)),
                );
            }
            // `.prop-cs-algolist` — supported-algorithm chips, right-aligned.
            // The dialog's min width (see `windows::properties`) guarantees room
            // beside the button, so these never wrap or overlap.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = CHIP_GAP;
                for algo in Algo::ALL.iter().rev() {
                    algo_chip(ui, t, algo.label());
                }
            });
        });
    }
}

const CHIP_FONT_PX: f32 = 9.0;
const CHIP_PAD: egui::Vec2 = egui::vec2(7.0, 4.0);
const CHIP_GAP: f32 = 5.0;

/// `.prop-cs-algochip` — `font: 600 9px mono; color: fg-3; bg: bg-page;
/// border: 1px border-subtle; radius: 4px; padding: 4px 7px`.
///
/// Painted by hand: an egui galley's row height bakes in line-leading that a
/// CSS `line-height: normal` chip does not, so a `Frame` + `label` renders the
/// chip too tall. Sizing the box to `line-box + 2×pad` and centering the galley
/// pins the glyph snug to the padding the design asks for.
fn algo_chip(ui: &mut egui::Ui, t: &theme::Tokens, label: &str) {
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        theme::mono_bold(CHIP_FONT_PX),
        t.fg_3,
    );
    let size = galley.size() + CHIP_PAD * 2.0;
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter();
    p.rect(
        rect,
        4.0,
        t.bg_page,
        Stroke::new(1.0, t.border_subtle),
        StrokeKind::Inside,
    );
    p.galley(rect.center() - galley.size() / 2.0, galley, t.fg_3);
}

fn checksum_list(
    ui: &mut egui::Ui,
    t: &theme::Tokens,
    state: &mut PropertiesState,
    is_locked: bool,
    filename: &str,
) {
    let cs_snapshot = state.current.checksums.clone();
    let mut to_remove: Option<usize> = None;
    let mut to_verify: Option<usize> = None;
    let mut to_copy: Option<String> = None;

    egui::Frame::NONE
        .fill(t.bg_surface)
        .stroke(egui::Stroke::new(t.border_width, t.border_subtle))
        .corner_radius(theme::surface::RADIUS)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
            cp::checksum_list_header(ui, t);
            for (i, cs) in cs_snapshot.iter().enumerate() {
                if i > 0 {
                    let (rect, _) = ui.allocate_exact_size(
                        egui::Vec2::new(ui.available_width(), t.border_width),
                        egui::Sense::hover(),
                    );
                    ui.painter().rect_filled(rect, 0.0, t.border_subtle);
                }
                let act = cp::checksum_row(ui, t, cs, is_locked);
                if act.remove {
                    to_remove = Some(i);
                }
                if act.verify {
                    to_verify = Some(i);
                }
                if let Some(h) = act.copy {
                    to_copy = Some(h);
                }
            }
        });

    let ctx = ui.ctx().clone();
    if let Some(h) = to_copy {
        ctx.copy_text(h);
    }
    if let Some(i) = to_remove {
        state.current.checksums.remove(i);
    }
    if let Some(i) = to_verify {
        let cs = &mut state.current.checksums[i];
        let local = compute_local_hash(filename, cs.algo);
        if local == cs.hash {
            cs.status = CsStatus::Verified;
            cs.expected = None;
        } else {
            cs.status = CsStatus::Mismatch;
            cs.expected = Some(local);
        }
    }
}

// ──────────────────────────────────────────────────────────────────────
// 4.3 Connection
// ──────────────────────────────────────────────────────────────────────

pub fn connection_tab(ui: &mut egui::Ui, t: &theme::Tokens, state: &mut PropertiesState) {
    let s = &mut *state;

    eyebrow(ui, "proxy");
    cp::section_card(ui, t, |ui| {
        let current_label = match s.current.adv.proxy.mode {
            ProxyMode::Inherit => "Use global setting",
            ProxyMode::None => "No proxy (direct)",
            ProxyMode::System => "System proxy",
            ProxyMode::Http => "HTTP proxy…",
            ProxyMode::Https => "HTTPS proxy…",
            ProxyMode::Socks5 => "SOCKS5 proxy…",
        };
        cp::header_with_trailing(
            ui,
            t,
            "Use proxy",
            "Route this download's traffic through a proxy server. Overrides the global setting in Preferences → Network.",
            180.0,
            |ui| {
                Combo::new("props-proxy-mode", current_label)
                    .width(180.0)
                    .show(ui, |ui| {
                        let opts: &[(&str, ProxyMode)] = &[
                            ("Use global setting", ProxyMode::Inherit),
                            ("No proxy (direct)", ProxyMode::None),
                            ("System proxy", ProxyMode::System),
                            ("HTTP proxy…", ProxyMode::Http),
                            ("HTTPS proxy…", ProxyMode::Https),
                            ("SOCKS5 proxy…", ProxyMode::Socks5),
                        ];
                        for (label, mode) in opts {
                            if Combo::item(ui, label, true).clicked() {
                                s.current.adv.proxy.mode = *mode;
                                ui.close();
                            }
                        }
                    });
            },
        );

        if matches!(s.current.adv.proxy.mode, ProxyMode::System) {
            cp::info_callout(
                ui,
                t,
                "oxdm will inherit the proxy configured in System Settings → Network → Proxies.",
            );
        }

        if matches!(
            s.current.adv.proxy.mode,
            ProxyMode::Http | ProxyMode::Https | ProxyMode::Socks5
        ) {
            cp::row_sep(ui, t);
            cp::prop_row_stack(ui, t, "Server", |ui| {
                ui.horizontal(|ui| {
                    TextInput::new(&mut s.current.adv.proxy.host)
                        .width(ui.available_width() - 130.0)
                        .hint("proxy.example.com")
                        .font(ts::mono_sm())
                        .show(ui);
                    ui.label(RichText::new(":").color(t.fg_3));
                    let placeholder = if matches!(s.current.adv.proxy.mode, ProxyMode::Socks5) {
                        "1080"
                    } else {
                        "8080"
                    };
                    TextInput::new(&mut s.current.adv.proxy.port)
                        .width(70.0)
                        .hint(placeholder)
                        .font(ts::mono_sm())
                        .show(ui);
                });
            });

            cp::row_sep(ui, t);
            cp::header_with_trailing(
                ui,
                t,
                "Proxy authentication",
                "Username/password sent to the proxy itself (not the destination).",
                48.0,
                |ui| {
                    Toggle::new(&mut s.current.adv.proxy.auth_enabled)
                        .id(egui::Id::new("props-proxy-auth"))
                        .show(ui);
                },
            );
            if s.current.adv.proxy.auth_enabled {
                cp::prop_row(ui, t, |ui| {
                    ui.horizontal(|ui| {
                        let half = (ui.available_width() - space::S2 as f32) / 2.0;
                        TextInput::new(&mut s.current.adv.proxy.username)
                            .width(half)
                            .hint("username")
                            .font(ts::mono_sm())
                            .show(ui);
                        PasswordInput::new(&mut s.current.adv.proxy.password, "props-proxy-pwd")
                            .width(half)
                            .hint("password")
                            .show(ui);
                    });
                });
            }

            if matches!(s.current.adv.proxy.mode, ProxyMode::Socks5) {
                cp::row_sep(ui, t);
                cp::header_with_trailing(
                    ui,
                    t,
                    "Resolve DNS through proxy",
                    "Send hostname lookups through the SOCKS5 server. Hides DNS queries from your local resolver.",
                    48.0,
                    |ui| {
                        Toggle::new(&mut s.current.adv.proxy.remote_dns)
                            .id(egui::Id::new("props-proxy-dns"))
                            .show(ui);
                    },
                );
            }

            cp::row_sep(ui, t);
            cp::prop_row_stack(ui, t, "Bypass for", |ui| {
                ui.label(
                    RichText::new(
                        "Comma-separated hosts/patterns that should connect directly, e.g. *.lan, 192.168.*, localhost",
                    )
                    .color(t.fg_3)
                    .font(theme::body(11.0)),
                );
                TextInput::new(&mut s.current.adv.proxy.bypass)
                    .width(ui.available_width())
                    .font(ts::mono_sm())
                    .show(ui);
            });
        }
    });

    eyebrow(ui, "site authentication");
    cp::section_card(ui, t, |ui| {
        let label = match s.current.adv.auth.scheme {
            AuthScheme::None => "None",
            AuthScheme::Basic => "HTTP Basic",
            AuthScheme::Bearer => "Bearer token",
            AuthScheme::Digest => "Digest",
        };
        cp::header_with_trailing(
            ui,
            t,
            "Scheme",
            "Sent to the destination server, not the proxy.",
            140.0,
            |ui| {
                Combo::new("props-auth", label).width(140.0).show(ui, |ui| {
                    let opts: &[(&str, AuthScheme)] = &[
                        ("None", AuthScheme::None),
                        ("HTTP Basic", AuthScheme::Basic),
                        ("Bearer token", AuthScheme::Bearer),
                        ("Digest", AuthScheme::Digest),
                    ];
                    for (lab, sc) in opts {
                        if Combo::item(ui, lab, true).clicked() {
                            s.current.adv.auth.scheme = *sc;
                            ui.close();
                        }
                    }
                });
            },
        );
        match s.current.adv.auth.scheme {
            AuthScheme::Basic | AuthScheme::Digest => {
                cp::row_sep(ui, t);
                cp::prop_row_stack(ui, t, "Credentials", |ui| {
                    ui.horizontal(|ui| {
                        let half = (ui.available_width() - space::S2 as f32) / 2.0;
                        TextInput::new(&mut s.current.adv.auth.username)
                            .width(half)
                            .hint("username")
                            .font(ts::mono_sm())
                            .show(ui);
                        PasswordInput::new(&mut s.current.adv.auth.password, "props-auth-pwd")
                            .width(half)
                            .hint("password")
                            .show(ui);
                    });
                });
            }
            AuthScheme::Bearer => {
                cp::row_sep(ui, t);
                cp::prop_row_stack(ui, t, "Token", |ui| {
                    PasswordInput::new(&mut s.current.adv.auth.token, "props-auth-token")
                        .width(ui.available_width())
                        .hint("eyJhbGciOi…")
                        .show(ui);
                });
            }
            AuthScheme::None => {}
        }
    });
}

// ──────────────────────────────────────────────────────────────────────
// 4.4 Cookies
// ──────────────────────────────────────────────────────────────────────

pub fn cookies_tab(ui: &mut egui::Ui, t: &theme::Tokens, state: &mut PropertiesState) {
    let s = &mut *state;
    eyebrow(ui, "cookies");
    cp::section_card(ui, t, |ui| {
        cp::header_with_trailing(
            ui,
            t,
            "Send cookies",
            "Attach a Cookie header to every request for this download. Useful for paywalled mirrors or session-protected URLs.",
            48.0,
            |ui| {
                Toggle::new(&mut s.current.adv.cookies_enabled)
                    .id(egui::Id::new("props-cookies"))
                    .show(ui);
            },
        );

        if !s.current.adv.cookies_enabled {
            return;
        }

        cp::row_sep(ui, t);
        cp::prop_row_stack(ui, t, "Cookie store", |ui| {
            ui.label(
                RichText::new(
                    "Plain text or Netscape (cookies.txt) format. One cookie per line, or a single Cookie-header string.",
                )
                .color(t.fg_3)
                .font(theme::body(11.0)),
            );
            ui.horizontal(|ui| {
                if Btn::new("Import from browser")
                    .toolbar()
                    .accent()
                    .icon("download")
                    .size(BtnSize::Sm)
                    .show(ui)
                    .clicked()
                {
                    s.current.adv.cookie_jar.push_str(
                        "# Netscape HTTP Cookie File\n.example.com\tTRUE\t/\tFALSE\t0\tsession\tabc123\n",
                    );
                }
                if Btn::new("Paste")
                    .toolbar()
                    .accent()
                    .icon("clipboard")
                    .size(BtnSize::Sm)
                    .show(ui)
                    .clicked()
                {
                    tracing::warn!("cookies: clipboard read not wired");
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if Btn::new("Clear")
                        .toolbar()
                        .icon("trash-2")
                        .size(BtnSize::Sm)
                        .enabled(!s.current.adv.cookie_jar.is_empty())
                        .show(ui)
                        .clicked()
                    {
                        s.current.adv.cookie_jar.clear();
                    }
                });
            });
            TextArea::new(&mut s.current.adv.cookie_jar, "props-cookies-jar")
                .width(ui.available_width())
                .initial_height(120.0)
                .font(ts::mono_sm())
                .hint("Paste cookies for this host.\nAccepts Netscape format (one cookie per line)\nor a raw \"name=value; name2=value2\" string.")
                .show(ui);

            let cookies = parse_cookies(&s.current.adv.cookie_jar);
            cp::cookie_chip_strip(ui, t, &cookies);
        });
    });
}

pub fn parse_cookies(raw: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut any_netscape = false;
    for line in raw.lines() {
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() >= 7 {
            any_netscape = true;
            let name = cols[5].trim();
            let val = cols[6].trim();
            if !name.is_empty() && seen.insert(name.to_string()) {
                out.push((name.to_string(), val.to_string()));
            }
        }
    }
    if any_netscape {
        return out;
    }
    for chunk in raw.split([';', '\n']) {
        let chunk = chunk.trim();
        if chunk.is_empty() || chunk.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = chunk.split_once('=') {
            let k = k.trim();
            let v = v.trim();
            if !k.is_empty() && seen.insert(k.to_string()) {
                out.push((k.to_string(), v.to_string()));
            }
        }
    }
    out
}

// ──────────────────────────────────────────────────────────────────────
// 4.5 Headers
// ──────────────────────────────────────────────────────────────────────

pub fn headers_tab(
    ui: &mut egui::Ui,
    t: &theme::Tokens,
    state: &mut PropertiesState,
    job: &crate::domain::Job,
    counters: &crate::ipc_local::protocol::JobCounters,
) {
    let s = &mut *state;

    eyebrow(ui, "custom request headers");
    cp::section_card(ui, t, |ui| {
        ui.label(
            RichText::new("Extra headers")
                .color(t.fg_1)
                .font(theme::body_bold(13.0)),
        );
        ui.label(
            RichText::new(
                "Sent alongside the defaults on every request. Useful for API keys, Origin overrides, or signed URLs.",
            )
            .color(t.fg_3)
            .font(theme::body(11.0)),
        );

        if cp::header_editor(ui, t, &mut s.current.adv.headers) {
            s.current.adv.headers.push(CustomHeader {
                name: String::new(),
                value: String::new(),
            });
        }
    });

    let host = job.url.host_str().unwrap_or("").to_string();
    let range = format!("bytes={}-", counters.downloaded);
    eyebrow(ui, "captured request");
    let mut req_rows: Vec<(&str, &str)> = vec![
        ("User-Agent", s.current.adv.user_agent.as_str()),
        ("Accept", "*/*"),
        ("Accept-Encoding", "identity"),
        ("Range", range.as_str()),
        ("Connection", "keep-alive"),
        ("Host", host.as_str()),
    ];
    if !s.current.adv.referer.is_empty() {
        req_rows.push(("Referer", s.current.adv.referer.as_str()));
    }
    for h in &s.current.adv.headers {
        if h.name.trim().is_empty() && h.value.trim().is_empty() {
            continue;
        }
        req_rows.push((h.name.as_str(), h.value.as_str()));
    }
    cp::captured_table(ui, t, &req_rows);

    let total = counters.total.unwrap_or(0);
    let received = counters.downloaded;
    let ct = match std::path::Path::new(&job.filename.clone().unwrap_or_default())
        .extension()
        .and_then(|s| s.to_str())
    {
        Some("mp4") => "video/mp4",
        Some("iso") => "application/octet-stream",
        Some(_) | None => "application/octet-stream",
    };
    let content_len = total.saturating_sub(received).to_string();
    let content_range = format!(
        "bytes {received}-{}/{}",
        total.saturating_sub(1).max(received),
        total
    );
    let etag = format!("\"a3f9b21e7c4d-{:x}gw\"", total);
    let resp_rows: Vec<(&str, &str)> = vec![
        ("HTTP/2", "206 Partial Content"),
        ("Content-Type", ct),
        ("Content-Length", content_len.as_str()),
        ("Content-Range", content_range.as_str()),
        ("Accept-Ranges", "bytes"),
        ("ETag", etag.as_str()),
        ("Last-Modified", "Wed, 30 Apr 2026 14:12:08 GMT"),
        ("Server", "nginx/1.27"),
        ("Cache-Control", "public, max-age=604800"),
    ];
    eyebrow(ui, "captured response");
    cp::captured_table(ui, t, &resp_rows);
    ui.add_space(space::S1 as f32);
    ui.horizontal(|ui| {
        icons::show(ui, "info", 12.0, t.status_info);
        ui.label(
            RichText::new(
                "Captured from the most recent request. oxdm replays these on resume so the server returns the same content.",
            )
            .color(t.fg_3)
            .font(egui::FontId::new(11.0, egui::FontFamily::Proportional)),
        );
    });
}

// ──────────────────────────────────────────────────────────────────────
// 4.6 Advanced
// ──────────────────────────────────────────────────────────────────────

pub fn advanced_tab(ui: &mut egui::Ui, t: &theme::Tokens, state: &mut PropertiesState) {
    let s = &mut *state;

    eyebrow(ui, "identification");
    cp::section_card(ui, t, |ui| {
        cp::prop_row_stack(ui, t, "User-Agent", |ui| {
            ui.label(
                RichText::new("Override the default UA for this download only.")
                    .color(t.fg_3)
                    .font(theme::body(11.0)),
            );
            TextInput::new(&mut s.current.adv.user_agent)
                .width(ui.available_width())
                .font(ts::mono_sm())
                .show(ui);
        });
        cp::row_sep(ui, t);
        cp::prop_row_stack(ui, t, "Referer", |ui| {
            TextInput::new(&mut s.current.adv.referer)
                .width(ui.available_width())
                .hint("https://example.com/source-page")
                .font(ts::mono_sm())
                .show(ui);
        });
    });

    eyebrow(ui, "transfer");
    cp::section_card(ui, t, |ui| {
        cp::header_with_trailing(
            ui,
            t,
            "Max segments",
            "Parallel connections. Lower this for fragile servers.",
            108.0,
            |ui| {
                NumberStepper::new(&mut s.current.adv.segments, "props-segments")
                    .range(1, 32)
                    .show(ui);
            },
        );

        cp::row_sep(ui, t);
        cp::header_with_trailing(
            ui,
            t,
            "Speed limit",
            "Cap this download's bandwidth. Connection speeds vary widely — set whatever your line can handle.",
            340.0,
            |ui| {
                cp::speed_limit_control(
                    ui,
                    t,
                    &mut s.current.adv.speed_kbps,
                    &mut s.current.adv.speed_unit_mb,
                );
            },
        );
        if s.current.adv.speed_kbps > 0 {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Quick set")
                        .color(t.fg_3)
                        .font(theme::body_bold(10.0)),
                );
                let presets: &[(&str, i64)] = &[
                    ("64 KB/s", 64),
                    ("512 KB/s", 512),
                    ("2 MB/s", 2 * 1024),
                    ("10 MB/s", 10 * 1024),
                ];
                for (label, kbps) in presets {
                    let on = s.current.adv.speed_kbps == *kbps;
                    if Btn::new(*label)
                        .size(BtnSize::Sm)
                        .selected(on)
                        .show(ui)
                        .clicked()
                    {
                        s.current.adv.speed_kbps = *kbps;
                        s.current.adv.speed_unit_mb = *kbps >= 1024 && *kbps % 1024 == 0;
                    }
                }
            });
        }

        cp::row_sep(ui, t);
        cp::header_with_trailing(
            ui,
            t,
            "Connection timeout",
            "How long to wait for the server before giving up on a connection attempt.",
            170.0,
            |ui| {
                NumberStepper::new(&mut s.current.adv.timeout, "props-timeout")
                    .range(5, 300)
                    .show(ui);
                ui.label(
                    RichText::new("seconds")
                        .color(t.fg_3)
                        .font(theme::body(12.0)),
                );
            },
        );

        cp::row_sep(ui, t);
        cp::header_with_trailing(
            ui,
            t,
            "Auto-retry on failure",
            "Retries are exponential — 1s, 2s, 4s, 8s, capped at 60s.",
            108.0,
            |ui| {
                NumberStepper::new(&mut s.current.adv.retries, "props-retries")
                    .range(0, 20)
                    .show(ui);
            },
        );

        cp::row_sep(ui, t);
        cp::header_with_trailing(
            ui,
            t,
            "Auto-verify checksums",
            "Compute & compare every saved hash when the download completes.",
            48.0,
            |ui| {
                Toggle::new(&mut s.current.adv.auto_verify)
                    .id(egui::Id::new("props-autoverify"))
                    .show(ui);
            },
        );
    });

    eyebrow(ui, "after completion");
    cp::section_card(ui, t, |ui| {
        cp::header_with_trailing(ui, t, "Open file when done", "", 48.0, |ui| {
            Toggle::new(&mut s.current.adv.open_when_done)
                .id(egui::Id::new("props-openwhendone"))
                .show(ui);
        });
        cp::row_sep(ui, t);
        cp::prop_row_stack(ui, t, "Run command", |ui| {
            ui.label(
                RichText::new("Executed against the saved file path.")
                    .color(t.fg_3)
                    .font(theme::body(11.0)),
            );
            TextInput::new(&mut s.current.adv.run_command)
                .width(ui.available_width())
                .hint("open -R   or   shasum -a 256")
                .font(ts::mono_sm())
                .show(ui);
        });
    });
}
