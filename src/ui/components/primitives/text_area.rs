//! Multi-line text input with a south-east drag handle for vertical
//! resize. Visually matches [`TextInput`] (same radius + stroke), but
//! uses `bg_page` for the field fill and a fixed 7px × 9px inner pad
//! per the design tokens.
//!
//! Height is persisted in egui memory keyed by `id_source` so the
//! component remembers the user's preferred size across frames.

use eframe::egui::{self, Color32, FontId, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};

use super::control::{CONTROL_RADIUS, INPUT_PAD_X, INPUT_PAD_Y};
use crate::ui::theme;

pub struct TextArea<'a> {
    value: &'a mut String,
    id_source: &'a str,
    hint: Option<String>,
    width: Option<f32>,
    font: Option<FontId>,
    initial_height: f32,
    min_height: f32,
    max_height: f32,
}

impl<'a> TextArea<'a> {
    pub fn new(value: &'a mut String, id_source: &'a str) -> Self {
        Self {
            value,
            id_source,
            hint: None,
            width: None,
            font: None,
            initial_height: 96.0,
            min_height: 60.0,
            max_height: 400.0,
        }
    }
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
    pub fn width(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }
    pub fn font(mut self, f: FontId) -> Self {
        self.font = Some(f);
        self
    }
    pub fn initial_height(mut self, h: f32) -> Self {
        self.initial_height = h;
        self
    }
    pub fn min_height(mut self, h: f32) -> Self {
        self.min_height = h;
        self
    }
    pub fn max_height(mut self, h: f32) -> Self {
        self.max_height = h;
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let t = theme::tokens(ui.ctx());
        let pad_x = INPUT_PAD_X;
        let pad_y = INPUT_PAD_Y;
        let grip_size = 10.0;

        let outer_w = self.width.unwrap_or_else(|| ui.available_width());
        let height_id = egui::Id::new(("oxdm-text-area-h", self.id_source));
        let mut height: f32 = ui
            .ctx()
            .data_mut(|d| *d.get_persisted_mut_or(height_id, self.initial_height))
            .clamp(self.min_height, self.max_height);

        let (rect, _bg_resp) = ui.allocate_exact_size(Vec2::new(outer_w, height), Sense::hover());

        // Frame.
        let radius = CONTROL_RADIUS as f32;
        ui.painter().rect_filled(rect, radius, t.bg_raised);
        ui.painter().rect_stroke(
            rect.shrink(t.border_width * 0.5),
            radius,
            Stroke::new(t.border_width, t.border_subtle),
            egui::StrokeKind::Middle,
        );

        // Inner text edit area.
        let inner = Rect::from_min_max(
            Pos2::new(rect.min.x + pad_x, rect.min.y + pad_y),
            Pos2::new(rect.max.x - pad_x, rect.max.y - pad_y),
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inner)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        let font = self.font.unwrap_or_else(|| theme::body(13.0));
        let mut edit = egui::TextEdit::multiline(self.value)
            .frame(egui::Frame::NONE)
            .margin(Vec2::ZERO)
            .desired_width(inner.width())
            .desired_rows(1)
            .font(font);
        if let Some(h) = self.hint {
            edit = edit.hint_text(egui::RichText::new(h).color(t.fg_4));
        }
        let edit_resp = child.add_sized(inner.size(), edit);

        // South-east resize grip. Drag the handle vertically to resize.
        let grip_rect = Rect::from_min_max(
            Pos2::new(rect.max.x - grip_size, rect.max.y - grip_size),
            rect.max,
        );
        let grip_id = egui::Id::new(("oxdm-text-area-grip", self.id_source));
        let grip = ui.interact(grip_rect, grip_id, Sense::drag());
        if grip.hovered() || grip.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        if grip.dragged() {
            height = (height + grip.drag_delta().y).clamp(self.min_height, self.max_height);
            ui.ctx().data_mut(|d| d.insert_persisted(height_id, height));
            ui.ctx().request_repaint();
        }

        // Paint grip: three short diagonal hairlines in the SE corner.
        // Hide while idle — only reveal on hover so the textarea matches
        // the design handoff in its rest state (handoff omits the grip).
        if grip.hovered() || grip.dragged() {
            paint_grip(ui, grip_rect, t.fg_2);
        }

        edit_resp
    }
}

fn paint_grip(ui: &Ui, rect: Rect, color: Color32) {
    let p = ui.painter();
    let stroke = Stroke::new(1.0, color);
    // Three diagonal ticks from SE corner moving inward.
    let pad = 2.0;
    let max = rect.max - Vec2::splat(pad);
    for i in 0..3 {
        let step = 3.0 * (i as f32 + 1.0);
        let a = Pos2::new(max.x - step, max.y);
        let b = Pos2::new(max.x, max.y - step);
        p.line_segment([a, b], stroke);
    }
}
