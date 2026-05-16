//! Per-checksum surface row used inside the Checksums tab:
//! algorithm · status pill · truncated hash · source · actions, plus
//! an EXPECTED / GOT diff strip when the hash mismatches.

use eframe::egui::{self, Align, CornerRadius, Layout, Pos2, Rect, RichText, Sense, Stroke, Vec2};

use crate::ui::color::rust;

use crate::ui::components::primitives::{Btn, BtnSize, copy_feedback};
use crate::ui::theme::{self, radius, space, ts};
use crate::ui::utils::icons;

use super::types::{Checksum, CsSource, CsStatus, soft_tint, truncate_mid};

// Shared column widths — `checksum_list_header` and `checksum_row` must
// agree so the labels line up over the cells.
// Algorithm / Status / Source are fixed. Hash flexes — fills whatever
// remains between Status and the trailing actions reservation. Actions
// is fixed so each row reserves the same right edge whether or not the
// trailing shield-check button is drawn.
const COL_ALGO: f32 = 70.0;
const COL_STATUS: f32 = 120.0;
const COL_SOURCE: f32 = 70.0;
const COL_ACTIONS: f32 = 96.0;

/// Render one row of cells using the shared column widths. The third
/// cell (hash) flexes — it consumes whatever horizontal space is left
/// after the other three columns + the actions reservation.
fn columns(
    ui: &mut egui::Ui,
    cell_h: f32,
    cells: [Box<dyn FnOnce(&mut egui::Ui) + '_>; 4],
    actions: Option<Box<dyn FnOnce(&mut egui::Ui) + '_>>,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        // Drop the 18px touch-target floor so the row is exactly `cell_h`
        // tall (the header passes a tight height; data rows pass 22).
        ui.spacing_mut().interact_size.y = 0.0;
        let total = ui.available_width();
        let hash_w = (total - COL_ALGO - COL_STATUS - COL_SOURCE - COL_ACTIONS).max(120.0);
        let widths = [COL_ALGO, COL_STATUS, hash_w, COL_SOURCE];
        let mut cells = cells.into_iter();
        for w in widths {
            let next = cells.next().unwrap();
            // Reserve the full column width so cells stay aligned even
            // when inner content is narrower than the column.
            let start_x = ui.cursor().min.x;
            ui.allocate_ui_with_layout(
                Vec2::new(w, cell_h),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.set_min_size(Vec2::new(w, cell_h));
                    next(ui)
                },
            );
            // Force the cursor forward to the next column boundary
            // regardless of how much the inner content consumed.
            let cur = ui.cursor();
            let new_min = egui::pos2(start_x + w, cur.min.y);
            ui.advance_cursor_after_rect(egui::Rect::from_min_max(
                cur.min,
                egui::pos2(new_min.x, cur.min.y),
            ));
        }
        // Reserve a fixed actions slot so the right edge of every row
        // lines up regardless of which trailing buttons are drawn.
        ui.allocate_ui_with_layout(
            Vec2::new(COL_ACTIONS, cell_h),
            Layout::right_to_left(Align::Center),
            |ui| {
                ui.set_min_size(Vec2::new(COL_ACTIONS, cell_h));
                if let Some(act) = actions {
                    act(ui);
                }
            },
        );
    });
}

/// Header strip with the four column labels. Now drawn as the *first
/// row* of the shared checksum table; callers should place the header
/// and each `checksum_row` inside the same `cp::section_card`.
pub fn checksum_list_header(ui: &mut egui::Ui, t: &theme::Tokens) {
    egui::Frame::NONE
        .fill(t.bg_sunken)
        // Round the top corners to match the enclosing card — otherwise the
        // sunken header's square corners poke past the card's rounded edge.
        .corner_radius(CornerRadius {
            nw: theme::surface::RADIUS as u8,
            ne: theme::surface::RADIUS as u8,
            sw: 0,
            se: 0,
        })
        // `.prop-cs-list-head { padding: 6px 12px; }`.
        .inner_margin(egui::Margin::symmetric(12, 6))
        .show(ui, |ui| {
            // Paint all four labels into one fixed-height row at the same
            // column x-offsets the data rows' `columns()` uses, each centred
            // by glyph *ink* on the shared row centre. Done by hand (not
            // `columns()` + `ui.label`) because a horizontal row won't
            // re-centre an earlier cell against a later one, and the line box
            // would read high — both make the labels drift vertically.
            let font = theme::body_bold(10.0);
            let total = ui.available_width();
            let hash_w = (total - COL_ALGO - COL_STATUS - COL_SOURCE - COL_ACTIONS).max(120.0);
            let cols = [
                ("ALGORITHM", 0.0),
                ("STATUS", COL_ALGO),
                ("HASH", COL_ALGO + COL_STATUS),
                ("SOURCE", COL_ALGO + COL_STATUS + hash_w),
            ];
            let probe = ui
                .painter()
                .layout_no_wrap("HASH".to_owned(), font.clone(), t.fg_3);
            let row_h = probe.mesh_bounds.height();
            let (rect, _) = ui.allocate_exact_size(Vec2::new(total, row_h), Sense::hover());
            for (label, x) in cols {
                let g = ui
                    .painter()
                    .layout_no_wrap(label.to_owned(), font.clone(), t.fg_3);
                let ink = g.mesh_bounds;
                let pos = Pos2::new(rect.left() + x, rect.center().y - ink.center().y);
                ui.painter().galley(pos, g, t.fg_3);
            }
        });
    // hairline divider below header
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), t.border_width),
        Sense::hover(),
    );
    ui.painter().rect_filled(rect, 0.0, t.border_subtle);
}

/// Outcome of a single frame of `checksum_row`. Driver loop applies
/// `verify` / `copy` / `remove` against its owned state.
#[derive(Default, Clone)]
pub struct ChecksumRowAction {
    pub copy: Option<String>,
    pub verify: bool,
    pub remove: bool,
}

/// Coloured status pill — "verified" / "mismatch" / "imported · not
/// verified" / "" (computed). Renders inside a soft-tinted capsule that
/// matches the handoff.
pub fn status_pill(ui: &mut egui::Ui, t: &theme::Tokens, status: CsStatus, source: CsSource) {
    let (icon, color, label, dashed) = match (status, source) {
        (CsStatus::Verified, _) => ("check", t.status_success, "verified", false),
        (CsStatus::Mismatch, _) => ("x", rust::R300, "mismatch", false),
        (CsStatus::Unverified, CsSource::Server) => ("globe", t.fg_3, "no source", true),
        (CsStatus::Unverified, CsSource::User) => ("user", t.fg_3, "imported · not verified", true),
        (CsStatus::Unverified, CsSource::Computed) => ("minus", t.fg_3, "", true),
    };

    // Pale fill matching the tone. Dashed pills (unverified) use the
    // surface color so only the dashed border carries the meaning.
    // Mismatch uses the design's exact translucent rust wash
    // (`rgba(190,60,40,0.12)`) so it composites over whatever sits behind.
    let fill = if dashed {
        t.bg_surface
    } else if status == CsStatus::Mismatch {
        egui::Color32::from_rgba_unmultiplied(190, 60, 40, 31)
    } else {
        soft_tint(color, t.bg_surface, 0.18)
    };
    // Solid chips carry the tone via fill alone — no border (design).
    let frame = egui::Frame::NONE
        .fill(fill)
        // Design chip radius is 4px.
        .corner_radius(4.0)
        // `.prop-cs-row .pill { padding: 2px 7px; }` for solid pills.
        .inner_margin(egui::Margin::symmetric(7, 2));
    let resp = frame.show(ui, |ui| {
        // Drop the 18px touch-target floor so the chip height is set by the
        // 11px label + 2×2 padding, not egui's default metric.
        ui.spacing_mut().interact_size.y = 0.0;
        if dashed {
            // Long labels ("imported · not verified") wrap to two lines —
            // cap the width to the STATUS column so they break instead of
            // pushing the HASH column right.
            ui.set_max_width(108.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.spacing_mut().item_spacing.y = 1.0;
                icons::show(ui, icon, 12.0, color);
                if !label.is_empty() {
                    ui.label(
                        RichText::new(label)
                            .color(color)
                            .font(theme::body_medium(10.0)),
                    );
                }
            });
        } else {
            // Solid single-word chip: paint icon + label manually so the
            // label centres by glyph *ink* (not the line box — caps have
            // empty descender space and otherwise read high). Icon centred
            // on the same line.
            // `.pill { font: 500 10px var(--font-body); }`.
            let font = theme::body_medium(10.0);
            let icon_sz = 12.0;
            let gap = 4.0;
            let galley = ui.painter().layout_no_wrap(label.to_string(), font, color);
            let ink = galley.mesh_bounds;
            let content_w = icon_sz + gap + galley.size().x;
            let row_h = icon_sz.max(ink.height());
            let (rect, _) = ui.allocate_exact_size(Vec2::new(content_w, row_h), Sense::hover());
            let icon_rect = Rect::from_min_size(
                Pos2::new(rect.left(), rect.center().y - icon_sz / 2.0),
                Vec2::splat(icon_sz),
            );
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(icon_rect));
            icons::show(&mut child, icon, icon_sz, color);
            // Place the galley so its ink centre lands on the row centre.
            let pos = Pos2::new(
                rect.left() + icon_sz + gap,
                rect.center().y - ink.center().y,
            );
            ui.painter().galley(pos, galley, color);
        }
    });
    // Dashed border overlay for unverified pills — matches the handoff
    // (the dashed outline signals an "open question" rather than a
    // committed state, complementing the solid-border verified/mismatch).
    if dashed {
        crate::ui::utils::dashed::paint_dashed_rect(
            ui.painter(),
            resp.response.rect,
            radius::SM as f32,
            Stroke::new(t.border_width, t.border_subtle),
            3.0,
            2.0,
        );
    }
}

/// Two-line EXPECTED / GOT diff strip rendered under a mismatching
/// checksum row.
pub fn mismatch_diff(ui: &mut egui::Ui, t: &theme::Tokens, expected: &str, got: &str) {
    // Empty first column (skip ALGORITHM), then a 3-column grid:
    // label | value | copy-button for EXPECTED and GOT. The grid
    // auto-sizes its label column to the wider "EXPECTED", so both values
    // share a left edge under the HASH column. The GOT value is struck
    // through — it's the bad hash. Each row's copy button copies the full
    // (untruncated) hash and shows the check-icon feedback.
    ui.horizontal(|ui| {
        // Kill egui's 18px touch-target floor so each grid row is only as
        // tall as its 11px text, not padded out — same fix used elsewhere.
        ui.spacing_mut().interact_size.y = 0.0;
        ui.add_space(COL_ALGO);
        // Per-row id so multiple mismatching rows don't share grid state.
        ui.push_id(("cs-mismatch", got), |ui| {
            egui::Grid::new("cs-mismatch-grid")
                .num_columns(3)
                .spacing(Vec2::new(space::S3 as f32, 2.0))
                .show(ui, |ui| {
                    let label = |ui: &mut egui::Ui, s: &str| {
                        ui.label(RichText::new(s).color(t.fg_3).font(theme::body_bold(10.0)));
                    };
                    let copy_btn = |ui: &mut egui::Ui, key: &str, value: &str| {
                        let id = ui.id().with(("cs-diff-copy", key));
                        copy_feedback::copy_button(ui, id, value.to_string(), BtnSize::Sm);
                    };
                    label(ui, "EXPECTED");
                    ui.label(
                        RichText::new(truncate_mid(expected, 14, 10))
                            .color(t.fg_2)
                            .font(ts::mono_sm()),
                    );
                    copy_btn(ui, "expected", expected);
                    ui.end_row();
                    label(ui, "GOT");
                    ui.label(
                        RichText::new(truncate_mid(got, 14, 10))
                            .color(rust::R300)
                            .strikethrough()
                            .font(ts::mono_sm()),
                    );
                    copy_btn(ui, "got", got);
                    ui.end_row();
                });
        });
    });
    let _ = (COL_STATUS, COL_SOURCE, COL_ACTIONS);
}

/// Render a single checksum row inside its own surface frame.
///
/// `is_locked` disables the trash button. Server-sourced verified hashes
/// can never be removed (spec §4.2.2). The shield-verify button only
/// appears for unverified rows.
pub fn checksum_row(
    ui: &mut egui::Ui,
    t: &theme::Tokens,
    cs: &Checksum,
    is_locked: bool,
) -> ChecksumRowAction {
    let mut out = ChecksumRowAction::default();

    egui::Frame::NONE
        // `.prop-cs-row { padding: 9px 12px; }`.
        .inner_margin(egui::Margin::symmetric(12, 9))
        .show(ui, |ui| {
            let algo_label = cs.algo.label();
            let short = truncate_mid(&cs.hash, 14, 10);
            let mono_color = match cs.status {
                CsStatus::Mismatch => rust::R300,
                _ => t.fg_1,
            };
            let source_label = match cs.source {
                CsSource::Server => "server",
                CsSource::Computed => "computed",
                CsSource::User => "you",
            };

            let mut copied: Option<String> = None;
            columns(
                ui,
                22.0,
                [
                    Box::new(|ui: &mut egui::Ui| {
                        ui.label(RichText::new(algo_label).color(t.fg_1).font(ts::mono_sm()));
                    }),
                    Box::new(|ui: &mut egui::Ui| {
                        status_pill(ui, t, cs.status, cs.source);
                    }),
                    Box::new(|ui: &mut egui::Ui| {
                        let resp = ui
                            .label(RichText::new(&short).color(mono_color).font(ts::mono_sm()))
                            .on_hover_text(&cs.hash);
                        if resp.interact(Sense::click()).clicked() {
                            copied = Some(cs.hash.clone());
                        }
                    }),
                    Box::new(|ui: &mut egui::Ui| {
                        ui.label(
                            RichText::new(source_label)
                                .color(t.fg_3)
                                .font(theme::body(11.0)),
                        );
                    }),
                ],
                Some(Box::new(|ui: &mut egui::Ui| {
                    let can_remove = !is_locked
                        && !(cs.source == CsSource::Server && cs.status == CsStatus::Verified);
                    if Btn::new("")
                        .toolbar()
                        .icon_only("trash-2")
                        .size(BtnSize::Sm)
                        .enabled(can_remove)
                        .show(ui)
                        .clicked()
                    {
                        out.remove = true;
                    }
                    let cid = ui.id().with(("cs-copy", cs.hash.as_str()));
                    copy_feedback::copy_button(ui, cid, cs.hash.clone(), BtnSize::Sm);
                    if cs.status == CsStatus::Unverified
                        && Btn::new("")
                            .toolbar()
                            .icon_only("shield-check")
                            .size(BtnSize::Sm)
                            .show(ui)
                            .clicked()
                    {
                        out.verify = true;
                    }
                })),
            );
            if let Some(h) = copied {
                out.copy = Some(h);
            }
            if cs.status == CsStatus::Mismatch {
                if let Some(local) = &cs.expected {
                    // EXPECTED = the published/imported hash (`cs.hash`);
                    // GOT = the locally-computed value (`cs.expected`), which
                    // is what mismatched and is struck through.
                    mismatch_diff(ui, t, &cs.hash, local);
                }
            }
        });
    let _ = radius::SM;
    out
}
