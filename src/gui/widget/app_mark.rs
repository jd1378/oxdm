//! The app's own mark, wherever oxdm has to point at itself.
//!
//! The design ships this as an SVG (`assets/logo-mark.svg`): a clay
//! plate with a rounded window and a dot. The app has the same mark as
//! a per-theme PNG that the tray and the window icon already use, so
//! that is what renders here — one artwork for the tray, the titlebar,
//! the About window and the extensions dialog, rather than a second
//! drawing of the same logo that drifts from it.
//!
//! The lettermark is the fallback for a build whose icons failed to
//! decode. It is deliberately plain: a missing logo should look like a
//! placeholder, not like a different logo.

use iced::widget::{container, text};
use iced::{Alignment, Element, Length};

use crate::gui::color;
use crate::gui::theme::{self, Tokens};

/// The mark at `size` px square.
///
/// The PNG draws its own rounded plate, so it sits bare; only the
/// fallback needs a tile to sit on.
pub fn app_mark<'a, M: 'a>(t: &Tokens, size: f32) -> Element<'a, M> {
    let t2 = *t;
    let tile_bg = color::mix(t.bg_surface, t.action_primary, 0.20);
    let handle = crate::gui::app_icon::image_handle(t.theme);
    let has_glyph = handle.is_some();
    let glyph: Element<'a, M> = match handle {
        Some(handle) => iced::widget::image(handle)
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
            .into(),
        None => text("OX")
            .font(theme::DISPLAY)
            // The lettermark reads as a mark rather than as text at
            // roughly a third of the plate.
            .size(size * 0.31)
            .color(t.action_primary)
            .into(),
    };
    container(glyph)
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(move |_| container::Style {
            background: (!has_glyph).then_some(tile_bg.into()),
            border: iced::Border {
                color: if has_glyph {
                    iced::Color::TRANSPARENT
                } else {
                    t2.border_default
                },
                width: if has_glyph { 0.0 } else { 1.0 },
                radius: theme::radius::LG.into(),
            },
            ..Default::default()
        })
        .into()
}
