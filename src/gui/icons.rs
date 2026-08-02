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

/// Lucide ships at stroke-width=2 on a 24px grid, which is also what the
/// design specifies for every glyph (`styles.css`: `.btn svg`,
/// `.tb-btn svg`, `.nav-item .icon svg`, …). The egui port thinned it to
/// 1.75 for a lighter feel; that cost real ink once glyphs shrank to the
/// design's sizes — at 16px a 1.75 stroke rasterises to 1.17px, so
/// antialiasing renders it as a partly-transparent line. Keep the
/// design's 2.
const STROKE_WIDTH: &str = "2";

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
    Element::new(SnappedIcon {
        handle: handle(name),
        size,
        tint: Tint::Fixed(color),
    })
}

/// Icon that swaps tint when the pointer is over **the icon itself**.
///
/// `svg::Status::Hovered` is computed from the svg's own bounds, NOT an
/// interactive ancestor's — inside a button this makes the icon recolor
/// only when the cursor is directly on the glyph, out of step with the
/// label beside it. Prefer [`icon_current`] there; this is for icons
/// that really are their own hit target.
pub fn icon_dyn<'a, M: 'a>(name: &str, size: f32, idle: Color, hovered: Color) -> Element<'a, M> {
    Element::new(SnappedIcon {
        handle: handle(name),
        size,
        tint: Tint::Hover { idle, hovered },
    })
}

/// Icon painted in the **inherited** foreground color — the CSS
/// `currentColor` behaviour the design assumes.
///
/// `button` draws its content with `renderer::Style { text_color }` set
/// from its own per-status style, which is how a `text` child follows
/// hover/press/disabled. The stock `svg` widget ignores that style and
/// takes an explicit color instead, so icons fell out of step with the
/// labels next to them. This widget reads `text_color` at draw time, so
/// one element covers every status of every interactive ancestor with
/// no hover state to track.
pub fn icon_current<'a, M: 'a>(name: &str, size: f32) -> Element<'a, M> {
    Element::new(SnappedIcon {
        handle: handle(name),
        size,
        tint: Tint::Current,
    })
}

/// How a [`SnappedIcon`] picks its color.
enum Tint {
    /// One fixed color.
    Fixed(Color),
    /// Swaps on hover over the glyph's own bounds — the stock `svg`
    /// widget's `Status` semantics.
    Hover { idle: Color, hovered: Color },
    /// Inherit the ancestor's `text_color` (CSS `currentColor`).
    Current,
}

/// Every icon goes through this widget so the glyph always lands on the
/// pixel grid: Lucide strokes are ~1.3px at the design's sizes, and a
/// fractional origin splits one across two pixel columns, which reads as
/// a faded glyph rather than a soft one.
struct SnappedIcon {
    handle: svg::Handle,
    size: f32,
    tint: Tint,
}

impl<M, R> iced::advanced::Widget<M, iced::Theme, R> for SnappedIcon
where
    R: iced::advanced::svg::Renderer,
{
    fn size(&self) -> iced::Size<Length> {
        iced::Size::new(Length::Fixed(self.size), Length::Fixed(self.size))
    }

    fn layout(
        &mut self,
        _tree: &mut iced::advanced::widget::Tree,
        _renderer: &R,
        limits: &iced::advanced::layout::Limits,
    ) -> iced::advanced::layout::Node {
        iced::advanced::layout::Node::new(limits.resolve(
            Length::Fixed(self.size),
            Length::Fixed(self.size),
            iced::Size::new(self.size, self.size),
        ))
    }

    fn draw(
        &self,
        _tree: &iced::advanced::widget::Tree,
        renderer: &mut R,
        _theme: &iced::Theme,
        style: &iced::advanced::renderer::Style,
        layout: iced::advanced::Layout<'_>,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &iced::Rectangle,
    ) {
        // Round onto the pixel grid: layout centring routinely lands a
        // glyph on a fractional origin, and a hairline stroke split
        // across two pixel columns reads as washed-out rather than
        // merely soft.
        let b = layout.bounds();
        let bounds = iced::Rectangle {
            x: b.x.round(),
            y: b.y.round(),
            ..b
        };
        let color = match self.tint {
            Tint::Fixed(color) => color,
            Tint::Hover { idle, hovered } => {
                if cursor.is_over(bounds) {
                    hovered
                } else {
                    idle
                }
            }
            Tint::Current => style.text_color,
        };
        renderer.draw_svg(
            iced::advanced::svg::Svg::new(self.handle.clone()).color(color),
            bounds,
            *viewport,
        );
    }
}
