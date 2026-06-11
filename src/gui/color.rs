//! Base color palette — mirrors `design/tokens.css` `--{earth,clay,moss,
//! ochre,rust,slate,gray}-NNN` scales. Theme-independent: themes pick
//! from these constants. Dark theme remaps a subset of clay (see
//! `clay::DARK_*`) per the design's "dark-mode clay tint remap" note.

use iced::Color;

pub const fn hex(c: u32) -> Color {
    let r = ((c >> 16) & 0xFF) as u8;
    let g = ((c >> 8) & 0xFF) as u8;
    let b = (c & 0xFF) as u8;
    Color::from_rgb8(r, g, b)
}

/// `color` with its alpha channel replaced by `a` (0.0–1.0).
pub fn with_alpha(color: Color, a: f32) -> Color {
    Color { a, ..color }
}

/// Linear blend; `t = 0` returns `a`, `t = 1` returns `b`.
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let l = |x: f32, y: f32| x * (1.0 - t) + y * t;
    Color {
        r: l(a.r, b.r),
        g: l(a.g, b.g),
        b: l(a.b, b.b),
        a: l(a.a, b.a),
    }
}

pub mod earth {
    use super::*;
    pub const E50: Color = hex(0xFAF6F0);
    pub const E100: Color = hex(0xF2EAD9);
    pub const E200: Color = hex(0xE6D7BD);
    pub const E300: Color = hex(0xD2BC97);
    pub const E400: Color = hex(0xB59873);
    pub const E500: Color = hex(0x8C6D4A);
    pub const E600: Color = hex(0x5E4A30);
    pub const E700: Color = hex(0x3F3220);
    pub const E800: Color = hex(0x2A2117);
    pub const E900: Color = hex(0x1A140D);
}

pub mod clay {
    use super::*;
    pub const C50: Color = hex(0xFBEFE7);
    pub const C100: Color = hex(0xF4D9C6);
    pub const C200: Color = hex(0xE9B595);
    pub const C300: Color = hex(0xDA8E63);
    pub const C400: Color = hex(0xC9703F);
    pub const C500: Color = hex(0xB25A2A);
    pub const C600: Color = hex(0x8E461F);
    pub const C700: Color = hex(0x6B3417);

    // Dark-mode clay remap (see tokens.css `.theme-dark` override).
    // Light-end shades become dark warm tints so they don't punch
    // bright holes in dark surfaces; clay-700 inverts to a light
    // clay so paired text on those tints stays readable.
    pub const DARK_C50: Color = hex(0x2B201A);
    pub const DARK_C100: Color = hex(0x3A2A20);
    pub const DARK_C200: Color = hex(0x4F3525);
    pub const DARK_C700: Color = hex(0xE9B595);
}

pub mod moss {
    use super::*;
    pub const M50: Color = hex(0xEEF1E4);
    pub const M100: Color = hex(0xD9E0BF);
    pub const M200: Color = hex(0xBDC894);
    pub const M300: Color = hex(0x9CAB6A);
    pub const M400: Color = hex(0x7A8B4A);
    pub const M500: Color = hex(0x5E6E36);
    pub const M600: Color = hex(0x455328);
}

pub mod ochre {
    use super::*;
    pub const O50: Color = hex(0xFBF1D8);
    pub const O100: Color = hex(0xF5DFA3);
    pub const O200: Color = hex(0xECC766);
    pub const O300: Color = hex(0xDDAA38);
    pub const O400: Color = hex(0xB98A21);
    pub const O500: Color = hex(0x8C681A);
}

pub mod rust {
    use super::*;
    pub const R50: Color = hex(0xF7E0D8);
    pub const R100: Color = hex(0xEBB69F);
    pub const R200: Color = hex(0xD78062);
    pub const R300: Color = hex(0xB85436);
    pub const R400: Color = hex(0x8E3B22);
    pub const R500: Color = hex(0x682814);
}

pub mod slate {
    use super::*;
    pub const S50: Color = hex(0xE6E8EC);
    pub const S100: Color = hex(0xBFC5CE);
    pub const S200: Color = hex(0x8E97A4);
    pub const S300: Color = hex(0x5E6877);
    pub const S400: Color = hex(0x3D4654);
}

pub mod gray {
    use super::*;
    pub const G50: Color = hex(0xF5F5F4);
    pub const G100: Color = hex(0xE7E6E3);
    pub const G200: Color = hex(0xD2D0CC);
    pub const G300: Color = hex(0xADAAA4);
    pub const G400: Color = hex(0x7A7672);
    pub const G500: Color = hex(0x525049);
    pub const G600: Color = hex(0x363430);
    pub const G700: Color = hex(0x25241F);
    pub const G800: Color = hex(0x181714);
    pub const G900: Color = hex(0x0E0D0B);
}

pub const WHITE: Color = Color::WHITE;
pub const BLACK: Color = Color::BLACK;
