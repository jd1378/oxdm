//! Lucide-icon helper. Bundles the same curated Lucide SVG set as the
//! egui UI (shared `icons_table.in`) and renders through iced's `svg`
//! widget, which rasterises via resvg on the tiny-skia backend.
//!
//! Tinting uses `svg::Style { color }` (iced recolors the rasterised
//! glyph), so no per-color SVG rewriting is needed. Lucide ships at
//! stroke-width=2 on a 24px grid; at in-app sizes (14–20px) that reads
//! heavy, so the bytes are rewritten once per icon to ~1.75 and cached.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use iced::widget::svg;
use iced::{Color, Element, Length};

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/gui/icons_table.in"
));

pub fn raw_svg(name: &str) -> Option<&'static [u8]> {
    ICONS.iter().find(|(n, _)| *n == name).map(|(_, b)| *b)
}

/// Lucide ships at stroke-width=2 (24px grid). At in-app sizes (14–20px)
/// that reads heavy/mechanical; trim to ~1.75 for a lighter feel without
/// losing presence.
const STROKE_WIDTH: &str = "1.75";

fn handle(name: &str) -> svg::Handle {
    static CACHE: OnceLock<Mutex<HashMap<&'static str, svg::Handle>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().unwrap();
    if let Some((static_name, bytes)) = ICONS.iter().find(|(n, _)| *n == name) {
        cache
            .entry(static_name)
            .or_insert_with(|| {
                let src = String::from_utf8_lossy(bytes);
                let thinned = src.replace(
                    "stroke-width=\"2\"",
                    &format!("stroke-width=\"{STROKE_WIDTH}\""),
                );
                svg::Handle::from_memory(thinned.into_bytes())
            })
            .clone()
    } else {
        // Render nothing, like the egui icon helper did — a missing
        // icon must not take the window down. Warn once per name
        // (handle() runs every frame).
        static WARNED: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
        let warned = WARNED.get_or_init(|| Mutex::new(std::collections::HashSet::new()));
        if warned.lock().unwrap().insert(name.to_owned()) {
            tracing::warn!("unknown icon name: {name}");
        }
        svg::Handle::from_memory(Vec::new())
    }
}

/// Icon tinted with a fixed color.
pub fn icon<'a, M: 'a>(name: &str, size: f32, color: Color) -> Element<'a, M> {
    svg(handle(name))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .style(move |_theme, _status| svg::Style { color: Some(color) })
        .into()
}

/// Icon that swaps tint when its interactive ancestor is hovered
/// (iced propagates hover to the svg's own `Status`).
pub fn icon_dyn<'a, M: 'a>(name: &str, size: f32, idle: Color, hovered: Color) -> Element<'a, M> {
    svg(handle(name))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .style(move |_theme, status| svg::Style {
            color: Some(match status {
                svg::Status::Idle => idle,
                svg::Status::Hovered => hovered,
            }),
        })
        .into()
}
