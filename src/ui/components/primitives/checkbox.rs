//! Square checkbox. Matches design/tokens.css `.s-checkbox`: 18×18
//! rounded square, clay fill + white check when on, bordered sunken
//! when off.

use eframe::egui::{self, Color32, CornerRadius, Pos2, Sense, Stroke, StrokeKind, Ui, Vec2};

use crate::ui::theme::{self, motion};

const BOX: f32 = 18.0;

pub struct Checkbox<'a> {
    on: &'a mut bool,
    enabled: bool,
    id_source: Option<egui::Id>,
}

impl<'a> Checkbox<'a> {
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
        let (rect, mut resp) = ui.allocate_exact_size(Vec2::splat(BOX), Sense::click());
        if !self.enabled {
            resp = resp.on_hover_cursor(egui::CursorIcon::NotAllowed);
        } else {
            resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        }
        if self.enabled && resp.clicked() {
            *self.on = !*self.on;
            resp.mark_changed();
        }

        let anim_id = self
            .id_source
            .unwrap_or_else(|| resp.id.with("checkbox-anim"));
        let lin = ui.ctx().animate_value_with_time(
            anim_id,
            if *self.on { 1.0 } else { 0.0 },
            motion::FAST,
        );
        let k = motion::ease_out(lin);

        let bg_off = t.bg_raised;
        let bg_on = t.action_primary;
        let border_off = t.border_default;
        let border_on = t.action_primary_press;
        let bg = motion::lerp_color(bg_off, bg_on, k);
        let border = motion::lerp_color(border_off, border_on, k);

        let alpha = if self.enabled { 1.0 } else { 0.5 };
        let r = CornerRadius::same(4);
        let painter = ui.painter();
        painter.rect_filled(rect, r, with_alpha(bg, alpha));
        painter.rect_stroke(
            rect,
            r,
            Stroke::new(1.0, with_alpha(border, alpha)),
            StrokeKind::Inside,
        );

        // Check glyph — two segments forming a tick. Animate alpha + scale.
        if k > 0.01 {
            let c = with_alpha(Color32::WHITE, alpha * k);
            let stroke = Stroke::new(2.0, c);
            let cx = rect.center().x;
            let cy = rect.center().y;
            // Anchor points scaled with k for a small pop-in.
            let s = 0.5 + 0.5 * k;
            let p1 = Pos2::new(cx - 4.0 * s, cy + 0.0 * s);
            let p2 = Pos2::new(cx - 1.0 * s, cy + 3.0 * s);
            let p3 = Pos2::new(cx + 4.0 * s, cy - 3.0 * s);
            painter.line_segment([p1, p2], stroke);
            painter.line_segment([p2, p3], stroke);
        }

        resp
    }
}

fn with_alpha(c: Color32, a: f32) -> Color32 {
    let a = (a.clamp(0.0, 1.0) * 255.0) as u8;
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), ((c.a() as u16 * a as u16) / 255) as u8)
}
