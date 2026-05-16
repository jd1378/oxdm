//! UI-side helpers for the Properties composites.
//!
//! The data types (`Algo`, `Checksum`, `CsSource`, `CsStatus`) used to
//! live here but have been promoted to `crate::domain::checksum` so the
//! IPC protocol and on-disk `Job` row can reference them without
//! pulling in any UI types. They are re-exported below for back-compat
//! with composite call sites.

pub use crate::domain::checksum::{Algo, Checksum, CsSource, CsStatus};

/// Truncate a string in the middle with an ellipsis. `s[..left] … s[-right..]`.
pub fn truncate_mid(s: &str, left: usize, right: usize) -> String {
    if s.chars().count() <= left + right + 1 {
        return s.to_string();
    }
    let lhs: String = s.chars().take(left).collect();
    let rhs: String = s
        .chars()
        .rev()
        .take(right)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{lhs}…{rhs}")
}

/// Blend `accent` and `base`. The convention used across the codebase
/// (see also `dialogs::soft_tint`, `table.rs::soft_tint`,
/// `windows::download::soft_tint`) is that small `t` produces a subtle
/// accent over base — e.g. `t=0.14` is `0.86 * accent + 0.14 * base`.
///
/// NOTE: the `dialogs::soft_tint` doc-comment says the opposite of what
/// the implementation does. The convention in this codebase is what's
/// implemented, not what's documented. Properties composites match the
/// other call sites so callers can use the same `t` magnitudes.
pub fn soft_tint(
    accent: eframe::egui::Color32,
    base: eframe::egui::Color32,
    t: f32,
) -> eframe::egui::Color32 {
    let lerp = |a: u8, b: u8| (a as f32 * (1.0 - t) + b as f32 * t) as u8;
    eframe::egui::Color32::from_rgb(
        lerp(base.r(), accent.r()),
        lerp(base.g(), accent.g()),
        lerp(base.b(), accent.b()),
    )
}

// `paint_dashed_rect` lives in `crate::ui::utils::dashed` so the
// Properties composites, the Add-URL dialog, and any future caller all
// share the same SVG-`stroke-dasharray`-style implementation.
