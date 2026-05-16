//! Pill switch (on/off). Matches `design/tokens.css .s-toggle`:
//! 36×20 track, white knob, clay fill when on, `bg_sunken` when off.

use eframe::egui::{self, Color32, CornerRadius, Sense, Stroke, StrokeKind, Ui, Vec2};

use crate::ui::theme::{self, motion};

const TRACK_W: f32 = 36.0;
const TRACK_H: f32 = 20.0;
const KNOB: f32 = 16.0;

pub struct Toggle<'a> {
    on: &'a mut bool,
    enabled: bool,
    id_source: Option<egui::Id>,
}

impl<'a> Toggle<'a> {
    pub fn new(on: &'a mut bool) -> Self {
        Self {
            on,
            enabled: true,
            id_source: None,
        }
    }

    pub fn enabled(mut self, v: bool) -> Self {
        self.enabled = v;
        self
    }

    pub fn id(mut self, id: egui::Id) -> Self {
        self.id_source = Some(id);
        self
    }

    pub fn show(self, ui: &mut Ui) -> egui::Response {
        let t = theme::tokens(ui.ctx());
        let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(TRACK_W, TRACK_H), Sense::click());
        if !self.enabled {
            resp = resp.on_hover_cursor(egui::CursorIcon::NotAllowed);
        } else {
            resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        }
        if self.enabled && resp.clicked() {
            *self.on = !*self.on;
            resp.mark_changed();
        }

        // Animate the on/off transition (linear → ease-out).
        let anim_id = self
            .id_source
            .unwrap_or_else(|| resp.id.with("toggle-anim"));
        let lin = ui.ctx().animate_value_with_time(
            anim_id,
            if *self.on { 1.0 } else { 0.0 },
            motion::FAST,
        );
        let k = motion::ease_out(lin);

        let bg_off = t.bg_sunken;
        let bg_on = t.action_primary;
        let border_off = t.border_default;
        let border_on = t.action_primary_press;
        let bg = motion::lerp_color(bg_off, bg_on, k);
        let border = motion::lerp_color(border_off, border_on, k);

        let painter = ui.painter();
        let r: CornerRadius = CornerRadius::same((TRACK_H / 2.0) as u8);
        let alpha = if self.enabled { 1.0 } else { 0.5 };
        painter.rect_filled(rect, r, with_alpha(bg, alpha));
        painter.rect_stroke(
            rect,
            r,
            Stroke::new(1.0, with_alpha(border, alpha)),
            StrokeKind::Inside,
        );

        // Symmetric 2px inset on all sides (matches vertical centering).
        let inset = (TRACK_H - KNOB) / 2.0;
        let x_off = rect.left() + inset;
        let x_on = rect.right() - inset - KNOB;
        let x = x_off + (x_on - x_off) * k;
        let y = rect.top() + inset;
        let knob_rect = egui::Rect::from_min_size(egui::pos2(x, y), Vec2::splat(KNOB));
        let knob_color = with_alpha(Color32::WHITE, alpha);
        let knob_r = CornerRadius::same((KNOB / 2.0) as u8);
        // Faint drop shadow ≈ `0 1px 2px rgba(0,0,0,0.25)`.
        let shadow = knob_rect.translate(Vec2::new(0.0, 1.0));
        painter.rect_filled(
            shadow,
            knob_r,
            Color32::from_black_alpha((28.0 * alpha) as u8),
        );
        painter.rect_filled(knob_rect, knob_r, knob_color);

        resp
    }
}

fn with_alpha(c: Color32, a: f32) -> Color32 {
    let a = (a.clamp(0.0, 1.0) * 255.0) as u8;
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), ((c.a() as u16 * a as u16) / 255) as u8)
}
