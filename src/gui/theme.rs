//! Design system: tokens (colors, spacing, radii, fonts) ported from
//! `design/handoff/tokens.json`. Mirrors `src/ui/theme.rs` (egui) with
//! framework types swapped to iced.
//!
//! Three palette themes: **utility** (cool gray + clay accent),
//! **warm** (cream + earth), and **dark** (matte near-black). All
//! share the same spacing / radius / type / motion scales; only
//! `color.*` swaps. `Theme::Light` maps to utility.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use iced::Color;

use crate::domain::{Settings, Theme};
use crate::gui::color::{clay, earth, gray, hex, mix, moss, ochre, rust, slate};

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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tokens {
    pub theme: ResolvedTheme,

    pub fg_1: Color,
    pub fg_2: Color,
    pub fg_3: Color,
    pub fg_4: Color,
    pub fg_inverse: Color,

    pub bg_page: Color,
    pub bg_surface: Color,
    pub bg_sunken: Color,
    pub bg_raised: Color,
    pub bg_inverse: Color,
    pub bg_sidebar: Color,
    pub bg_titlebar: Color,

    pub border_subtle: Color,
    pub border_default: Color,
    pub border_strong: Color,
    pub border_brand: Color,

    pub action_primary: Color,
    pub action_primary_press: Color,
    pub action_primary_fg: Color,
    pub action_primary_shadow: Color,

    pub pill_active_bg: Color,
    pub pill_active_fg: Color,

    pub status_success: Color,
    pub status_success_bg: Color,
    pub status_warning: Color,
    pub status_warning_bg: Color,
    pub status_danger: Color,
    pub status_danger_bg: Color,
    pub status_info: Color,
    pub status_info_bg: Color,

    pub progress_track: Color,
    pub progress_fill: Color,

    pub cat_compressed: Color,
    pub cat_programs: Color,
    pub cat_videos: Color,
    pub cat_music: Color,
    pub cat_pictures: Color,
    pub cat_documents: Color,

    pub border_width: f32,
    pub border_width_thick: f32,
    pub border_width_hairline: f32,

    pub focus_ring: Color,
    pub focus_ring_gap: Color,
    pub row_selected_bg: Color,
    pub row_hover_bg: Color,
    pub row_selhover_bg: Color,
    pub bg_sunken_hover: Color,
    pub bg_surface_hover: Color,
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
            bg_surface: Color::WHITE,
            bg_sunken: gray::G100,
            bg_raised: Color::WHITE,
            bg_inverse: gray::G800,
            bg_sidebar: gray::G100,
            bg_titlebar: gray::G100,
            border_subtle: gray::G200,
            border_default: gray::G300,
            border_strong: gray::G700,
            border_brand: clay::C400,
            action_primary: clay::C400,
            action_primary_press: clay::C500,
            action_primary_fg: Color::WHITE,
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
            focus_ring_gap: Color::WHITE,
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

    /// Theme tokens for resolved theme + user overrides, mirroring
    /// egui `theme::apply`'s override handling.
    pub fn from_settings(settings: &Settings) -> Self {
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
        t
    }

    /// iced `Theme` whose palette derives from these tokens. Widgets
    /// are styled per-widget from `Tokens`; the palette mostly feeds
    /// default text/background colors and built-in widget fallbacks.
    pub fn iced_theme(&self) -> iced::Theme {
        iced::Theme::custom(
            match self.theme {
                ResolvedTheme::Light => "oxdm-light",
                ResolvedTheme::Dark => "oxdm-dark",
                ResolvedTheme::Warm => "oxdm-warm",
            }
            .to_owned(),
            iced::theme::Palette {
                background: self.bg_page,
                text: self.fg_1,
                primary: self.action_primary,
                success: self.status_success,
                warning: self.status_warning,
                danger: self.status_danger,
            },
        )
    }

    /// Selection highlight used by text inputs (egui used
    /// `action_primary` mixed 20% into the surface).
    pub fn selection_bg(&self) -> Color {
        mix(self.bg_surface, self.action_primary, 0.20)
    }
}

fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();
    let hexstr = s.strip_prefix('#')?;
    let parse = |range: std::ops::Range<usize>| u8::from_str_radix(&hexstr[range], 16).ok();
    match hexstr.len() {
        6 => Some(Color::from_rgb8(parse(0..2)?, parse(2..4)?, parse(4..6)?)),
        8 => Some(Color::from_rgba8(
            parse(0..2)?,
            parse(2..4)?,
            parse(4..6)?,
            parse(6..8)? as f32 / 255.0,
        )),
        _ => None,
    }
}

pub mod space {
    pub const S0: f32 = 2.0;
    pub const S1: f32 = 4.0;
    pub const S2: f32 = 8.0;
    pub const S3: f32 = 12.0;
    pub const S4: f32 = 16.0;
    pub const S5: f32 = 20.0;
    pub const S6: f32 = 24.0;
    pub const S7: f32 = 32.0;
    pub const S8: f32 = 40.0;
    pub const S9: f32 = 48.0;
    pub const S10: f32 = 64.0;
    pub const ROW_MIN: f32 = 56.0;
    pub const TAP_MIN: f32 = 44.0;
}

pub mod radius {
    pub const XS: f32 = 6.0;
    /// Corner radius for form controls (design `.input`/`.btn` = 5px).
    /// Distinct from the global `XS` token so controls can diverge.
    pub const CTRL: f32 = 5.0;
    pub const SM: f32 = 10.0;
    pub const MD: f32 = 12.0;
    pub const LG: f32 = 16.0;
    pub const XL: f32 = 22.0;
    pub const XXL: f32 = 28.0;
    pub const PILL: f32 = 255.0;
}

/// Standard padding presets `(x, y)`. Names match `tokens.json::padding`.
#[allow(dead_code)]
pub mod pad {
    pub const BTN: (f32, f32) = (12.0, 6.0);
    pub const BTN_COMPACT: (f32, f32) = (8.0, 4.0);
    pub const BTN_LARGE: (f32, f32) = (16.0, 10.0);
    pub const ICON_BUTTON_SM: (f32, f32) = (2.0, 2.0);
    pub const INPUT: (f32, f32) = (10.0, 8.0);
    pub const ROW: (f32, f32) = (10.0, 8.0);
    pub const DIALOG: (f32, f32) = (16.0, 14.0);
    pub const DIALOG_FOOTER: (f32, f32) = (14.0, 10.0);
    pub const SECTION: (f32, f32) = (16.0, 12.0);
    pub const CARD: (f32, f32) = (12.0, 12.0);
    pub const CHIP: (f32, f32) = (8.0, 4.0);
    pub const TOOLTIP: (f32, f32) = (8.0, 5.0);
}

/// Animation duration tokens (seconds). Source: tokens.json::motion.
#[allow(dead_code)]
pub mod motion {
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

    /// Sinusoidal pulse opacity for "live" indicators; `period_s`
    /// matches `16_animations.md` (1.4s for download/tab live dots).
    /// Returns alpha in `[0.35, 1.0]`.
    pub fn pulse_alpha(time_s: f32, period_s: f32) -> f32 {
        let phase = (time_s % period_s) / period_s;
        let cos = (phase * std::f32::consts::TAU).cos();
        0.675 + 0.325 * cos
    }
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

/// Shared sizing for form controls (buttons, inputs, dropdowns, search
/// fields, etc). Every control reads from here so changing one value
/// updates the whole UI in lockstep.
pub mod control {
    use super::radius;
    /// Compact control (e.g. table-row inline buttons, status-bar buttons).
    pub const H_SM: f32 = 22.0;
    /// Default control height for buttons, inputs, dropdowns.
    pub const H_MD: f32 = 28.0;
    /// Hero / primary CTAs.
    pub const H_LG: f32 = 32.0;
    /// Corner radius for form controls (buttons, inputs, dropdowns).
    pub const RADIUS: f32 = radius::CTRL;
    /// Horizontal inner padding for inputs (text input, file input, text
    /// area). Design `.input` pad-x = 10px.
    pub const INPUT_PAD_X: f32 = 10.0;
    /// Vertical inner padding for multi-line inputs (text area).
    pub const INPUT_PAD_Y: f32 = 7.0;
}

/// Shared sizing for surface containers (cards, dialogs, frames,
/// banners). Distinct from `control::RADIUS` so cards can scale
/// independently from form controls.
pub mod surface {
    use super::radius;
    /// Corner radius for surface containers (cards, frames, banners).
    pub const RADIUS: f32 = radius::SM;
}

pub mod fonts {
    /// All bundled font binaries, registered via `application.font(..)`.
    pub static ALL: &[&[u8]] = &[
        include_bytes!("../../assets/fonts/PlusJakartaSans-Regular.ttf"),
        include_bytes!("../../assets/fonts/PlusJakartaSans-Medium.ttf"),
        include_bytes!("../../assets/fonts/PlusJakartaSans-SemiBold.ttf"),
        include_bytes!("../../assets/fonts/PlusJakartaSans-Bold.ttf"),
        include_bytes!("../../assets/fonts/Fraunces72pt-Regular.ttf"),
        include_bytes!("../../assets/fonts/Fraunces72pt-SemiBold.ttf"),
        include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf"),
        include_bytes!("../../assets/fonts/JetBrainsMono-Medium.ttf"),
        include_bytes!("../../assets/fonts/JetBrainsMono-Bold.ttf"),
    ];
}

use iced::Font;
use iced::font::{Family, Weight};

const fn jakarta(weight: Weight) -> Font {
    Font {
        family: Family::Name("Plus Jakarta Sans"),
        weight,
        ..Font::DEFAULT
    }
}
const fn fraunces(weight: Weight) -> Font {
    Font {
        family: Family::Name("Fraunces 72pt"),
        weight,
        ..Font::DEFAULT
    }
}
const fn jbmono(weight: Weight) -> Font {
    Font {
        family: Family::Name("JetBrains Mono"),
        weight,
        ..Font::DEFAULT
    }
}

/// 400 — default body text (Jakarta Regular).
pub const BODY: Font = jakarta(Weight::Normal);
/// 500 — medium emphasis body (most labels in the design).
pub const BODY_MEDIUM: Font = jakarta(Weight::Medium);
/// 600 — bold labels / eyebrows (Jakarta SemiBold; egui's
/// `body_bold` family resolved to the SemiBold face first).
pub const BODY_BOLD: Font = jakarta(Weight::Semibold);
/// Display serif (Fraunces SemiBold) for headings / hero numbers.
pub const DISPLAY: Font = fraunces(Weight::Semibold);
/// Display serif regular fallback weight.
pub const DISPLAY_REGULAR: Font = fraunces(Weight::Normal);
/// Mono (JetBrains Mono Medium — egui's `mono` family resolved to
/// the Medium face first).
pub const MONO: Font = jbmono(Weight::Medium);
/// Mono regular (used where egui fell back to JBMono Regular).
pub const MONO_REGULAR: Font = jbmono(Weight::Normal);
/// Mono bold (700) — e.g. `.prop-hero-ext`.
pub const MONO_BOLD: Font = jbmono(Weight::Bold);

/// Typography scale: `(Font, size)` pairs matching `tokens.json::font`
/// (same values as `src/ui/theme.rs::ts`).
#[allow(dead_code)]
pub mod ts {
    use super::*;
    pub const XS: (Font, f32) = (BODY_MEDIUM, 12.0);
    pub const SM: (Font, f32) = (BODY_MEDIUM, 13.0);
    pub const BASE: (Font, f32) = (BODY, 14.0);
    pub const MD: (Font, f32) = (BODY_MEDIUM, 16.0);
    pub const LG: (Font, f32) = (BODY_MEDIUM, 18.0);
    pub const XL: (Font, f32) = (BODY_MEDIUM, 22.0);
    pub const XXL: (Font, f32) = (BODY_MEDIUM, 28.0);
    pub const XXXL: (Font, f32) = (BODY_MEDIUM, 36.0);
    /// 11 / 600 — uppercase + letter-spacing applied by caller.
    pub const EYEBROW: (Font, f32) = (BODY_BOLD, 11.0);
    pub const LABEL: (Font, f32) = (BODY_BOLD, 11.0);
    pub const COUNT: (Font, f32) = (BODY_MEDIUM, 11.0);
    pub const MONO_SM: (Font, f32) = (MONO, 11.0);
}

/// Default body text: 13px Jakarta Regular (matches egui
/// `TextStyle::Body`).
pub const BODY_SIZE: f32 = 13.0;
