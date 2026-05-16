//! Custom modal overlay that excludes the custom titlebar from its
//! click-blocking backdrop, so the user can still drag the window
//! around while a dialog is open.
//!
//! `egui::Modal` covers the entire viewport with an input-blocking
//! layer, which intercepts the drag handle in our own
//! `components::titlebar`. This helper draws a backdrop that starts
//! immediately *below* the titlebar instead, leaving the chrome
//! free.

use eframe::egui;

use crate::ui::theme;

const BACKDROP_ALPHA: u8 = 120;

/// Memory key under which the titlebar component stores the y of its
/// rendered bottom edge (including the separator hairline). The modal
/// helper reads this so the backdrop starts exactly at the first
/// content pixel, not at a fixed `TITLEBAR_H` that drifts when the
/// panel's stroke or inner-margin changes.
fn titlebar_bottom_id() -> egui::Id {
    egui::Id::new("oxdm-titlebar-bottom-y")
}

/// Record the absolute y at which the custom titlebar ends. Called
/// from windows that draw a titlebar so [`show`] can position its
/// backdrop precisely below it.
pub fn set_titlebar_bottom(ctx: &egui::Context, y: f32) {
    ctx.data_mut(|d| d.insert_temp(titlebar_bottom_id(), y));
}

fn titlebar_bottom(ctx: &egui::Context) -> f32 {
    let stored: Option<f32> = ctx.data(|d| d.get_temp(titlebar_bottom_id()));
    stored.unwrap_or_else(|| ctx.content_rect().top() + theme::size::TITLEBAR_H)
}

/// Render a modal-style dialog centered on the viewport. Returns
/// whatever `content` returned.
///
/// `id` namespaces the two layers we spawn (backdrop + content); pass
/// a stable string id per dialog. `frame` styles the content card.
pub fn show<R>(
    ctx: &egui::Context,
    id: egui::Id,
    frame: egui::Frame,
    content: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let screen = ctx.content_rect();
    let backdrop_rect =
        egui::Rect::from_min_max(egui::pos2(screen.left(), titlebar_bottom(ctx)), screen.max);

    // Backdrop layer: a single rectangle below the titlebar that
    // intercepts both clicks and drags so the underlying UI is not
    // interactive while the modal is up.
    egui::Area::new(id.with("backdrop"))
        .order(egui::Order::Middle)
        .interactable(true)
        .fixed_pos(backdrop_rect.min)
        .show(ctx, |ui| {
            let (rect, _) =
                ui.allocate_exact_size(backdrop_rect.size(), egui::Sense::click_and_drag());
            ui.painter()
                .rect_filled(rect, 0.0, egui::Color32::from_black_alpha(BACKDROP_ALPHA));
        });

    // Content layer: anchored to the viewport center but constrained
    // to the area below the titlebar via the `pivot_pos` trick — the
    // anchor is computed from the centre of the *backdrop* rect.
    let pivot = backdrop_rect.center();
    let inner_resp = egui::Area::new(id)
        .order(egui::Order::Foreground)
        .interactable(true)
        .fixed_pos(pivot)
        .pivot(egui::Align2::CENTER_CENTER)
        .show(ctx, |ui| frame.show(ui, content).inner);
    inner_resp.inner
}
