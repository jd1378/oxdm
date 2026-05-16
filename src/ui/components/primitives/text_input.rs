//! Single-line text input matching the project's control sizing.
//!
//! Wraps `egui::TextEdit::singleline` in a `Frame` so every input renders
//! at `CONTROL_H_MD` tall with `CONTROL_RADIUS` corners — same shape as
//! buttons and dropdowns. Caller drives width, hint, font, password, and
//! enabled state via builder methods.

use eframe::egui::{
    self, Align, FontId, Layout, Pos2, Rect, Response, Sense, Stroke, TextEdit, Ui, Vec2,
};

use super::control::{CONTROL_H_MD, CONTROL_RADIUS, INPUT_PAD_X};
use crate::ui::theme;

pub struct TextInput<'a> {
    value: &'a mut String,
    hint: Option<String>,
    width: Option<f32>,
    font: Option<FontId>,
    password: bool,
    enabled: bool,
    char_limit: Option<usize>,
}

impl<'a> TextInput<'a> {
    pub fn new(value: &'a mut String) -> Self {
        Self {
            value,
            hint: None,
            width: None,
            font: None,
            password: false,
            enabled: true,
            char_limit: None,
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
    pub fn password(mut self, p: bool) -> Self {
        self.password = p;
        self
    }
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }
    pub fn char_limit(mut self, n: usize) -> Self {
        self.char_limit = Some(n);
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let t = theme::tokens(ui.ctx());
        let outer_w = self.width.unwrap_or_else(|| ui.available_width());
        let h = CONTROL_H_MD;
        let pad_x = INPUT_PAD_X;
        let radius = CONTROL_RADIUS as f32;
        let stroke = Stroke::new(t.border_width, t.border_subtle);

        // Allocate the exact outer rect — same shape as `Combo` so adjacent
        // controls share pixel-perfect baselines. Painting the frame at
        // `rect.expand(border_width)` mirrors how `egui::Frame` inflates
        // its widget rect by `inner_margin + stroke_width`, so a TextInput
        // and a Combo allocated at the same `(outer_w, h)` end up with
        // identical visible footprints.
        let (rect, frame_resp) = ui.allocate_exact_size(Vec2::new(outer_w, h), Sense::hover());
        let paint_rect = rect.expand(t.border_width);
        ui.painter().rect(
            paint_rect,
            radius,
            t.bg_raised,
            stroke,
            egui::StrokeKind::Inside,
        );

        let inner_rect = Rect::from_min_max(
            Pos2::new(rect.min.x + pad_x, rect.min.y),
            Pos2::new(rect.max.x - pad_x, rect.max.y),
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inner_rect)
                .layout(Layout::left_to_right(Align::Center)),
        );
        let font = self.font.unwrap_or_else(|| theme::body(13.0));
        let mut edit = TextEdit::singleline(self.value)
            .frame(egui::Frame::NONE)
            .margin(Vec2::ZERO)
            .desired_width(inner_rect.width())
            .font(font)
            .vertical_align(Align::Center)
            .password(self.password);
        if let Some(h) = self.hint {
            edit = edit.hint_text(egui::RichText::new(h).color(t.fg_4));
        }
        if let Some(n) = self.char_limit {
            edit = edit.char_limit(n);
        }
        let edit_resp = child.add_enabled(self.enabled, edit);

        if !self.enabled && frame_resp.contains_pointer() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::NotAllowed);
        }
        edit_resp
    }
}

/// Convenience wrapper for the common case (fixed width + hint).
pub fn text_input(ui: &mut Ui, value: &mut String, hint: &str, width: f32) -> Response {
    TextInput::new(value).hint(hint).width(width).show(ui)
}
