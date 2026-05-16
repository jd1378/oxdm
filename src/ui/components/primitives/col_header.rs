//! Table column headers — plain and sortable.

use eframe::egui::{self, Align, Layout, Pos2, Rect, Response, Sense, Ui, Vec2};

use super::clickable::Clickable;
use crate::ui::theme;
use crate::ui::utils::icons;

pub fn col_header(ui: &mut Ui, label: &str) {
    col_header_aligned(ui, label, Align::LEFT);
}

pub fn col_header_aligned(ui: &mut Ui, label: &str, align: Align) {
    let t = theme::tokens(ui.ctx());
    let upper = label.to_uppercase();
    let mut layout = egui::text::LayoutJob::default();
    layout.append(
        &upper,
        0.0,
        egui::TextFormat {
            font_id: theme::body_bold(11.0),
            color: t.fg_3,
            extra_letter_spacing: 0.8,
            ..Default::default()
        },
    );
    let layout_dir = match align {
        Align::RIGHT => Layout::right_to_left(Align::Center),
        Align::Center => Layout::top_down(Align::Center),
        _ => Layout::left_to_right(Align::Center),
    };
    ui.with_layout(layout_dir, |ui| {
        ui.label(layout);
    });
}

/// Clickable column header with sort indicator. Returns the click
/// response. When `active`, paints a chevron (up = ascending,
/// down = descending) next to the label.
pub fn col_header_sortable(
    ui: &mut Ui,
    label: &str,
    align: Align,
    active: bool,
    desc: bool,
) -> Response {
    let t = theme::tokens(ui.ctx());
    let rect = ui.available_rect_before_wrap();
    let resp = ui.allocate_rect(rect, Sense::click());

    let color = if active || resp.hovered() {
        t.fg_2
    } else {
        t.fg_3
    };

    let mut layout = egui::text::LayoutJob::default();
    layout.append(
        &label.to_uppercase(),
        0.0,
        egui::TextFormat {
            font_id: theme::body_bold(11.0),
            color,
            extra_letter_spacing: 0.8,
            ..Default::default()
        },
    );
    let painter = ui.painter().clone();
    let galley = painter.layout_job(layout);
    let text_w = galley.size().x;
    let text_h = galley.size().y;

    let icon_size = 11.0;
    let gap = 4.0;
    let cy = rect.center().y;

    let (text_x, icon_x) = match align {
        Align::RIGHT => {
            let end = rect.right();
            if active {
                let icon_x = end - icon_size;
                (icon_x - gap - text_w, icon_x)
            } else {
                (end - text_w, end)
            }
        }
        _ => {
            let text_x = rect.left();
            (text_x, text_x + text_w + gap)
        }
    };

    painter.galley(Pos2::new(text_x, cy - text_h / 2.0), galley, color);
    if active {
        let name = if desc { "chevron-down" } else { "chevron-up" };
        let img = icons::icon(ui.ctx(), name, icon_size, color);
        let irect = Rect::from_min_size(
            Pos2::new(icon_x, cy - icon_size / 2.0),
            Vec2::splat(icon_size),
        );
        img.paint_at(ui, irect);
    }
    resp.clickable()
}
