//! Primary, secondary, ghost, and danger buttons.

use eframe::egui::{
    self, Color32, CornerRadius, Rect, Response, Sense, Stroke, Ui, Vec2, WidgetText,
};

use super::clickable::Clickable;
use super::control::{CONTROL_H_LG, CONTROL_H_MD, CONTROL_H_SM, CONTROL_RADIUS};
use super::util::{darken, mix, text_string};
use crate::ui::theme::{self, Tokens};
use crate::ui::utils::icons;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BtnVariant {
    /// Filled accent (clay). Use for the single primary action in a context.
    Primary,
    /// Bordered neutral. Default safe choice.
    Secondary,
    /// No border. Transparent at rest; tints on hover/press. The
    /// historical "ghost" — used for toolbar-style tertiary actions.
    Toolbar,
    /// No border, no background tint at any state. Only the text/icon
    /// foreground changes between rest and hover. Pure-text affordance.
    Ghost,
    /// Bordered danger (red border + red fg). Reversible destructive.
    Danger,
    /// Filled danger (solid red bg, white fg). Irreversible destructive.
    /// Per `design/handoff/11_confirm_dialog.md` — used for "Quit while
    /// downloading", "Discard changes", "Delete forever".
    DangerFilled,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BtnSize {
    Sm,
    Md,
    Lg,
}

pub struct Btn<'a> {
    label: WidgetText,
    icon: Option<&'static str>,
    variant: BtnVariant,
    size: BtnSize,
    icon_only: bool,
    enabled: bool,
    selected: bool,
    min_width: Option<f32>,
    tooltip: Option<&'a str>,
    font_size: Option<f32>,
    icon_size: Option<f32>,
    /// Tints the foreground (text + icon) with `action_primary`. Only
    /// affects borderless variants (`Toolbar` / `Ghost`). Used for inline
    /// affordances like `+ Add header` or `↓ Import from browser` that
    /// the handoff paints in the brand orange.
    accent: bool,
}

/// Sizing intermediates shared between [`Btn::measured_size`] and
/// [`Btn::show`] so the two can never drift.
struct Metrics {
    w: f32,
    h: f32,
    pad_x: f32,
    font_size: f32,
    icon_size: f32,
    label_size: Vec2,
    icon_w: f32,
}

impl<'a> Btn<'a> {
    pub fn new(label: impl Into<WidgetText>) -> Self {
        Self {
            label: label.into(),
            icon: None,
            variant: BtnVariant::Secondary,
            size: BtnSize::Md,
            icon_only: false,
            enabled: true,
            selected: false,
            min_width: None,
            tooltip: None,
            font_size: None,
            icon_size: None,
            accent: false,
        }
    }
    /// Render the foreground in `action_primary`. No-op on filled
    /// variants. See the field doc on [`Btn::accent`] for rationale.
    pub fn accent(mut self) -> Self {
        self.accent = true;
        self
    }
    pub fn primary(mut self) -> Self {
        self.variant = BtnVariant::Primary;
        self
    }
    pub fn toolbar(mut self) -> Self {
        self.variant = BtnVariant::Toolbar;
        self
    }
    pub fn ghost(mut self) -> Self {
        self.variant = BtnVariant::Ghost;
        self
    }
    pub fn danger(mut self) -> Self {
        self.variant = BtnVariant::Danger;
        self
    }
    pub fn danger_filled(mut self) -> Self {
        self.variant = BtnVariant::DangerFilled;
        self
    }
    pub fn variant(mut self, v: BtnVariant) -> Self {
        self.variant = v;
        self
    }
    pub fn size(mut self, s: BtnSize) -> Self {
        self.size = s;
        self
    }
    pub fn icon(mut self, name: &'static str) -> Self {
        self.icon = Some(name);
        self
    }
    pub fn icon_only(mut self, name: &'static str) -> Self {
        self.icon = Some(name);
        self.icon_only = true;
        self
    }
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }
    pub fn selected(mut self, s: bool) -> Self {
        self.selected = s;
        self
    }
    pub fn min_width(mut self, w: f32) -> Self {
        self.min_width = Some(w);
        self
    }
    pub fn tooltip(mut self, t: &'a str) -> Self {
        self.tooltip = Some(t);
        self
    }
    pub fn font_size(mut self, s: f32) -> Self {
        self.font_size = Some(s);
        self
    }
    pub fn icon_size(mut self, s: f32) -> Self {
        self.icon_size = Some(s);
        self
    }

    /// Size the button will occupy without rendering it. Mirrors the
    /// sizing logic in `show` so callers can reserve exact space — e.g.
    /// bounding a sibling cell's width in a flex/manual row. Single
    /// source of truth shared with `show` via [`Btn::metrics`].
    pub fn measured_size(&self, ui: &Ui) -> Vec2 {
        let m = self.metrics(ui);
        Vec2::new(m.w, m.h)
    }

    fn metrics(&self, ui: &Ui) -> Metrics {
        let (h, pad_x, default_font_size, default_icon_size) = match self.size {
            BtnSize::Sm => (CONTROL_H_SM, 10.0, 12.0, 16.0),
            BtnSize::Md => (CONTROL_H_MD, 14.0, 13.0, 17.0),
            BtnSize::Lg => (CONTROL_H_LG, 18.0, 14.0, 20.0),
        };
        let font_size = self.font_size.unwrap_or(default_font_size);
        let icon_size = self.icon_size.unwrap_or(default_icon_size);
        let pad_x = if self.icon_only {
            (h - icon_size) * 0.5
        } else {
            pad_x
        };

        let label_size = if self.icon_only {
            Vec2::ZERO
        } else {
            let galley = ui.painter().layout_no_wrap(
                text_string(&self.label),
                theme::body_bold(font_size),
                Color32::WHITE,
            );
            galley.size()
        };
        let icon_w = if self.icon.is_some() {
            if self.icon_only {
                icon_size
            } else {
                icon_size + 6.0
            }
        } else {
            0.0
        };
        let mut w = pad_x * 2.0 + icon_w + label_size.x;
        if let Some(mw) = self.min_width {
            w = w.max(mw);
        }
        if self.icon_only {
            w = h;
        }
        Metrics {
            w,
            h,
            pad_x,
            font_size,
            icon_size,
            label_size,
            icon_w,
        }
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let t = theme::tokens(ui.ctx());
        let Metrics {
            w,
            h,
            pad_x,
            font_size,
            icon_size,
            label_size,
            icon_w,
        } = self.metrics(ui);

        let sense = if self.enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let size = Vec2::new(w, h);
        let mut resp = ui.allocate_response(size, sense);
        let rect = resp.rect;
        if !self.enabled && resp.contains_pointer() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::NotAllowed);
        }
        if let Some(tip) = self.tooltip {
            resp = resp.on_hover_text(tip);
        }
        resp = resp.clickable();

        let pressed = self.enabled && resp.is_pointer_button_down_on();
        let hovered = self.enabled && resp.hovered();

        let (bg, fg, border) = colours(
            &t,
            self.variant,
            self.selected,
            hovered,
            pressed,
            self.enabled,
            self.accent,
        );
        let painter = ui.painter().clone();
        let rounding: CornerRadius = CONTROL_RADIUS.into();
        let body = rect;

        painter.rect_filled(body, rounding, bg);
        if border.width > 0.0 {
            let inset = border.width * 0.5;
            let stroke_rect = body.shrink(inset);
            let inset_u = inset.round().max(0.0) as u8;
            let stroke_rounding = CornerRadius {
                nw: rounding.nw.saturating_sub(inset_u),
                ne: rounding.ne.saturating_sub(inset_u),
                sw: rounding.sw.saturating_sub(inset_u),
                se: rounding.se.saturating_sub(inset_u),
            };
            painter.rect_stroke(
                stroke_rect,
                stroke_rounding,
                border,
                egui::StrokeKind::Middle,
            );
        }

        // Horizontal centring: shrink the inner content rect to the
        // natural width of icon + label and offset it inside the
        // padded rect. We do this manually because egui's
        // `Layout::left_to_right(Align::Center).with_main_align(Center)`
        // doesn't reposition children once they've been added in a
        // single frame.
        let pad_rect = body.shrink2(Vec2::new(pad_x, 0.0));
        let natural_content_w = (icon_w + label_size.x).min(pad_rect.width());
        let extra_w = (pad_rect.width() - natural_content_w).max(0.0);
        let content_rect = Rect::from_min_size(
            egui::Pos2::new(pad_rect.min.x + extra_w * 0.5, pad_rect.min.y),
            Vec2::new(natural_content_w, pad_rect.height()),
        );
        if self.icon_only {
            let img = icons::icon(ui.ctx(), self.icon.unwrap_or("circle"), icon_size, fg);
            let icon_rect = Rect::from_center_size(body.center(), Vec2::splat(icon_size));
            img.paint_at(ui, icon_rect);
        } else {
            // Use egui's layout system to place the icon + label inside
            // the button rect. `Layout::left_to_right(Align::Center)`
            // resolves cross-axis centering from the widget's
            // intrinsic size (font ascent/descent), which is more
            // accurate than centering the line-height box.
            // Centre the icon + label as a group on both axes. The
            // default `left_to_right(Center)` pins content to the left
            // edge of the rect, which leaves a full-width button (e.g.
            // "Add header") with the label hugging the left.
            let mut child = ui.new_child(
                egui::UiBuilder::new().max_rect(content_rect).layout(
                    egui::Layout::left_to_right(egui::Align::Center)
                        .with_main_align(egui::Align::Center),
                ),
            );
            child.spacing_mut().item_spacing.x = 6.0;
            if let Some(name) = self.icon {
                let img = icons::icon(ui.ctx(), name, icon_size, fg);
                let (icon_r, _) = child.allocate_exact_size(Vec2::splat(icon_size), Sense::hover());
                img.paint_at(&child, icon_r);
            }
            child.add(
                egui::Label::new(
                    egui::RichText::new(text_string(&self.label))
                        .font(theme::body_bold(font_size))
                        .color(fg),
                )
                .selectable(false)
                .wrap_mode(egui::TextWrapMode::Extend),
            );
            let _ = painter;
        }

        resp
    }
}

fn colours(
    t: &Tokens,
    v: BtnVariant,
    selected: bool,
    hovered: bool,
    pressed: bool,
    enabled: bool,
    accent: bool,
) -> (Color32, Color32, Stroke) {
    let (bg, fg, border) = match v {
        BtnVariant::Primary => {
            use crate::ui::color::clay;
            let bg = if pressed {
                clay::C600
            } else if hovered {
                darken(clay::C500, 0.06)
            } else {
                clay::C400
            };
            (bg, Color32::WHITE, Stroke::new(1.0, clay::C500))
        }
        BtnVariant::DangerFilled => {
            let bg = if pressed {
                darken(t.status_danger, 0.15)
            } else if hovered {
                darken(t.status_danger, 0.07)
            } else {
                t.status_danger
            };
            (bg, Color32::WHITE, Stroke::NONE)
        }
        BtnVariant::Danger => {
            let bg = if pressed {
                mix(t.status_danger_bg, t.bg_page, 0.4)
            } else if hovered {
                t.status_danger_bg
            } else {
                Color32::TRANSPARENT
            };
            (
                bg,
                t.status_danger,
                Stroke::new(t.border_width, t.status_danger),
            )
        }
        BtnVariant::Secondary => {
            let bg = if pressed {
                t.bg_sunken
            } else if hovered {
                mix(t.bg_raised, t.bg_sunken, 0.5)
            } else {
                t.bg_raised
            };
            (
                bg,
                t.fg_1,
                Stroke::new(
                    t.border_width,
                    if selected {
                        t.border_brand
                    } else {
                        t.border_default
                    },
                ),
            )
        }
        BtnVariant::Toolbar => {
            let bg = if pressed {
                t.bg_sunken
            } else if hovered {
                mix(t.bg_page, t.bg_sunken, 0.55)
            } else {
                Color32::TRANSPARENT
            };
            let fg = if accent {
                if pressed {
                    darken(t.action_primary, 0.10)
                } else {
                    t.action_primary
                }
            } else if hovered {
                t.fg_1
            } else {
                t.fg_2
            };
            (bg, fg, Stroke::NONE)
        }
        BtnVariant::Ghost => {
            // Transparent at every state — only the foreground colour
            // tints on hover/press. Use for in-line text affordances
            // that shouldn't paint a chip behind themselves.
            let fg = if accent {
                if pressed {
                    darken(t.action_primary, 0.10)
                } else {
                    t.action_primary
                }
            } else if pressed || hovered {
                t.fg_1
            } else {
                t.fg_2
            };
            (Color32::TRANSPARENT, fg, Stroke::NONE)
        }
    };
    if !enabled {
        // Preserve transparent backgrounds (e.g. ghost buttons) — mixing
        // a transparent rgb(0,0,0) with bg_page produces mud.
        let new_bg = if bg == Color32::TRANSPARENT {
            Color32::TRANSPARENT
        } else {
            mix(bg, t.bg_page, 0.5)
        };
        return (
            new_bg,
            t.fg_4,
            Stroke::new(border.width, mix(border.color, t.bg_page, 0.5)),
        );
    }
    if selected
        && matches!(
            v,
            BtnVariant::Secondary | BtnVariant::Toolbar | BtnVariant::Ghost
        )
    {
        return (
            t.bg_sunken,
            t.action_primary,
            Stroke::new(t.border_width, t.border_brand),
        );
    }
    (bg, fg, border)
}
