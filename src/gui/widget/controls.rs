//! Toggle (pill switch), checkbox, combo (pick_list), segmented
//! group, number stepper — styled to the design system.

use iced::widget::{checkbox as iced_checkbox, container, pick_list, row, text, toggler};
use iced::{Alignment, Border, Color, Element, Length};

use crate::gui::color::with_alpha;
use crate::gui::icons;
use crate::gui::theme::{self, Tokens};
use crate::gui::widget::button::{Btn, BtnSize};

/// Pill switch 36×20, white knob 16, clay track when on.
pub fn toggle<'a, M: Clone + 'a>(
    t: &Tokens,
    on: bool,
    enabled: bool,
    on_toggle: impl Fn(bool) -> M + 'a,
) -> Element<'a, M> {
    let t = *t;
    let mut tg = toggler(on).size(20.0).style(move |_th, status| {
        let disabled = matches!(status, toggler::Status::Disabled { .. });
        let alpha = if disabled { 0.5 } else { 1.0 };
        let (track, track_border) = if on {
            (t.action_primary, t.action_primary_press)
        } else {
            (t.bg_sunken, t.border_default)
        };
        toggler::Style {
            background: with_alpha(track, alpha).into(),
            background_border_width: 1.0,
            background_border_color: with_alpha(track_border, alpha),
            foreground: with_alpha(Color::WHITE, alpha).into(),
            foreground_border_width: 0.0,
            foreground_border_color: Color::TRANSPARENT,
            text_color: None,
            border_radius: None,
            padding_ratio: 2.0 / 20.0,
        }
    });
    if enabled {
        tg = tg.on_toggle(on_toggle);
    }
    tg.into()
}

/// 18px checkbox, radius 4, clay fill when checked, white tick.
/// Label rendered by iced at body 13 / fg_1.
pub fn checkbox<'a, M: Clone + 'a>(
    t: &Tokens,
    label: impl Into<String>,
    checked: bool,
    enabled: bool,
    on_toggle: impl Fn(bool) -> M + 'a,
) -> Element<'a, M> {
    let t = *t;
    let mut cb = iced_checkbox(checked)
        .label(label.into())
        .size(18.0)
        .spacing(theme::space::S2)
        .text_size(13.0)
        .font(theme::BODY)
        .style(move |_th, status| {
            let (checked, disabled) = match status {
                iced_checkbox::Status::Active { is_checked } => (is_checked, false),
                iced_checkbox::Status::Hovered { is_checked } => (is_checked, false),
                iced_checkbox::Status::Disabled { is_checked } => (is_checked, true),
            };
            let alpha = if disabled { 0.5 } else { 1.0 };
            let (bg, border) = if checked {
                (t.action_primary, t.action_primary_press)
            } else {
                (t.bg_raised, t.border_default)
            };
            iced_checkbox::Style {
                background: with_alpha(bg, alpha).into(),
                icon_color: with_alpha(Color::WHITE, alpha),
                border: Border {
                    color: with_alpha(border, alpha),
                    width: 1.0,
                    radius: 4.0.into(),
                },
                text_color: Some(with_alpha(t.fg_1, alpha)),
            }
        });
    if enabled {
        cb = cb.on_toggle(on_toggle);
    }
    cb.into()
}

/// Dropdown styled like the design's Combo: H_MD, bg_raised,
/// border_subtle, chevron handle.
pub fn combo<'a, M, T>(
    t: &Tokens,
    options: Vec<T>,
    selected: Option<T>,
    on_select: impl Fn(T) -> M + 'a,
    width: impl Into<Length>,
) -> Element<'a, M>
where
    T: ToString + PartialEq + Clone + 'a,
    M: Clone + 'a,
{
    let t = *t;
    let width = width.into();
    let list = pick_list(options, selected, on_select)
        .width(Length::Fill)
        .text_size(13.0)
        .font(theme::BODY)
        // Default Arrow handle is a heavy filled triangle; the design
        // uses a thin 14px Lucide chevron, overlaid below.
        .handle(pick_list::Handle::None)
        // Pad to control::H_MD like TextInput (13px text ~17px line).
        .padding([
            (theme::control::H_MD - 13.0 * 1.3) / 2.0,
            theme::control::INPUT_PAD_X,
        ])
        .style(move |_th, status| {
            let hovered = matches!(
                status,
                pick_list::Status::Hovered | pick_list::Status::Opened { is_hovered: true }
            );
            pick_list::Style {
                text_color: t.fg_1,
                placeholder_color: t.fg_4,
                handle_color: t.fg_3,
                background: if hovered {
                    t.bg_surface_hover.into()
                } else {
                    t.bg_raised.into()
                },
                border: Border {
                    color: t.border_subtle,
                    width: t.border_width,
                    radius: theme::control::RADIUS.into(),
                },
            }
        })
        .menu_style(move |_th| iced::widget::overlay::menu::Style {
            background: t.bg_raised.into(),
            border: Border {
                color: t.border_default,
                width: 1.0,
                radius: theme::radius::SM.into(),
            },
            text_color: t.fg_1,
            selected_text_color: t.action_primary,
            selected_background: t.bg_sunken.into(),
            shadow: iced::Shadow {
                color: with_alpha(Color::BLACK, 80.0 / 255.0),
                offset: iced::Vector::new(0.0, 4.0),
                blur_radius: 16.0,
            },
        });
    let chevron = container(
        container(icons::icon("chevron-down", 14.0, t.fg_3)).padding(iced::Padding {
            right: theme::control::INPUT_PAD_X,
            ..Default::default()
        }),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .align_x(iced::alignment::Horizontal::Right)
    .align_y(Alignment::Center);
    container(iced::widget::stack![
        container(list)
            .height(Length::Fixed(theme::control::H_MD))
            .align_y(Alignment::Center),
        chevron
    ])
    .width(width)
    .into()
}

/// Horizontal button group, 4px gaps; selected option gets the
/// secondary-selected look (sunken bg, brand border, accent text).
pub fn segmented<'a, M: Clone + 'a>(
    t: &Tokens,
    options: &[(&'a str, Option<&'a str>)],
    selected: usize,
    size: BtnSize,
    msg: impl Fn(usize) -> M,
) -> Element<'a, M> {
    let mut r = row![].spacing(4.0).align_y(Alignment::Center);
    for (i, (label, icon)) in options.iter().enumerate() {
        let mut b = Btn::new(*label)
            .secondary()
            .pill()
            .size(size)
            .selected(i == selected)
            .on_press(msg(i));
        if let Some(icon) = icon {
            b = b.icon(icon);
        }
        r = r.push(b.view(t));
    }
    r.into()
}

/// `[-] value [+]` stepper, 88px default, mono value.
pub fn number_stepper<'a, M: Clone + 'a>(
    t: &Tokens,
    value: i64,
    min: i64,
    max: i64,
    enabled: bool,
    msg: impl Fn(i64) -> M,
) -> Element<'a, M> {
    let t2 = *t;
    let seg = |el: Element<'a, M>| el;
    let arrow = |name: &'static str, target: Option<i64>, msg: Option<M>| {
        let enabled_btn = enabled && target.is_some();
        Btn::new("")
            .toolbar()
            .icon_only(name)
            .size(BtnSize::Md)
            .icon_size(14.0)
            .enabled(enabled_btn)
            .on_press_maybe(msg)
            .view(&t2)
    };
    let dec = (value > min).then(|| msg(value - 1));
    let inc = (value < max).then(|| msg(value + 1));
    let content = row![
        seg(arrow("minus", (value > min).then_some(value - 1), dec)),
        container(
            text(value.to_string())
                .font(theme::MONO)
                .size(11.0)
                .color(if enabled { t.fg_1 } else { t.fg_4 })
        )
        .width(Length::Fixed(32.0))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .height(Length::Fill),
        seg(arrow("plus", (value < max).then_some(value + 1), inc)),
    ]
    .align_y(Alignment::Center);

    container(content)
        .height(Length::Fixed(theme::control::H_MD))
        .style(move |_| container::Style {
            background: Some(t2.bg_raised.into()),
            border: Border {
                color: t2.border_subtle,
                width: 1.0,
                radius: theme::control::RADIUS.into(),
            },
            ..Default::default()
        })
        .into()
}

/// Sortable column header: uppercase bold 11, chevron when active.
pub fn col_header_sortable<'a, M: Clone + 'a>(
    t: &Tokens,
    label: &str,
    active: bool,
    desc: bool,
    on_press: M,
) -> Element<'a, M> {
    let color = if active { t.fg_2 } else { t.fg_3 };
    let mut content = row![crate::gui::widget::ellipsized(
        label.to_uppercase(),
        theme::BODY_BOLD,
        11.0,
        color,
    )]
    .spacing(4.0)
    .align_y(Alignment::Center);
    if active {
        content = content.push(icons::icon(
            if desc { "chevron-down" } else { "chevron-up" },
            11.0,
            color,
        ));
    }
    iced::widget::mouse_area(content)
        .on_press(on_press)
        .interaction(iced::mouse::Interaction::Pointer)
        .into()
}

/// Plain column header.
pub fn col_header<'a, M: 'a>(t: &Tokens, label: &str) -> Element<'a, M> {
    text(label.to_uppercase())
        .font(theme::BODY_BOLD)
        .size(11.0)
        .color(t.fg_3)
        .into()
}
