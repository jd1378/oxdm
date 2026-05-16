//! Numeric stepper: a small bordered control with `-` button, value,
//! `+` button. Clamps to a closed `[min, max]` range. Matches the
//! Properties dialog `NumberStepper` from the design handoff
//! (09b_properties_dialog.md §4.6).

use eframe::egui::{self, Color32, CornerRadius, Sense, Stroke, StrokeKind, Ui, Vec2};

use super::control::{CONTROL_H_MD, CONTROL_RADIUS};
use crate::ui::theme::{self, ts};
use crate::ui::utils::icons;

pub struct NumberStepper<'a> {
    value: &'a mut i64,
    min: i64,
    max: i64,
    width: f32,
    enabled: bool,
    suffix: Option<&'static str>,
    id_source: &'a str,
}

impl<'a> NumberStepper<'a> {
    pub fn new(value: &'a mut i64, id_source: &'a str) -> Self {
        Self {
            value,
            min: i64::MIN,
            max: i64::MAX,
            width: 88.0,
            enabled: true,
            suffix: None,
            id_source,
        }
    }
    pub fn range(mut self, min: i64, max: i64) -> Self {
        self.min = min;
        self.max = max;
        self
    }
    pub fn width(mut self, w: f32) -> Self {
        self.width = w;
        self
    }
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }
    pub fn suffix(mut self, s: &'static str) -> Self {
        self.suffix = Some(s);
        self
    }

    pub fn show(self, ui: &mut Ui) -> egui::Response {
        let t = theme::tokens(ui.ctx());
        let h = CONTROL_H_MD;
        let btn_w = 28.0;
        let mid_w = (self.width - 2.0 * btn_w).max(28.0);

        // Reserve `width × h` via the high-level allocator. We then
        // manually advance the cursor in case the placer's alignment
        // doesn't push parent layouts past the right edge.
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(self.width, h), Sense::hover());
        let r: CornerRadius = (CONTROL_RADIUS as f32).into();
        ui.painter().rect_filled(rect, r, t.bg_raised);
        ui.painter().rect_stroke(
            rect,
            r,
            Stroke::new(t.border_width, t.border_subtle),
            StrokeKind::Inside,
        );

        let left_rect = egui::Rect::from_min_size(rect.left_top(), Vec2::new(btn_w, h));
        let mid_rect = egui::Rect::from_min_size(left_rect.right_top(), Vec2::new(mid_w, h));
        let right_rect = egui::Rect::from_min_size(mid_rect.right_top(), Vec2::new(btn_w, h));

        // Buttons paint as transparent overlays with hover tint.
        let paint_btn =
            |ui: &Ui, r: egui::Rect, name: &'static str, enabled: bool| -> egui::Response {
                let id =
                    egui::Id::new(("number-stepper-btn", name, r.left() as i32, r.top() as i32));
                let resp = ui.interact(r, id, Sense::click());
                if enabled && resp.hovered() {
                    ui.painter()
                        .rect_filled(r, CornerRadius::ZERO, t.row_hover_bg);
                }
                let color = if enabled { t.fg_2 } else { t.fg_4 };
                let ic = icons::icon(ui.ctx(), name, 14.0, color);
                let ic_rect = egui::Rect::from_center_size(r.center(), Vec2::splat(14.0));
                ic.paint_at(ui, ic_rect);
                resp
            };
        let can_dec = self.enabled && *self.value > self.min;
        let can_inc = self.enabled && *self.value < self.max;
        let dec = paint_btn(ui, left_rect, "minus", can_dec);
        let inc = paint_btn(ui, right_rect, "plus", can_inc);

        if can_dec && dec.clicked() {
            *self.value = (*self.value - 1).max(self.min);
        }
        if can_inc && inc.clicked() {
            *self.value = (*self.value + 1).min(self.max);
        }

        // Value (mono, centered).
        let mut text = self.value.to_string();
        // Allow direct editing as text via embedded TextEdit.
        let id = egui::Id::new(("number-stepper-edit", self.id_source));
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(mid_rect.shrink2(Vec2::new(4.0, 2.0))),
            |ui| {
                ui.add_enabled_ui(self.enabled, |ui| {
                    let edit = egui::TextEdit::singleline(&mut text)
                        .id(id)
                        .frame(egui::Frame::NONE)
                        .horizontal_align(egui::Align::Center)
                        .font(ts::mono_sm())
                        .text_color(t.fg_1)
                        .desired_width(mid_w - 8.0);
                    let r = ui.add(edit);
                    if r.changed() {
                        if let Ok(v) = text.trim().parse::<i64>() {
                            *self.value = v.clamp(self.min, self.max);
                        }
                    }
                });
            },
        );

        // Vertical separators between segments.
        let sep = Stroke::new(t.border_width, t.border_subtle);
        ui.painter()
            .line_segment([left_rect.right_top(), left_rect.right_bottom()], sep);
        ui.painter()
            .line_segment([right_rect.left_top(), right_rect.left_bottom()], sep);

        if let Some(suffix) = self.suffix {
            ui.label(
                egui::RichText::new(suffix)
                    .color(t.fg_3)
                    .font(theme::body(12.0)),
            );
        }

        // Re-advance the parent cursor past the full stepper rect.
        // The inner `scope_builder` for the embedded TextEdit
        // *rewinds* parent's cursor to its (smaller) min_rect, so a
        // sibling widget rendered right after a NumberStepper would
        // paint on top of the stepper's right-hand "+" button. Bumping
        // the cursor here restores the expected left-to-right rhythm.
        ui.advance_cursor_after_rect(rect);

        // Silence unused warnings.
        let _ = (Color32::TRANSPARENT, resp);
        ui.interact(
            rect,
            egui::Id::new(("ns-outer", self.id_source)),
            Sense::hover(),
        )
    }
}
