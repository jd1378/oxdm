//! Reusable window chrome.
//!
//! Centralises borderless-window setup so every oxdm window — whether
//! a top-level [`run_native`](eframe::run_native) viewport or an
//! immediate child viewport — gets the same custom titlebar, hides
//! native decorations on Linux/Windows, and exposes invisible resize
//! handles via [`crate::ui::utils::resize`].

use eframe::egui::{self, Vec2, ViewportBuilder};

use crate::ui::components::titlebar;
pub use crate::ui::utils::resize::ChromeStyle;

/// Auto-resize controller for windows that grow/shrink to fit their
/// content. Tracks the last height we requested so a user-initiated
/// drag isn't fought every frame by a snap-back. Optionally locks the
/// height axis entirely (user cannot drag vertically) by pinning
/// `MinInnerSize.y == MaxInnerSize.y == target` whenever the content
/// height changes.
pub struct AutoResize {
    pub max_h: f32,
    pub lock_height: bool,
    /// Preserved as `MinInnerSize.x` when locking height, so the
    /// width-axis min set at viewport build isn't clobbered.
    pub min_w: f32,
    last_target_h: Option<f32>,
}

impl AutoResize {
    pub fn new(max_h: f32, lock_height: bool, min_w: f32) -> Self {
        Self {
            max_h,
            lock_height,
            min_w,
            last_target_h: None,
        }
    }

    /// Apply `target_h` (clamped to `max_h`). Fires `InnerSize` only
    /// when target *changes*, so user drags survive between content
    /// changes. With `lock_height`, also pins min/max inner-size y.
    pub fn apply(&mut self, ctx: &egui::Context, target_h: f32) {
        let target_h = target_h.min(self.max_h);
        let cur = ctx.content_rect();
        let changed = self
            .last_target_h
            .map(|prev| (target_h - prev).abs() > 1.5)
            .unwrap_or(true);
        if changed {
            if (target_h - cur.height()).abs() > 1.5 {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(
                    cur.width(),
                    target_h,
                )));
            }
            if self.lock_height {
                ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(Vec2::new(
                    self.min_w, target_h,
                )));
                ctx.send_viewport_cmd(egui::ViewportCommand::MaxInnerSize(Vec2::new(
                    f32::INFINITY,
                    target_h,
                )));
            }
        }
        self.last_target_h = Some(target_h);
    }
}

/// Build a `ViewportBuilder` with oxdm's standard window flags:
/// borderless on Linux/Windows, native chrome on macOS, app icon
/// applied if available.
pub fn viewport_builder(title: &str, size: (f32, f32), min: Option<(f32, f32)>) -> ViewportBuilder {
    let mut vp = ViewportBuilder::default()
        .with_title(title)
        .with_inner_size([size.0, size.1])
        .with_resizable(true);
    if let Some((mw, mh)) = min {
        vp = vp.with_min_inner_size([mw, mh]);
    }
    if titlebar::use_custom() {
        vp = vp.with_decorations(false).with_transparent(false);
    }
    if let Some(i) = crate::ui::icon::window_icon_data() {
        vp = vp.with_icon(i);
    }
    vp
}

/// Forward to the titlebar's `raw_input_hook` helper. Every shell's
/// `eframe::App::raw_input_hook` should call this so OS-drag-modal-loop
/// swallowed mouse-ups get synthesized into `RawInput` before
/// `begin_pass` — otherwise hover stays stuck after a window drag.
pub fn raw_input_hook(ctx: &egui::Context, raw_input: &mut egui::RawInput) {
    titlebar::drain_drag_release(ctx.viewport_id(), raw_input);
}
