//! Search text field with leading magnifier icon.

use eframe::egui::{self, Pos2, Rect, Response, Sense, TextStyle, Ui, Vec2};

use super::control::{CONTROL_H_SM, CONTROL_RADIUS};
use crate::ui::theme;
use crate::ui::utils::icons;

pub fn search_field(ui: &mut Ui, value: &mut String, placeholder: &str, width: f32) -> Response {
    let t = theme::tokens(ui.ctx());
    let w = width.max(60.0);
    let h = CONTROL_H_SM;
    let pad_x = theme::space::S2 as f32;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    ui.painter()
        .rect_filled(rect, CONTROL_RADIUS as f32, t.bg_sunken);

    let icon_size = 13.0;
    let icon_rect = Rect::from_min_size(
        Pos2::new(rect.min.x + pad_x, rect.center().y - icon_size * 0.5),
        Vec2::splat(icon_size),
    );
    icons::icon(ui.ctx(), "search", icon_size, t.fg_3).paint_at(ui, icon_rect);

    let edit_x = icon_rect.max.x + theme::space::S2 as f32;
    let edit_rect = Rect::from_min_max(
        Pos2::new(edit_x, rect.min.y),
        Pos2::new(rect.max.x - pad_x, rect.max.y),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(edit_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.spacing_mut().interact_size.y = 0.0;
    let hint = egui::RichText::new(placeholder).color(t.fg_4);
    let edit = egui::TextEdit::singleline(value)
        .frame(egui::Frame::NONE)
        .margin(Vec2::ZERO)
        .desired_width(edit_rect.width())
        .hint_text(hint)
        .font(TextStyle::Body);
    child.add(edit)
}
