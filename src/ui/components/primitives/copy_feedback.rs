//! Transient "copied" feedback for copy buttons: the copy icon swaps to
//! a check for a short window after a successful clipboard write, then
//! reverts. Keyed by a caller-supplied [`egui::Id`] so each button
//! tracks its own state across frames (immediate mode has no per-widget
//! storage of its own).
//!
//! Usage:
//! ```ignore
//! let id = ui.id().with("my-copy");
//! if Btn::new("").toolbar().icon_only(copy_feedback::icon(ui.ctx(), id)).show(ui).clicked() {
//!     copy_feedback::commit(ui.ctx(), id, value);
//! }
//! ```

use std::time::Duration;

use eframe::egui;

use super::{Btn, BtnSize};

/// How long the check icon shows after a copy.
const FEEDBACK_SECS: f64 = 1.2;

/// A toolbar copy button with built-in "copied → check" feedback. Copies
/// `value` and swaps its icon to a check for ~1.2s on click. Returns
/// whether it was clicked this frame. `id` must be stable per logical
/// button (salt with the value's key when several share a parent).
pub fn copy_button(
    ui: &mut egui::Ui,
    id: egui::Id,
    value: impl Into<String>,
    size: BtnSize,
) -> bool {
    let clicked = Btn::new("")
        .toolbar()
        .icon_only(icon(ui.ctx(), id))
        .size(size)
        .show(ui)
        .clicked();
    if clicked {
        commit(ui.ctx(), id, value);
    }
    clicked
}

/// Icon name for a copy button: `check` for ~1.2s after the last
/// [`commit`] on this `id`, otherwise `copy`. Requests a repaint while
/// the check is showing so it reverts on its own.
pub fn icon(ctx: &egui::Context, id: egui::Id) -> &'static str {
    let copied_at: Option<f64> = ctx.data(|d| d.get_temp(id));
    let now = ctx.input(|i| i.time);
    if copied_at.is_some_and(|t| now - t < FEEDBACK_SECS) {
        ctx.request_repaint_after(Duration::from_millis(150));
        "check"
    } else {
        "copy"
    }
}

/// Write `value` to the clipboard and start the check-icon feedback for
/// `id`.
pub fn commit(ctx: &egui::Context, id: egui::Id, value: impl Into<String>) {
    ctx.copy_text(value.into());
    let now = ctx.input(|i| i.time);
    ctx.data_mut(|d| d.insert_temp(id, now));
}
