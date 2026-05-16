//! Cookie chip preview strip — small surface pills that abbreviate
//! parsed `name=value` cookies. Used at the foot of the Cookies tab.
//!
//! Sized to match `design/styles.css` `.prop-cookies-chip`:
//!
//! ```text
//! font: 500 10.5px mono   padding: 2px 7px   radius: 999 (pill)
//! bg:   bg_sunken         border: border_subtle
//! name: fg_1 600          value: fg_3 (dim)
//! ```
//!
//! The two-tone styling (name dark + value dim) is the "multi-colour"
//! the handoff calls out — one chip carries two colours.

use eframe::egui::{self, FontFamily, FontId, RichText, Stroke, Vec2};
use egui_flex::{Flex, FlexAlign, FlexItem};

use crate::ui::theme;

use super::types::truncate_mid;

const CHIP_FONT: f32 = 10.5;

fn chip_font() -> FontId {
    FontId::new(CHIP_FONT, FontFamily::Name(theme::FAMILY_MONO.into()))
}

/// Render up to six cookies as chips; collapse the remainder to
/// "+N more". `cookies` is an already-parsed `(name, value)` list — see
/// `dialogs::properties::parse_cookies`.
pub fn cookie_chip_strip(ui: &mut egui::Ui, t: &theme::Tokens, cookies: &[(String, String)]) {
    // Drop the default 18px interact-size floor so the lead label
    // measures by its own font height rather than a fixed row metric —
    // otherwise the label would sit above the chips' vertical centre.
    ui.spacing_mut().interact_size.y = 0.0;
    let lead = if cookies.is_empty() {
        "No cookies parsed yet.".to_string()
    } else {
        let n = cookies.len();
        format!("Will send {n} cookie{}:", if n == 1 { "" } else { "s" })
    };
    // Flex with vertical centring + wrap, matching the CSS
    // `.prop-cookies-preview { display:flex; align-items:center;
    // flex-wrap:wrap; gap:6px }`.
    Flex::horizontal()
        .align_items(FlexAlign::Center)
        .wrap(true)
        .gap(Vec2::new(6.0, 4.0))
        .w_full()
        .show(ui, |flex| {
            flex.add_ui(FlexItem::new(), |ui| {
                ui.label(RichText::new(&lead).color(t.fg_3).font(theme::body(11.0)));
            });
            if cookies.is_empty() {
                return;
            }
            for (name, val) in cookies.iter().take(6) {
                let short_val = truncate_mid(val, 7, 7);
                let mut job = egui::text::LayoutJob::default();
                let name_fmt = egui::TextFormat {
                    font_id: chip_font(),
                    color: t.fg_1,
                    ..Default::default()
                };
                let dim_fmt = egui::TextFormat {
                    font_id: chip_font(),
                    color: t.fg_3,
                    ..Default::default()
                };
                job.append(name, 0.0, name_fmt);
                job.append("=", 0.0, dim_fmt.clone());
                job.append(&short_val, 0.0, dim_fmt);
                let hover = format!("{name}={val}");
                flex.add_ui(FlexItem::new(), |ui| {
                    egui::Frame::NONE
                        .fill(t.bg_sunken)
                        .stroke(Stroke::new(t.border_width, t.border_subtle))
                        .corner_radius(999.0)
                        .inner_margin(egui::Margin::symmetric(7, 2))
                        .show(ui, |ui| {
                            ui.label(job).on_hover_text(hover);
                        });
                });
            }
            if cookies.len() > 6 {
                let extra = format!("+{} more", cookies.len() - 6);
                flex.add_ui(FlexItem::new(), |ui| {
                    ui.label(RichText::new(extra).color(t.fg_3).font(chip_font()));
                });
            }
        });
}
