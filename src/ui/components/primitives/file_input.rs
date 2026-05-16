//! Path text input with a trailing folder-picker icon rendered inside
//! the same frame. Replaces the previous "text input + separate folder
//! button" pattern so the composite reads as a single control with the
//! same width and visual weight as adjacent comboboxes.
//!
//! The component is rendering-only: it reports whether the picker icon
//! was clicked via [`FileInputResponse::browse`]. Opening the system
//! dialog (rfd, async runtime context, starting dir/name) is the
//! caller's responsibility.

use eframe::egui::{self, Align, FontId, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};

use super::control::{CONTROL_H_MD, CONTROL_RADIUS, INPUT_PAD_X};
use crate::ui::theme;
use crate::ui::utils::icons;

pub struct FileInput<'a> {
    value: &'a mut String,
    hint: Option<String>,
    width: Option<f32>,
    font: Option<FontId>,
    icon: &'static str,
    tooltip: Option<&'a str>,
    interactive: bool,
    id_salt: Option<egui::Id>,
}

pub struct FileInputResponse {
    /// Response of the inner `TextEdit` (focus, changed, etc.).
    pub text: Response,
    /// Response of the trailing picker icon. Use `.clicked()` to detect.
    pub browse: Response,
}

impl<'a> FileInput<'a> {
    pub fn new(value: &'a mut String) -> Self {
        Self {
            value,
            hint: None,
            width: None,
            font: None,
            icon: "folder",
            tooltip: None,
            interactive: true,
            id_salt: None,
        }
    }
    /// Disambiguate this instance's widget ids. Required when two
    /// `FileInput`s render under a parent `Ui` that shares the same `Id`
    /// (e.g. sibling rows built by the same helper) — otherwise their
    /// button ids collide and egui paints a "First/Second" clash overlay.
    pub fn id_salt(mut self, salt: impl std::hash::Hash) -> Self {
        self.id_salt = Some(egui::Id::new(salt));
        self
    }
    /// Read-only display mode: the text field shows the value but can't be
    /// edited or focused. Use for fields that mirror immutable state (e.g.
    /// a job's URL / save path in Properties) while keeping the framed
    /// input look + a trailing action button (copy, reveal, …).
    pub fn interactive(mut self, yes: bool) -> Self {
        self.interactive = yes;
        self
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
    pub fn icon(mut self, name: &'static str) -> Self {
        self.icon = name;
        self
    }
    pub fn tooltip(mut self, t: &'a str) -> Self {
        self.tooltip = Some(t);
        self
    }

    pub fn show(self, ui: &mut Ui) -> FileInputResponse {
        let t = theme::tokens(ui.ctx());
        let outer_w = self.width.unwrap_or_else(|| ui.available_width());
        let h = CONTROL_H_MD;
        let pad_x = INPUT_PAD_X;
        let icon_size = 16.0;
        let icon_cell_w = h; // square picker frame
        let gap = 6.0; // explicit gap matching legacy layout
        let radius = CONTROL_RADIUS as f32;
        let stroke = Stroke::new(t.border_width, t.border_subtle);

        // Reserve full composite rect — fixed width, two children.
        let (rect, _bg) = ui.allocate_exact_size(Vec2::new(outer_w, h), Sense::hover());

        let text_rect = Rect::from_min_max(
            rect.min,
            Pos2::new(rect.max.x - icon_cell_w - gap, rect.max.y),
        );
        let icon_rect =
            Rect::from_min_max(Pos2::new(rect.max.x - icon_cell_w, rect.min.y), rect.max);

        // --- Text input frame ---
        ui.painter().rect_filled(text_rect, radius, t.bg_raised);
        ui.painter().rect_stroke(
            text_rect.shrink(t.border_width * 0.5),
            radius,
            stroke,
            egui::StrokeKind::Middle,
        );

        let edit_rect = Rect::from_min_max(
            Pos2::new(text_rect.min.x + pad_x, text_rect.min.y),
            Pos2::new(text_rect.max.x - pad_x, text_rect.max.y),
        );
        let base_id = self.id_salt.unwrap_or_else(|| ui.id());
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .id_salt(base_id)
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
            .interactive(self.interactive)
            .vertical_align(Align::Center);
        if let Some(h) = self.hint {
            edit = edit.hint_text(egui::RichText::new(h).color(t.fg_4));
        }
        let text_resp = child.add(edit);

        // --- Picker button frame ---
        let browse_id = base_id.with("oxdm-file-input-browse");
        let mut browse = ui.interact(icon_rect, browse_id, Sense::click());
        if let Some(tip) = self.tooltip {
            browse = browse.on_hover_text(tip);
        }
        browse = browse.on_hover_cursor(egui::CursorIcon::PointingHand);

        let pressed = browse.is_pointer_button_down_on();
        let hovered = browse.hovered();
        let btn_bg = if pressed || hovered {
            t.bg_surface_hover
        } else {
            t.bg_raised
        };
        ui.painter().rect_filled(icon_rect, radius, btn_bg);
        ui.painter().rect_stroke(
            icon_rect.shrink(t.border_width * 0.5),
            radius,
            stroke,
            egui::StrokeKind::Middle,
        );

        let icon_color = if hovered { t.fg_1 } else { t.fg_3 };
        icons::icon(ui.ctx(), self.icon, icon_size, icon_color).paint_at(
            ui,
            Rect::from_center_size(icon_rect.center(), Vec2::splat(icon_size)),
        );

        FileInputResponse {
            text: text_resp,
            browse,
        }
    }
}
