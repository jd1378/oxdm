//! Password input — `TextEdit` framed identically to a `TextInput`,
//! with a trailing eye icon rendered *inside* the same rounded frame.
//!
//! Behaviour:
//! - **Hold-to-reveal.** The field is masked by default. While the
//!   user holds the mouse button down on the eye icon, the input is
//!   shown in plaintext; releasing snaps it back to masked. No
//!   sticky toggle — single-glance check only.
//!
//! The bound `&mut String` is always the plaintext value. Callers
//! that need to prefill from a stored secret should decrypt at
//! dialog-open time and assign the plaintext directly; the component
//! itself stays stateless aside from egui's normal text-edit memory.

use eframe::egui::{self, Align, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};

use super::control::{CONTROL_H_MD, CONTROL_RADIUS, INPUT_PAD_X};
use crate::ui::theme;
use crate::ui::utils::icons;

pub struct PasswordInput<'a> {
    value: &'a mut String,
    id_salt: egui::Id,
    hint: Option<String>,
    width: Option<f32>,
    font: Option<egui::FontId>,
    enabled: bool,
}

impl<'a> PasswordInput<'a> {
    pub fn new(value: &'a mut String, id_salt: impl std::hash::Hash) -> Self {
        Self {
            value,
            id_salt: egui::Id::new(("password-input", id_salt)),
            hint: None,
            width: None,
            font: None,
            enabled: true,
        }
    }
    pub fn hint(mut self, h: impl Into<String>) -> Self {
        self.hint = Some(h.into());
        self
    }
    pub fn width(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }
    pub fn font(mut self, f: egui::FontId) -> Self {
        self.font = Some(f);
        self
    }
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let t = theme::tokens(ui.ctx());
        let outer_w = self.width.unwrap_or_else(|| ui.available_width());
        let h = CONTROL_H_MD;
        let pad_x = INPUT_PAD_X;
        let radius = CONTROL_RADIUS as f32;
        let stroke = Stroke::new(t.border_width, t.border_subtle);
        let icon_size = 16.0;
        let icon_cell_w = h;

        let (rect, _bg) = ui.allocate_exact_size(Vec2::new(outer_w, h), Sense::hover());
        let paint_rect = rect.expand(t.border_width);
        ui.painter().rect(
            paint_rect,
            radius,
            t.bg_raised,
            stroke,
            egui::StrokeKind::Inside,
        );

        // Eye only makes sense once there is something to peek at.
        // An empty field with a dangling toggle reads as decoration.
        let show_eye = !self.value.is_empty();
        let icon_rect =
            Rect::from_min_max(Pos2::new(rect.max.x - icon_cell_w, rect.min.y), rect.max);
        // One id regardless of state so the rect's id stays stable
        // across frames as the eye appears / disappears with the
        // value — egui warns otherwise.
        let eye_id = self.id_salt.with("eye");
        let sense = if show_eye && self.enabled {
            Sense::click_and_drag()
        } else {
            Sense::hover()
        };
        let icon_resp = {
            let r = ui.interact(icon_rect, eye_id, sense);
            if show_eye && self.enabled {
                r.on_hover_cursor(egui::CursorIcon::PointingHand)
            } else {
                r
            }
        };
        let revealing = show_eye && self.enabled && icon_resp.is_pointer_button_down_on();

        let edit_rect = Rect::from_min_max(
            Pos2::new(rect.min.x + pad_x, rect.min.y),
            Pos2::new(rect.max.x - icon_cell_w, rect.max.y),
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(edit_rect)
                .layout(egui::Layout::left_to_right(Align::Center)),
        );
        child.spacing_mut().interact_size.y = 0.0;
        let font = self.font.unwrap_or_else(|| theme::body(13.0));
        let mut edit = egui::TextEdit::singleline(self.value)
            .frame(egui::Frame::NONE)
            .margin(Vec2::ZERO)
            .desired_width(edit_rect.width())
            .font(font)
            .vertical_align(Align::Center)
            .password(!revealing);
        if let Some(h) = self.hint {
            edit = edit.hint_text(egui::RichText::new(h).color(t.fg_4));
        }
        let text_resp = child.add_enabled(self.enabled, edit);

        if show_eye {
            let color = if !self.enabled {
                t.fg_4
            } else if icon_resp.is_pointer_button_down_on() {
                t.fg_1
            } else if icon_resp.hovered() {
                t.fg_2
            } else {
                t.fg_3
            };
            let icon = if revealing { "eye-off" } else { "eye" };
            icons::icon(ui.ctx(), icon, icon_size, color).paint_at(
                ui,
                Rect::from_center_size(icon_rect.center(), Vec2::splat(icon_size)),
            );
        }

        if !self.enabled && ui.rect_contains_pointer(rect) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::NotAllowed);
        }

        text_resp
    }
}
