//! Window chrome: custom titlebar, edge-resize handles, modal scrim.

pub mod resize;
pub mod titlebar;

use iced::Task;
use iced::window;

/// Window-level actions emitted by the titlebar / resize handles.
/// `Resize` carries the compass direction of the grabbed edge.
#[derive(Debug, Clone, Copy)]
pub enum WindowControl {
    Drag,
    Minimize,
    ToggleMaximize,
    Close,
    Resize(window::Direction),
}

/// Translate a [`WindowControl`] into the corresponding window task.
pub fn window_task<M: Send + 'static>(control: WindowControl) -> Task<M> {
    match control {
        WindowControl::Drag => window::latest().and_then(window::drag),
        WindowControl::Minimize => window::latest().and_then(|id| window::minimize(id, true)),
        WindowControl::ToggleMaximize => window::latest().and_then(window::toggle_maximize),
        WindowControl::Close => window::latest().and_then(window::close),
        WindowControl::Resize(direction) => {
            window::latest().and_then(move |id| window::drag_resize(id, direction))
        }
    }
}

/// Wrap a window's root view in the design's 1px black window ring
/// (`.win` box-shadow `0 0 0 1px rgba(0,0,0,.6)` — borderless windows
/// otherwise blend into whatever is behind them). Padding insets the
/// content by the border width so the child's background cannot paint
/// over the ring.
const BORDER_W: f32 = 1.0;

/// Vertical space the painted chrome takes out of a window before the
/// page starts: the ring's top and bottom border, the titlebar, and the
/// hairline under it. Zero where the OS decorates the window.
///
/// Windows whose height is a measurement of their contents add this to
/// it. The measurements were all taken on OS-decorated windows, so
/// without it the last row of content falls under the footer and the
/// page scrolls by exactly this much.
pub fn overhead_h() -> f32 {
    if titlebar::use_custom() {
        titlebar::chrome_h() + 2.0 * BORDER_W
    } else {
        0.0
    }
}

pub fn framed<'a, M: 'a>(content: impl Into<iced::Element<'a, M>>) -> iced::Element<'a, M> {
    // A decorated window already has an OS frame; the ring would be a
    // black line inside it.
    if !titlebar::use_custom() {
        return content.into();
    }
    iced::widget::container(content)
        .width(iced::Length::Fill)
        .height(iced::Length::Fill)
        .padding(BORDER_W)
        .style(|_| iced::widget::container::Style {
            border: iced::Border {
                color: iced::Color::BLACK,
                width: BORDER_W,
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

/// Snap the window back to its minimum size when a resize event
/// reports a smaller one. Belt-and-braces on top of winit's
/// `min_inner_size`: headless X servers have no WM to enforce hints,
/// and some compositors ignore them for borderless windows.
pub fn enforce_min_size<M: Send + 'static>(size: iced::Size, min: iced::Size) -> Task<M> {
    if size.width >= min.width && size.height >= min.height {
        return Task::none();
    }
    let clamped = iced::Size::new(size.width.max(min.width), size.height.max(min.height));
    window::latest().and_then(move |id| window::resize(id, clamped))
}

/// Default borderless window settings shared by all oxdm windows.
pub fn window_settings(size: iced::Size, min_size: iced::Size) -> window::Settings {
    #[allow(unused_mut)]
    let mut platform_specific = window::settings::PlatformSpecific::default();
    #[cfg(target_os = "linux")]
    {
        platform_specific.application_id = String::from("oxdm");
    }
    window::Settings {
        size,
        min_size: Some(min_size),
        // The one place decorations are chosen; every painted piece of
        // chrome branches on the same predicate so the two can't
        // disagree.
        decorations: !titlebar::use_custom(),
        platform_specific,
        exit_on_close_request: true,
        icon: app_icon(),
        ..Default::default()
    }
}

fn app_icon() -> Option<window::Icon> {
    let (rgba, w, h) = crate::gui::app_icon::window_icon_data()?;
    window::icon::from_rgba(rgba, w, h).ok()
}
