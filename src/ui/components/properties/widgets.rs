//! Small surface widgets reused across the Properties tabs:
//! `section_card`, `kv_row`, `path_row`, `url_row`, `lock_banner`,
//! `info_callout`, `phase_pill`, `captured_kv`, `status_banner`.
//!
//! Each is a thin closed-loop renderer — no app state, no side effects
//! other than a single optional click. Callers pass already-formatted
//! strings + colors; these widgets only own visual styling.

use eframe::egui::{self, Align, Layout, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use egui_flex::{Flex, FlexAlign, FlexItem};

use crate::ui::color::rust;
use crate::ui::components::primitives::copy_feedback as cp_copy;
use crate::ui::components::primitives::{Btn, BtnSize};
use crate::ui::theme::{self, radius, space, ts};
use crate::ui::utils::icons;

use super::types::soft_tint;

/// Header row: bold title + sub-text on the left, a trailing widget
/// (dropdown, toggle, button) on the right. Reserves `trail_w` for the
/// right-hand widget up front so the sub-text can wrap inside the
/// remaining width instead of running behind the trailing widget.
///
/// Matches the recurring layout used by the Properties tabs' "Use
/// proxy", "Proxy authentication", "Send cookies", "Scheme" rows.
pub fn header_with_trailing(
    ui: &mut egui::Ui,
    t: &theme::Tokens,
    title: &str,
    sub: &str,
    trail_w: f32,
    trail: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            // Drop the 18px touch-target floor so the row's height is the
            // content's height + the frame's 10px top/bottom padding only.
            // Without this, ui.horizontal pads the row to ~18px even when the
            // content is shorter, which adds visible empty space above the
            // title (the band the design doesn't show).
            ui.spacing_mut().interact_size.y = 0.0;
            // Manual fixed-height row — NOT egui_flex. Under a ScrollArea
            // (`auto_shrink([false, …])`) flex's cross-axis height tracked
            // the viewport, so the card grew/shrank with the window and the
            // content drifted off-centre. Here the row height is pinned to
            // the title+sub block's measured height, so it's identical at
            // any window size. The title+sub block (always the tallest
            // child) fills the row; the trailing cell vertically centres
            // within the same height.
            let gap = space::S2 as f32;
            let title_font = theme::body_medium(12.0);
            let sub_font = theme::body(11.0);
            let row_gap_y = 2.0;
            let title_g = ui
                .painter()
                .layout_no_wrap(title.to_owned(), title_font.clone(), t.fg_1);
            let sub_g = ui
                .painter()
                .layout_no_wrap(sub.to_owned(), sub_font.clone(), t.fg_3);
            let row_h = title_g.size().y + row_gap_y + sub_g.size().y;
            // Reserve the full-width row at the fixed height up front with
            // `allocate_exact_size` — this both fills the card width and
            // avoids the content-driven sizing that made `allocate_ui` /
            // `set_min_width` shrink the card or oscillate (request_discard
            // hang) inside the ScrollArea. Children paint into fixed sub-rects.
            let avail = ui.available_width();
            let (rect, _) = ui.allocate_exact_size(Vec2::new(avail, row_h), Sense::hover());
            // The trailing reservation yields to the title+sub column on
            // narrow widths, so the label never collapses to 0 and wraps
            // vertically. Both cells truncate rather than wrap, keeping the
            // row at its measured single-line height.
            let min_left = title_g.size().x.max(sub_g.size().x);
            let trail_w = trail_w.min((avail - gap - min_left).max(0.0));
            let left_w = (avail - trail_w - gap).max(0.0);
            let left_rect =
                Rect::from_min_max(rect.min, Pos2::new(rect.left() + left_w, rect.bottom()));
            let trail_rect =
                Rect::from_min_max(Pos2::new(rect.right() - trail_w, rect.top()), rect.max);

            // Title + sub, filling the row height exactly (top-aligned, but
            // the row height == their height so it reads centered).
            let mut lui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(left_rect)
                    .layout(Layout::top_down(Align::Min)),
            );
            lui.spacing_mut().interact_size.y = 0.0;
            lui.spacing_mut().item_spacing.y = row_gap_y;
            lui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            lui.label(RichText::new(title).color(t.fg_1).font(title_font));
            lui.label(RichText::new(sub).color(t.fg_3).font(sub_font));

            // Trailing cell vertically centred in the row; left-to-right so
            // multi-widget trails (Speed limit's Unlimited/Limit/value/KB|MB)
            // read in order.
            let mut tui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(trail_rect)
                    .layout(Layout::left_to_right(Align::Center)),
            );
            tui.spacing_mut().item_spacing.x = gap;
            tui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            trail(&mut tui);
        });
}

/// White / surface card with a hairline border. Carries **no** inner
/// padding — matches CSS `.prop-section-body { background; border;
/// border-radius; overflow: hidden }`. Each child row (kv_row,
/// path_row, header_with_trailing, …) supplies its own `10px 12px`
/// padding, so wrapping the card in extra margin would double the
/// padding on the top/bottom edges. Children also stack with no inter-
/// row spacing — sibling separators are drawn explicitly via hairline
/// rects in callers that want them.
pub fn section_card(ui: &mut egui::Ui, t: &theme::Tokens, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::NONE
        .fill(t.bg_surface)
        .stroke(Stroke::new(t.border_width, t.border_subtle))
        .corner_radius(theme::surface::RADIUS)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            body(ui);
        });
}

/// One row inside a `section_card` — adds the standard `.prop-row`
/// padding (10px × 12px) around the body. Use when the body isn't a
/// `kv_row`/`path_row`/`header_with_trailing` (those already carry
/// their own padding).
pub fn prop_row(ui: &mut egui::Ui, _t: &theme::Tokens, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.spacing_mut().interact_size.y = 0.0;
            body(ui);
        });
}

/// Hairline separator drawn between sibling rows inside a section_card.
/// Matches CSS `.prop-row { border-bottom: 1px solid border-subtle }`.
/// Callers paint this between rows (not before the first / after the
/// last), so the card's own border carries the outer edges.
pub fn row_sep(ui: &mut egui::Ui, t: &theme::Tokens) {
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), t.border_width),
        Sense::hover(),
    );
    ui.painter().rect_filled(rect, 0.0, t.border_subtle);
}

/// Stacked variant — label on top in a small-caps eyebrow weight,
/// content below, both inside one padded `.prop-row.stack` row with
/// 6px gap. JSX equivalent: `<PropRow label=… stack>…</PropRow>`.
pub fn prop_row_stack(
    ui: &mut egui::Ui,
    t: &theme::Tokens,
    label: &str,
    body: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.spacing_mut().interact_size.y = 0.0;
            ui.spacing_mut().item_spacing.y = 6.0;
            // `.prop-row-lbl { font: 500 12px body; color: fg-1 }`.
            ui.label(
                RichText::new(label)
                    .color(t.fg_1)
                    .font(theme::body_medium(12.0)),
            );
            body(ui);
        });
}

/// Key / value row inside a surface frame. `mono` switches the value
/// font to mono-sm (hex hashes, paths, etc.).
pub fn kv_row(ui: &mut egui::Ui, t: &theme::Tokens, label: &str, value: &str, mono: bool) {
    // `.prop-row { padding: 10px 12px; border-bottom: 1px solid border-subtle }`.
    // The border-bottom is drawn explicitly by the caller (or by the
    // `kv_rows`-style helper) between siblings inside the card frame.
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.spacing_mut().interact_size.y = 0.0;
            ui.horizontal(|ui| {
                // Match the `prop_row_stack` label (URL / Save to): 500-weight
                // 12px in fg_1, so all field labels in the dialog read alike.
                ui.label(
                    RichText::new(label)
                        .color(t.fg_1)
                        .font(theme::body_medium(12.0)),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let font = if mono {
                        ts::mono_sm()
                    } else {
                        theme::body(13.0)
                    };
                    // Values read one step softer than labels (design
                    // `.prop-row-value { color: var(--fg-2) }`).
                    ui.label(RichText::new(value).color(t.fg_2).font(font));
                });
            });
        });
}

/// Folder-icon + path text + open-in-finder button. Returns `true` if
/// the user clicked the button this frame.
pub fn path_row(ui: &mut egui::Ui, t: &theme::Tokens, path: &str) -> bool {
    let mut clicked = false;
    egui::Frame::NONE
        .fill(t.bg_raised)
        .stroke(Stroke::new(t.border_width, t.border_subtle))
        .corner_radius(radius::SM as f32)
        // CSS `.prop-path { padding: 5px 8px; }`.
        .inner_margin(egui::Margin::symmetric(8, 5))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                icons::show(ui, "folder", 14.0, t.fg_3);
                ui.label(RichText::new(path).color(t.fg_1).font(ts::mono_sm()));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if Btn::new("")
                        .toolbar()
                        .icon_only("folder")
                        .size(BtnSize::Sm)
                        .show(ui)
                        .clicked()
                    {
                        clicked = true;
                    }
                });
            });
        });
    clicked
}

/// URL row with a copy-to-clipboard button. Copy is handled internally
/// via `ui.ctx().copy_text(...)` — the URL is the only data the caller
/// passes in.
pub fn url_row(ui: &mut egui::Ui, t: &theme::Tokens, url: &str) {
    let url_owned = url.to_string();
    egui::Frame::NONE
        .fill(t.bg_raised)
        .stroke(Stroke::new(t.border_width, t.border_subtle))
        .corner_radius(radius::SM as f32)
        // Matches `.prop-path` (5px 8px) — URL row shares the visual.
        .inner_margin(egui::Margin::symmetric(8, 5))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&url_owned).color(t.fg_1).font(ts::mono_sm()));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let id = ui.id().with("url-row-copy");
                    cp_copy::copy_button(ui, id, url_owned.clone(), BtnSize::Sm);
                });
            });
        });
}

/// Top-of-tab banner explaining read-only state while a download is
/// running. Warning-tinted lock icon + a single line of guidance.
pub fn lock_banner(ui: &mut egui::Ui, t: &theme::Tokens) {
    egui::Frame::NONE
        .fill(t.status_warning_bg)
        .stroke(Stroke::new(t.border_width, t.status_warning))
        .corner_radius(radius::SM as f32)
        // `.prop-lock-banner { padding: 10px 12px; }`.
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                icons::show(ui, "lock", 14.0, t.status_warning);
                ui.label(
                    RichText::new(
                        "Settings are read-only while this download is running. Pause it to edit — your changes take effect when you resume.",
                    )
                    .color(t.fg_2)
                    .font(theme::body(12.0)),
                );
            });
        });
}

/// Smaller success-tinted variant used in the Checksums tab. Adding
/// checksums is allowed even while a download runs.
pub fn lock_banner_checksums(ui: &mut egui::Ui, t: &theme::Tokens) {
    egui::Frame::NONE
        .fill(soft_tint(t.status_success, t.bg_surface, 0.08))
        .stroke(Stroke::new(t.border_width, soft_tint(t.status_success, t.bg_surface, 0.45)))
        .corner_radius(radius::SM as f32)
        // `.prop-cs-lockhint { padding: 6px 10px; }`.
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                icons::show(ui, "lock", 13.0, t.status_success);
                ui.label(
                    RichText::new(
                        "Adding checksums is allowed even while the download is running — verification doesn't touch the transfer.",
                    )
                    .color(t.fg_2)
                    .font(theme::body(12.0)),
                );
            });
        });
}

/// Info-tinted callout box with a single message. Used by Connection
/// when System Proxy is selected.
pub fn info_callout(ui: &mut egui::Ui, t: &theme::Tokens, msg: &str) {
    egui::Frame::NONE
        .fill(t.bg_sunken)
        .stroke(Stroke::new(t.border_width, t.border_subtle))
        .corner_radius(radius::SM as f32)
        // `.prop-note { padding: 8px 12px; }`.
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                icons::show(ui, "info", 14.0, t.status_info);
                ui.label(RichText::new(msg).color(t.fg_2).font(theme::body(12.0)));
            });
        });
}

/// Phase pill — small soft-tinted capsule with a coloured dot and
/// uppercase label. Renders at the right edge of the hero strip.
pub fn phase_pill(ui: &mut egui::Ui, t: &theme::Tokens, color: egui::Color32, label: &str) {
    egui::Frame::NONE
        // `.prop-status-pill { background: var(--bg-page); border: 1px
        // solid var(--border-subtle); }` — flat page fill + subtle
        // border; the phase colour lives only on the dot + label.
        .fill(t.bg_page)
        .stroke(Stroke::new(t.border_width, t.border_subtle))
        .corner_radius(10.0)
        // Design pill height is 22px. Ink (~8px) + 2×6px vertical padding
        // + hairline border lands there; horizontal padding stays 9px.
        .inner_margin(egui::Margin::symmetric(9, 6))
        .show(ui, |ui| {
            ui.spacing_mut().interact_size.y = 0.0;
            let font = theme::body_bold(10.0);
            // Centre the glyph *ink*, not the line box. The uppercase label
            // has no descenders, so egui's line box carries empty space
            // below the caps; centring the box (e.g. via `ui.label` or
            // symmetric padding) makes the caps read top-heavy. Size the row
            // to `mesh_bounds` (the painted glyph extent) and place the
            // galley so that ink fills the row exactly → optically centred at
            // any pixel ratio.
            let galley = ui.painter().layout_no_wrap(label.to_string(), font, color);
            let ink = galley.mesh_bounds;
            let text_w = galley.size().x;
            let gap = 5.0;
            let dot_d = 6.0;
            let (rect, _) = ui.allocate_exact_size(
                Vec2::new(dot_d + gap + text_w, ink.height()),
                Sense::hover(),
            );
            // All-caps reads optically high even when the ink is
            // geometrically centred (the eye weights the glyph mass below
            // its midline), so drop the whole row 1px. Dot + text move
            // together to stay aligned.
            let optical = 1.0;
            ui.painter().circle_filled(
                Pos2::new(rect.left() + 3.0, rect.center().y + optical),
                3.0,
                color,
            );
            // ink.min.y is the galley-local offset of the first painted
            // pixel; subtracting it lands ink.top on rect.top.
            ui.painter().galley(
                Pos2::new(rect.left() + dot_d + gap, rect.top() - ink.min.y + optical),
                galley,
                color,
            );
        });
}

/// Single captured-request header line — fixed 140px label column +
/// mono value. Used in the Headers tab "captured request" / "captured
/// response" panels.
pub fn captured_kv(ui: &mut egui::Ui, t: &theme::Tokens, k: &str, v: &str) {
    ui.horizontal(|ui| {
        ui.allocate_ui(Vec2::new(140.0, 18.0), |ui| {
            ui.label(RichText::new(k).color(t.fg_2).font(ts::mono_sm()));
        });
        ui.label(RichText::new(v).color(t.fg_1).font(ts::mono_sm()));
    });
}

/// Configuration for the top-of-Checksums status banner. Picks fill +
/// stroke + icon color based on the worst observed status.
#[derive(Clone, Copy)]
pub enum BannerTone {
    Neutral,
    Success,
    Danger,
    Partial,
}

/// Status banner at the top of the Checksums tab. The action button is
/// optional; returns `true` when clicked.
///
/// `_icon` is kept for ABI compatibility but ignored — the tone now
/// picks the lucide icon (`shield` / `shield-check` / `shield-alert` /
/// `shield-question`).
pub fn status_banner(
    ui: &mut egui::Ui,
    t: &theme::Tokens,
    tone: BannerTone,
    _icon: &'static str,
    title: &str,
    sub: &str,
    action: Option<&str>,
) -> bool {
    let (icon_name, icon_color, title_color, bg, stroke_color) = match tone {
        // `.pcb-none .pcb-icon { color: var(--ochre-400); }`.
        BannerTone::Neutral => (
            "shield-question",
            crate::ui::color::ochre::O400,
            t.fg_1,
            t.bg_surface,
            t.border_subtle,
        ),
        BannerTone::Success => (
            "shield-check",
            t.status_success,
            t.fg_1,
            soft_tint(t.status_success, t.bg_surface, 0.07),
            soft_tint(t.status_success, t.bg_surface, 0.35),
        ),
        BannerTone::Danger => (
            "shield-alert",
            rust::R300,
            rust::R300,
            // `.pcb-mismatch { background: rgba(190,60,40,0.05);
            //                  border-color: rgba(190,60,40,0.35); }`.
            egui::Color32::from_rgba_unmultiplied(190, 60, 40, 13),
            egui::Color32::from_rgba_unmultiplied(190, 60, 40, 89),
        ),
        BannerTone::Partial => (
            "shield-question",
            t.fg_3,
            t.fg_1,
            t.bg_surface,
            t.border_subtle,
        ),
    };

    // Icon tile border: subtle by default, but the danger variant tints it
    // to match the card (`.pcb-mismatch .pcb-icon` → rgba(190,60,40,0.35)).
    let tile_border = if matches!(tone, BannerTone::Danger) {
        egui::Color32::from_rgba_unmultiplied(190, 60, 40, 89)
    } else {
        t.border_subtle
    };

    let mut clicked = false;
    egui::Frame::NONE
        .fill(bg)
        .stroke(Stroke::new(t.border_width, stroke_color))
        .corner_radius(theme::surface::RADIUS)
        // `.prop-cs-banner { padding: 12px 14px; }`.
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            // egui_flex gives the three cells a shared vertical centre line
            // (its whole reason for being here). It can't, however, *bound* a
            // wrapping cell: a `grow` cell reports the text's full single-line
            // width as intrinsic → the card overflows, and `grow + shrink +
            // Wrap` oscillates → the app hangs. So we measure the action
            // button up front and hand the title+sub cell an explicit `basis`
            // = leftover width. A definite width makes wrap deterministic —
            // no overflow, no oscillation, still centered. See egui_flex notes.
            let gap = space::S2 as f32;
            let tile = 36.0;
            let btn = action.map(|lab| Btn::new(lab).icon("shield-check").size(BtnSize::Sm));
            let btn_w = btn.as_ref().map_or(0.0, |b| b.measured_size(ui).x);
            let n_gaps = if btn.is_some() { 2.0 } else { 1.0 };
            let text_w = (ui.available_width() - tile - btn_w - gap * n_gaps).max(0.0);

            Flex::horizontal()
                .align_items(FlexAlign::Center)
                .gap(Vec2::new(gap, 0.0))
                .w_full()
                .show(ui, |flex| {
                    // Icon in a soft-tinted rounded square tile.
                    flex.add_ui(FlexItem::new(), |ui| {
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(tile), Sense::hover());
                        // `.pcb-icon { background: var(--bg-page); border: 1px
                        // solid var(--border-subtle); border-radius: 8px; }`.
                        ui.painter().rect_filled(rect, 8.0, t.bg_page);
                        ui.painter().rect_stroke(
                            rect,
                            8.0,
                            Stroke::new(t.border_width, tile_border),
                            egui::StrokeKind::Inside,
                        );
                        // Design: 18px icon centred in the 36px tile.
                        let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(18.0));
                        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(icon_rect));
                        icons::show(&mut child, icon_name, 18.0, icon_color);
                    });
                    // Title + sub column, bounded to the leftover width.
                    flex.add_ui(FlexItem::new().basis(text_w), |ui| {
                        ui.vertical(|ui| {
                            ui.spacing_mut().interact_size.y = 0.0;
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                            ui.label(banner_title_job(title, title_color));
                            ui.label(RichText::new(sub).color(t.fg_3).font(theme::body(12.0)));
                        });
                    });
                    if let Some(b) = btn {
                        flex.add_ui(FlexItem::new(), |ui| {
                            if b.show(ui).clicked() {
                                clicked = true;
                            }
                        });
                    }
                });
        });
    clicked
}

/// Build the banner title as a `LayoutJob`. If the string contains the
/// whole-word `not` (between spaces), render it bold so the danger
/// banner matches the handoff ("do **not** trust this file"). The
/// surrounding text uses the medium weight so the bold "not" reads as
/// emphasised within an otherwise non-emphatic title.
fn banner_title_job(title: &str, color: egui::Color32) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let plain = TextFormat {
        font_id: theme::body_medium(14.0),
        color,
        ..Default::default()
    };
    let strong = TextFormat {
        font_id: theme::body_bold(14.0),
        color,
        ..Default::default()
    };
    let mut job = LayoutJob::default();
    let needle = " not ";
    if let Some(i) = title.find(needle) {
        job.append(&title[..i], 0.0, plain.clone());
        job.append(" ", 0.0, plain.clone());
        job.append("not", 0.0, strong);
        job.append(&title[i + needle.len() - 1..], 0.0, plain);
    } else {
        job.append(title, 0.0, plain);
    }
    job
}
