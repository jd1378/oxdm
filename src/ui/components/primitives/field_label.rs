//! Form field label rendered above an input. Distinct from `eyebrow`
//! (which is an accent-coloured section heading): this is a neutral
//! field caption — `fg_2`, weight 600, 11px, 0.02em letter-spacing.

use eframe::egui::{self, InnerResponse, Ui};

use crate::ui::theme;

pub fn field_label(ui: &mut Ui, text: &str) {
    let t = theme::tokens(ui.ctx());
    let upper = text.to_uppercase();
    let mut layout = egui::text::LayoutJob::default();
    layout.append(
        &upper,
        0.0,
        egui::TextFormat {
            font_id: theme::body_bold(11.0),
            color: t.fg_2,
            extra_letter_spacing: 0.22, // 0.02em × 11px
            ..Default::default()
        },
    );
    ui.label(layout);
}

/// Field label + body stacked as one group, with zero internal gap so
/// the input sits flush under its label. Inherits the parent's
/// `item_spacing.y` between field groups — call this inside any
/// vertical `Ui` (including `columns(..)` cells).
pub fn labeled<R>(ui: &mut Ui, text: &str, body: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = theme::space::S1 as f32;
        field_label(ui, text);
        body(ui)
    })
}
