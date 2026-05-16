//! Speed-limit control — segmented Unlimited/Limit-to picker with a
//! number input and KB/s · MB/s unit toggle when limited. Mutates a
//! `(value_kbps, unit_mb)` pair owned by the caller.

use eframe::egui::{self};

use crate::ui::components::primitives::{BtnSize, NumberStepper, segmented_sized};
use crate::ui::theme::{self};

/// Render the control. `speed_kbps == 0` means unlimited; the picker
/// auto-seeds to 512 KB/s the first time the user flips to Limit.
pub fn speed_limit_control(
    ui: &mut egui::Ui,
    _t: &theme::Tokens,
    speed_kbps: &mut i64,
    unit_mb: &mut bool,
) {
    speed_limit_control_with_id(ui, _t, speed_kbps, unit_mb, "props-speed-limit");
}

/// Same as [`speed_limit_control`] but with a caller-controlled id seed
/// so that multiple instances on one page (fixtures, demos) don't
/// collide on the persistent egui widget id.
pub fn speed_limit_control_with_id(
    ui: &mut egui::Ui,
    _t: &theme::Tokens,
    speed_kbps: &mut i64,
    unit_mb: &mut bool,
    id_source: &'static str,
) {
    let unlimited_selected = *speed_kbps == 0;
    let picked = segmented_sized(
        ui,
        &[("Unlimited", None), ("Limit to", None)],
        if unlimited_selected { 0 } else { 1 },
        BtnSize::Sm,
    );
    match picked {
        Some(0) => *speed_kbps = 0,
        Some(1) => {
            if *speed_kbps == 0 {
                *speed_kbps = 512;
            }
        }
        _ => {}
    }
    // Always render the value + unit so the row keeps its full width
    // across both states. Disable them while Unlimited is on — the
    // value reads as `—` and the segmented unit picker greys out.
    ui.add_enabled_ui(!unlimited_selected, |ui| {
        let mut display = if *unit_mb {
            (*speed_kbps / 1024).max(1)
        } else {
            (*speed_kbps).max(1)
        };
        let resp = NumberStepper::new(&mut display, id_source)
            .range(1, 10_000)
            .width(80.0)
            .show(ui);
        if resp.changed() && !unlimited_selected {
            *speed_kbps = if *unit_mb { display * 1024 } else { display };
        }
        let unit_pick = segmented_sized(
            ui,
            &[("KB/s", None), ("MB/s", None)],
            if *unit_mb { 1 } else { 0 },
            BtnSize::Sm,
        );
        if !unlimited_selected {
            match unit_pick {
                Some(0) => *unit_mb = false,
                Some(1) => *unit_mb = true,
                _ => {}
            }
        }
    });
}
