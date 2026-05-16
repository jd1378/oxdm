//! Pill-shaped count badge.

use eframe::egui::{Color32, CornerRadius, Pos2, Sense, Ui, Vec2};

use crate::ui::theme::{self, radius};

pub fn pill_count(ui: &mut Ui, n: usize, fg: Color32, bg: Color32) {
    let txt = ui
        .painter()
        .layout_no_wrap(n.to_string(), theme::body_bold(11.0), fg);
    let pad_x = 6.0;
    let h = 16.0;
    let w = (txt.size().x + pad_x * 2.0).max(h);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::from(radius::PILL as f32), bg);
    let text_pos = Pos2::new(
        rect.center().x - txt.size().x / 2.0,
        rect.center().y - txt.size().y / 2.0,
    );
    ui.painter().galley(text_pos, txt, fg);
}
