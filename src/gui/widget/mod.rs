//! Design-system widget library (port of `src/ui/components/primitives`).

pub mod button;
pub mod cards;
pub mod controls;
pub mod inputs;
pub mod pills;
pub mod striped;

pub use button::{Btn, BtnSize};
pub use cards::{TabBtn, card, collapsible_card, hairline, section_card, vdivider, vscroll};
pub use controls::{
    checkbox, col_header, col_header_sortable, combo, number_stepper, segmented, toggle,
};
pub use inputs::{FileInput, PasswordInput, TextInput, search_field};
pub use pills::{
    dot, eyebrow, field_label, inline_progress, pill_count, pill_progress, status_dot, swatch,
};
pub use striped::{RateChart, rate_chart, striped_progress};
