//! Surface card containers — plain `card` and a `collapsible_card`
//! with chevron header.

use eframe::egui::{
    self, Align, Layout, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2, WidgetText,
};

use super::control::CONTROL_RADIUS;
use crate::ui::color::clay;
use crate::ui::theme::{self, space};
use crate::ui::utils::icons;

pub fn card<R>(ui: &mut Ui, padding: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let t = theme::tokens(ui.ctx());
    egui::Frame::NONE
        .fill(t.bg_surface)
        .stroke(Stroke::new(t.border_width, t.border_subtle))
        .corner_radius(CONTROL_RADIUS)
        .inner_margin(padding)
        .show(ui, add)
        .inner
}

/// Collapsible card. Renders a header row (title left, chevron, optional
/// right-side WidgetText). Returns the body's result when expanded.
/// Like [`collapsible_card`], but without any border / background fill.
/// Just a clickable header row (chevron + title) that toggles the body.
/// Hover tints the chevron + title to clay-400 to signal affordance.
pub fn collapsible_section<R>(
    ui: &mut Ui,
    state_id: egui::Id,
    title: &str,
    default_open: bool,
    body: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    let t = theme::tokens(ui.ctx());
    let mut open: bool = ui
        .ctx()
        .data_mut(|d| *d.get_persisted_mut_or(state_id, default_open));

    let pad_x = 0.0;
    let pad_y = space::S1 as f32;
    let chev_size = 12.0;
    let title_font = theme::body_bold(12.0);
    let gap = 6.0;

    // Measure title height so the header row sizes itself like its content.
    let measure = ui
        .painter()
        .layout_no_wrap(title.to_string(), title_font.clone(), t.fg_2);
    let header_inner_h = measure.size().y.max(chev_size);
    let header_w = ui.available_width();
    let (header_rect, header_resp) = ui.allocate_exact_size(
        Vec2::new(header_w, header_inner_h + pad_y * 2.0),
        Sense::click(),
    );

    let hovered = header_resp.hovered();
    if header_resp.clicked() {
        open = !open;
        ui.ctx().data_mut(|d| d.insert_persisted(state_id, open));
    }
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    let fg = if hovered { clay::C400 } else { t.fg_2 };

    let chev_name = if open {
        "chevron-down"
    } else {
        "chevron-right"
    };
    let chev_pos = Pos2::new(
        header_rect.left() + pad_x,
        header_rect.center().y - chev_size * 0.5,
    );
    let chev_rect = Rect::from_min_size(chev_pos, Vec2::splat(chev_size));
    icons::icon(ui.ctx(), chev_name, chev_size, fg).paint_at(ui, chev_rect);

    let text_galley = ui
        .painter()
        .layout_no_wrap(title.to_string(), title_font, fg);
    let text_pos = Pos2::new(
        chev_rect.right() + gap,
        header_rect.center().y - text_galley.size().y * 0.5,
    );
    ui.painter().galley(text_pos, text_galley, fg);

    let mut result = None;
    if open {
        ui.add_space(pad_y);
        result = Some(body(ui));
    }
    result
}

pub fn collapsible_card<R>(
    ui: &mut Ui,
    state_id: egui::Id,
    title: &str,
    right: Option<WidgetText>,
    default_open: bool,
    body: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    let t = theme::tokens(ui.ctx());
    let mut open: bool = ui
        .ctx()
        .data_mut(|d| *d.get_persisted_mut_or(state_id, default_open));

    let mut result = None;
    let margin = if open {
        egui::Margin {
            left: space::S3,
            right: space::S3,
            top: space::S1,
            bottom: space::S3,
        }
    } else {
        egui::Margin {
            left: space::S3,
            right: space::S3,
            top: space::S1,
            bottom: space::S1,
        }
    };
    let mut header_rect = egui::Rect::NOTHING;
    let mut hover_bg_idx: Option<egui::layers::ShapeIdx> = None;
    let frame_resp = egui::Frame::NONE
        .fill(t.bg_surface)
        .stroke(Stroke::new(t.border_width, t.border_subtle))
        .corner_radius(CONTROL_RADIUS)
        .inner_margin(margin)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            hover_bg_idx = Some(ui.painter().add(egui::Shape::Noop));
            let header = ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let chev = if open {
                    "chevron-down"
                } else {
                    "chevron-right"
                };
                let resp = icons::show(ui, chev, 12.0, t.fg_2);
                ui.label(RichText::new(title).color(t.fg_1).font(theme::body(13.0)));
                if let Some(r) = right {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(r);
                    });
                }
                resp
            });
            header_rect = header.response.rect;
            if open {
                let sep_y = header_rect.bottom() + space::S1 as f32;
                ui.add_space((sep_y - ui.cursor().top()).max(0.0));
                let avail_w = ui.available_width();
                let x0 = ui.cursor().left() - space::S3 as f32;
                let x1 = x0 + avail_w + space::S3 as f32 * 2.0;
                ui.painter().line_segment(
                    [Pos2::new(x0, sep_y), Pos2::new(x1, sep_y)],
                    Stroke::new(1.0, t.border_subtle),
                );
                ui.add_space(space::S3 as f32);
                result = Some(body(ui));
            }
        });
    let card_rect = frame_resp.response.rect;
    let hover_rect = if open {
        let sep_y = header_rect.bottom() + space::S1 as f32;
        egui::Rect::from_min_max(card_rect.min, Pos2::new(card_rect.right(), sep_y))
    } else {
        card_rect
    };
    let card = ui.interact(hover_rect, state_id.with("card"), Sense::click());
    if card.clicked() {
        open = !open;
        ui.ctx().data_mut(|d| d.insert_persisted(state_id, open));
    }
    if card.hovered() {
        ui.set_cursor_icon(egui::CursorIcon::PointingHand);
        let rounding = if open {
            egui::CornerRadius {
                nw: CONTROL_RADIUS,
                ne: CONTROL_RADIUS,
                sw: 0,
                se: 0,
            }
        } else {
            egui::CornerRadius::same(CONTROL_RADIUS)
        };
        if let Some(idx) = hover_bg_idx {
            ui.painter().set(
                idx,
                egui::Shape::rect_filled(hover_rect, rounding, t.bg_sunken),
            );
        }
    }
    result
}
