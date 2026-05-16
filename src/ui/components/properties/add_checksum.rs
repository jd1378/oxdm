//! "Add checksum manually" inline form. The caller owns the form's
//! editable state (`AddChecksumState`); this widget only renders it
//! and reports user intent (`AddChecksumOutcome`).
//!
//! Spec: design/handoff/09b_properties_dialog.md §4.2.3.

use std::collections::HashSet;

use eframe::egui::{self, Align, Layout, Rect, RichText, Sense, Stroke, Vec2};

use crate::ui::components::primitives::{Btn, BtnSize, Checkbox, TextArea};
use crate::ui::theme::{self, space, ts};
use crate::ui::utils::icons;

use super::types::{Algo, soft_tint};

/// Caller-owned form state. `auto_detect` flips off whenever the user
/// clicks an algorithm chip manually so the picker doesn't fight them.
#[derive(Debug, Clone)]
pub struct AddChecksumState {
    pub algo: Algo,
    pub hash: String,
    pub auto_detect: bool,
}

impl Default for AddChecksumState {
    fn default() -> Self {
        Self {
            algo: Algo::Md5,
            hash: String::new(),
            auto_detect: true,
        }
    }
}

/// What the user did this frame. `save` is `Some((effective_algo,
/// canonical_hex))` when the Save button was clicked and the inputs
/// pass validation. The caller decides what to do with the value
/// (compute local hash, compare, push into the checksums list, …).
#[derive(Default)]
pub struct AddChecksumOutcome {
    pub cancel: bool,
    pub save: Option<(Algo, String)>,
}

/// Render the form. `existing` is the set of algorithms already present
/// in the checksum list — used to grey out duplicates in the picker and
/// flag duplicate hashes in the live validation hint.
pub fn add_checksum_form(
    ui: &mut egui::Ui,
    t: &theme::Tokens,
    state: &mut AddChecksumState,
    existing: &HashSet<Algo>,
) -> AddChecksumOutcome {
    let mut out = AddChecksumOutcome::default();

    egui::Frame::NONE
        .fill(soft_tint(t.action_primary, t.bg_surface, 0.08))
        .stroke(Stroke::new(t.border_width, t.action_primary))
        .corner_radius(theme::surface::RADIUS)
        // `.prop-add-cs { padding: 12px 14px 14px; }`.
        .inner_margin(egui::Margin { left: 14, right: 14, top: 12, bottom: 14 })
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = space::S2 as f32;
            ui.horizontal(|ui| {
                icons::show(ui, "plus", 14.0, t.action_primary);
                ui.label(
                    RichText::new("ADD CHECKSUM MANUALLY")
                        .color(t.action_primary)
                        .font(theme::body_bold(11.0)),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if Btn::new("")
                        .toolbar()
                        .icon_only("x")
                        .size(BtnSize::Sm)
                        .show(ui)
                        .clicked()
                    {
                        out.cancel = true;
                    }
                });
            });

            ui.label(
                RichText::new("Algorithm")
                    .color(t.fg_2)
                    .font(theme::body_bold(12.0)),
            );
            ui.horizontal(|ui| {
                for algo in Algo::ALL.iter().copied() {
                    let in_list = existing.contains(&algo);
                    let resp = Btn::new(algo.label())
                        .size(BtnSize::Sm)
                        .selected(state.algo == algo)
                        .enabled(!in_list)
                        .show(ui);
                    if resp.clicked() {
                        state.algo = algo;
                        state.auto_detect = false;
                    }
                }
            });

            // Auto-detect based on length.
            let canon: String = state
                .hash
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>();
            let canon_lc = canon.to_ascii_lowercase();
            let detected = Algo::ALL
                .iter()
                .copied()
                .find(|a| a.hex_len() == canon_lc.chars().count() && !existing.contains(a));
            let effective = if state.auto_detect {
                detected.unwrap_or(state.algo)
            } else {
                state.algo
            };

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(
                    RichText::new("Hash")
                        .color(t.fg_2)
                        .font(theme::body_bold(12.0)),
                );
                // "· expects <N> hex characters" — bold the numeral to
                // match the handoff (hash label has emphatic count).
                let mut job = egui::text::LayoutJob::default();
                let plain = egui::TextFormat {
                    font_id: theme::body(12.0),
                    color: t.fg_3,
                    ..Default::default()
                };
                let strong = egui::TextFormat {
                    font_id: theme::body_bold(12.0),
                    color: t.fg_2,
                    ..Default::default()
                };
                job.append("· expects ", 0.0, plain.clone());
                job.append(&format!("{}", effective.hex_len()), 0.0, strong);
                job.append(" hex characters", 0.0, plain);
                ui.label(job);
            });
            let rows = match effective {
                Algo::Sha512 => 4,
                Algo::Sha384 => 3,
                _ => 3,
            };
            let row_h = 18.0;
            let initial_h = rows as f32 * row_h + 16.0;
            let ta_id = format!("props-add-checksum-hash-{:?}", ui.next_auto_id());
            TextArea::new(&mut state.hash, &ta_id)
                .font(ts::mono_sm())
                .hint(format!(
                    "Paste the {} hash from the publisher's website…",
                    effective.label()
                ))
                .initial_height(initial_h)
                .min_height(initial_h)
                .show(ui);

            // Live status + counter + fill bar.
            let target = effective.hex_len();
            let len = canon_lc.chars().count();
            let is_hex = canon_lc.chars().all(|c| c.is_ascii_hexdigit());
            let duplicate = existing.contains(&effective);
            let (msg, color) = if canon_lc.is_empty() {
                (
                    "Paste a hex hash. Whitespace and a leading filename are removed automatically.".to_string(),
                    t.fg_3,
                )
            } else if !is_hex {
                ("× Contains non-hex characters.".to_string(), t.status_danger)
            } else if len > target {
                (
                    format!("⚠ {} too many — too long for {}.", len - target, effective.label()),
                    t.status_danger,
                )
            } else if len < target {
                (format!("− {} more characters needed", target - len), t.fg_3)
            } else if duplicate {
                (
                    format!("⚠ {} is already in the list.", effective.label()),
                    t.status_danger,
                )
            } else {
                (format!("✓ Looks like a valid {} hash.", effective.label()), t.status_success)
            };
            // Counter + fill bar on a single row (handoff layout): the
            // mono counter sits left, the fill bar consumes the rest of
            // the line. Helper / status text drops below as its own line.
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{len}/{target}"))
                        .color(t.fg_3)
                        .font(ts::mono_sm()),
                );
                ui.add_space(space::S2 as f32);
                let bar_w = ui.available_width();
                let (bar_rect, _) =
                    ui.allocate_exact_size(Vec2::new(bar_w, 3.0), Sense::hover());
                ui.painter().rect_filled(bar_rect, 1.5, t.bg_sunken);
                // Ease the fill width toward the target fraction so pasting /
                // editing the hash slides the bar instead of snapping.
                let target_frac = (len as f32 / target.max(1) as f32).clamp(0.0, 1.0);
                let frac = ui.ctx().animate_value_with_time(
                    egui::Id::new("props-add-checksum-fill"),
                    target_frac,
                    0.15,
                );
                let fill_w = bar_rect.width() * frac;
                let fill_rect =
                    Rect::from_min_size(bar_rect.left_top(), Vec2::new(fill_w, 3.0));
                ui.painter().rect_filled(fill_rect, 1.5, color);
            });
            // Helper / status line on its own row beneath the counter.
            ui.label(RichText::new(msg).color(color).font(theme::body(11.0)));

            // Auto-detect toggle.
            ui.horizontal(|ui| {
                Checkbox::new(&mut state.auto_detect)
                    .id(ui.next_auto_id().with("props-autodetect"))
                    .show(ui);
                ui.add_space(6.0);
                ui.label(
                    RichText::new("Auto-detect algorithm from hash length")
                        .color(t.fg_2)
                        .font(theme::body(12.0)),
                );
                ui.label(
                    RichText::new(" — overrides the picker above when a hash is recognized.")
                        .color(t.fg_3)
                        .font(theme::body(11.0)),
                );
            });

            ui.separator();
            ui.horizontal(|ui| {
                if Btn::new("Cancel").ghost().show(ui).clicked() {
                    out.cancel = true;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let can_save = is_hex && len == target && !duplicate;
                    if Btn::new(format!("Save {}", effective.label()))
                        .primary()
                        .icon("check")
                        .enabled(can_save)
                        .show(ui)
                        .clicked()
                    {
                        out.save = Some((effective, canon_lc.clone()));
                    }
                });
            });
        });

    out
}
