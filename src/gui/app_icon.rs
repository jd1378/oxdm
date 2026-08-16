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

/// The launcher icon, undecoded, for `platform::install_desktop_entry`
/// to write into the user's icon theme. The opaque variant for the same
/// reason [`window_icon_data`] uses it: the desktop draws this one on a
/// background oxdm neither picks nor can read.
pub const LAUNCHER_PNG: &[u8] = IDLE_LIGHT_PNG;

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

/// The sizes a StatusNotifierItem offers the host, smallest first.
///
/// `IconPixmap` is an *array* of the same picture at several sizes and
/// the host picks the one nearest its panel; handing it one 512px image
/// for a 22px slot leaves the scaling to whatever single-pass filter
/// the host happens to use, which is where the glyph's thin strokes go
/// to die. 22/24 are the usual panel heights, the rest cover HiDPI
/// panels without shipping the full 512 (a megabyte of pixels over
/// D-Bus on every property read).
#[cfg(target_os = "linux")]
const TRAY_PX: [u32; 5] = [22, 24, 32, 48, 64];

/// One [`ksni::Icon`] from `d`, scaled to `px` square.
///
/// SNI wants ARGB32 in network byte order, so each pixel's alpha moves
/// from last to first. Lanczos for the same reason [`image_handle`]
/// uses it: a clean box of source pixels per destination pixel.
#[cfg(target_os = "linux")]
fn ksni_icon_at(d: &Decoded, px: u32) -> Option<ksni::Icon> {
    let src = image::RgbaImage::from_raw(d.width, d.height, d.rgba.clone())?;
    let scaled = if d.width == px && d.height == px {
        src
    } else {
        image::imageops::resize(&src, px, px, image::imageops::FilterType::Lanczos3)
    };
    let mut argb = scaled.into_raw();
    for pixel in argb.chunks_exact_mut(4) {
        pixel.rotate_right(1);
    }
    Some(ksni::Icon {
        width: px as i32,
        height: px as i32,
        data: argb,
    })
}

/// The size ladder for one glyph.
///
/// Cached: the tray republishes its icon on every job update, and
/// rescaling 512px five times per update is real work to redo for a
/// picture that never changes.
#[cfg(target_os = "linux")]
fn ksni_ladder(d: &Decoded) -> Vec<ksni::Icon> {
    TRAY_PX
        .iter()
        .filter_map(|&px| ksni_icon_at(d, px))
        .collect()
}

#[cfg(target_os = "linux")]
pub fn ksni_icon_normal(theme: ResolvedTheme) -> Vec<ksni::Icon> {
    static DARK: OnceLock<Vec<ksni::Icon>> = OnceLock::new();
    static LIGHT: OnceLock<Vec<ksni::Icon>> = OnceLock::new();
    match theme {
        ResolvedTheme::Light | ResolvedTheme::Warm => {
            DARK.get_or_init(|| idle_dark().map(ksni_ladder).unwrap_or_default())
        }
        ResolvedTheme::Dark => {
            LIGHT.get_or_init(|| idle_light().map(ksni_ladder).unwrap_or_default())
        }
    }
    .clone()
}

#[cfg(target_os = "linux")]
pub fn ksni_icon_downloading(theme: ResolvedTheme) -> Vec<ksni::Icon> {
    static DARK: OnceLock<Vec<ksni::Icon>> = OnceLock::new();
    static LIGHT: OnceLock<Vec<ksni::Icon>> = OnceLock::new();
    match theme {
        ResolvedTheme::Light | ResolvedTheme::Warm => {
            DARK.get_or_init(|| play_dark().map(ksni_ladder).unwrap_or_default())
        }
        ResolvedTheme::Dark => {
            LIGHT.get_or_init(|| play_light().map(ksni_ladder).unwrap_or_default())
        }
    }
    .clone()
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
