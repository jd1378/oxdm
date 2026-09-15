//! Headless layout measurement, for tests that assert geometry.
//!
//! The tiny-skia renderer needs no window, display or GPU, so a widget
//! tree can be laid out in a unit test and its nodes measured. That is
//! the only way to assert a layout rule ("the name gives way before the
//! number does") without a human comparing screenshots.

use iced::advanced::layout;
use iced::advanced::widget::Tree;
use iced::{Element, Pixels, Size};

/// The bundled faces, loaded once into the global font system: text has
/// to measure against the fonts the app ships, not whatever the host
/// running the tests happens to have installed.
fn renderer() -> iced::Renderer {
    static FONTS: std::sync::Once = std::sync::Once::new();
    FONTS.call_once(|| {
        let mut fs = iced::advanced::graphics::text::font_system()
            .write()
            .expect("font system");
        for face in crate::gui::theme::fonts::ALL {
            fs.load_font(std::borrow::Cow::Borrowed(*face));
        }
    });
    iced::Renderer::new(crate::gui::theme::BODY, Pixels(13.0))
}

/// Lays `el` out in a `size`-sized space and returns the root node,
/// whose `children()` are the nodes of `el`'s own children.
pub fn measure<M>(el: impl Into<Element<'static, M>>, size: Size) -> layout::Node {
    let mut el = el.into();
    let mut tree = Tree::new(el.as_widget());
    el.as_widget_mut().layout(
        &mut tree,
        &renderer(),
        &layout::Limits::new(Size::ZERO, size),
    )
}
