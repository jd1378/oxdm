//! Menu primitives shared between the table's right-click context menu
//! and the `Combo` dropdown popup. Everything renders against the same
//! row height (`ROW_H`), padding (`pad_x = space::S2`), icon size, and
//! `label → kbd` gap so popups stack consistently across the app.
//!
//! Public surface:
//!
//! - [`item`] — leaf row, leading icon + label + optional kbd shortcut.
//! - [`item_plain`] — leaf row without a reserved icon column. Use in
//!   menus where every entry is icon-less (e.g. queue picker) so labels
//!   sit flush against the left pad.
//! - [`separator`] — 1px hairline divider.
//! - [`submenu`] — cascading submenu: leading icon + label + trailing
//!   `chevron-right`, opens on hover, closes when pointer leaves both
//!   trigger and popup, never reacts to clicks itself.
//! - [`measure_width`] / [`measure_width_plain`] — pre-measure all
//!   possible rows so the menu sizes itself to its widest natural row
//!   (+ a fixed `LABEL_TO_KBD_GAP`) instead of an arbitrary 220-ish.
//!
//! Callers are expected to use the returned width with [`Ui::set_width`]
//! so every row's right-aligned kbd column lines up.

use eframe::egui::{self, Color32, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};

use super::Clickable;
use crate::ui::theme::{self, radius, space};

/// Fixed gap between a leaf row's label and its right-aligned shortcut.
pub const LABEL_TO_KBD_GAP: f32 = 24.0;
/// Square cell width reserved for the leading icon in iconed menus.
pub const ICON_SIZE: f32 = 15.0;
/// Row height for every menu row (leaf, plain, submenu trigger).
pub const ROW_H: f32 = 28.0;

/// Leaf row with optional leading icon and optional trailing kbd shortcut.
pub fn item(
    ui: &mut Ui,
    icon: Option<&'static str>,
    label: &str,
    kbd: Option<&str>,
    enabled: bool,
) -> Response {
    paint_row(ui, icon, label, kbd, enabled, true)
}

/// Leaf row WITHOUT a reserved icon column — for menus where no entry
/// has an icon. Labels sit flush against the left padding.
pub fn item_plain(ui: &mut Ui, label: &str, kbd: Option<&str>, enabled: bool) -> Response {
    paint_row(ui, None, label, kbd, enabled, false)
}

/// 1px hairline separator. No vertical padding — relies on the menu's
/// `item_spacing.y = 0` and adjacent rows being 28px tall.
pub fn separator(ui: &mut Ui) {
    let t = theme::tokens(ui.ctx());
    ui.add_space(space::S0 as f32);
    let rect = ui.available_rect_before_wrap();
    let y = rect.top();
    ui.painter().line_segment(
        [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
        Stroke::new(1.0, t.border_subtle),
    );
    ui.add_space(space::S0 as f32);
}

/// Width of the widest natural row in `items` — pass to `ui.set_width`
/// so every kbd-aligned column lines up across the menu.
///
/// Items are `(icon?, label, kbd?)` triples. `icon = None` is allowed
/// in iconed menus — the column is still reserved for alignment.
pub fn measure_width(ctx: &egui::Context, items: &[(Option<&str>, &str, Option<&str>)]) -> f32 {
    measure_width_inner(ctx, items, true)
}

/// Width for an icon-less menu. Items are `(label, kbd?)` pairs.
pub fn measure_width_plain(ctx: &egui::Context, items: &[(&str, Option<&str>)]) -> f32 {
    let triples: Vec<(Option<&str>, &str, Option<&str>)> =
        items.iter().map(|(l, k)| (None, *l, *k)).collect();
    measure_width_inner(ctx, &triples, false)
}

/// Cascading submenu. Renders trigger as `[icon] label … [chevron-right]`,
/// opens on hover, closes when pointer leaves both trigger and popup,
/// never reacts to clicks itself (so the parent menu doesn't get
/// dismissed by a press on the trigger).
pub fn submenu(ui: &mut Ui, icon: &'static str, label: &str, contents: impl FnOnce(&mut Ui)) {
    let row_w = ui.available_width();
    let resp = paint_submenu_row(ui, icon, label, row_w);

    let state_id = resp.id.with("oxdm-submenu-open");
    let popup_rect_id = state_id.with("popup_rect");
    let mut open: bool = ui
        .ctx()
        .data(|d| d.get_temp::<bool>(state_id).unwrap_or(false));

    // Stored popup rect from the previous frame so we can tell if the
    // pointer is still over the popup before deciding to close.
    let last_popup_rect: Option<egui::Rect> =
        ui.ctx().data(|d| d.get_temp::<egui::Rect>(popup_rect_id));
    let hover_pos = ui.ctx().pointer_hover_pos();
    let in_row = hover_pos.is_some_and(|p| resp.rect.contains(p));
    let in_popup = hover_pos
        .zip(last_popup_rect)
        .map(|(p, r)| r.contains(p))
        .unwrap_or(false);

    if resp.hovered() {
        open = true;
    }
    if open && !in_row && !in_popup {
        open = false;
    }

    let popup = egui::Popup::from_response(&resp)
        .id(state_id.with("popup"))
        .align(egui::RectAlign::RIGHT_START)
        .layout(egui::Layout::top_down(egui::Align::Min))
        // Overlap parent menu by 4px so the hover path isn't broken.
        .gap(-(space::S1 as f32))
        .frame(egui::Frame::menu(ui.style()))
        .open(open)
        // Hover-leave drives the close — don't let stray clicks dismiss.
        .close_behavior(egui::PopupCloseBehavior::IgnoreClicks)
        .show(|ui| {
            if resp.is_pointer_button_down_on() || resp.hovered() {
                ui.ctx().move_to_top(ui.layer_id());
            }
            contents(ui);
        });

    if let Some(p) = &popup {
        ui.ctx()
            .data_mut(|d| d.insert_temp(popup_rect_id, p.response.rect));
    } else {
        ui.ctx().data_mut(|d| d.remove::<egui::Rect>(popup_rect_id));
    }
    if popup.is_none() {
        ui.ctx().data_mut(|d| d.remove::<bool>(state_id));
    } else {
        ui.ctx().data_mut(|d| d.insert_temp(state_id, open));
    }
}

// ──────────────────────────────────────────────────────────────────────
// internals
// ──────────────────────────────────────────────────────────────────────

fn measure_width_inner(
    ctx: &egui::Context,
    items: &[(Option<&str>, &str, Option<&str>)],
    reserve_icon: bool,
) -> f32 {
    let pad_x = space::S2 as f32;
    let icon_gap = space::S2 as f32;
    let painter = ctx.debug_painter();
    let mut max_w: f32 = 0.0;
    for (_icon, label, kbd) in items {
        let label_w = painter
            .layout_no_wrap(label.to_string(), theme::body(13.0), Color32::WHITE)
            .size()
            .x;
        let kbd_w = kbd
            .map(|k| {
                painter
                    .layout_no_wrap(k.to_string(), theme::mono(11.0), Color32::WHITE)
                    .size()
                    .x
            })
            .unwrap_or(0.0);
        let kbd_extra = if kbd.is_some() {
            LABEL_TO_KBD_GAP + kbd_w
        } else {
            0.0
        };
        let icon_extra = if reserve_icon {
            ICON_SIZE + icon_gap
        } else {
            0.0
        };
        let w = pad_x + icon_extra + label_w + kbd_extra + pad_x;
        max_w = max_w.max(w);
    }
    max_w
}

fn paint_row(
    ui: &mut Ui,
    icon: Option<&'static str>,
    label: &str,
    kbd: Option<&str>,
    enabled: bool,
    reserve_icon: bool,
) -> Response {
    let h = ROW_H;
    let w = ui.available_width();
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(w, h), sense);
    if !enabled {
        resp = resp.on_hover_cursor(egui::CursorIcon::NotAllowed);
    } else {
        resp = resp.clickable();
    }
    let hovered = enabled && resp.hovered();
    let t = theme::tokens(ui.ctx());
    let bg = if hovered {
        t.bg_sunken
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, radius::XS as f32, bg);

    let painter = ui.painter().clone();
    let pad_x = space::S2 as f32;
    let cy = rect.center().y;
    let fg = if enabled { t.fg_1 } else { t.fg_4 };
    let muted = if enabled { t.fg_3 } else { t.fg_4 };

    let label_x = if reserve_icon {
        let icon_rect = Rect::from_min_size(
            Pos2::new(rect.left() + pad_x, cy - ICON_SIZE / 2.0),
            Vec2::splat(ICON_SIZE),
        );
        if let Some(name) = icon {
            crate::ui::utils::icons::icon(ui.ctx(), name, ICON_SIZE, fg).paint_at(ui, icon_rect);
        }
        icon_rect.right() + space::S2 as f32
    } else {
        rect.left() + pad_x
    };

    let label_galley = painter.layout_no_wrap(label.into(), theme::body(13.0), fg);
    painter.galley(
        Pos2::new(label_x, cy - label_galley.size().y / 2.0),
        label_galley,
        fg,
    );

    if let Some(k) = kbd {
        let right = rect.right() - pad_x;
        let g = painter.layout_no_wrap(k.into(), theme::mono(11.0), muted);
        painter.galley(
            Pos2::new(right - g.size().x, cy - g.size().y / 2.0),
            g,
            muted,
        );
    }
    resp
}

fn paint_submenu_row(ui: &mut Ui, icon: &'static str, label: &str, width: f32) -> Response {
    let t = theme::tokens(ui.ctx());
    let h = ROW_H;
    // Absorb click events without acting on them — `Sense::click()`
    // would register a click and dismiss the parent menu under
    // `CloseOnClick`. Use `Sense::hover` and rely on hover to open.
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, h), Sense::hover());
    let hovered = resp.hovered();
    let bg = if hovered {
        t.bg_sunken
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, radius::XS as f32, bg);

    let painter = ui.painter().clone();
    let pad_x = space::S2 as f32;
    let cy = rect.center().y;

    let icon_rect = Rect::from_min_size(
        Pos2::new(rect.left() + pad_x, cy - ICON_SIZE / 2.0),
        Vec2::splat(ICON_SIZE),
    );
    crate::ui::utils::icons::icon(ui.ctx(), icon, ICON_SIZE, t.fg_1).paint_at(ui, icon_rect);

    let label_galley = painter.layout_no_wrap(label.into(), theme::body(13.0), t.fg_1);
    let lx = icon_rect.right() + space::S2 as f32;
    painter.galley(
        Pos2::new(lx, cy - label_galley.size().y / 2.0),
        label_galley,
        t.fg_1,
    );

    let chev_size = 12.0;
    let chev_rect = Rect::from_min_size(
        Pos2::new(rect.right() - pad_x - chev_size, cy - chev_size / 2.0),
        Vec2::splat(chev_size),
    );
    crate::ui::utils::icons::icon(ui.ctx(), "chevron-right", chev_size, t.fg_3)
        .paint_at(ui, chev_rect);

    resp
}
