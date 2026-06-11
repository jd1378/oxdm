//! Decoded app/tray icons. Variants per resolved theme: the `_dark`
//! PNGs are the dark-coloured glyphs used on a *light* background, and
//! vice versa.

use std::sync::OnceLock;

use crate::gui::theme::ResolvedTheme;

const IDLE_LIGHT_PNG: &[u8] = include_bytes!("../../assets/oxdm_idle_light.png");
const IDLE_DARK_PNG: &[u8] = include_bytes!("../../assets/oxdm_idle_dark.png");
const PLAY_LIGHT_PNG: &[u8] = include_bytes!("../../assets/oxdm_play_light.png");
const PLAY_DARK_PNG: &[u8] = include_bytes!("../../assets/oxdm_play_dark.png");

pub struct Decoded {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

fn decode(bytes: &[u8], label: &'static str) -> Option<Decoded> {
    let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .map_err(|e| tracing::warn!(error = %e, %label, "decode tray png"))
        .ok()?
        .to_rgba8();
    let (width, height) = img.dimensions();
    Some(Decoded {
        rgba: img.into_raw(),
        width,
        height,
    })
}

fn idle_dark() -> Option<&'static Decoded> {
    static CELL: OnceLock<Option<Decoded>> = OnceLock::new();
    CELL.get_or_init(|| decode(IDLE_DARK_PNG, "oxdm_idle_dark.png"))
        .as_ref()
}
fn idle_light() -> Option<&'static Decoded> {
    static CELL: OnceLock<Option<Decoded>> = OnceLock::new();
    CELL.get_or_init(|| decode(IDLE_LIGHT_PNG, "oxdm_idle_light.png"))
        .as_ref()
}
fn play_dark() -> Option<&'static Decoded> {
    static CELL: OnceLock<Option<Decoded>> = OnceLock::new();
    CELL.get_or_init(|| decode(PLAY_DARK_PNG, "oxdm_play_dark.png"))
        .as_ref()
}
fn play_light() -> Option<&'static Decoded> {
    static CELL: OnceLock<Option<Decoded>> = OnceLock::new();
    CELL.get_or_init(|| decode(PLAY_LIGHT_PNG, "oxdm_play_light.png"))
        .as_ref()
}

/// Idle (no active download) icon for the given resolved theme.
/// Light theme uses the dark-coloured glyph; dark theme uses the
/// light-coloured glyph.
pub fn normal(theme: ResolvedTheme) -> Option<&'static Decoded> {
    match theme {
        ResolvedTheme::Light | ResolvedTheme::Warm => idle_dark(),
        ResolvedTheme::Dark => idle_light(),
    }
}

pub fn downloading(theme: ResolvedTheme) -> Option<&'static Decoded> {
    match theme {
        ResolvedTheme::Light | ResolvedTheme::Warm => play_dark(),
        ResolvedTheme::Dark => play_light(),
    }
}

/// RGBA bytes + dimensions for the window icon (framework-agnostic).
pub fn window_icon_data() -> Option<(Vec<u8>, u32, u32)> {
    let d = normal(crate::gui::theme::system_theme())?;
    Some((d.rgba.clone(), d.width, d.height))
}

#[cfg(target_os = "linux")]
pub fn ksni_icon(d: &Decoded) -> ksni::Icon {
    let mut argb = d.rgba.clone();
    for px in argb.chunks_exact_mut(4) {
        px.rotate_right(1);
    }
    ksni::Icon {
        width: d.width as i32,
        height: d.height as i32,
        data: argb,
    }
}

#[cfg(target_os = "linux")]
pub fn ksni_icon_normal(theme: ResolvedTheme) -> Vec<ksni::Icon> {
    normal(theme).map(ksni_icon).into_iter().collect()
}

#[cfg(target_os = "linux")]
pub fn ksni_icon_downloading(theme: ResolvedTheme) -> Vec<ksni::Icon> {
    downloading(theme).map(ksni_icon).into_iter().collect()
}

#[cfg(not(target_os = "linux"))]
pub fn tray_icon_normal(theme: ResolvedTheme) -> Option<tray_icon::Icon> {
    let d = normal(theme)?;
    tray_icon::Icon::from_rgba(d.rgba.clone(), d.width, d.height).ok()
}

#[cfg(not(target_os = "linux"))]
pub fn tray_icon_downloading(theme: ResolvedTheme) -> Option<tray_icon::Icon> {
    let d = downloading(theme)?;
    tray_icon::Icon::from_rgba(d.rgba.clone(), d.width, d.height).ok()
}
