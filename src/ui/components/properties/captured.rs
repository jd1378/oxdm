//! Captured request / captured response table — properly framed card
//! with a small-caps eyebrow header and bordered rows.
//!
//! Replaces the inline `captured_kv` pattern from the Headers tab.

use eframe::egui::{self, RichText, Stroke, Vec2};

use crate::ui::theme::{self, radius, space, ts};

/// Render a bordered card whose rows are the (name, value) pairs
/// supplied by the caller. Each row has a hairline divider between
/// siblings; the outer card has a single border.
///
/// Name column is mono-sm fg_2, value column is mono-sm fg_1. The caller
/// is expected to render the small-caps eyebrow above this widget.
pub fn captured_table(ui: &mut egui::Ui, t: &theme::Tokens, rows: &[(&str, &str)]) {
    let name_col_w: f32 = 140.0;

    egui::Frame::NONE
        .fill(t.bg_surface)
        .stroke(Stroke::new(t.border_width, t.border_subtle))
        .corner_radius(radius::SM as f32)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            for (i, (name, value)) in rows.iter().enumerate() {
                if i > 0 {
                    // Hairline divider between rows.
                    let (rect, _) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), t.border_width),
                        egui::Sense::hover(),
                    );
                    ui.painter().rect_filled(rect, 0.0, t.border_subtle);
                }
                egui::Frame::NONE
                    // `.prop-hdr-row { padding: 6px 12px; }`.
                    .inner_margin(egui::Margin::symmetric(12, 6))
                    .show(ui, |ui| {
                        // Drop the default 18px interact-size floor so the
                        // row's height is set by the 11px mono text +
                        // padding (matches `.prop-hdr-row`'s tight rhythm).
                        ui.spacing_mut().interact_size.y = 0.0;
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = space::S3 as f32;
                            // Name column — fixed 140px wide, height
                            // follows the label rather than a hardcoded
                            // 18 (which inflated rows).
                            let row_h = ui.text_style_height(&egui::TextStyle::Body);
                            let (name_rect, _) = ui.allocate_exact_size(
                                Vec2::new(name_col_w, row_h),
                                egui::Sense::hover(),
                            );
                            ui.painter().text(
                                name_rect.left_center(),
                                egui::Align2::LEFT_CENTER,
                                *name,
                                ts::mono_sm(),
                                t.fg_2,
                            );
                            ui.label(RichText::new(*value).color(t.fg_1).font(ts::mono_sm()));
                        });
                    });
            }
        });
}
