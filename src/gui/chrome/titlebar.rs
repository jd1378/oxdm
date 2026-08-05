//! Custom client-side titlebar: drag region, centered title, window
//! controls (minimize / maximize / close), 32px per design
//! (`.win-titlebar`). Linux/Windows only — macOS keeps its native bar
//! and every painted piece of chrome collapses there (see
//! [`use_custom`]).

use iced::widget::{container, mouse_area, row};
use iced::{Alignment, Color, Element, Length};

use crate::gui::chrome::WindowControl;
use crate::gui::icons;
use crate::gui::theme::{self, Tokens};

/// Height of the painted bar itself. Only meaningful where the painted
/// chrome exists at all — see [`use_custom`].
pub const HEIGHT: f32 = theme::size::TITLEBAR_H;

const BTN_SIDE: f32 = 24.0;

/// Whether the custom (painted) titlebar is used on this platform.
/// macOS keeps its native decorations (`chrome::window_settings` sets
/// `decorations` from the same condition), so painting our own bar
/// there would stack a second title bar under the traffic lights.
pub fn use_custom() -> bool {
    !cfg!(target_os = "macos")
}

/// Vertical space the painted chrome occupies at the top of a window:
/// the bar plus its hairline, or nothing when the OS draws its own.
/// Overlay layers sit *below* this, so they convert window-space y with
/// it.
pub fn chrome_h() -> f32 {
    if use_custom() { HEIGHT + 1.0 } else { 0.0 }
}

fn control_button<'a, M: Clone + 'a>(
    t: &Tokens,
    icon: &'a str,
    danger: bool,
    msg: M,
) -> Element<'a, M> {
    let t = *t;
    let idle = if danger { t.status_danger } else { t.fg_2 };
    let hover_fg = if danger { Color::WHITE } else { t.fg_1 };
    iced::widget::button(
        // Tint comes from the button's per-status `text_color` below,
        // so the glyph flips with the whole control, not just when the
        // pointer is on the 14px icon itself.
        container(icons::icon_current(icon, 14.0))
            .center_x(Length::Fill)
            .center_y(Length::Fill),
    )
    .width(Length::Fixed(BTN_SIDE))
    .height(Length::Fixed(BTN_SIDE))
    .padding(0)
    .style(move |_th, status| {
        let bg = match status {
            iced::widget::button::Status::Hovered | iced::widget::button::Status::Pressed => {
                Some(if danger { t.status_danger } else { t.bg_raised })
            }
            _ => None,
        };
        let fg = match status {
            iced::widget::button::Status::Hovered | iced::widget::button::Status::Pressed => {
                hover_fg
            }
            _ => idle,
        };
        iced::widget::button::Style {
            background: bg.map(Into::into),
            text_color: fg,
            border: iced::Border {
                radius: theme::control::RADIUS.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    })
    .on_press(msg)
    .into()
}

/// Render the titlebar and the hairline that separates it from the
/// window body. `on_control` maps window controls into the window's
/// message type. Collapses to nothing where the OS decorates the
/// window itself, so callers need no platform branch of their own.
pub fn titlebar<'a, M: Clone + 'a>(
    t: &Tokens,
    title: &str,
    maximized: bool,
    on_control: impl Fn(WindowControl) -> M + 'a,
) -> Element<'a, M> {
    if !use_custom() {
        return iced::widget::Space::new().height(Length::Fixed(0.0)).into();
    }
    let t2 = *t;

    // Ellipsized, not `text`: a title can be a full URL (a download with
    // no resolved filename yet), which would otherwise wrap out of the
    // fixed-height bar and shove the controls off-screen.
    let title_el: Element<'a, M> =
        crate::gui::widget::ellipsized(title.to_owned(), theme::BODY_BOLD, 13.0, t.fg_2);

    // Drag region: everything except the trailing controls strip.
    let drag_region = mouse_area(
        container(title_el)
            .width(Length::Fill)
            .height(Length::Fixed(HEIGHT))
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            // offset title optically toward true window center: the
            // controls strip (3 × 24 + gaps + pad ≈ 96) is balanced by
            // left padding of the same width.
            .padding(iced::Padding {
                left: 96.0,
                ..Default::default()
            }),
    )
    .on_press(on_control(WindowControl::Drag))
    .on_double_click(on_control(WindowControl::ToggleMaximize));

    let controls = row![
        control_button(t, "minus", false, on_control(WindowControl::Minimize)),
        control_button(
            t,
            if maximized { "copy" } else { "square" },
            false,
            on_control(WindowControl::ToggleMaximize)
        ),
        control_button(t, "x", true, on_control(WindowControl::Close)),
    ]
    .spacing(theme::space::S2)
    .align_y(Alignment::Center);

    let bar = container(
        row![
            drag_region,
            container(controls)
                .height(Length::Fixed(HEIGHT))
                .align_y(Alignment::Center)
                .padding(iced::Padding {
                    right: 8.0,
                    ..Default::default()
                }),
        ]
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .height(Length::Fixed(HEIGHT))
    .style(move |_| container::Style {
        background: Some(t2.bg_titlebar.into()),
        ..Default::default()
    });

    // The hairline belongs to the bar, not to the body: it is what
    // `chrome_h` counts, and it must disappear with the bar.
    iced::widget::column![bar, crate::gui::widget::hairline(t.border_subtle)].into()
}
