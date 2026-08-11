//! The copy button, and the moment of feedback it owes.
//!
//! Copying puts nothing on screen: the clipboard is invisible, so a
//! button that looks identical before and after leaves the user
//! pressing it twice to be sure. Every copy in the app answers the same
//! way — the glyph becomes a check for a moment, and a labelled button
//! says "Copied" while it does.

use crate::gui::widget::Btn;

/// How long the check stays — long enough to read, short enough that it
/// never reads as a mode the button is stuck in.
pub const COPIED_MS: u64 = 1400;

/// A copy button, ready for the caller's own variant and size. An empty
/// `label` renders the icon alone, which is how most of them appear.
///
/// `copied` is this button's own state, so two copy buttons in one
/// window confirm independently.
pub fn copy_btn<'a, M: Clone + 'a>(label: &str, copied: bool, msg: M) -> Btn<'a, M> {
    let icon = if copied { "check" } else { "copy" };
    let b = Btn::new(if label.is_empty() {
        String::new()
    } else if copied {
        "Copied".to_owned()
    } else {
        label.to_owned()
    })
    .on_press(msg);
    if label.is_empty() {
        b.icon_only(icon)
    } else {
        b.icon(icon)
    }
}

/// The wait before a copy button drops its check.
pub async fn expire() {
    tokio::time::sleep(std::time::Duration::from_millis(COPIED_MS)).await;
}
