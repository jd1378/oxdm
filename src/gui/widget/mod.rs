//! Design-system widget library (port of `src/ui/components/primitives`).

pub mod button;
pub mod cards;
pub mod controls;
pub mod ellipsis;
pub mod error_panel;
pub mod inputs;
pub mod overflow;
pub mod pills;
pub mod settings;
pub mod striped;

pub use button::{Btn, BtnSize};
pub use cards::{
    SCROLL_GUTTER, TabBtn, card, collapsible_card, hairline, section_card, sibling, surface,
    vdivider, vscroll,
};
pub use controls::{
    checkbox, col_header, col_header_sortable, combo, number_stepper, segmented, toggle,
};
pub use ellipsis::ellipsized;
pub use inputs::{FileInput, PasswordInput, TextInput, search_field};
pub use overflow::overflowing;
pub use pills::{
    ProgressTone, chip, dot, eyebrow, field_label, inline_progress, pill_count, pill_progress,
    status_dot, swatch,
};
pub use settings::{
    SECTION_GAP, set_group, set_note, set_row, set_row_panel, set_row_stack, set_rows, set_section,
    set_section_danger,
};
pub use striped::{RateChart, rate_chart, striped_progress};
