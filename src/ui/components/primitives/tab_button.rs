//! Underlined tab header button with optional leading icon and trailing
//! count pill. Renders the label via the painter (not `ui.label`) so
//! it does NOT capture text-selection drags.

use eframe::egui::{self, CornerRadius, Pos2, Rect, Response, Sense, Ui, Vec2};

use super::clickable::Clickable;
use crate::ui::theme::{self, radius};
use crate::ui::utils::icons;

pub struct TabBtn<'a> {
    label: &'a str,
    icon: Option<&'static str>,
    count: Option<usize>,
    active: bool,
    font_size: f32,
    icon_size: f32,
    pad_x: f32,
    height: f32,
}

impl<'a> TabBtn<'a> {
    pub fn new(label: &'a str) -> Self {
        Self {
            label,
            icon: None,
            count: None,
            active: false,
            font_size: 12.0,
            icon_size: 20.0,
            pad_x: 14.0,
            height: 36.0,
        }
    }
    pub fn icon(mut self, name: &'static str) -> Self {
        self.icon = Some(name);
        self
    }
    /// Leading-icon edge length in points (default 20).
    pub fn icon_size(mut self, s: f32) -> Self {
        self.icon_size = s;
        self
    }
    /// Horizontal padding inside the tab (default 14). Set 0 for a
    /// compact bar where the row's `item_spacing` provides the gap.
    pub fn pad_x(mut self, p: f32) -> Self {
        self.pad_x = p;
        self
    }
    /// Total tab height in points (default 36).
    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }
    pub fn count(mut self, n: usize) -> Self {
        self.count = Some(n);
        self
    }
    pub fn active(mut self, a: bool) -> Self {
        self.active = a;
        self
    }
    pub fn font_size(mut self, s: f32) -> Self {
        self.font_size = s;
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let t = theme::tokens(ui.ctx());
        let icon_size = self.icon_size;
        let icon_gap = 6.0;
        let pill_gap = 8.0;
        let pad_x = self.pad_x;

        let measure_galley = ui.painter().layout_no_wrap(
            self.label.to_owned(),
            theme::body_bold(self.font_size),
            t.fg_3,
        );
        let count_measure = self.count.map(|n| {
            ui.painter()
                .layout_no_wrap(n.to_string(), theme::mono(10.0), t.fg_2)
        });

        let mut content_w = measure_galley.size().x;
        if self.icon.is_some() {
            content_w += icon_size + icon_gap;
        }
        let pill_w = if let Some(ref g) = count_measure {
            (g.size().x + 12.0).max(18.0)
        } else {
            0.0
        };
        if pill_w > 0.0 {
            content_w += pill_gap + pill_w;
        }
        let w = content_w + pad_x * 2.0;
        let h = self.height;

        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());

        // Animate the fg_3 → fg_1 colour shift so hover/active fades in
        // instead of snapping (CSS `transition` equivalent). Active tabs
        // hold at fg_1. `animate_bool_with_time` auto-requests repaints
        // while in flight and settles on its own.
        let lit = self.active || resp.hovered();
        let anim = ui
            .ctx()
            .animate_bool_with_time(resp.id.with("tab-fg"), lit, 0.12);
        let fg = {
            let a = egui::Rgba::from(t.fg_3);
            let b = egui::Rgba::from(t.fg_1);
            egui::Color32::from(a * (1.0 - anim) + b * anim)
        };
        if resp.hovered() {
            ui.set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let label_galley = ui.painter().layout_no_wrap(
            self.label.to_owned(),
            theme::body_bold(self.font_size),
            fg,
        );
        let (pill_bg, pill_fg) = if self.active {
            (t.pill_active_bg, t.pill_active_fg)
        } else {
            (t.bg_sunken, t.fg_2)
        };
        let count_galley = self.count.map(|n| {
            ui.painter()
                .layout_no_wrap(n.to_string(), theme::mono(10.0), pill_fg)
        });
        let painter = ui.painter().clone();
        let cy = rect.center().y;
        let mut x = rect.left() + pad_x;

        if let Some(name) = self.icon {
            let img = icons::icon(ui.ctx(), name, icon_size, fg);
            let irect =
                Rect::from_min_size(Pos2::new(x, cy - icon_size / 2.0), Vec2::splat(icon_size));
            img.paint_at(ui, irect);
            x += icon_size + icon_gap;
        }
        painter.galley(
            Pos2::new(x, cy - label_galley.size().y / 2.0),
            label_galley.clone(),
            fg,
        );
        x += label_galley.size().x;

        if let Some(cg) = count_galley {
            x += pill_gap;
            let ph = 16.0;
            let pill_rect = Rect::from_min_size(Pos2::new(x, cy - ph / 2.0), Vec2::new(pill_w, ph));
            painter.rect_filled(pill_rect, CornerRadius::from(radius::PILL as f32), pill_bg);
            painter.galley(
                Pos2::new(
                    pill_rect.center().x - cg.size().x / 2.0,
                    pill_rect.center().y - cg.size().y / 2.0,
                ),
                cg,
                pill_fg,
            );
        }

        if self.active {
            let ul = Rect::from_min_max(
                Pos2::new(rect.left(), rect.bottom() - 2.0),
                Pos2::new(rect.right(), rect.bottom()),
            );
            // Active-tab underline is clay-400 across all themes (design).
            painter.rect_filled(ul, CornerRadius::ZERO, crate::ui::color::clay::C400);
        }
        resp.clickable()
    }
}
