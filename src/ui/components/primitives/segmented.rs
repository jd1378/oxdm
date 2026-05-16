//! Horizontal group of buttons with a single active selection.

use eframe::egui::Ui;

use super::button::{Btn, BtnSize};

/// Returns `Some(idx)` when the user picks a new option this frame.
pub fn segmented(
    ui: &mut Ui,
    options: &[(&'static str, Option<&'static str>)],
    selected: usize,
) -> Option<usize> {
    segmented_sized(ui, options, selected, BtnSize::Md)
}

/// Same as [`segmented`] but with a caller-controlled size — use
/// [`BtnSize::Sm`] when the row is tight (Properties Speed limit).
pub fn segmented_sized(
    ui: &mut Ui,
    options: &[(&'static str, Option<&'static str>)],
    selected: usize,
    size: BtnSize,
) -> Option<usize> {
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (i, (label, icon)) in options.iter().enumerate() {
            let mut b = Btn::new(*label).size(size).selected(i == selected);
            if let Some(name) = icon {
                b = b.icon(name);
            }
            if b.show(ui).clicked() {
                picked = Some(i);
            }
        }
    });
    picked
}
