//! Decoded app/tray icons. Variants per resolved theme: the `_dark`
//! PNGs are the dark-coloured glyphs used on a *light* background, and
//! vice versa.

use std::sync::OnceLock;

use crate::gui::theme::ResolvedTheme;

const IDLE_LIGHT_PNG: &[u8] = include_bytes!("../../assets/oxdm_idle_light.png");
const IDLE_DARK_PNG: &[u8] = include_bytes!("../../assets/oxdm_idle_dark.png");
const PLAY_LIGHT_PNG: &[u8] = include_bytes!("../../assets/oxdm_play_light.png");
const PLAY_DARK_PNG: &[u8] = include_bytes!("../../assets/oxdm_play_dark.png");
/// Full-colour app mark, drawn large in the About dialog — the tray
/// glyphs above are shaped for a 22px status area, not for 64px.
const ABOUT_LIGHT_PNG: &[u8] = include_bytes!("../../assets/oxdm_about_light.png");
const ABOUT_DARK_PNG: &[u8] = include_bytes!("../../assets/oxdm_about_dark.png");

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
fn about_dark() -> Option<&'static Decoded> {
    static CELL: OnceLock<Option<Decoded>> = OnceLock::new();
    CELL.get_or_init(|| decode(ABOUT_DARK_PNG, "oxdm_about_dark.png"))
        .as_ref()
}
fn about_light() -> Option<&'static Decoded> {
    static CELL: OnceLock<Option<Decoded>> = OnceLock::new();
    CELL.get_or_init(|| decode(ABOUT_LIGHT_PNG, "oxdm_about_light.png"))
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

/// Long edge of the pre-scaled in-app glyph. The About dialog draws it
/// at 64pt, so this covers a 2× display exactly; anything the renderer
/// still has to do from here is a clean 2:1 (or 1:1) step.
const IN_APP_PX: u32 = 128;

/// The app glyph as an iced image handle, for in-app surfaces (About).
///
/// Downscaled here rather than by the renderer: iced's image filters are
/// single-pass, so shrinking 512→64 in one hop samples ~1 source pixel
/// in 64 and turns the glyph's thin strokes into aliased grit. Lanczos
/// with a full box of source pixels is what makes it read smooth.
///
/// Cached per variant: `Handle::from_rgba` takes ownership of the
/// pixels, so a fresh copy every frame would be churn in a view that
/// redraws with the rest of the window.
pub fn image_handle(theme: ResolvedTheme) -> Option<iced::widget::image::Handle> {
    fn handle(d: &Decoded) -> iced::widget::image::Handle {
        let Some(src) = image::RgbaImage::from_raw(d.width, d.height, d.rgba.clone()) else {
            return iced::widget::image::Handle::from_rgba(d.width, d.height, d.rgba.clone());
        };
        if d.width <= IN_APP_PX && d.height <= IN_APP_PX {
            return iced::widget::image::Handle::from_rgba(d.width, d.height, d.rgba.clone());
        }
        let long = d.width.max(d.height) as f32;
        let (w, h) = (
            ((d.width as f32 / long) * IN_APP_PX as f32)
                .round()
                .max(1.0) as u32,
            ((d.height as f32 / long) * IN_APP_PX as f32)
                .round()
                .max(1.0) as u32,
        );
        let scaled = image::imageops::resize(&src, w, h, image::imageops::FilterType::Lanczos3);
        iced::widget::image::Handle::from_rgba(w, h, scaled.into_raw())
    }
    static LIGHT_GLYPH: OnceLock<Option<iced::widget::image::Handle>> = OnceLock::new();
    static DARK_GLYPH: OnceLock<Option<iced::widget::image::Handle>> = OnceLock::new();
    match theme {
        ResolvedTheme::Light | ResolvedTheme::Warm => {
            DARK_GLYPH.get_or_init(|| about_dark().map(handle))
        }
        ResolvedTheme::Dark => LIGHT_GLYPH.get_or_init(|| about_light().map(handle)),
    }
    .clone()
}

/// RGBA bytes + dimensions for the window icon (framework-agnostic).
///
/// Always the opaque variant, whatever theme the app is running in.
/// This icon is drawn by the *desktop* — taskbar, dock, Alt-Tab — on a
/// background oxdm does not choose and cannot read. The tray glyphs
/// above are a different job: a status area asks for a flat shape in
/// the panel's own colour, and one of those on a dark taskbar is a
/// black arrow on nothing.
pub fn window_icon_data() -> Option<(Vec<u8>, u32, u32)> {
    let d = idle_light()?;
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
