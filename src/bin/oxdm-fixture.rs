//! Visual-test fixture binary. Mounts a named leaf widget (or small
//! widget grid) in a minimal eframe app — no daemon, no IPC, no
//! database. Useful for per-component sweep against the design
//! handoff, where the cost of standing up the full app per widget is
//! prohibitive.
//!
//! Usage:
//!   oxdm-fixture --list
//!   oxdm-fixture --fixture buttons [--theme warm] [--size 800x600]
//!   oxdm-fixture --fixture buttons --snap /tmp/buttons.png
//!
//! `--snap` paints once, requests an `egui::ViewportCommand::Screenshot`,
//! writes the PNG, and exits. No window-manager / xdotool needed.
//!
//! Adding a fixture: implement a `fn(&mut Ui)` and register it in
//! [`FIXTURES`]. Keep each closed-loop — no app state, no IPC, only
//! widgets that read from `theme::tokens`.

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{self, Align2, Color32, RichText, Vec2, ViewportCommand};

use oxdm::domain::{Settings, Theme};
use oxdm::ui::components::primitives::{
    self, Btn, BtnSize, BtnVariant, Combo, PasswordInput, TabBtn, TextInput, Toggle,
};
use oxdm::ui::components::properties as cp;
use oxdm::ui::components::titlebar;
use oxdm::ui::theme::{self, radius, space, ts};
use oxdm::ui::utils::icons;

// ──────────────────────────────────────────────────────────────────────
// Fixture registry
// ──────────────────────────────────────────────────────────────────────

type FixtureFn = fn(&mut egui::Ui);

const FIXTURES: &[(&str, FixtureFn)] = &[
    ("buttons", fixtures::buttons),
    ("tabs", fixtures::tabs),
    ("progress", fixtures::progress),
    ("inline-progress", fixtures::inline_progress),
    ("striped-progress", fixtures::striped_progress),
    ("pills", fixtures::pills),
    ("typography", fixtures::typography),
    ("tokens", fixtures::tokens),
    ("cards", fixtures::cards),
    ("icon-row", fixtures::icon_row_card),
    ("search", fixtures::search),
    ("icons", fixtures::icons),
    ("titlebar", fixtures::titlebar),
    ("col-headers", fixtures::col_headers),
    ("segmented", fixtures::segmented),
    ("collapsible", fixtures::collapsible),
    ("inputs", fixtures::inputs),
    ("password", fixtures::password),
    ("toggle", fixtures::toggle),
    ("number-stepper", fixtures::number_stepper),
    ("props-section-card", fixtures::props_section_card),
    ("props-kv-row", fixtures::props_kv_row),
    ("props-path-row", fixtures::props_path_row),
    ("props-url-row", fixtures::props_url_row),
    ("props-lock-banner", fixtures::props_lock_banner),
    ("props-info-callout", fixtures::props_info_callout),
    ("props-phase-pill", fixtures::props_phase_pill),
    ("props-status-banner", fixtures::props_status_banner),
    ("props-status-pill", fixtures::props_status_pill),
    ("props-checksum-row", fixtures::props_checksum_row),
    ("props-mismatch-diff", fixtures::props_mismatch_diff),
    ("props-add-checksum", fixtures::props_add_checksum),
    ("props-speed-limit", fixtures::props_speed_limit),
    ("props-header-editor", fixtures::props_header_editor),
    ("props-captured-kv", fixtures::props_captured_kv),
    ("props-captured-request", fixtures::props_captured_request),
    ("props-captured-response", fixtures::props_captured_response),
    ("props-cookie-chips", fixtures::props_cookie_chips),
    ("props-tab-general", fixtures::props_tab_general),
    (
        "props-tab-general-with-checksum",
        fixtures::props_tab_general_with_checksum,
    ),
    (
        "props-tab-checksum-empty",
        fixtures::props_tab_checksum_empty,
    ),
    (
        "props-tab-checksum-verified",
        fixtures::props_tab_checksum_verified,
    ),
    (
        "props-tab-checksum-mixed",
        fixtures::props_tab_checksum_mixed,
    ),
    (
        "props-tab-checksum-mismatch",
        fixtures::props_tab_checksum_mismatch,
    ),
    ("props-tab-checksum-add", fixtures::props_tab_checksum_add),
    ("props-tab-connection", fixtures::props_tab_connection),
    (
        "props-tab-connection-http-proxy",
        fixtures::props_tab_connection_http_proxy,
    ),
    (
        "props-tab-connection-socks-proxy",
        fixtures::props_tab_connection_socks_proxy,
    ),
    (
        "props-tab-connection-system-proxy",
        fixtures::props_tab_connection_system_proxy,
    ),
    (
        "props-tab-connection-auth-basic",
        fixtures::props_tab_connection_auth_basic,
    ),
    (
        "props-tab-connection-auth-bearer",
        fixtures::props_tab_connection_auth_bearer,
    ),
    (
        "props-tab-connection-auth-digest",
        fixtures::props_tab_connection_auth_digest,
    ),
    (
        "props-tab-cookies-disabled",
        fixtures::props_tab_cookies_disabled,
    ),
    (
        "props-tab-cookies-enabled",
        fixtures::props_tab_cookies_enabled,
    ),
    ("props-tab-headers", fixtures::props_tab_headers),
    (
        "props-tab-headers-add-header",
        fixtures::props_tab_headers_add_header,
    ),
    ("props-tab-advanced-top", fixtures::props_tab_advanced_top),
];

// ──────────────────────────────────────────────────────────────────────
// CLI
// ──────────────────────────────────────────────────────────────────────

#[derive(Debug)]
struct Opts {
    fixture: String,
    theme: Theme,
    width: f32,
    height: f32,
    snap: Option<String>,
}

fn parse_args() -> Result<Option<Opts>, String> {
    let mut args = std::env::args().skip(1);
    let mut fixture: Option<String> = None;
    let mut theme = Theme::Light;
    let mut width = 960.0;
    let mut height = 720.0;
    let mut snap: Option<String> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--list" => {
                for (name, _) in FIXTURES {
                    println!("{name}");
                }
                return Ok(None);
            }
            "--fixture" => {
                fixture = Some(args.next().ok_or("--fixture needs a value")?);
            }
            "--theme" => {
                theme = match args.next().as_deref() {
                    Some("light") | Some("utility") => Theme::Light,
                    Some("dark") => Theme::Dark,
                    Some("warm") => Theme::Warm,
                    Some("system") => Theme::System,
                    Some(s) => return Err(format!("unknown theme: {s}")),
                    None => return Err("--theme needs a value".into()),
                };
            }
            "--size" => {
                let v = args.next().ok_or("--size needs WxH")?;
                let (w, h) = v.split_once('x').ok_or("--size must be WxH")?;
                width = w.parse().map_err(|e| format!("bad width: {e}"))?;
                height = h.parse().map_err(|e| format!("bad height: {e}"))?;
            }
            "--snap" => {
                snap = Some(args.next().ok_or("--snap needs a path")?);
            }
            "--help" | "-h" => {
                eprintln!(
                    "usage: oxdm-fixture --fixture NAME [--theme TH] [--size WxH] [--snap PATH]\n\
                     options:\n\
                       --list             list available fixtures and exit\n\
                       --fixture NAME     fixture to render (required)\n\
                       --theme TH         light | dark | warm | system (default: light)\n\
                       --size WxH         window size in points (default: 960x720)\n\
                       --snap PATH        capture screenshot to PATH, then exit"
                );
                return Ok(None);
            }
            other => return Err(format!("unknown arg: {other}")),
        }
    }
    let fixture = fixture.ok_or("--fixture is required (or pass --list)")?;
    Ok(Some(Opts {
        fixture,
        theme,
        width,
        height,
        snap,
    }))
}

// ──────────────────────────────────────────────────────────────────────
// App
// ──────────────────────────────────────────────────────────────────────

struct FixtureApp {
    fixture: FixtureFn,
    snap: Option<String>,
    /// Latched once we have requested a screenshot, so we don't fire
    /// repeatedly across the few frames before the result arrives.
    snap_requested: Arc<AtomicBool>,
    /// SVG icons (`egui_extras::SvgLoader`) load and rasterise across
    /// the first ~10 frames. Capturing too early gives blank icon rects.
    /// We delay the snap request until this many frames have rendered.
    frames_rendered: u32,
    settings: Settings,
}

const SNAP_WARMUP_FRAMES: u32 = 30;

impl eframe::App for FixtureApp {
    fn ui(&mut self, root_ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &root_ui.ctx().clone();
        // Re-apply theme each frame: cheap, and ensures tokens land
        // before any fixture asks for them.
        theme::apply(ctx, &self.settings);

        egui::CentralPanel::default().show_inside(root_ui, |ui| {
            (self.fixture)(ui);
        });

        self.frames_rendered = self.frames_rendered.saturating_add(1);

        if let Some(path) = self.snap.as_ref() {
            if self.frames_rendered < SNAP_WARMUP_FRAMES {
                // SVG icons + font atlases settle in the first several
                // frames; capturing too early renders blank icon rects.
                ctx.request_repaint();
                return;
            }
            if !self.snap_requested.swap(true, Ordering::SeqCst) {
                ctx.send_viewport_cmd(ViewportCommand::Screenshot(egui::UserData::default()));
                ctx.request_repaint();
            }
            // Drain raw events looking for our screenshot result.
            let mut got = None;
            ctx.input(|i| {
                for evt in &i.raw.events {
                    if let egui::Event::Screenshot { image, .. } = evt {
                        got = Some(std::sync::Arc::clone(image));
                    }
                }
            });
            if let Some(img) = got {
                if let Err(e) = save_color_image(&img, path) {
                    eprintln!("screenshot save failed: {e}");
                }
                ctx.send_viewport_cmd(ViewportCommand::Close);
            } else {
                // Force another frame so the screenshot event arrives.
                ctx.request_repaint();
            }
        }
    }
}

fn save_color_image(img: &egui::ColorImage, path: &str) -> std::io::Result<()> {
    let (w, h) = (img.size[0] as u32, img.size[1] as u32);
    let mut buf: Vec<u8> = Vec::with_capacity(img.pixels.len() * 4);
    for px in &img.pixels {
        buf.push(px.r());
        buf.push(px.g());
        buf.push(px.b());
        buf.push(px.a());
    }
    let img = image::RgbaImage::from_raw(w, h, buf)
        .ok_or_else(|| std::io::Error::other("color image buffer size mismatch"))?;
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent).ok();
    }
    img.save(path)
        .map_err(|e| std::io::Error::other(format!("png save: {e}")))?;
    Ok(())
}

// ──────────────────────────────────────────────────────────────────────
// Fixtures
// ──────────────────────────────────────────────────────────────────────

mod fixtures {
    use super::*;

    pub fn buttons(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        ui.spacing_mut().item_spacing = Vec2::new(space::S3 as f32, space::S3 as f32);
        section(ui, "Variants × default state");
        ui.horizontal(|ui| {
            Btn::new("Primary").primary().show(ui);
            Btn::new("Secondary").show(ui);
            Btn::new("Ghost").toolbar().show(ui);
            Btn::new("Danger").danger().show(ui);
            Btn::new("DangerFilled").danger_filled().show(ui);
        });
        ui.add_space(space::S3 as f32);
        section(ui, "Sizes (Primary)");
        ui.horizontal(|ui| {
            Btn::new("Sm").primary().size(BtnSize::Sm).show(ui);
            Btn::new("Md").primary().size(BtnSize::Md).show(ui);
            Btn::new("Lg").primary().size(BtnSize::Lg).show(ui);
        });
        ui.add_space(space::S3 as f32);
        section(ui, "With icon + icon-only");
        ui.horizontal(|ui| {
            Btn::new("Add").primary().icon("plus").show(ui);
            Btn::new("Pause").icon("pause").show(ui);
            Btn::new("").toolbar().icon_only("more-horizontal").show(ui);
            Btn::new("").toolbar().icon_only("x").show(ui);
        });
        ui.add_space(space::S3 as f32);
        section(ui, "Disabled vs selected");
        ui.horizontal(|ui| {
            Btn::new("Disabled Primary")
                .primary()
                .enabled(false)
                .show(ui);
            Btn::new("Disabled Secondary").enabled(false).show(ui);
            Btn::new("Selected").selected(true).show(ui);
            Btn::new("Selected Ghost").toolbar().selected(true).show(ui);
        });
        ui.add_space(space::S3 as f32);
        section(ui, "Tone variants — all six");
        for variant in [
            BtnVariant::Primary,
            BtnVariant::Secondary,
            BtnVariant::Toolbar,
            BtnVariant::Danger,
            BtnVariant::DangerFilled,
        ] {
            ui.horizontal(|ui| {
                let label = format!("{variant:?}");
                ui.add_space(space::S1 as f32);
                ui.label(RichText::new(&label).font(ts::xs()).color(t.fg_3));
                ui.add_space(space::S2 as f32);
                Btn::new("default").variant(variant).show(ui);
                Btn::new("disabled")
                    .variant(variant)
                    .enabled(false)
                    .show(ui);
                Btn::new("selected")
                    .variant(variant)
                    .selected(true)
                    .show(ui);
            });
        }
    }

    pub fn tabs(ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.x = space::S4 as f32;
        section(ui, "TabBtn states");
        ui.horizontal(|ui| {
            TabBtn::new("All").active(true).count(42).show(ui);
            TabBtn::new("Active").icon("activity").count(3).show(ui);
            TabBtn::new("Finished").icon("check").count(39).show(ui);
            TabBtn::new("Failed").icon("alert-circle").count(0).show(ui);
        });
    }

    pub fn progress(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        ui.spacing_mut().item_spacing.y = space::S3 as f32;
        section(ui, "pill_progress @ 0 / 30 / 75 / 100");
        for frac in [0.0, 0.30, 0.75, 1.0] {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{:>3}%", (frac * 100.0) as i32))
                        .font(ts::count())
                        .color(t.fg_3),
                );
                primitives::pill_progress(ui, frac, 280.0, 10.0, t.progress_track, t.progress_fill);
            });
        }
        ui.add_space(space::S3 as f32);
        section(ui, "Status-tinted progress");
        for (label, fill) in [
            ("success", t.status_success),
            ("warning", t.status_warning),
            ("danger", t.status_danger),
        ] {
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).font(ts::count()).color(fill));
                primitives::pill_progress(ui, 0.6, 280.0, 8.0, t.progress_track, fill);
            });
        }
    }

    pub fn inline_progress(ui: &mut egui::Ui) {
        use eframe::egui::{Rect, Sense, Vec2 as V2};
        let t = theme::tokens(ui.ctx());
        ui.spacing_mut().item_spacing.y = space::S3 as f32;

        // Each row: a left label column + the bar rect. We allocate the
        // bar rect explicitly (the primitive paints with `Painter`, not
        // through egui layout) so its width / height stay deterministic
        // across runs.
        let bar_w = 280.0;
        let bar_h = 22.0;
        let draw_row = |ui: &mut egui::Ui, label: &str, frac: f32, selected: bool| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{label:>9}"))
                        .font(ts::count())
                        .color(t.fg_3),
                );
                let (rect, _) = ui.allocate_exact_size(V2::new(bar_w, bar_h), Sense::hover());
                primitives::inline_progress(
                    &ui.painter().clone(),
                    rect,
                    &t,
                    frac,
                    "Downloading",
                    selected,
                );
            });
        };

        section(ui, "inline_progress — unselected (clay-300 @ alpha 100)");
        for frac in [0.0, 0.01, 0.04, 0.30, 0.75, 0.99, 1.0] {
            draw_row(ui, &format!("{:>3}%", (frac * 100.0) as i32), frac, false);
        }
        ui.add_space(space::S3 as f32);
        section(ui, "inline_progress — selected (clay-300 @ alpha 150)");
        for frac in [0.0, 0.01, 0.04, 0.30, 0.75, 0.99, 1.0] {
            draw_row(ui, &format!("{:>3}%", (frac * 100.0) as i32), frac, true);
        }
        ui.add_space(space::S3 as f32);
        section(ui, "edge cases — 100% bleed check, narrow bar");
        // Bigger bar so the rounded corners are obvious.
        ui.horizontal(|ui| {
            ui.label(RichText::new("wide 100%").font(ts::count()).color(t.fg_3));
            let (rect, _) = ui.allocate_exact_size(V2::new(420.0, 28.0), Sense::hover());
            primitives::inline_progress(&ui.painter().clone(), rect, &t, 1.0, "Done", false);
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new("narrow 4%").font(ts::count()).color(t.fg_3));
            let (rect, _) = ui.allocate_exact_size(V2::new(160.0, 22.0), Sense::hover());
            primitives::inline_progress(&ui.painter().clone(), rect, &t, 0.04, "Paused", false);
        });
        // Suppress unused-variant warning for `Rect`.
        let _ = Rect::NOTHING;
    }

    pub fn striped_progress(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        ui.spacing_mut().item_spacing.y = space::S3 as f32;

        // Active-download gradient pair from `download::window`.
        let grad = Some((
            Color32::from_rgb(0xC9, 0x70, 0x3F),
            Color32::from_rgb(0xDA, 0x8E, 0x63),
        ));

        // CentralPanel fill is `bg_page` per theme::apply; pass that so
        // the corner-mask outside-stroke blends with the fixture bg.
        let bg = t.bg_page;
        let draw = |ui: &mut egui::Ui,
                    label: &str,
                    frac: f32,
                    gradient: Option<(Color32, Color32)>,
                    animate: bool| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{label:>9}"))
                        .font(ts::count())
                        .color(t.fg_3),
                );
                primitives::striped_progress(
                    ui,
                    frac,
                    420.0,
                    10.0,
                    t.progress_track,
                    t.progress_fill,
                    gradient,
                    animate,
                    bg,
                );
            });
        };

        section(ui, "striped_progress — solid fill, no stripes");
        for frac in [0.0, 0.01, 0.04, 0.30, 0.75, 0.99, 1.0] {
            draw(
                ui,
                &format!("{:>3}%", (frac * 100.0) as i32),
                frac,
                None,
                false,
            );
        }
        ui.add_space(space::S3 as f32);
        section(ui, "striped_progress — gradient fill, no stripes");
        for frac in [0.0, 0.01, 0.04, 0.30, 0.75, 0.99, 1.0] {
            draw(
                ui,
                &format!("{:>3}%", (frac * 100.0) as i32),
                frac,
                grad,
                false,
            );
        }
        ui.add_space(space::S3 as f32);
        section(ui, "striped_progress — gradient + stripes (active)");
        for frac in [0.04, 0.30, 0.75, 1.0] {
            draw(
                ui,
                &format!("{:>3}%", (frac * 100.0) as i32),
                frac,
                grad,
                true,
            );
        }
        ui.add_space(space::S3 as f32);
        section(ui, "edge cases — taller bar, narrow bar");
        ui.horizontal(|ui| {
            ui.label(RichText::new("tall 4%").font(ts::count()).color(t.fg_3));
            primitives::striped_progress(
                ui,
                0.04,
                420.0,
                22.0,
                t.progress_track,
                t.progress_fill,
                grad,
                false,
                bg,
            );
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new("narrow 4%").font(ts::count()).color(t.fg_3));
            primitives::striped_progress(
                ui,
                0.04,
                160.0,
                10.0,
                t.progress_track,
                t.progress_fill,
                grad,
                false,
                bg,
            );
        });
    }

    pub fn pills(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        ui.spacing_mut().item_spacing = Vec2::new(space::S3 as f32, space::S2 as f32);
        section(ui, "pill_count");
        ui.horizontal(|ui| {
            for n in [1usize, 9, 42, 128, 1024] {
                primitives::pill_count(ui, n, t.fg_2, t.bg_sunken);
            }
        });
        ui.add_space(space::S3 as f32);
        section(ui, "status_dot");
        for (label, color) in [
            ("Idle", t.fg_3),
            ("Running", t.status_success),
            ("Paused", t.status_warning),
            ("Failed", t.status_danger),
        ] {
            primitives::status_dot(ui, color, label, 12.0);
        }
    }

    pub fn typography(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        section(ui, "Type scale (tokens.json::font)");
        let rows = [
            ("3xl", ts::xxxl()),
            ("2xl", ts::xxl()),
            ("xl", ts::xl()),
            ("lg", ts::lg()),
            ("md", ts::md()),
            ("base", ts::base()),
            ("sm", ts::sm()),
            ("xs", ts::xs()),
            ("eyebrow", ts::eyebrow()),
            ("label", ts::label()),
            ("count", ts::count()),
            ("mono_sm", ts::mono_sm()),
        ];
        ui.spacing_mut().item_spacing.y = space::S2 as f32;
        for (name, font) in rows {
            ui.horizontal(|ui| {
                ui.add_space(space::S1 as f32);
                ui.label(
                    RichText::new(format!("{name:<8}"))
                        .font(ts::mono_sm())
                        .color(t.fg_3),
                );
                ui.label(
                    RichText::new("Quick brown fox · 1234567890")
                        .font(font.clone())
                        .color(t.fg_1),
                );
                ui.label(
                    RichText::new(format!("size={:.0}", font.size))
                        .font(ts::xs())
                        .color(t.fg_4),
                );
            });
        }
    }

    pub fn tokens(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        section(ui, "Foreground");
        swatch_row(
            ui,
            &[
                ("fg_1", t.fg_1),
                ("fg_2", t.fg_2),
                ("fg_3", t.fg_3),
                ("fg_4", t.fg_4),
                ("fg_inverse", t.fg_inverse),
            ],
        );
        section(ui, "Background");
        swatch_row(
            ui,
            &[
                ("bg_page", t.bg_page),
                ("bg_surface", t.bg_surface),
                ("bg_sunken", t.bg_sunken),
                ("bg_raised", t.bg_raised),
                ("bg_inverse", t.bg_inverse),
            ],
        );
        section(ui, "Border");
        swatch_row(
            ui,
            &[
                ("border_subtle", t.border_subtle),
                ("border_default", t.border_default),
                ("border_strong", t.border_strong),
                ("border_brand", t.border_brand),
            ],
        );
        section(ui, "Action / status");
        swatch_row(
            ui,
            &[
                ("action_primary", t.action_primary),
                ("action_primary_press", t.action_primary_press),
                ("status_success", t.status_success),
                ("status_warning", t.status_warning),
                ("status_danger", t.status_danger),
                ("status_info", t.status_info),
            ],
        );
        section(ui, "Row states");
        swatch_row(
            ui,
            &[
                ("row_hover_bg", t.row_hover_bg),
                ("row_selected_bg", t.row_selected_bg),
                ("row_selhover_bg", t.row_selhover_bg),
            ],
        );
    }

    pub fn cards(ui: &mut egui::Ui) {
        let _ = ui;
        let t = theme::tokens(ui.ctx());
        section(ui, "card()");
        primitives::card(ui, space::S3 as f32, |ui| {
            ui.label(RichText::new("Card body").font(ts::base()).color(t.fg_1));
            ui.label(RichText::new("Secondary line").font(ts::sm()).color(t.fg_3));
        });
        ui.add_space(space::S3 as f32);
        section(ui, "collapsible_card() — uses persisted state id");
        primitives::collapsible_card(
            ui,
            egui::Id::new("fixture-coll-1"),
            "Collapsible card",
            None,
            true,
            |ui| {
                ui.label(
                    RichText::new("Body content lives here.")
                        .font(ts::base())
                        .color(t.fg_2),
                );
            },
        );
    }

    /// `icon_row` — the flex-based header card (square tile | grow
    /// title+meta column | right cell). Exercises the vertical centering
    /// that egui_flex provides and the long-text truncation in the middle
    /// cell. Mirrors `download::header_card` / `add::detected_card`. Kept
    /// as a fixture so a regression in flex centering (e.g. swapping it
    /// for a manual layout that doesn't re-centre earlier cells) is caught
    /// in a snapshot rather than only in the live window.
    pub fn icon_row_card(ui: &mut egui::Ui) {
        use oxdm::ui::components::icon_row::icon_row;
        let t = theme::tokens(ui.ctx());
        section(ui, "icon_row — tile | grow+truncate middle | right pill");
        primitives::card(ui, space::S3 as f32, |ui| {
            icon_row(
                ui,
                56.0,
                |ui, rect| {
                    ui.painter()
                        .rect_filled(rect, radius::SM as f32, t.bg_raised);
                    let painter = ui.painter().clone();
                    let g =
                        painter.layout_no_wrap("ZIP".to_string(), theme::body_bold(13.0), t.fg_2);
                    painter.galley(
                        egui::Pos2::new(
                            rect.center().x - g.size().x / 2.0,
                            rect.center().y - g.size().y / 2.0,
                        ),
                        g,
                        t.fg_2,
                    );
                },
                |ui| {
                    ui.label(
                        RichText::new("ubuntu-24.04.1-desktop-amd64-very-long-filename.iso")
                            .font(theme::body_bold(14.0))
                            .color(t.fg_1),
                    );
                    ui.label(
                        RichText::new("ipv4.download.thinkbroadband.com · Other · resumable")
                            .color(t.fg_3)
                            .font(theme::body(11.0)),
                    );
                },
                |ui| {
                    cp::phase_pill(ui, &t, t.fg_3, "QUEUED");
                },
            );
        });
    }

    pub fn search(ui: &mut egui::Ui) {
        section(ui, "search_field — empty");
        let mut empty = String::new();
        primitives::search_field(ui, &mut empty, "Search downloads…", 320.0);
        ui.add_space(space::S3 as f32);
        section(ui, "search_field — populated");
        let mut filled = "ubuntu-24.04-desktop".to_owned();
        primitives::search_field(ui, &mut filled, "Search downloads…", 320.0);
    }

    /// Grid of every Lucide icon registered in `icons_table.in`. Useful
    /// for design audits — verifies each glyph rasterises at multiple
    /// sizes and theme tints.
    pub fn icons(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        section(ui, "All registered icons @ 18px");
        // We can't enumerate names without reaching into icons_table —
        // hard-code the canonical list. If the list rots we'll see
        // missing icons in the grid.
        const NAMES: &[&str] = &[
            "activity",
            "archive",
            "arrow-left",
            "arrow-right",
            "bell",
            "calendar",
            "check",
            "chevron-down",
            "chevron-left",
            "chevron-right",
            "chevron-up",
            "circle",
            "circle-alert",
            "circle-check",
            "circle-x",
            "clipboard",
            "clock",
            "cloud-upload",
            "cog",
            "copy",
            "database",
            "download",
            "ellipsis",
            "eye",
            "eye-off",
            "file",
            "file-text",
            "film",
            "folder",
            "folder-plus",
            "gauge",
            "globe",
            "hard-drive",
            "house",
            "image",
            "info",
            "key",
            "layers",
            "link",
            "list",
            "lock",
            "minus",
            "moon",
            "music",
            "package",
            "pause",
            "pencil",
            "play",
            "plus",
            "power",
            "puzzle",
            "refresh-cw",
            "rotate-cw",
            "save",
            "scissors",
            "search",
            "settings",
            "shield",
            "square",
            "terminal",
            "trash-2",
            "triangle-alert",
            "x",
            "more-horizontal",
        ];
        const COLS: usize = 9;
        let cell: f32 = 72.0;
        for (i, _name) in NAMES.iter().enumerate() {
            if i % COLS == 0 {
                ui.horizontal_wrapped(|ui| {
                    for j in 0..COLS {
                        let k = i + j;
                        if k >= NAMES.len() {
                            break;
                        }
                        let nm = NAMES[k];
                        let (rect, _) =
                            ui.allocate_exact_size(Vec2::new(cell, cell), egui::Sense::hover());
                        let pr: egui::CornerRadius = radius::SM.into();
                        ui.painter().rect_filled(rect, pr, t.bg_surface);
                        ui.painter().rect_stroke(
                            rect,
                            pr,
                            egui::Stroke::new(t.border_width_hairline, t.border_subtle),
                            egui::StrokeKind::Inside,
                        );
                        let icon_size = 22.0;
                        let icon_rect = egui::Rect::from_center_size(
                            egui::pos2(rect.center().x, rect.top() + 24.0),
                            Vec2::splat(icon_size),
                        );
                        icons::icon(ui.ctx(), nm, icon_size, t.fg_1).paint_at(ui, icon_rect);
                        ui.painter().text(
                            egui::pos2(rect.center().x, rect.bottom() - 18.0),
                            Align2::CENTER_CENTER,
                            nm.to_string(),
                            ts::xs(),
                            t.fg_3,
                        );
                    }
                });
            }
        }
    }

    pub fn titlebar(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        let ctx = ui.ctx().clone();
        section(ui, "titlebar::show — main window");
        ui.push_id("titlebar-main", |ui| {
            egui::Frame::NONE.fill(t.bg_titlebar).show(ui, |ui| {
                titlebar::show(ui, &ctx, "oxdm — Main");
            });
        });
        ui.add_space(space::S4 as f32);
        section(ui, "titlebar — dialog title");
        ui.push_id("titlebar-dialog", |ui| {
            egui::Frame::NONE.fill(t.bg_titlebar).show(ui, |ui| {
                titlebar::show(ui, &ctx, "oxdm — Remove download");
            });
        });
    }

    pub fn col_headers(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        ui.spacing_mut().item_spacing.y = space::S3 as f32;
        section(ui, "col_header — left + right alignment");
        egui::Frame::NONE
            .fill(t.bg_surface)
            .stroke(egui::Stroke::new(t.border_width_hairline, t.border_subtle))
            .inner_margin(space::S3 as f32)
            .show(ui, |ui| {
                ui.allocate_ui_with_layout(
                    Vec2::new(560.0, 28.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_max_width(280.0);
                        primitives::col_header(ui, "Name");
                    },
                );
            });
        section(ui, "col_header_sortable — every active/desc combination");
        let combos = [
            ("Name", egui::Align::LEFT, false, false),
            ("Size", egui::Align::RIGHT, true, false),
            ("Status", egui::Align::LEFT, true, true),
            ("Speed", egui::Align::RIGHT, false, false),
        ];
        for (label, align, active, desc) in combos {
            egui::Frame::NONE
                .fill(t.bg_surface)
                .stroke(egui::Stroke::new(t.border_width_hairline, t.border_subtle))
                .inner_margin(space::S3 as f32)
                .show(ui, |ui| {
                    ui.allocate_ui_with_layout(
                        Vec2::new(220.0, 24.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            primitives::col_header_sortable(ui, label, align, active, desc);
                        },
                    );
                });
        }
    }

    pub fn segmented(ui: &mut egui::Ui) {
        section(ui, "segmented — three options, first selected");
        let _ = primitives::segmented(
            ui,
            &[
                ("All", Some("list")),
                ("Active", Some("activity")),
                ("Done", Some("check")),
            ],
            0,
        );
        ui.add_space(space::S3 as f32);
        section(ui, "segmented — middle option selected");
        let _ = primitives::segmented(
            ui,
            &[
                ("Light", Some("moon")),
                ("Warm", Some("sun")),
                ("Dark", Some("moon")),
            ],
            1,
        );
        ui.add_space(space::S3 as f32);
        section(ui, "segmented — text-only");
        let _ = primitives::segmented(
            ui,
            &[
                ("Day", None),
                ("Week", None),
                ("Month", None),
                ("Year", None),
            ],
            2,
        );
    }

    pub fn collapsible(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        section(ui, "collapsible_card — open + closed");
        primitives::collapsible_card(
            ui,
            egui::Id::new("fixture-collap-open"),
            "Open card",
            None,
            true,
            |ui| {
                ui.label(
                    egui::RichText::new(
                        "Body content with two lines.\nSecond line wraps to next row.",
                    )
                    .font(ts::base())
                    .color(t.fg_2),
                );
            },
        );
        ui.add_space(space::S2 as f32);
        primitives::collapsible_card(
            ui,
            egui::Id::new("fixture-collap-closed"),
            "Closed card",
            Some(
                egui::RichText::new("3 items")
                    .font(ts::sm())
                    .color(t.fg_3)
                    .into(),
            ),
            false,
            |_| {},
        );
    }

    /// Side-by-side TextInput + Combo for pixel-parity checks. Both
    /// allocated at identical widths so any drift in frame height,
    /// stroke position, or right-edge alignment is visually obvious.
    pub fn inputs(ui: &mut egui::Ui) {
        let w = 240.0;
        section(ui, "TextInput vs Combo (same width)");
        ui.spacing_mut().item_spacing.x = 12.0;
        ui.spacing_mut().item_spacing.y = 12.0;

        let mut text = "127.0.0.1:1080".to_owned();
        ui.horizontal(|ui| {
            TextInput::new(&mut text)
                .width(w)
                .hint("host:port")
                .show(ui);
            Combo::new("fixture-combo-a", "None (direct)")
                .width(w)
                .show(ui, |ui| {
                    let _ = Combo::item(ui, "None (direct)", true);
                    let _ = Combo::item(ui, "HTTP", true);
                    let _ = Combo::item(ui, "SOCKS5", true);
                });
        });

        ui.add_space(space::S2 as f32);
        section(ui, "Stacked — verify identical right edge + height");
        TextInput::new(&mut text).width(w).show(ui);
        Combo::new("fixture-combo-b", "Compressed")
            .width(w)
            .show(ui, |ui| {
                let _ = Combo::item(ui, "Compressed", true);
                let _ = Combo::item(ui, "Programs", true);
                let _ = Combo::item(ui, "Videos", true);
            });

        ui.add_space(space::S3 as f32);
        section(ui, "Disabled state (cursor + tint)");
        ui.horizontal(|ui| {
            TextInput::new(&mut text)
                .width(w)
                .enabled(false)
                .hint("disabled")
                .show(ui);
            Combo::new("fixture-combo-c", "Locked")
                .width(w)
                .enabled(false)
                .show(ui, |_| {});
        });
    }

    /// PasswordInput component — three states:
    /// 1. Empty (hint visible, hold eye to reveal nothing).
    /// 2. Pre-filled (hold eye reveals plaintext).
    /// 3. Disabled.
    pub fn password(ui: &mut egui::Ui) {
        let w = 280.0;
        ui.spacing_mut().item_spacing.x = space::S3 as f32;
        ui.spacing_mut().item_spacing.y = space::S3 as f32;

        section(ui, "Empty — hold the eye to peek at typed value");
        let mut fresh = String::new();
        PasswordInput::new(&mut fresh, "fx-pw-fresh")
            .hint("type a password")
            .width(w)
            .show(ui);

        ui.add_space(space::S2 as f32);
        section(ui, "Pre-filled plaintext");
        let mut typed = "letmein".to_string();
        PasswordInput::new(&mut typed, "fx-pw-typed")
            .width(w)
            .show(ui);

        ui.add_space(space::S2 as f32);
        section(ui, "Disabled");
        let mut dis = "secret".to_string();
        PasswordInput::new(&mut dis, "fx-pw-dis")
            .width(w)
            .enabled(false)
            .show(ui);
    }

    pub fn toggle(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        ui.spacing_mut().item_spacing = Vec2::new(space::S3 as f32, space::S3 as f32);

        section(ui, "Toggle — off / on");
        ui.horizontal(|ui| {
            let mut off = false;
            Toggle::new(&mut off)
                .id(egui::Id::new("fx-tog-off"))
                .show(ui);
            let mut on = true;
            Toggle::new(&mut on).id(egui::Id::new("fx-tog-on")).show(ui);
        });

        ui.add_space(space::S3 as f32);
        section(ui, "Disabled");
        ui.horizontal(|ui| {
            let mut off = false;
            Toggle::new(&mut off)
                .enabled(false)
                .id(egui::Id::new("fx-tog-doff"))
                .show(ui);
            let mut on = true;
            Toggle::new(&mut on)
                .enabled(false)
                .id(egui::Id::new("fx-tog-don"))
                .show(ui);
        });

        ui.add_space(space::S3 as f32);
        section(ui, "Inline with label");
        ui.horizontal(|ui| {
            let mut on = true;
            Toggle::new(&mut on)
                .id(egui::Id::new("fx-tog-lbl"))
                .show(ui);
            ui.label(
                RichText::new("Enable feature")
                    .font(ts::base())
                    .color(t.fg_1),
            );
        });
    }

    pub fn number_stepper(ui: &mut egui::Ui) {
        use oxdm::ui::components::primitives::NumberStepper;
        section(ui, "NumberStepper — clamps to range");
        let mut v1: i64 = 8;
        ui.horizontal(|ui| {
            NumberStepper::new(&mut v1, "fx-ns-1").range(1, 32).show(ui);
            ui.label("Max segments (1..32)");
        });
        let mut v2: i64 = 30;
        ui.horizontal(|ui| {
            NumberStepper::new(&mut v2, "fx-ns-2")
                .range(5, 300)
                .width(110.0)
                .show(ui);
            ui.label("Timeout (5..300) — wider");
        });
        let mut v3: i64 = 0;
        ui.horizontal(|ui| {
            NumberStepper::new(&mut v3, "fx-ns-3").range(0, 20).show(ui);
            ui.label("At min: − disabled");
        });
        let mut v4: i64 = 20;
        ui.horizontal(|ui| {
            NumberStepper::new(&mut v4, "fx-ns-4").range(0, 20).show(ui);
            ui.label("At max: + disabled");
        });
    }

    // ──────────────────────────────────────────────────────────────────
    // Properties dialog composites
    // ──────────────────────────────────────────────────────────────────

    pub fn props_section_card(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        ui.spacing_mut().item_spacing.y = space::S3 as f32;
        section(ui, "section_card — empty + populated");
        cp::section_card(ui, &t, |ui| {
            ui.label(
                RichText::new("Empty card body")
                    .color(t.fg_3)
                    .font(ts::sm()),
            );
        });
        cp::section_card(ui, &t, |ui| {
            ui.label(
                RichText::new("Heading")
                    .color(t.fg_1)
                    .font(theme::body_bold(13.0)),
            );
            ui.label(
                RichText::new("Body text under the heading.")
                    .color(t.fg_3)
                    .font(theme::body(11.0)),
            );
        });
    }

    pub fn props_kv_row(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        ui.spacing_mut().item_spacing.y = space::S2 as f32;
        section(ui, "kv_row — proportional values");
        cp::kv_row(ui, &t, "Name", "ubuntu-24.04-desktop-amd64.iso", false);
        cp::kv_row(ui, &t, "Type", "Compressed · .iso", false);
        cp::kv_row(ui, &t, "Started", "May 18", false);
        section(ui, "kv_row — mono values");
        cp::kv_row(ui, &t, "Size", "5.6 GB  (5_998_443_008 bytes)", true);
        cp::kv_row(ui, &t, "Server", "releases.ubuntu.com", true);
    }

    pub fn props_path_row(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        section(ui, "path_row");
        let _ = cp::path_row(ui, &t, "/Users/javad/Downloads/iso");
        ui.add_space(space::S2 as f32);
        let _ = cp::path_row(ui, &t, "/very/long/nested/path/to/somewhere/deep");
    }

    pub fn props_url_row(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        section(ui, "url_row — short + long");
        cp::url_row(ui, &t, "https://example.com/file.iso");
        ui.add_space(space::S2 as f32);
        cp::url_row(
            ui,
            &t,
            "https://releases.ubuntu.com/24.04.1/ubuntu-24.04.1-desktop-amd64.iso",
        );
    }

    pub fn props_lock_banner(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        section(ui, "lock_banner — main (warning)");
        cp::lock_banner(ui, &t);
        ui.add_space(space::S3 as f32);
        section(ui, "lock_banner_checksums — success-tinted");
        cp::lock_banner_checksums(ui, &t);
    }

    pub fn props_info_callout(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        section(ui, "info_callout");
        cp::info_callout(
            ui,
            &t,
            "oxdm will inherit the proxy configured in System Settings → Network → Proxies.",
        );
    }

    pub fn props_phase_pill(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        ui.spacing_mut().item_spacing.x = space::S3 as f32;
        section(ui, "phase_pill — every phase");
        let rows: &[(egui::Color32, &str)] = &[
            (t.action_primary, "DOWNLOADING"),
            (t.status_success, "COMPLETE"),
            (t.status_danger, "FAILED"),
            (t.fg_3, "CANCELLED"),
            (t.fg_3, "PAUSED"),
            (t.status_info, "QUEUED"),
        ];
        ui.horizontal_wrapped(|ui| {
            for (color, label) in rows {
                cp::phase_pill(ui, &t, *color, label);
            }
        });
    }

    pub fn props_status_banner(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        ui.spacing_mut().item_spacing.y = space::S3 as f32;
        section(
            ui,
            "status_banner — neutral / verified / mismatch / partial",
        );
        let _ = cp::status_banner(
            ui,
            &t,
            cp::BannerTone::Neutral,
            "shield",
            "No checksums on file",
            "Add a hash from the publisher's website to verify the file's integrity. MD5, SHA-1, SHA-256, SHA-384 and SHA-512 are supported.",
            None,
        );
        let _ = cp::status_banner(
            ui,
            &t,
            cp::BannerTone::Success,
            "shield",
            "All 2 hashes verified",
            "Every saved hash matches the locally-computed value for ubuntu-24.04.1-desktop-amd64.iso.",
            None,
        );
        let _ = cp::status_banner(
            ui,
            &t,
            cp::BannerTone::Danger,
            "shield",
            "1 mismatch — do not trust this file",
            "At least one saved hash does not match the actual contents on disk. The file may be corrupt, intercepted, or from a different version.",
            None,
        );
        let _ = cp::status_banner(
            ui,
            &t,
            cp::BannerTone::Partial,
            "shield",
            "1/3 verified · 2 pending",
            "Some hashes still need to be verified against the file on disk.",
            Some("Verify 2"),
        );
        let _ = cp::status_banner(
            ui,
            &t,
            cp::BannerTone::Danger,
            "shield",
            "1 mismatch — do not trust this file",
            "At least one saved hash does not match the actual contents on disk. The file may be corrupt, intercepted, or from a different version.",
            Some("Verify 1"),
        );
    }

    pub fn props_status_pill(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        use cp::{CsSource, CsStatus};
        ui.spacing_mut().item_spacing.y = space::S2 as f32;
        section(ui, "status_pill — every (status × source)");
        let rows: &[(CsStatus, CsSource, &str)] = &[
            (CsStatus::Verified, CsSource::Server, "verified · server"),
            (CsStatus::Mismatch, CsSource::User, "mismatch · user"),
            (
                CsStatus::Unverified,
                CsSource::Server,
                "unverified · server",
            ),
            (CsStatus::Unverified, CsSource::User, "unverified · user"),
            (
                CsStatus::Unverified,
                CsSource::Computed,
                "unverified · computed",
            ),
        ];
        for (st, src, lab) in rows {
            ui.horizontal(|ui| {
                cp::status_pill(ui, &t, *st, *src);
                ui.label(RichText::new(*lab).color(t.fg_3).font(ts::xs()));
            });
        }
    }

    pub fn props_checksum_row(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        use cp::{Algo, Checksum, CsSource, CsStatus};
        ui.spacing_mut().item_spacing.y = space::S2 as f32;
        section(
            ui,
            "checksum_list_header + rows in all five status combinations",
        );

        let sample =
            |algo: Algo, hash: &str, source: CsSource, status: CsStatus, expected: Option<&str>| {
                Checksum {
                    algo,
                    hash: hash.to_string(),
                    source,
                    status,
                    expected: expected.map(str::to_string),
                }
            };

        let rows = [
            sample(
                Algo::Sha256,
                "ab12cd34ef56789012345678abcdef0123456789abcdef0123456789ab345678",
                CsSource::Server,
                CsStatus::Verified,
                None,
            ),
            sample(
                Algo::Md5,
                "0123456789abcdef0123456789abcdef",
                CsSource::User,
                CsStatus::Mismatch,
                Some("ffeeddccbbaa99887766554433221100"),
            ),
            sample(
                Algo::Sha1,
                "1234567890abcdef1234567890abcdef12345678",
                CsSource::Server,
                CsStatus::Unverified,
                None,
            ),
            sample(
                Algo::Sha512,
                "deadbeefcafef00d1234567890abcdef0123456789abcdef0123456789abcdefdeadbeefcafef00d1234567890abcdef0123456789abcdef0123456789abcdef",
                CsSource::User,
                CsStatus::Unverified,
                None,
            ),
            sample(
                Algo::Sha384,
                "abc123def456789012345678901234567890abcdef0123456789abcdef012345abcdefabcdefabcdefabcdefabcd",
                CsSource::Computed,
                CsStatus::Unverified,
                None,
            ),
        ];

        egui::Frame::NONE
            .fill(t.bg_surface)
            .stroke(egui::Stroke::new(t.border_width, t.border_subtle))
            .corner_radius(theme::surface::RADIUS)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                cp::checksum_list_header(ui, &t);
                for (i, cs) in rows.iter().enumerate() {
                    if i > 0 {
                        let (rect, _) = ui.allocate_exact_size(
                            egui::Vec2::new(ui.available_width(), t.border_width),
                            egui::Sense::hover(),
                        );
                        ui.painter().rect_filled(rect, 0.0, t.border_subtle);
                    }
                    let _ = cp::checksum_row(ui, &t, cs, false);
                }
            });
        ui.add_space(space::S3 as f32);
        section(ui, "locked variant — trash disabled");
        egui::Frame::NONE
            .fill(t.bg_surface)
            .stroke(egui::Stroke::new(t.border_width, t.border_subtle))
            .corner_radius(theme::surface::RADIUS)
            .show(ui, |ui| {
                let _ = cp::checksum_row(ui, &t, &rows[2], true);
            });
    }

    pub fn props_mismatch_diff(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        section(ui, "mismatch_diff — EXPECTED / GOT strip");
        cp::section_card(ui, &t, |ui| {
            cp::mismatch_diff(
                ui,
                &t,
                "ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100",
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            );
        });
    }

    pub fn props_add_checksum(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        use cp::{AddChecksumState, Algo};
        ui.spacing_mut().item_spacing.y = space::S3 as f32;

        section(ui, "add_checksum_form — empty");
        let mut s1 = AddChecksumState::default();
        let _ = cp::add_checksum_form(ui, &t, &mut s1, &std::collections::HashSet::new());

        section(ui, "add_checksum_form — typing (short hex)");
        let mut s2 = AddChecksumState {
            algo: Algo::Sha256,
            hash: "abc123".to_string(),
            auto_detect: false,
        };
        let _ = cp::add_checksum_form(ui, &t, &mut s2, &std::collections::HashSet::new());

        section(ui, "add_checksum_form — valid SHA-256 (saveable)");
        let mut s3 = AddChecksumState {
            algo: Algo::Sha256,
            hash: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
            auto_detect: false,
        };
        let _ = cp::add_checksum_form(ui, &t, &mut s3, &std::collections::HashSet::new());

        section(ui, "add_checksum_form — duplicate algorithm");
        let mut s4 = AddChecksumState {
            algo: Algo::Md5,
            hash: "0123456789abcdef0123456789abcdef".to_string(),
            auto_detect: false,
        };
        let mut existing = std::collections::HashSet::new();
        existing.insert(Algo::Md5);
        let _ = cp::add_checksum_form(ui, &t, &mut s4, &existing);
    }

    pub fn props_speed_limit(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        ui.spacing_mut().item_spacing.y = space::S4 as f32;
        section(ui, "speed_limit_control — unlimited");
        let mut kbps1 = 0i64;
        let mut mb1 = false;
        ui.horizontal(|ui| {
            cp::speed_limit_control_with_id(ui, &t, &mut kbps1, &mut mb1, "fx-speed-1");
        });
        section(ui, "speed_limit_control — limited 512 KB/s");
        let mut kbps2 = 512i64;
        let mut mb2 = false;
        ui.horizontal(|ui| {
            cp::speed_limit_control_with_id(ui, &t, &mut kbps2, &mut mb2, "fx-speed-2");
        });
        section(ui, "speed_limit_control — limited 10 MB/s");
        let mut kbps3 = 10 * 1024i64;
        let mut mb3 = true;
        ui.horizontal(|ui| {
            cp::speed_limit_control_with_id(ui, &t, &mut kbps3, &mut mb3, "fx-speed-3");
        });
    }

    /// Local header-row impl for the fixture only — avoids pulling the
    /// full `Advanced` model in just to populate one widget.
    struct FxHeader {
        name: String,
        value: String,
    }
    impl cp::HeaderRow for FxHeader {
        fn name(&mut self) -> &mut String {
            &mut self.name
        }
        fn value(&mut self) -> &mut String {
            &mut self.value
        }
    }

    pub fn props_header_editor(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        ui.spacing_mut().item_spacing.y = space::S3 as f32;
        section(ui, "header_editor — empty (placeholder copy)");
        let mut empty: Vec<FxHeader> = Vec::new();
        let _ = cp::header_editor(ui, &t, &mut empty);

        section(ui, "header_editor — three rows");
        let mut rows = vec![
            FxHeader {
                name: "Authorization".to_string(),
                value: "Bearer eyJhbGciOiJIUzI1NiJ9".to_string(),
            },
            FxHeader {
                name: "X-Forwarded-For".to_string(),
                value: "203.0.113.42".to_string(),
            },
            FxHeader {
                name: "".to_string(),
                value: "".to_string(),
            },
        ];
        let _ = cp::header_editor(ui, &t, &mut rows);
    }

    pub fn props_captured_request(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        ui.spacing_mut().item_spacing.y = space::S1 as f32;
        section(ui, "captured request");
        cp::captured_table(
            ui,
            &t,
            &[
                ("User-Agent", "oxdm/2.4.1 (Macintosh; arm64; like wget)"),
                ("Accept", "*/*"),
                ("Accept-Encoding", "identity"),
                ("Range", "bytes=5368709120-"),
                ("Connection", "keep-alive"),
                ("Host", "releases.ubuntu.com"),
            ],
        );
    }

    pub fn props_captured_response(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        ui.spacing_mut().item_spacing.y = space::S1 as f32;
        section(ui, "captured response");
        cp::captured_table(
            ui,
            &t,
            &[
                ("HTTP/2", "206 Partial Content"),
                ("Content-Type", "application/octet-stream"),
                ("Content-Length", "0"),
                ("Content-Range", "bytes 5368709120-5368709119/5368709120"),
                ("Accept-Ranges", "bytes"),
                ("ETag", "\"a3f9b21e7c4d-2gse2gw\""),
                ("Last-Modified", "Wed, 30 Apr 2026 14:12:08 GMT"),
                ("Server", "nginx/1.27"),
                ("Cache-Control", "public, max-age=604800"),
            ],
        );
    }

    pub fn props_captured_kv(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        ui.spacing_mut().item_spacing.y = space::S1 as f32;
        section(ui, "captured_kv — request headers");
        cp::section_card(ui, &t, |ui| {
            cp::captured_kv(
                ui,
                &t,
                "User-Agent",
                "oxdm/2.4.1 (Macintosh; arm64; like wget)",
            );
            cp::captured_kv(ui, &t, "Accept", "*/*");
            cp::captured_kv(ui, &t, "Accept-Encoding", "identity");
            cp::captured_kv(ui, &t, "Range", "bytes=4096-");
            cp::captured_kv(ui, &t, "Connection", "keep-alive");
            cp::captured_kv(ui, &t, "Host", "releases.ubuntu.com");
        });
    }

    pub fn props_cookie_chips(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        ui.spacing_mut().item_spacing.y = space::S3 as f32;
        section(ui, "cookie_chip_strip — empty");
        cp::cookie_chip_strip(ui, &t, &[]);
        section(ui, "cookie_chip_strip — three cookies");
        let three = vec![
            ("session".to_string(), "abc123def456ghi789".to_string()),
            (
                "__cf_bm".to_string(),
                "qwertyuiopasdfghjkl1234567890".to_string(),
            ),
            ("locale".to_string(), "en-US".to_string()),
        ];
        cp::cookie_chip_strip(ui, &t, &three);
        section(ui, "cookie_chip_strip — eight (overflow)");
        let many: Vec<(String, String)> = (0..8)
            .map(|i| (format!("k{i}"), format!("v{i}xxxxxxx{i}")))
            .collect();
        cp::cookie_chip_strip(ui, &t, &many);
    }

    // helpers
    fn section(ui: &mut egui::Ui, title: &str) {
        let t = theme::tokens(ui.ctx());
        ui.add_space(space::S2 as f32);
        let upper: String = title.to_uppercase();
        let mut job = egui::text::LayoutJob::default();
        job.append(
            &upper,
            0.0,
            egui::TextFormat {
                font_id: ts::eyebrow(),
                color: t.fg_3,
                extra_letter_spacing: 1.4,
                ..Default::default()
            },
        );
        ui.label(job);
        ui.add_space(space::S1 as f32);
    }

    // ────────────────────────────────────────────────────────────────
    // Properties dialog — assembled tab fixtures
    // ────────────────────────────────────────────────────────────────

    use oxdm::domain::{
        Advanced, AuthScheme, Category, CustomHeader, Job, JobId, JobStatus, Phase, ProxyMode,
    };
    use oxdm::ipc_local::protocol::JobCounters;
    use oxdm::ui::dialogs::properties::{
        self as props, Checksum, CsSource, CsStatus, PropertiesState,
    };
    use std::path::PathBuf;

    fn demo_job() -> Job {
        Job {
            id: JobId::default(),
            url: url::Url::parse("https://cdn.gopro.com/GoPro_Hero12_FieldTest_RAW_2026.mp4")
                .unwrap(),
            save_dir: PathBuf::from("/home/user/Downloads/Videos"),
            filename: Some("GoPro_Hero12_FieldTest_RAW_2026.mp4".into()),
            referrer: None,
            headers: Default::default(),
            max_connections: None,
            proxy: None,
            auth_user: None,
            enc_auth_password: None,
            enc_proxy_password: None,
            enc_cookies: None,
            speed_limit_override: None,
            queue_id: Default::default(),
            created_at: chrono::DateTime::parse_from_rfc3339("2026-05-20T10:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            status: JobStatus::default(),
            advanced: Advanced::default(),
            checksums: Vec::new(),
            category: Category::Videos,
        }
    }

    fn demo_counters() -> JobCounters {
        JobCounters {
            id: JobId::default(),
            phase: Phase::Downloading,
            downloaded: 1_258_291_200,
            total: Some(2_516_582_400),
            speed_bps: 0.0,
            is_resumable: 1,
            running: true,
            parts: Vec::new(),
        }
    }

    fn demo_cats() -> indexmap::IndexMap<Category, Vec<String>> {
        let mut m = indexmap::IndexMap::new();
        m.insert(
            Category::Videos,
            vec!["mp4".into(), "mkv".into(), "mov".into()],
        );
        m
    }

    fn demo_state() -> PropertiesState {
        let mut s = PropertiesState::new(JobId::default());
        // Populate the source fields so the General-tab fixture shows the
        // URL / Save-to inputs filled (mirrors a real hydrated dialog).
        s.current.url = "https://cdn.gopro.com/GoPro_Hero12_FieldTest_RAW_2026.mp4".to_string();
        s.current.save_path =
            "/home/javad/Downloads/GoPro_Hero12_FieldTest_RAW_2026.mp4".to_string();
        s.original = Some(s.current.clone());
        s
    }

    fn tab_scaffold(ui: &mut egui::Ui, render: impl FnOnce(&mut egui::Ui)) {
        let t = theme::tokens(ui.ctx());
        // Mirror the real dialog's content shell exactly: a `bg_page`
        // fill, a vertical `ScrollArea` with `auto_shrink([false; 2])`,
        // then the padded inner frame. The ScrollArea matters — it drives
        // the width that flex/grow cells measure against, and omitting it
        // here previously hid a layout hang (egui_flex `request_discard`
        // loop) that only manifests inside a scroll viewport. Keep in sync
        // with `properties.rs`'s CentralPanel block.
        egui::Frame::NONE.fill(t.bg_page).show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = space::S3 as f32;
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::symmetric(space::S4, space::S3))
                        .show(ui, |ui| {
                            render(ui);
                        });
                });
        });
    }

    pub fn props_tab_general(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        let mut s = demo_state();
        let job = demo_job();
        let counters = demo_counters();
        let cats = demo_cats();
        tab_scaffold(ui, |ui| {
            props::general_tab(ui, &t, &mut s, &cats, &job, &counters, Phase::Downloading);
        });
    }

    pub fn props_tab_general_with_checksum(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        let mut s = demo_state();
        s.current.checksums = vec![Checksum {
            algo: cp::Algo::Sha256,
            hash: "9b74c9897bac770ffc029102a200c5de".repeat(2),
            source: CsSource::User,
            status: CsStatus::Verified,
            expected: None,
        }];
        let job = demo_job();
        let counters = demo_counters();
        let cats = demo_cats();
        tab_scaffold(ui, |ui| {
            props::general_tab(ui, &t, &mut s, &cats, &job, &counters, Phase::Downloading);
        });
    }

    fn cs(algo: cp::Algo, status: CsStatus, source: CsSource, expected: Option<&str>) -> Checksum {
        let hash: String = std::iter::repeat('a').take(algo.hex_len()).collect();
        Checksum {
            algo,
            hash,
            source,
            status,
            expected: expected.map(|s| {
                let mut v: String = s.into();
                while v.len() < algo.hex_len() {
                    v.push('b');
                }
                v.truncate(algo.hex_len());
                v
            }),
        }
    }

    pub fn props_tab_checksum_empty(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        let mut s = demo_state();
        tab_scaffold(ui, |ui| {
            props::checksums_tab(ui, &t, &mut s, false, "GoPro_Hero12_FieldTest_RAW_2026.mp4");
        });
    }

    pub fn props_tab_checksum_verified(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        let mut s = demo_state();
        s.current.checksums = vec![
            cs(cp::Algo::Sha256, CsStatus::Verified, CsSource::User, None),
            cs(cp::Algo::Sha512, CsStatus::Verified, CsSource::Server, None),
        ];
        tab_scaffold(ui, |ui| {
            props::checksums_tab(ui, &t, &mut s, false, "GoPro_Hero12_FieldTest_RAW_2026.mp4");
        });
    }

    pub fn props_tab_checksum_mixed(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        let mut s = demo_state();
        s.current.checksums = vec![
            cs(cp::Algo::Sha256, CsStatus::Verified, CsSource::User, None),
            cs(cp::Algo::Sha1, CsStatus::Unverified, CsSource::User, None),
            cs(cp::Algo::Md5, CsStatus::Verified, CsSource::Server, None),
        ];
        tab_scaffold(ui, |ui| {
            props::checksums_tab(ui, &t, &mut s, false, "GoPro_Hero12_FieldTest_RAW_2026.mp4");
        });
    }

    pub fn props_tab_checksum_mismatch(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        let mut s = demo_state();
        s.current.checksums = vec![
            cs(
                cp::Algo::Sha256,
                CsStatus::Mismatch,
                CsSource::User,
                Some("c0ffee"),
            ),
            cs(cp::Algo::Sha1, CsStatus::Verified, CsSource::Server, None),
        ];
        tab_scaffold(ui, |ui| {
            props::checksums_tab(ui, &t, &mut s, false, "GoPro_Hero12_FieldTest_RAW_2026.mp4");
        });
    }

    pub fn props_tab_checksum_add(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        let mut s = demo_state();
        s.adding = true;
        s.add.algo = cp::Algo::Sha256;
        s.add.hash.clear();
        s.add.auto_detect = true;
        tab_scaffold(ui, |ui| {
            props::checksums_tab(ui, &t, &mut s, false, "GoPro_Hero12_FieldTest_RAW_2026.mp4");
        });
    }

    pub fn props_tab_connection(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        let mut s = demo_state();
        tab_scaffold(ui, |ui| props::connection_tab(ui, &t, &mut s));
    }

    pub fn props_tab_connection_http_proxy(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        let mut s = demo_state();
        s.current.adv.proxy.mode = ProxyMode::Http;
        s.current.adv.proxy.host = "proxy.lan".into();
        s.current.adv.proxy.port = "8080".into();
        s.current.adv.proxy.auth_enabled = true;
        s.current.adv.proxy.username = "alice".into();
        s.current.adv.proxy.password = "hunter2".into();
        tab_scaffold(ui, |ui| props::connection_tab(ui, &t, &mut s));
    }

    pub fn props_tab_connection_socks_proxy(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        let mut s = demo_state();
        s.current.adv.proxy.mode = ProxyMode::Socks5;
        s.current.adv.proxy.host = "socks.lan".into();
        s.current.adv.proxy.port = "1080".into();
        tab_scaffold(ui, |ui| props::connection_tab(ui, &t, &mut s));
    }

    pub fn props_tab_connection_system_proxy(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        let mut s = demo_state();
        s.current.adv.proxy.mode = ProxyMode::System;
        tab_scaffold(ui, |ui| props::connection_tab(ui, &t, &mut s));
    }

    pub fn props_tab_connection_auth_basic(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        let mut s = demo_state();
        s.current.adv.auth.scheme = AuthScheme::Basic;
        s.current.adv.auth.username = "alice".into();
        s.current.adv.auth.password = "secret".into();
        tab_scaffold(ui, |ui| props::connection_tab(ui, &t, &mut s));
    }

    pub fn props_tab_connection_auth_bearer(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        let mut s = demo_state();
        s.current.adv.auth.scheme = AuthScheme::Bearer;
        s.current.adv.auth.token = "eyJhbGciOi...".into();
        tab_scaffold(ui, |ui| props::connection_tab(ui, &t, &mut s));
    }

    pub fn props_tab_connection_auth_digest(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        let mut s = demo_state();
        s.current.adv.auth.scheme = AuthScheme::Digest;
        s.current.adv.auth.username = "alice".into();
        s.current.adv.auth.password = "secret".into();
        tab_scaffold(ui, |ui| props::connection_tab(ui, &t, &mut s));
    }

    pub fn props_tab_cookies_disabled(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        let mut s = demo_state();
        s.current.adv.cookies_enabled = false;
        tab_scaffold(ui, |ui| props::cookies_tab(ui, &t, &mut s));
    }

    pub fn props_tab_cookies_enabled(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        let mut s = demo_state();
        s.current.adv.cookies_enabled = true;
        s.current.adv.cookie_jar = "sessionid=abc123; csrftoken=xyz789; theme=dark".into();
        tab_scaffold(ui, |ui| props::cookies_tab(ui, &t, &mut s));
    }

    pub fn props_tab_headers(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        let mut s = demo_state();
        s.current.adv.headers = vec![
            CustomHeader {
                name: "X-Api-Key".into(),
                value: "abc123def456".into(),
            },
            CustomHeader {
                name: "Origin".into(),
                value: "https://app.example.com".into(),
            },
        ];
        let job = demo_job();
        let counters = demo_counters();
        tab_scaffold(ui, |ui| props::headers_tab(ui, &t, &mut s, &job, &counters));
    }

    pub fn props_tab_headers_add_header(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        let mut s = demo_state();
        s.current.adv.headers = vec![
            CustomHeader {
                name: "X-Api-Key".into(),
                value: "abc123def456".into(),
            },
            CustomHeader {
                name: String::new(),
                value: String::new(),
            },
        ];
        let job = demo_job();
        let counters = demo_counters();
        tab_scaffold(ui, |ui| props::headers_tab(ui, &t, &mut s, &job, &counters));
    }

    pub fn props_tab_advanced_top(ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        let mut s = demo_state();
        tab_scaffold(ui, |ui| props::advanced_tab(ui, &t, &mut s));
    }

    fn swatch_row(ui: &mut egui::Ui, items: &[(&str, Color32)]) {
        let t = theme::tokens(ui.ctx());
        ui.horizontal_wrapped(|ui| {
            for (name, color) in items {
                let sz = Vec2::new(72.0, 56.0);
                let (rect, _) = ui.allocate_exact_size(sz, egui::Sense::hover());
                let r: egui::CornerRadius = radius::SM.into();
                ui.painter().rect_filled(rect, r, *color);
                ui.painter().rect_stroke(
                    rect,
                    r,
                    egui::Stroke::new(t.border_width_hairline, t.border_subtle),
                    egui::StrokeKind::Inside,
                );
                let label_pos = rect.left_bottom() + Vec2::new(0.0, 4.0);
                ui.painter()
                    .text(label_pos, Align2::LEFT_TOP, *name, ts::xs(), t.fg_2);
            }
        });
    }
}

// ──────────────────────────────────────────────────────────────────────
// Entrypoint
// ──────────────────────────────────────────────────────────────────────

fn main() -> ExitCode {
    let opts = match parse_args() {
        Ok(Some(o)) => o,
        Ok(None) => return ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };

    let fixture = FIXTURES
        .iter()
        .find(|(n, _)| *n == opts.fixture.as_str())
        .map(|(_, f)| *f);
    let Some(fixture) = fixture else {
        eprintln!("unknown fixture: {} (try --list)", opts.fixture);
        return ExitCode::from(2);
    };

    let settings = Settings {
        theme: opts.theme,
        ..Settings::default()
    };

    let app = FixtureApp {
        fixture,
        snap: opts.snap.clone(),
        snap_requested: Arc::new(AtomicBool::new(false)),
        frames_rendered: 0,
        settings,
    };

    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([opts.width, opts.height])
            .with_title(format!("oxdm-fixture · {}", opts.fixture)),
        ..Default::default()
    };

    if let Err(e) = eframe::run_native(
        "oxdm-fixture",
        native,
        Box::new(move |cc| {
            theme::install_fonts(&cc.egui_ctx);
            icons::install_loaders(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    ) {
        eprintln!("eframe error: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
