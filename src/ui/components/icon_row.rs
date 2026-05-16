//! Horizontal card row used by header-style cards.
//!
//! Layout: square icon tile on the left, a title + subtitle column in
//! the middle that grows, and a right cell (e.g. SIZE eyebrow or % readout).
//! All three cells share the same vertical center.

use eframe::egui::{self, Align2, Rect, Sense, Ui, Vec2};
use egui_flex::{Flex, FlexAlign, FlexItem};

use crate::ui::theme::space;

pub fn icon_row(
    ui: &mut Ui,
    tile: f32,
    paint_tile: impl FnOnce(&mut Ui, Rect),
    middle: impl FnOnce(&mut Ui),
    right: impl FnOnce(&mut Ui),
) {
    Flex::horizontal()
        .align_items(FlexAlign::Center)
        .gap(Vec2::new(space::S3 as f32, 0.0))
        .w_full()
        .show(ui, |flex| {
            flex.add_ui(FlexItem::new(), |ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(tile), Sense::hover());
                paint_tile(ui, rect);
            });
            flex.add_ui(
                FlexItem::new()
                    .grow(1.0)
                    // `.shrink()` tells flex this cell may collapse
                    // below its intrinsic width when the row overflows.
                    // Without it, an unbounded child label widens the
                    // cell past the row and the labels never truncate.
                    .shrink()
                    .align_self_content(Align2::LEFT_CENTER),
                |ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().interact_size.y = 0.0;
                        ui.spacing_mut().item_spacing.y = space::S0 as f32;
                        // Default wrap mode → Truncate so unbounded
                        // labels (filename, host) collapse with ellipsis
                        // instead of inflating the flex cell past the
                        // row. Labels still report their min width as
                        // ~ellipsis-glyph-only, letting flex compute a
                        // sane allocation.
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                        middle(ui);
                    });
                },
            );
            flex.add_ui(FlexItem::new(), right);
        });
}
