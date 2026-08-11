//! Design-system widget library (port of `src/ui/components/primitives`).

pub mod app_mark;
pub mod button;
pub mod cards;
pub mod controls;
pub mod copy;
mod dashed;
pub mod ellipsis;
pub mod error_panel;
pub mod inputs;
pub mod integrity;
pub mod pills;
pub mod settings;
pub mod striped;
pub mod wash;

pub use app_mark::app_mark;
pub use button::{Btn, BtnSize};
pub use cards::{
    GHOST_ALPHA, SCROLL_GUTTER, TabBtn, card, collapsible_card, drag_ghost, hairline, section_card,
    section_card_count, sibling, surface, vdivider, vscroll,
};
pub use controls::{
    checkbox, col_header, col_header_sortable, combo, locked_combo, number_stepper, segmented,
    toggle,
};
pub use dashed::{dashed_frame, dashed_rule};
pub use ellipsis::ellipsized;
pub use inputs::{FileInput, PasswordInput, TextInput, search_field};
pub use pills::{
    Mark, ProgressTone, TRACKING_EM, chip, dot, eyebrow, field_label, inline_progress, pill_count,
    pill_progress, pulse_dot, status_dot, status_mark, swatch, tracked_caps,
};
pub use settings::{
    SECTION_GAP, set_group, set_note, set_row, set_row_flat, set_row_groups, set_row_panel,
    set_row_stack, set_rows, set_rows_flat, set_section, set_section_danger,
};
pub use striped::{RateChart, rate_chart, striped_progress};
