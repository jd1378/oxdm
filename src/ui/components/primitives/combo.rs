//! Dropdown matching [`TextInput`] in height, radius, stroke and inner
//! padding. The egui built-in `ComboBox` paints its button via the
//! generic widget visuals, which drift sub-pixel against our custom
//! input frame; this primitive sidesteps that by allocating a fixed
//! `outer_w × CONTROL_H_MD` rect and painting the frame itself.
//!
//! The `contents` closure receives the popup ui and returns whatever
//! the caller wants to know (e.g. `Option<NewSelection>`). The popup
//! width matches the button so list items align with the trigger.

use eframe::egui::{
    self, Align2, FontId, InnerResponse, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2,
};

use super::control::{CONTROL_H_MD, CONTROL_RADIUS, INPUT_PAD_X};
use super::menu as mu;
use crate::ui::theme;
use crate::ui::utils::icons;

pub struct Combo<'a> {
    id_source: &'a str,
    selected_text: String,
    width: Option<f32>,
    font: Option<FontId>,
    enabled: bool,
}

impl<'a> Combo<'a> {
    /// Render a single option row inside a `Combo` popup. Wraps
    /// `menu::item_plain` so dropdown styling can later diverge from
    /// context-menu rows without touching every call site. Returns the
    /// row's `Response` — caller checks `.clicked()` and applies the
    /// selection.
    pub fn item(ui: &mut Ui, label: &str, enabled: bool) -> Response {
        mu::item_plain(ui, label, None, enabled)
    }

    pub fn new(id_source: &'a str, selected_text: impl Into<String>) -> Self {
        Self {
            id_source,
            selected_text: selected_text.into(),
            width: None,
            font: None,
            enabled: true,
        }
    }
    pub fn width(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }
    pub fn font(mut self, f: FontId) -> Self {
        self.font = Some(f);
        self
    }
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }

    pub fn show<R>(
        self,
        ui: &mut Ui,
        contents: impl FnOnce(&mut Ui) -> R,
    ) -> InnerResponse<Option<R>> {
        let t = theme::tokens(ui.ctx());
        let outer_w = self.width.unwrap_or_else(|| ui.available_width());
        let h = CONTROL_H_MD;
        let pad_x = INPUT_PAD_X;
        let icon_size = 14.0;
        let radius = CONTROL_RADIUS as f32;

        let sense = if self.enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let (rect, response) = ui.allocate_exact_size(Vec2::new(outer_w, h), sense);
        let hovered = self.enabled && response.hovered();

        // Frame.
        let bg = if hovered {
            t.bg_surface_hover
        } else {
            t.bg_raised
        };
        // Paint fill + stroke as a single `RectShape` (via `painter.rect`),
        // matching the way egui's `Frame` tessellates a rounded border. Two
        // separate `rect_filled` + `rect_stroke` calls double-AA the rounded
        // corners and produce a visibly fuzzy edge. `StrokeKind::Outside`
        // mirrors Frame's "grow outward" geometry so the visible widget
        // ends up the same total size as a TextInput allocated at `rect`.
        // Match `egui::Frame`'s `widget_rect = content_rect + inner_margin
        // + stroke.width` geometry. TextInput allocates `outer_w` for the
        // OUTER width, but Frame then paints its rounded rect at
        // `content + inner_margin + stroke_width`, i.e. 2px wider than
        // the allocation. Expanding the painted rect here keeps the
        // visible combo footprint identical to a TextInput at the same
        // `width(w)`.
        let paint_rect = rect.expand(t.border_width);
        ui.painter().rect(
            paint_rect,
            radius,
            bg,
            Stroke::new(t.border_width, t.border_subtle),
            egui::StrokeKind::Inside,
        );

        // Selected text on the left.
        let font = self.font.unwrap_or_else(|| theme::body(13.0));
        let text_color = if self.enabled { t.fg_1 } else { t.fg_3 };
        let galley = ui
            .painter()
            .layout_no_wrap(self.selected_text.clone(), font, text_color);
        let text_rect = Rect::from_min_max(
            Pos2::new(rect.min.x + pad_x, rect.min.y),
            Pos2::new(rect.max.x - icon_size - pad_x, rect.max.y),
        );
        let text_pos = Align2::LEFT_CENTER.align_size_within_rect(galley.size(), text_rect);
        ui.painter().galley(text_pos.min, galley, text_color);

        // Chevron icon on the right.
        let icon_color = if self.enabled { t.fg_3 } else { t.fg_4 };
        let icon_rect = Rect::from_center_size(
            Pos2::new(rect.max.x - pad_x - icon_size * 0.5, rect.center().y),
            Vec2::splat(icon_size),
        );
        icons::icon(ui.ctx(), "chevron-down", icon_size, icon_color).paint_at(ui, icon_rect);

        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if !self.enabled && response.contains_pointer() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::NotAllowed);
        }

        let popup = egui::Popup::menu(&response)
            .id(ui.make_persistent_id(("oxdm-combo", self.id_source)))
            .width(outer_w)
            .show(|ui| {
                ui.set_min_width(outer_w);
                contents(ui)
            });
        let inner = popup.map(|r| r.inner);

        InnerResponse { response, inner }
    }
}
