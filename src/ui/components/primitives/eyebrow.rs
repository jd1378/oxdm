//! Small uppercase "eyebrow" label above a section heading.
//!
//! CSS spec (`.prop-section-head`):
//!
//! ```text
//! font: 700 10px body   color: fg_3
//! text-transform: uppercase   letter-spacing: 0.08em
//! margin-bottom: 8px   padding-left: 2px
//! ```

use eframe::egui::{self, Ui};

use crate::ui::theme;

pub fn eyebrow(ui: &mut Ui, text: &str) {
    let t = theme::tokens(ui.ctx());
    let upper: String = text.to_uppercase();
    let mut layout = egui::text::LayoutJob::default();
    layout.append(
        &upper,
        0.0,
        egui::TextFormat {
            font_id: theme::body_bold(10.0),
            color: t.fg_3,
            // 0.08em at 10px ≈ 0.8px.
            extra_letter_spacing: 0.8,
            ..Default::default()
        },
    );
    egui::Frame::NONE
        // No bottom margin: the surrounding column's item-spacing already
        // separates the head from its card; an extra 8px doubled the gap.
        .inner_margin(egui::Margin {
            left: 2,
            right: 0,
            top: 0,
            bottom: 0,
        })
        .show(ui, |ui| {
            ui.label(layout);
        });
}
