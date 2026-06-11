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
        decorations: cfg!(target_os = "macos"),
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
