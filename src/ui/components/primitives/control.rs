//! Re-export of shared control sizing constants from the theme module
//! (`app::theme::control`). Lives here so widget code can refer to
//! `control::CONTROL_H_MD` without dragging in the rest of `theme`.

pub use crate::ui::theme::control::{
    H_LG as CONTROL_H_LG, H_MD as CONTROL_H_MD, H_SM as CONTROL_H_SM, INPUT_PAD_X, INPUT_PAD_Y,
    RADIUS as CONTROL_RADIUS,
};
