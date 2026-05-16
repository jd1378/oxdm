//! Reusable widgets shared by every window. Built on top of the design
//! tokens in `app::theme`. Widgets here are thin wrappers over `egui`
//! primitives — keep behaviour minimal so callers stay in control of
//! layout and state.

mod button;
mod card;
mod checkbox;
mod clickable;
mod col_header;
mod combo;
pub mod control;
pub mod copy_feedback;
mod eyebrow;
mod field_label;
mod file_input;
mod inline_progress;
pub mod menu;
mod number_stepper;
mod password;
mod pill_count;
mod pill_progress;
mod search_field;
mod segmented;
mod status_dot;
mod striped_progress;
mod tab_button;
mod text_area;
mod text_input;
mod toggle;
mod util;

pub use button::{Btn, BtnSize, BtnVariant};
pub use card::{card, collapsible_card, collapsible_section};
pub use checkbox::Checkbox;
pub use clickable::Clickable;
pub use col_header::{col_header, col_header_aligned, col_header_sortable};
pub use combo::Combo;
pub use eyebrow::eyebrow;
pub use field_label::{field_label, labeled};
pub use file_input::{FileInput, FileInputResponse};
pub use inline_progress::inline_progress;
pub use number_stepper::NumberStepper;
pub use password::PasswordInput;
pub use pill_count::pill_count;
pub use pill_progress::pill_progress;
pub use search_field::search_field;
pub use segmented::{segmented, segmented_sized};
pub use status_dot::status_dot;
pub use striped_progress::striped_progress;
pub use tab_button::TabBtn;
pub use text_area::TextArea;
pub use text_input::{TextInput, text_input};
pub use toggle::Toggle;
