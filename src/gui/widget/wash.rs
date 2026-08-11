//! A soft elliptical glow, for the places the design puts a
//! `radial-gradient` behind artwork.
//!
//! iced draws linear gradients only. resvg can draw a radial one, but
//! going through the svg widget puts the SVG's own aspect ratio between
//! the drawing and the box it should fill, and the glow lands
//! off-centre and clipped. So the falloff is computed here, as pixels,
//! and stretched by the image widget — which does exactly what it is
//! told.
//!
//! The bitmap is tiny (the gradient has no detail to lose) and scaled
//! up with linear filtering, so the cost is a cache lookup and a blit.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use iced::widget::image;

/// Bitmap resolution. A radial falloff over ~500 px has nothing to say
/// at a finer grain than this, and interpolation does the rest.
const W: u32 = 96;
const H: u32 = 48;

/// Where the falloff reaches zero, as a fraction of the box's half
/// extent. Below 1.0 on purpose: a glow that still carries colour where
/// its box ends shows a straight edge, which is the one thing it must
/// not have.
const EXTENT: f32 = 0.92;

/// Opacity at the centre.
const PEAK: f32 = 0.9;

/// An elliptical `tint`-to-transparent glow, centred, sized to whatever
/// box it is drawn into.
pub fn radial(tint: iced::Color) -> image::Handle {
    let key = [
        (tint.r * 255.0) as u8,
        (tint.g * 255.0) as u8,
        (tint.b * 255.0) as u8,
    ];
    static CACHE: OnceLock<Mutex<HashMap<[u8; 3], image::Handle>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().unwrap();
    cache.entry(key).or_insert_with(|| render(key)).clone()
}

fn render(rgb: [u8; 3]) -> image::Handle {
    let mut px = Vec::with_capacity((W * H * 4) as usize);
    for y in 0..H {
        for x in 0..W {
            // Distance from the centre in units of the half-extent, so
            // the shape follows the box and comes out elliptical when
            // the box is wide.
            let dx = (x as f32 + 0.5) / (W as f32 / 2.0) - 1.0;
            let dy = (y as f32 + 0.5) / (H as f32 / 2.0) - 1.0;
            let d = (dx * dx + dy * dy).sqrt() / EXTENT;
            // Squared falloff: linear reads as a disc with a visible
            // rim, this reads as light.
            let a = (1.0 - d).clamp(0.0, 1.0).powf(2.0) * PEAK;
            px.extend_from_slice(&[rgb[0], rgb[1], rgb[2], (a * 255.0) as u8]);
        }
    }
    image::Handle::from_rgba(W, H, px)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The glow must be at its strongest in the middle and *fully*
    /// transparent at every edge — a fade that is still tinted where
    /// its box ends draws a straight line across the artwork.
    #[test]
    fn the_falloff_finishes_before_the_edge() {
        let alpha = |x: u32, y: u32| -> u8 {
            let dx = (x as f32 + 0.5) / (W as f32 / 2.0) - 1.0;
            let dy = (y as f32 + 0.5) / (H as f32 / 2.0) - 1.0;
            let d = (dx * dx + dy * dy).sqrt() / EXTENT;
            ((1.0 - d).clamp(0.0, 1.0).powf(2.0) * PEAK * 255.0) as u8
        };
        assert!(alpha(W / 2, H / 2) > 200, "the centre carries the colour");
        for x in 0..W {
            assert_eq!(alpha(x, 0), 0, "top edge at x={x}");
            assert_eq!(alpha(x, H - 1), 0, "bottom edge at x={x}");
        }
        for y in 0..H {
            assert_eq!(alpha(0, y), 0, "left edge at y={y}");
            assert_eq!(alpha(W - 1, y), 0, "right edge at y={y}");
        }
    }

    #[test]
    fn one_bitmap_per_tint() {
        let a = radial(iced::Color::from_rgb(0.8, 0.5, 0.3));
        let b = radial(iced::Color::from_rgb(0.8, 0.5, 0.3));
        assert_eq!(a.id(), b.id(), "the same tint reuses its bitmap");
    }
}
