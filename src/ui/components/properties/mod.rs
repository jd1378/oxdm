//! Reusable composites that make up the Properties dialog. These sit
//! between primitives (Btn, TextInput, …) and the dialog itself; they
//! own visual styling but no app state. Each is fixture-covered in
//! `src/bin/oxdm-fixture.rs`.

mod add_checksum;
mod captured;
mod checksum_row;
mod cookie_chips;
mod header_editor;
mod speed_limit;
mod types;
mod widgets;

pub use add_checksum::{AddChecksumOutcome, AddChecksumState, add_checksum_form};
pub use captured::captured_table;
pub use checksum_row::{
    ChecksumRowAction, checksum_list_header, checksum_row, mismatch_diff, status_pill,
};
pub use cookie_chips::cookie_chip_strip;
pub use header_editor::{HeaderRow, header_editor};
pub use speed_limit::{speed_limit_control, speed_limit_control_with_id};
pub use types::{Algo, Checksum, CsSource, CsStatus, soft_tint, truncate_mid};
pub use widgets::{
    BannerTone, captured_kv, header_with_trailing, info_callout, kv_row, lock_banner,
    lock_banner_checksums, path_row, phase_pill, prop_row, prop_row_stack, row_sep, section_card,
    status_banner, url_row,
};
