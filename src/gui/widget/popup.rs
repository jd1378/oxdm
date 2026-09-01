//! Where a popup anchored at a click belongs.

/// Top-left corner for a popup of `size` opened at `anchor`, inside a
/// window area of `win`. Coordinates are the overlay stack's, so the
/// caller subtracts the painted titlebar from the pointer first.
///
/// The popup opens at the pointer. With no room for it there it flips
/// to the other side rather than sliding back over the click, which is
/// what every menu does and what keeps the thing that was clicked
/// visible. Clamping is the last word: a popup taller than the window
/// has to start at the top whichever way it opens.
pub fn anchored(anchor: (f32, f32), size: (f32, f32), win: (f32, f32)) -> iced::Padding {
    let ((cx, cy), (w, h), (ww, wh)) = (anchor, size, win);
    iced::Padding {
        left: if cx + w > ww { cx - w } else { cx }.clamp(0.0, (ww - w).max(0.0)),
        top: if cy + h > wh { cy - h } else { cy }.clamp(0.0, (wh - h).max(0.0)),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_popup_opens_at_the_pointer_when_there_is_room() {
        let p = anchored((100.0, 200.0), (150.0, 220.0), (800.0, 600.0));
        assert_eq!((p.left, p.top), (100.0, 200.0));
    }

    /// Sliding back over the click would cover the row the popup was
    /// opened from; flipping keeps it visible.
    #[test]
    fn a_popup_with_no_room_flips_to_the_other_side() {
        let p = anchored((700.0, 500.0), (150.0, 220.0), (800.0, 600.0));
        assert_eq!((p.left, p.top), (550.0, 280.0));
    }

    #[test]
    fn a_popup_larger_than_the_window_starts_at_its_edge() {
        let p = anchored((10.0, 10.0), (900.0, 700.0), (800.0, 600.0));
        assert_eq!((p.left, p.top), (0.0, 0.0));
    }
}
