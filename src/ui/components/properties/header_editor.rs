//! Custom-request-header editor — list of `(name, value)` rows plus
//! a trailing "Add header" button. Mutates the caller's `Vec` in place.

use eframe::egui::{self, RichText};

use crate::ui::components::primitives::{Btn, BtnSize, TextInput};
use crate::ui::theme::{self, ts};

/// Minimal shape of a header row. Matches `dialogs::properties::CustomHeader`
/// but is owned here so composite stays self-contained.
pub trait HeaderRow {
    fn name(&mut self) -> &mut String;
    fn value(&mut self) -> &mut String;
}

/// Render the editor. Returns `true` if the user clicked "Add header".
/// The caller is responsible for pushing a fresh empty row in response.
pub fn header_editor<H: HeaderRow>(
    ui: &mut egui::Ui,
    t: &theme::Tokens,
    rows: &mut Vec<H>,
) -> bool {
    if rows.is_empty() {
        ui.label(
            RichText::new(
                "No custom headers. Click Add header to override or supplement what oxdm sends.",
            )
            .color(t.fg_3)
            .font(theme::body(11.0)),
        );
    }
    let mut to_remove: Option<usize> = None;
    for (i, h) in rows.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            TextInput::new(h.name())
                .width(160.0)
                .hint("Header-Name")
                .font(ts::mono_sm())
                .show(ui);
            ui.label(RichText::new(":").color(t.fg_3));
            TextInput::new(h.value())
                .width(ui.available_width() - 50.0)
                .hint("value")
                .font(ts::mono_sm())
                .show(ui);
            if Btn::new("")
                .toolbar()
                .icon_only("x")
                .size(BtnSize::Sm)
                .show(ui)
                .clicked()
            {
                to_remove = Some(i);
            }
        });
    }
    if let Some(i) = to_remove {
        rows.remove(i);
    }
    Btn::new("Add header")
        .toolbar()
        .accent()
        .icon("plus")
        .size(BtnSize::Sm)
        .show(ui)
        .clicked()
}
