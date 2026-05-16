//! Status dot + label pair.

use eframe::egui::{Color32, RichText, Sense, Ui, Vec2, WidgetText};

use super::util::text_string;
use crate::ui::theme;

pub fn status_dot(ui: &mut Ui, color: Color32, label: impl Into<WidgetText>, font_size: f32) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
        ui.painter().circle_filled(rect.center(), 4.0, color);
        let label_text = label.into();
        ui.label(
            RichText::new(text_string(&label_text))
                .color(color)
                .font(theme::body_bold(font_size)),
        );
    });
}
