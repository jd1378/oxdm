//! Design system: tokens (colors, spacing, radii, fonts) ported from
//! `design/handoff/tokens.json`, applied to egui `Visuals` and stored
//! in `ctx.data` for widgets to read via [`tokens`].
//!
//! Three palette themes: **utility** (cool gray + clay accent),
//! **warm** (cream + earth), and **dark** (matte near-black). All
//! share the same spacing / radius / type / motion scales; only
//! `color.*` and `shadow.*` swap. `Theme::Light` maps to utility.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use eframe::egui::{self, Color32, FontFamily, FontId, Stroke, Visuals};

use crate::domain::{Settings, Theme};
use crate::ui::color::{clay, earth, gray, moss, ochre, rust, slate};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ResolvedTheme {
    Light = 0,
    Dark = 1,
    Warm = 2,
}

pub fn resolve(theme: Theme) -> ResolvedTheme {
    match theme {
        Theme::Light => ResolvedTheme::Light,
        Theme::Dark => ResolvedTheme::Dark,
        Theme::Warm => ResolvedTheme::Warm,
        Theme::System => system_theme(),
    }
}

static SYSTEM_THEME: AtomicU8 = AtomicU8::new(u8::MAX);
type Listener = Box<dyn Fn(ResolvedTheme) + Send + Sync>;
static LISTENERS: OnceLock<Mutex<Vec<Listener>>> = OnceLock::new();
static POLL_STARTED: OnceLock<()> = OnceLock::new();

fn detect_once() -> ResolvedTheme {
    let detected = if tokio::runtime::Handle::try_current().is_ok() {
        dark_light::detect()
    } else {
        match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt.block_on(async { dark_light::detect() }),
            Err(_) => return ResolvedTheme::Dark,
        }
    };
    match detected {
        Ok(dark_light::Mode::Light) => ResolvedTheme::Light,
        Ok(dark_light::Mode::Dark) => ResolvedTheme::Dark,
        _ => ResolvedTheme::Dark,
    }
}

fn theme_from_u8(v: u8) -> ResolvedTheme {
    if v == ResolvedTheme::Light as u8 {
        ResolvedTheme::Light
    } else if v == ResolvedTheme::Warm as u8 {
        ResolvedTheme::Warm
    } else {
        ResolvedTheme::Dark
    }
}

fn ensure_poller() {
    POLL_STARTED.get_or_init(|| {
        let init = detect_once();
        SYSTEM_THEME.store(init as u8, Ordering::Relaxed);
        let _ = std::thread::Builder::new()
            .name("oxdm-theme-poll".into())
            .spawn(|| {
                loop {
                    std::thread::sleep(Duration::from_secs(2));
                    let cur = detect_once();
                    let prev = theme_from_u8(SYSTEM_THEME.swap(cur as u8, Ordering::Relaxed));
                    if prev != cur
                        && let Some(lock) = LISTENERS.get()
                    {
                        let listeners = lock.lock().unwrap();
                        for f in listeners.iter() {
                            f(cur);
                        }
                    }
                }
            });
    });
}

/// Current OS-level light/dark preference. Polled in background; cheap.
pub fn system_theme() -> ResolvedTheme {
    ensure_poller();
    let v = SYSTEM_THEME.load(Ordering::Relaxed);
    if v == u8::MAX {
        let cur = detect_once();
        SYSTEM_THEME.store(cur as u8, Ordering::Relaxed);
        cur
    } else {
        theme_from_u8(v)
    }
}

/// Register a callback fired on a background thread when the OS-level
/// light/dark preference changes.
pub fn on_system_theme_change<F>(cb: F)
where
    F: Fn(ResolvedTheme) + Send + Sync + 'static,
{
    ensure_poller();
    let lock = LISTENERS.get_or_init(|| Mutex::new(Vec::new()));
    lock.lock().unwrap().push(Box::new(cb));
}

#[derive(Debug, Clone, Copy)]
pub struct Tokens {
    pub theme: ResolvedTheme,

    pub fg_1: Color32,
    pub fg_2: Color32,
    pub fg_3: Color32,
    pub fg_4: Color32,
    pub fg_inverse: Color32,

    pub bg_page: Color32,
    pub bg_surface: Color32,
    pub bg_sunken: Color32,
    pub bg_raised: Color32,
    pub bg_inverse: Color32,
    pub bg_sidebar: Color32,
    pub bg_titlebar: Color32,

    pub border_subtle: Color32,
    pub border_default: Color32,
    pub border_strong: Color32,
    pub border_brand: Color32,

    pub action_primary: Color32,
    pub action_primary_press: Color32,
    pub action_primary_fg: Color32,
    pub action_primary_shadow: Color32,

    pub pill_active_bg: Color32,
    pub pill_active_fg: Color32,

    pub status_success: Color32,
    pub status_success_bg: Color32,
    pub status_warning: Color32,
    pub status_warning_bg: Color32,
    pub status_danger: Color32,
    pub status_danger_bg: Color32,
    pub status_info: Color32,
    pub status_info_bg: Color32,

    pub progress_track: Color32,
    pub progress_fill: Color32,

    pub cat_compressed: Color32,
    pub cat_programs: Color32,
    pub cat_videos: Color32,
    pub cat_music: Color32,
    pub cat_pictures: Color32,
    pub cat_documents: Color32,

    pub border_width: f32,
    pub border_width_thick: f32,
    pub border_width_hairline: f32,

    pub focus_ring: Color32,
    pub focus_ring_gap: Color32,
    pub row_selected_bg: Color32,
    pub row_hover_bg: Color32,
    pub row_selhover_bg: Color32,
    pub bg_sunken_hover: Color32,
    pub bg_surface_hover: Color32,
}

impl Tokens {
    /// Light theme — sources from `theme-utility` in
    /// `design/tokens.css`: cool neutral grays with the clay accent.
    pub fn light() -> Self {
        Self {
            theme: ResolvedTheme::Light,
            fg_1: gray::G800,
            fg_2: gray::G500,
            fg_3: gray::G400,
            fg_4: gray::G300,
            fg_inverse: gray::G50,
            bg_page: gray::G50,
            bg_surface: Color32::WHITE,
            bg_sunken: gray::G100,
            bg_raised: Color32::WHITE,
            bg_inverse: gray::G800,
            bg_sidebar: gray::G100,
            bg_titlebar: gray::G100,
            border_subtle: gray::G200,
            border_default: gray::G300,
            border_strong: gray::G700,
            border_brand: clay::C400,
            action_primary: clay::C400,
            action_primary_press: clay::C500,
            action_primary_fg: Color32::WHITE,
            action_primary_shadow: clay::C600,
            pill_active_bg: clay::C100,
            pill_active_fg: clay::C700,
            status_success: moss::M500,
            status_success_bg: moss::M50,
            status_warning: ochre::O400,
            status_warning_bg: ochre::O50,
            status_danger: rust::R300,
            status_danger_bg: rust::R50,
            status_info: slate::S300,
            status_info_bg: slate::S50,
            progress_track: gray::G200,
            progress_fill: clay::C400,
            cat_compressed: clay::C400,
            cat_programs: slate::S300,
            cat_videos: clay::C500,
            cat_music: moss::M400,
            cat_pictures: ochre::O300,
            cat_documents: slate::S300,
            border_width: 1.0,
            border_width_thick: 3.0,
            border_width_hairline: 1.0,
            focus_ring: gray::G800,
            focus_ring_gap: Color32::WHITE,
            row_selected_bg: clay::C50,
            row_hover_bg: gray::G100,
            row_selhover_bg: clay::C100,
            bg_sunken_hover: hex(0xDCDAD5),
            bg_surface_hover: hex(0xF0EFEC),
        }
    }

    /// Warm cream + earth palette. From `tokens.json::color.warm`.
    pub fn warm() -> Self {
        Self {
            theme: ResolvedTheme::Warm,
            fg_1: earth::E800,
            fg_2: earth::E600,
            fg_3: hex(0x8A8278),
            fg_4: hex(0xB0A89C),
            fg_inverse: earth::E50,
            bg_page: hex(0xF5F1E9),
            bg_surface: hex(0xFAF7F0),
            bg_sunken: hex(0xEBE6DB),
            bg_raised: hex(0xFFFDF7),
            bg_inverse: earth::E800,
            bg_sidebar: hex(0xEBE6DB),
            bg_titlebar: hex(0xEBE6DB),
            border_subtle: earth::E200,
            border_default: earth::E300,
            border_strong: earth::E700,
            border_brand: clay::C400,
            action_primary: clay::C400,
            action_primary_press: clay::C500,
            action_primary_fg: hex(0xFFFDF7),
            action_primary_shadow: clay::C600,
            pill_active_bg: clay::C100,
            pill_active_fg: clay::C700,
            status_success: moss::M400,
            status_success_bg: moss::M50,
            status_warning: ochre::O300,
            status_warning_bg: ochre::O50,
            status_danger: rust::R300,
            status_danger_bg: rust::R50,
            status_info: slate::S300,
            status_info_bg: slate::S50,
            progress_track: earth::E200,
            progress_fill: clay::C400,
            cat_compressed: clay::C400,
            cat_programs: slate::S300,
            cat_videos: clay::C500,
            cat_music: moss::M400,
            cat_pictures: ochre::O300,
            cat_documents: slate::S300,
            border_width: 1.0,
            border_width_thick: 3.0,
            border_width_hairline: 1.0,
            focus_ring: earth::E700,
            focus_ring_gap: hex(0xFAF7F0),
            row_selected_bg: clay::C50,
            row_hover_bg: hex(0xEBE6DB),
            row_selhover_bg: clay::C100,
            bg_sunken_hover: hex(0xDFD8C8),
            bg_surface_hover: hex(0xF1ECE0),
        }
    }

    pub fn dark() -> Self {
        // Dark-mode clay remap (per tokens.css): clay-50/100/200 become
        // dark warm tints, clay-700 inverts to light clay for paired text.
        Self {
            theme: ResolvedTheme::Dark,
            fg_1: hex(0xF0EEE8),
            fg_2: hex(0xB8B5AC),
            fg_3: hex(0x8A8780),
            fg_4: hex(0x5A5852),
            fg_inverse: earth::E900,
            bg_page: hex(0x1F1E1B),
            bg_surface: hex(0x282723),
            bg_sunken: hex(0x181715),
            bg_raised: hex(0x2F2E2A),
            bg_inverse: hex(0xF0EEE8),
            bg_sidebar: hex(0x181715),
            bg_titlebar: hex(0x181715),
            border_subtle: hex(0x2F2E2A),
            border_default: hex(0x3A3935),
            border_strong: hex(0x5A5852),
            border_brand: clay::C300,
            action_primary: clay::C300,
            action_primary_press: clay::C200,
            action_primary_fg: hex(0x181715),
            action_primary_shadow: clay::C500,
            pill_active_bg: clay::DARK_C100,
            pill_active_fg: clay::DARK_C700,
            status_success: moss::M300,
            status_success_bg: hex(0x2A2E20),
            status_warning: ochre::O300,
            status_warning_bg: hex(0x2C2618),
            status_danger: rust::R200,
            status_danger_bg: hex(0x2C1E18),
            status_info: slate::S200,
            status_info_bg: hex(0x1F222A),
            progress_track: hex(0x3A3935),
            progress_fill: clay::C300,
            cat_compressed: clay::C300,
            cat_programs: slate::S200,
            cat_videos: clay::C300,
            cat_music: moss::M300,
            cat_pictures: ochre::O200,
            cat_documents: slate::S200,
            border_width: 1.0,
            border_width_thick: 3.0,
            border_width_hairline: 1.0,
            focus_ring: hex(0xF0EEE8),
            focus_ring_gap: hex(0x282723),
            row_selected_bg: clay::DARK_C50,
            row_hover_bg: hex(0x181715),
            row_selhover_bg: clay::DARK_C100,
            bg_sunken_hover: hex(0x222119),
            bg_surface_hover: hex(0x302F2B),
        }
    }
}

const fn hex(c: u32) -> Color32 {
    let r = ((c >> 16) & 0xFF) as u8;
    let g = ((c >> 8) & 0xFF) as u8;
    let b = (c & 0xFF) as u8;
    Color32::from_rgb(r, g, b)
}

pub mod space {
    pub const S0: i8 = 2;
    pub const S1: i8 = 4;
    pub const S2: i8 = 8;
    pub const S3: i8 = 12;
    pub const S4: i8 = 16;
    pub const S5: i8 = 20;
    pub const S6: i8 = 24;
    pub const S7: i8 = 32;
    pub const S8: i8 = 40;
    pub const S9: i8 = 48;
    pub const S10: i8 = 64;
    pub const ROW_MIN: f32 = 56.0;
    pub const TAP_MIN: f32 = 44.0;
}

pub mod radius {
    pub const XS: u8 = 6;
    pub const SM: u8 = 10;
    pub const MD: u8 = 12;
    pub const LG: u8 = 16;
    pub const XL: u8 = 22;
    pub const XXL: u8 = 28;
    pub const PILL: u8 = 255;
}

/// Standard padding presets. Names match `tokens.json::padding`.
#[allow(dead_code)]
pub mod pad {
    use eframe::egui::Vec2;
    use eframe::egui::vec2;
    pub const BTN: Vec2 = vec2(12.0, 6.0);
    pub const BTN_COMPACT: Vec2 = vec2(8.0, 4.0);
    pub const BTN_LARGE: Vec2 = vec2(16.0, 10.0);
    pub const ICON_BUTTON_SM: Vec2 = vec2(2.0, 2.0);
    pub const INPUT: Vec2 = vec2(10.0, 8.0);
    pub const ROW: Vec2 = vec2(10.0, 8.0);
    pub const DIALOG: Vec2 = vec2(16.0, 14.0);
    pub const DIALOG_FOOTER: Vec2 = vec2(14.0, 10.0);
    pub const SECTION: Vec2 = vec2(16.0, 12.0);
    pub const CARD: Vec2 = vec2(12.0, 12.0);
    pub const CHIP: Vec2 = vec2(8.0, 4.0);
    pub const TOOLTIP: Vec2 = vec2(8.0, 5.0);
}

/// Animation duration tokens (seconds — egui uses `f32` seconds in
/// `Context::animate_value_with_time`). Source: tokens.json::motion.
///
/// Easing: egui's `animate_value_with_time` is linear. Wrap its
/// output through [`ease_out`] for the design's
/// `cubic-bezier(0.22, 1, 0.36, 1)` curve.
///
/// Reduce-motion: callers honouring [`Settings::reduce_motion`]
/// should bypass animation entirely (use the target value directly)
/// per `16_animations.md §4`.
#[allow(dead_code)]
pub mod motion {
    use eframe::egui::Color32;
    pub const FAST: f32 = 0.14;
    pub const BASE: f32 = 0.22;
    pub const SLOW: f32 = 0.36;

    /// Approximation of CSS `cubic-bezier(0.22, 1, 0.36, 1)`. Input
    /// clamped to `[0, 1]`. Equivalent to `1 - (1 - t)^3`.
    pub fn ease_out(t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        let inv = 1.0 - t;
        1.0 - inv * inv * inv
    }

    /// Linear color blend. `t=0` returns `a`, `t=1` returns `b`.
    pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
        let t = t.clamp(0.0, 1.0);
        let l = |x: u8, y: u8| ((x as f32) * (1.0 - t) + (y as f32) * t) as u8;
        Color32::from_rgba_unmultiplied(
            l(a.r(), b.r()),
            l(a.g(), b.g()),
            l(a.b(), b.b()),
            l(a.a(), b.a()),
        )
    }

    /// Sinusoidal pulse opacity for "live" indicators. `time` is
    /// `ui.input(|i| i.time)`; `period_s` matches `16_animations.md`
    /// (1.4s for download/tab live dots). Returns alpha in `[0.35,1.0]`
    /// matching the keyframe `0%/100% { 1 }, 50% { 0.35 }`.
    pub fn pulse_alpha(time: f64, period_s: f32) -> f32 {
        let phase = (time as f32 % period_s) / period_s;
        let cos = (phase * std::f32::consts::TAU).cos();
        // cos: [1,-1] → [1.0, 0.35]
        0.675 + 0.325 * cos
    }
}

/// z-layer constants. egui has no true z-index; these are used to
/// pick `egui::Order` and as relative ordering when stacking
/// `Area`s. Source: tokens.json::z_layers.
#[allow(dead_code)]
pub mod z {
    pub const BASE: i32 = 0;
    pub const TOOLBAR: i32 = 5;
    pub const SIDEBAR: i32 = 5;
    pub const TAB_STRIP: i32 = 5;
    pub const CONTEXT_MENU: i32 = 30;
    pub const DROPDOWN: i32 = 30;
    pub const TRAY_MENU: i32 = 35;
    pub const DIALOG_BACKDROP: i32 = 38;
    pub const DIALOG: i32 = 40;
    pub const CONFIRM_DIALOG: i32 = 50;
    pub const TOAST: i32 = 60;
    pub const TOOLTIP: i32 = 70;
}

/// Fixed component sizes. Source: tokens.json::size.
#[allow(dead_code)]
pub mod size {
    pub const ICON_XS: f32 = 10.0;
    pub const ICON_SM: f32 = 12.0;
    pub const ICON_MD: f32 = 14.0;
    pub const ICON_LG: f32 = 16.0;
    pub const ICON_XL: f32 = 20.0;
    pub const TITLEBAR_H: f32 = 32.0;
    pub const TOOLBAR_H: f32 = 44.0;
    pub const STATUSBAR_H: f32 = 28.0;
    pub const TAB_H: f32 = 36.0;
    pub const ROW_ACTIVE_H: f32 = 60.0;
    pub const ROW_DONE_H: f32 = 48.0;
    pub const SIDEBAR_W: f32 = 220.0;
    pub const SCROLLBAR_W: f32 = 10.0;
    pub const TRAFFIC_DOT: f32 = 12.0;
    pub const DIALOG_MIN_W: f32 = 360.0;
    pub const DIALOG_MAX_W: f32 = 760.0;
    pub const TRAY_W: f32 = 360.0;
    pub const TOOLTIP_MAX_W: f32 = 280.0;
    pub const TOAST_W: f32 = 360.0;
}

/// Opacity multipliers. Source: tokens.json::opacity.
#[allow(dead_code)]
pub mod opacity {
    pub const DISABLED_BG: f32 = 0.5;
    pub const DISABLED_FG: f32 = 0.7;
    pub const SCRIM: f32 = 0.45;
    pub const OVERLAY_LIGHT: f32 = 0.08;
    pub const OVERLAY_STRONG: f32 = 0.18;
}

/// Back-compat re-export. Prefer `space::ROW_MIN`.
pub const ROW_MIN: f32 = space::ROW_MIN;

/// Shared sizing for form controls (buttons, inputs, dropdowns, search
/// fields, etc). Every control reads from here so changing one value
/// updates the whole UI in lockstep.
pub mod control {
    use super::radius;
    /// Compact control (e.g. table-row inline buttons, status-bar buttons).
    pub const H_SM: f32 = 26.0;
    /// Default control height for buttons, inputs, dropdowns.
    pub const H_MD: f32 = 32.0;
    /// Hero / primary CTAs.
    pub const H_LG: f32 = 40.0;
    /// Corner radius for form controls (buttons, inputs, dropdowns).
    pub const RADIUS: u8 = radius::XS;
    /// Horizontal inner padding for inputs (text input, file input, text
    /// area). Matches `space::S2`.
    pub const INPUT_PAD_X: f32 = 8.0;
    /// Vertical inner padding for multi-line inputs (text area). Matches
    /// `space::S2`. Single-line inputs use 0 vertical padding and rely on
    /// the fixed control height.
    pub const INPUT_PAD_Y: f32 = 7.0;
}

/// Shared sizing for surface containers (cards, dialogs, frames,
/// banners). Distinct from `control::RADIUS` so cards can scale
/// independently from form controls.
pub mod surface {
    use super::radius;
    /// Corner radius for surface containers (cards, frames, banners).
    pub const RADIUS: u8 = radius::SM;
}

pub const FAMILY_DISPLAY: &str = "fraunces";
pub const FAMILY_BODY: &str = "jakarta";
pub const FAMILY_BODY_MEDIUM: &str = "jakarta-medium";
pub const FAMILY_BODY_BOLD: &str = "jakarta-bold";
pub const FAMILY_MONO: &str = "jbmono";
pub const FAMILY_MONO_BOLD: &str = "jbmono-bold";

pub fn body(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FAMILY_BODY.into()))
}
pub fn body_medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FAMILY_BODY_MEDIUM.into()))
}
pub fn body_bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FAMILY_BODY_BOLD.into()))
}
pub fn display(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FAMILY_DISPLAY.into()))
}
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FAMILY_MONO.into()))
}
/// JetBrains Mono Bold (700). Matches design rules like
/// `.prop-hero-ext { font: 700 11px var(--font-mono); }`.
pub fn mono_bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FAMILY_MONO_BOLD.into()))
}

/// Typography scale. Each function returns a [`FontId`] matching one
/// entry in `tokens.json::font`. Weight is encoded as the font family
/// (egui has no synthetic weights). Where a CSS rule requires
/// uppercase, tabular numerals, or letter-spacing, the caller must
/// apply it — `FontId` cannot carry those properties.
#[allow(dead_code)]
pub mod ts {
    use super::*;
    /// 12 / 500.
    pub fn xs() -> FontId {
        body_medium(12.0)
    }
    /// 13 / 500.
    pub fn sm() -> FontId {
        body_medium(13.0)
    }
    /// 14 / 400 — default body text.
    pub fn base() -> FontId {
        body(14.0)
    }
    /// 16 / 500.
    pub fn md() -> FontId {
        body_medium(16.0)
    }
    /// 18 / 500.
    pub fn lg() -> FontId {
        body_medium(18.0)
    }
    /// 22 / 500.
    pub fn xl() -> FontId {
        body_medium(22.0)
    }
    /// 28 / 500.
    pub fn xxl() -> FontId {
        body_medium(28.0)
    }
    /// 36 / 500.
    pub fn xxxl() -> FontId {
        body_medium(36.0)
    }
    /// 11 / 600 — uppercase + 0.06em letter-spacing applied by caller.
    pub fn eyebrow() -> FontId {
        body_bold(11.0)
    }
    /// 11 / 600.
    pub fn label() -> FontId {
        body_bold(11.0)
    }
    /// 11 / 500 — body font (Jakarta), tabular numerals caller-applied.
    pub fn count() -> FontId {
        body_medium(11.0)
    }
    /// 11 / 500 / mono.
    pub fn mono_sm() -> FontId {
        mono(11.0)
    }
}

pub fn install_fonts(ctx: &egui::Context) {
    use egui::{FontData, FontDefinitions};
    use std::sync::Arc;
    let mut defs = FontDefinitions::default();

    macro_rules! font {
        ($name:literal, $path:literal) => {{
            defs.font_data.insert(
                $name.to_owned(),
                Arc::new(FontData::from_static(include_bytes!(concat!(
                    "../../assets/fonts/",
                    $path
                )))),
            );
        }};
    }
    font!("Jakarta", "PlusJakartaSans-Regular.ttf");
    font!("JakartaMedium", "PlusJakartaSans-Medium.ttf");
    font!("JakartaSemiBold", "PlusJakartaSans-SemiBold.ttf");
    font!("JakartaBold", "PlusJakartaSans-Bold.ttf");
    font!("Fraunces", "Fraunces72pt-SemiBold.ttf");
    font!("FrauncesReg", "Fraunces72pt-Regular.ttf");
    font!("JBMono", "JetBrainsMono-Regular.ttf");
    font!("JBMonoMedium", "JetBrainsMono-Medium.ttf");
    font!("JBMonoBold", "JetBrainsMono-Bold.ttf");

    defs.families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "Jakarta".into());
    defs.families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "JBMono".into());

    defs.families.insert(
        FontFamily::Name(FAMILY_BODY.into()),
        vec!["Jakarta".into(), "JakartaMedium".into()],
    );
    defs.families.insert(
        FontFamily::Name(FAMILY_BODY_MEDIUM.into()),
        vec!["JakartaMedium".into(), "Jakarta".into()],
    );
    defs.families.insert(
        FontFamily::Name(FAMILY_BODY_BOLD.into()),
        vec![
            "JakartaSemiBold".into(),
            "JakartaBold".into(),
            "Jakarta".into(),
        ],
    );
    defs.families.insert(
        FontFamily::Name(FAMILY_DISPLAY.into()),
        vec!["Fraunces".into(), "FrauncesReg".into(), "Jakarta".into()],
    );
    defs.families.insert(
        FontFamily::Name(FAMILY_MONO.into()),
        vec!["JBMonoMedium".into(), "JBMono".into()],
    );
    defs.families.insert(
        FontFamily::Name(FAMILY_MONO_BOLD.into()),
        vec!["JBMonoBold".into(), "JBMonoMedium".into(), "JBMono".into()],
    );

    ctx.set_fonts(defs);
}

pub fn tokens(ctx: &egui::Context) -> Tokens {
    ctx.data(|d| d.get_temp::<Tokens>(egui::Id::NULL))
        .unwrap_or_else(Tokens::light)
}

pub fn apply(ctx: &egui::Context, settings: &Settings) {
    let resolved = resolve(settings.theme);
    let mut t = match resolved {
        ResolvedTheme::Light => Tokens::light(),
        ResolvedTheme::Dark => Tokens::dark(),
        ResolvedTheme::Warm => Tokens::warm(),
    };

    if let Some(accent) = settings.theme_overrides.get("accent")
        && let Some(c) = parse_color(accent)
    {
        t.action_primary = c;
        t.border_brand = c;
        t.progress_fill = c;
    }
    if let Some(bg) = settings.theme_overrides.get("bg")
        && let Some(c) = parse_color(bg)
    {
        t.bg_page = c;
        t.bg_surface = c;
    }
    if let Some(text) = settings.theme_overrides.get("text")
        && let Some(c) = parse_color(text)
    {
        t.fg_1 = c;
    }

    ctx.data_mut(|d| d.insert_temp(egui::Id::NULL, t));

    let mut visuals = match resolved {
        ResolvedTheme::Dark => Visuals::dark(),
        ResolvedTheme::Light | ResolvedTheme::Warm => Visuals::light(),
    };
    visuals.override_text_color = Some(t.fg_1);
    visuals.interact_cursor = Some(egui::CursorIcon::PointingHand);
    visuals.panel_fill = t.bg_page;
    visuals.window_fill = t.bg_surface;
    visuals.window_stroke = Stroke::new(t.border_width, t.border_default);
    visuals.faint_bg_color = t.bg_sunken;
    visuals.extreme_bg_color = t.bg_sunken;
    visuals.window_corner_radius = radius::LG.into();
    visuals.menu_corner_radius = radius::SM.into();
    visuals.selection.bg_fill = mix(t.action_primary, t.bg_surface, 0.20);
    visuals.selection.stroke = Stroke::new(1.0, t.action_primary);
    visuals.hyperlink_color = t.action_primary;

    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = t.bg_surface;
    widgets.noninteractive.weak_bg_fill = t.bg_surface;
    widgets.noninteractive.bg_stroke = Stroke::new(t.border_width, t.border_subtle);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, t.fg_2);
    widgets.noninteractive.corner_radius = control::RADIUS.into();

    widgets.inactive.bg_fill = t.bg_raised;
    widgets.inactive.weak_bg_fill = t.bg_raised;
    widgets.inactive.bg_stroke = Stroke::new(t.border_width, t.border_subtle);
    widgets.inactive.fg_stroke = Stroke::new(1.0, t.fg_1);
    widgets.inactive.corner_radius = control::RADIUS.into();
    widgets.inactive.expansion = 0.0;

    widgets.hovered.bg_fill = t.bg_sunken;
    widgets.hovered.weak_bg_fill = t.bg_sunken;
    widgets.hovered.bg_stroke = Stroke::new(t.border_width, t.border_default);
    widgets.hovered.fg_stroke = Stroke::new(1.0, t.fg_1);
    widgets.hovered.corner_radius = control::RADIUS.into();
    widgets.hovered.expansion = 0.0;

    widgets.active.bg_fill = t.bg_sunken;
    widgets.active.weak_bg_fill = t.bg_sunken;
    widgets.active.bg_stroke = Stroke::new(t.border_width, t.border_brand);
    widgets.active.fg_stroke = Stroke::new(1.0, t.fg_1);
    widgets.active.corner_radius = control::RADIUS.into();
    widgets.active.expansion = 0.0;

    widgets.open.bg_fill = t.bg_sunken;
    widgets.open.weak_bg_fill = t.bg_sunken;
    widgets.open.bg_stroke = Stroke::new(t.border_width, t.border_default);
    widgets.open.fg_stroke = Stroke::new(1.0, t.fg_1);
    widgets.open.corner_radius = control::RADIUS.into();

    ctx.set_visuals(visuals);

    let mut style = (*ctx.global_style()).clone();
    style.spacing.item_spacing = egui::vec2(space::S2 as f32, space::S1 as f32);
    style.spacing.button_padding = egui::vec2(space::S3 as f32, space::S1 as f32);
    style.spacing.window_margin = egui::Margin::same(space::S4);
    style.spacing.menu_margin = egui::Margin::same(space::S1);
    style.spacing.interact_size.y = control::H_MD;
    style.spacing.icon_width = 14.0;
    style.spacing.icon_width_inner = 10.0;
    style.spacing.scroll.bar_width = 10.0;
    // Keep scroll bar 1px off the right/bottom viewport edge so it
    // doesn't paint on top of the hairline window border drawn by
    // `utils::resize`. egui 0.34 defaults this to 0.0.
    style.spacing.scroll.bar_outer_margin = t.border_width_hairline;

    use egui::TextStyle;
    style.text_styles.insert(TextStyle::Body, body(13.0));
    style.text_styles.insert(TextStyle::Button, body(13.0));
    style.text_styles.insert(TextStyle::Small, body(11.0));
    style.text_styles.insert(TextStyle::Monospace, mono(12.0));
    style.text_styles.insert(TextStyle::Heading, display(20.0));

    ctx.set_global_style(style);
}

fn parse_color(s: &str) -> Option<Color32> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        match hex.len() {
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some(Color32::from_rgb(r, g, b))
            }
            8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
                Some(Color32::from_rgba_unmultiplied(r, g, b, a))
            }
            _ => None,
        }
    } else {
        None
    }
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let lerp = |x: u8, y: u8| ((x as f32) * (1.0 - t) + (y as f32) * t) as u8;
    Color32::from_rgba_unmultiplied(
        lerp(b.r(), a.r()),
        lerp(b.g(), a.g()),
        lerp(b.b(), a.b()),
        255,
    )
}
