//! Lucide-icon helper. Bundles a curated set of Lucide SVGs and
//! renders them through `egui_extras` (svg → texture via resvg).
//!
//! Lucide SVGs use `stroke="currentColor"`. resvg does not understand
//! `currentColor`, so we rewrite it on the fly to a hex literal matching
//! the requested colour and serve the result with a cache-keyed
//! `bytes://` URI.
//!
//! Crispness: `egui_extras::SvgLoader` rasterises at the `SizeHint`
//! egui passes, ignoring SVG `width`/`height`. To avoid the fractional
//! stroke-width blur Lucide icons exhibit at non-24px sizes (egui issue
//! 3501), we load the texture directly with `try_load_texture` at a
//! `SizeHint` 2× the display's physical pixel size, then paint it at
//! the requested rect. The GPU minification step is sharper than
//! resvg rasterising at the exact display pixels.

use eframe::egui::{
    self, Color32, Pos2, Rect, Response, Sense, TextureOptions, Ui, Vec2,
    load::{SizeHint, TexturePoll},
};

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/ui/utils/icons_table.in"
));

pub fn raw_svg(name: &str) -> Option<&'static [u8]> {
    ICONS.iter().find(|(n, _)| *n == name).map(|(_, b)| *b)
}

/// Lucide ships at stroke-width=2 (24px grid). At in-app sizes (14–20px)
/// that reads heavy/mechanical; trim to ~1.75 for a lighter feel without
/// losing presence.
const STROKE_WIDTH: f32 = 1.75;

pub fn ensure_bytes(ctx: &egui::Context, name: &'static str, color: Color32) -> Option<String> {
    let stroke_centi = (STROKE_WIDTH * 100.0).round() as u32;
    let uri = format!(
        "bytes://lucide-{}-{:02X}{:02X}{:02X}-s{}.svg",
        name,
        color.r(),
        color.g(),
        color.b(),
        stroke_centi,
    );
    let key = egui::Id::new(("icon-bytes-registered", &uri));
    let already = ctx.data(|d| d.get_temp::<bool>(key).unwrap_or(false));
    if !already {
        let raw = raw_svg(name)?;
        let s = std::str::from_utf8(raw).ok()?;
        let hex = format!("#{:02X}{:02X}{:02X}", color.r(), color.g(), color.b());
        let stroke = format!("{STROKE_WIDTH}");
        let replaced = s
            .replace("currentColor", &hex)
            .replace("stroke-width=\"2\"", &format!("stroke-width=\"{stroke}\""));
        ctx.include_bytes(uri.clone(), replaced.into_bytes());
        ctx.data_mut(|d| d.insert_temp(key, true));
    }
    Some(uri)
}

fn paint_uri(ui: &Ui, rect: Rect, uri: &str) {
    let ctx = ui.ctx();
    let ppp = ctx.pixels_per_point().max(1.0);
    let max_side = rect.width().max(rect.height()).max(1.0);
    // Raster at the integer multiple of Lucide's 24px source grid that's
    // nearest-above the icon's *physical* pixel size, so the 2-unit stroke
    // maps to whole raster pixels (uniform AA). We deliberately do NOT
    // oversample 2× here: for small icons (≤24px) that forced a 48px raster
    // downsampled >3× with bilinear (no mipmaps), which aliased the thin
    // strokes into a jagged look. A ~native raster (24px for a 13–20px
    // icon) downscales gently and reads crisp.
    let phys = max_side * ppp;
    let raster_px = ((phys / 24.0).ceil().max(1.0) * 24.0) as u32;
    // Snap paint rect to physical pixel grid so strokes don't sample at
    // sub-pixel offsets (causes per-icon stroke-width variance).
    let snap = |v: f32| (v * ppp).round() / ppp;
    let snapped = Rect::from_min_max(
        Pos2::new(snap(rect.min.x), snap(rect.min.y)),
        Pos2::new(snap(rect.max.x), snap(rect.max.y)),
    );
    let opts = TextureOptions::LINEAR;
    match ctx.try_load_texture(
        uri,
        opts,
        SizeHint::Size {
            width: raster_px,
            height: raster_px,
            maintain_aspect_ratio: true,
        },
    ) {
        Ok(TexturePoll::Ready { texture }) => {
            ui.painter().image(
                texture.id,
                snapped,
                Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        Ok(TexturePoll::Pending { .. }) => {
            ctx.request_repaint();
        }
        Err(_) => {}
    }
}

/// Icon handle with deferred rendering via `paint_at` or `Widget`.
pub struct Icon {
    uri: Option<String>,
    size: f32,
}

impl Icon {
    pub fn paint_at(&self, ui: &Ui, rect: Rect) {
        if let Some(u) = &self.uri {
            paint_uri(ui, rect, u);
        }
    }
}

impl egui::Widget for Icon {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(self.size), Sense::hover());
        if let Some(u) = &self.uri {
            paint_uri(ui, rect, u);
        }
        resp
    }
}

/// Build an [`Icon`] for the given Lucide name at `size` (egui points),
/// tinted to `color`.
pub fn icon(ctx: &egui::Context, name: &'static str, size: f32, color: Color32) -> Icon {
    Icon {
        uri: ensure_bytes(ctx, name, color),
        size,
    }
}

/// Allocate `size`×`size` and paint the icon inline.
pub fn show(ui: &mut Ui, name: &'static str, size: f32, color: Color32) -> Response {
    let i = icon(ui.ctx(), name, size, color);
    ui.add(i)
}

/// Same as [`show`] but uses the active theme's primary text colour.
pub fn show_default(ui: &mut Ui, name: &'static str, size: f32) -> Response {
    let c = crate::ui::theme::tokens(ui.ctx()).fg_2;
    show(ui, name, size, c)
}

pub fn install_loaders(ctx: &egui::Context) {
    egui_extras::install_image_loaders(ctx);
}
