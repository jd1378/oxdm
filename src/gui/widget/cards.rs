//! Surface containers: card, section card, collapsible card, tab
//! button, banners.

use iced::widget::{column, container, mouse_area, row, text};
use iced::{Alignment, Border, Color, Element, Length};

use crate::gui::color::clay;
use crate::gui::icons;
use crate::gui::theme::{self, Tokens};
use crate::gui::widget::pills;

/// Plain card: bg_surface, border_subtle, control radius, padding.
pub fn card<'a, M: 'a>(t: &Tokens, padding: f32, content: Element<'a, M>) -> Element<'a, M> {
    let t = *t;
    container(content)
        .padding(padding)
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: Some(t.bg_surface.into()),
            border: Border {
                color: t.border_subtle,
                width: 1.0,
                radius: theme::surface::RADIUS.into(),
            },
            // 1px borders blur into a 2px band off the pixel grid.
            snap: true,
            ..Default::default()
        })
        .into()
}

/// Section card with icon + bold title header, then body.
pub fn section_card<'a, M: 'a>(
    t: &Tokens,
    icon: &str,
    title: &str,
    body: Element<'a, M>,
) -> Element<'a, M> {
    let header = row![
        icons::icon(icon, 17.0, t.fg_2),
        text(title.to_owned())
            .font(theme::BODY_BOLD)
            .size(13.0)
            .color(t.fg_1),
    ]
    .spacing(theme::space::S2)
    .align_y(Alignment::Center);
    card(
        t,
        theme::space::S3,
        column![header, body].spacing(theme::space::S3).into(),
    )
}

/// Collapsible card. Open state lives in window state; emits
/// `on_toggle` when the header is clicked.
pub fn collapsible_card<'a, M: Clone + 'a>(
    t: &Tokens,
    title: &str,
    right: Option<Element<'a, M>>,
    open: bool,
    on_toggle: M,
    body: impl FnOnce() -> Element<'a, M>,
) -> Element<'a, M> {
    let t2 = *t;
    let chevron = if open {
        "chevron-down"
    } else {
        "chevron-right"
    };
    let mut header_row = row![
        icons::icon_dyn(chevron, 12.0, t.fg_2, clay::C400),
        text(title.to_owned())
            .font(theme::BODY_BOLD)
            .size(13.0)
            .color(t.fg_1),
    ]
    .spacing(6.0)
    .align_y(Alignment::Center);
    if let Some(right) = right {
        header_row = header_row
            .push(iced::widget::Space::new().width(Length::Fill))
            .push(right);
    }
    let header: Element<'a, M> = mouse_area(
        container(header_row)
            .width(Length::Fill)
            .padding([theme::space::S2 + 2.0, theme::space::S3]),
    )
    .on_press(on_toggle)
    .interaction(iced::mouse::Interaction::Pointer)
    .into();

    let inner: Element<'a, M> = if open {
        column![
            header,
            container(iced::widget::Space::new())
                .width(Length::Fill)
                .height(Length::Fixed(1.0))
                .style(move |_| container::Style {
                    background: Some(t2.border_subtle.into()),
                    ..Default::default()
                }),
            container(body())
                .width(Length::Fill)
                .padding(theme::space::S3),
        ]
        .into()
    } else {
        header
    };

    container(inner)
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: Some(t2.bg_surface.into()),
            border: Border {
                color: t2.border_subtle,
                width: 1.0,
                radius: theme::surface::RADIUS.into(),
            },
            ..Default::default()
        })
        .into()
}

/// Tab button: icon + bold label + optional count pill; 2px clay
/// underline when active.
pub struct TabBtn<'a, M> {
    label: &'a str,
    icon: Option<&'a str>,
    icon_size: f32,
    pad_x: f32,
    height: f32,
    count: Option<u64>,
    active: bool,
    font_size: f32,
    bottom_gap: f32,
    on_press: Option<M>,
}

impl<'a, M: Clone + 'a> TabBtn<'a, M> {
    pub fn new(label: &'a str) -> Self {
        Self {
            label,
            icon: None,
            icon_size: 20.0,
            pad_x: 14.0,
            height: 36.0,
            bottom_gap: 0.0,
            count: None,
            active: false,
            font_size: 12.0,
            on_press: None,
        }
    }
    pub fn icon(mut self, name: &'a str) -> Self {
        self.icon = Some(name);
        self
    }
    pub fn icon_size(mut self, size: f32) -> Self {
        self.icon_size = size;
        self
    }
    pub fn pad_x(mut self, pad: f32) -> Self {
        self.pad_x = pad;
        self
    }
    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }
    /// Extra space between the label (centered in `height`) and the
    /// underline, growing the tab downward.
    pub fn bottom_gap(mut self, gap: f32) -> Self {
        self.bottom_gap = gap;
        self
    }
    pub fn count(mut self, n: u64) -> Self {
        self.count = Some(n);
        self
    }
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }
    pub fn font_size(mut self, size: f32) -> Self {
        self.font_size = size;
        self
    }
    pub fn on_press(mut self, msg: M) -> Self {
        self.on_press = Some(msg);
        self
    }

    pub fn view(self, t: &Tokens) -> Element<'a, M> {
        let fg = if self.active { t.fg_1 } else { t.fg_3 };
        let mut content = row![].spacing(6.0).align_y(Alignment::Center);
        if let Some(icon) = self.icon {
            content = content.push(if self.active {
                icons::icon(icon, self.icon_size, fg)
            } else {
                icons::icon_dyn(icon, self.icon_size, fg, t.fg_1)
            });
        }
        content = content.push(
            text(self.label.to_owned())
                .font(theme::BODY_BOLD)
                .size(self.font_size)
                .color(fg),
        );
        if let Some(n) = self.count {
            let (bg, fg) = if self.active {
                (t.pill_active_bg, t.pill_active_fg)
            } else {
                (t.bg_sunken, t.fg_2)
            };
            content = content.push(container(pills::pill_count(n, fg, bg)).padding(
                iced::Padding {
                    left: 2.0,
                    ..Default::default()
                },
            ));
        }

        let underline_color = if self.active {
            clay::C400
        } else {
            Color::TRANSPARENT
        };
        // stack sizes to the (shrink) content; the Fill-width underline
        // then matches the content width exactly.
        let body = iced::widget::stack![
            container(
                container(content)
                    .height(Length::Fixed(self.height))
                    .align_y(Alignment::Center)
            )
            .height(Length::Fixed(self.height + self.bottom_gap))
            .padding([0.0, self.pad_x])
            .align_y(iced::alignment::Vertical::Top),
            container(
                container(iced::widget::Space::new())
                    .width(Length::Fill)
                    .height(Length::Fixed(2.0))
                    .style(move |_| container::Style {
                        background: Some(underline_color.into()),
                        ..Default::default()
                    })
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .align_y(iced::alignment::Vertical::Bottom),
        ];

        let mut area = mouse_area(body).interaction(iced::mouse::Interaction::Pointer);
        if let Some(msg) = self.on_press {
            area = area.on_press(msg);
        }
        area.into()
    }
}

/// Width of the gutter reserved for the scrollbar rail.
pub const SCROLL_GUTTER: f32 = 12.0;

/// Pad a non-scrolling sibling of a [`vscroll`] on the right by the
/// gutter width so its edge lines up with the scroll content (the
/// outer container's right padding is reduced by the same amount).
pub fn sibling<'a, M: 'a>(el: Element<'a, M>) -> Element<'a, M> {
    container(el)
        .width(Length::Fill)
        .padding(iced::Padding {
            right: SCROLL_GUTTER,
            ..Default::default()
        })
        .into()
}

/// Vertical scrollable with a reserved right gutter so the scrollbar
/// rail never covers content (egui reserved `bar_width` the same way).
/// Rail + thumb follow the design scrollbar spec (10px rail, thin
/// rounded thumb — `theme::scrollbar_style`).
pub fn vscroll<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> iced::widget::Scrollable<'a, M> {
    iced::widget::scrollable(
        container(content)
            .width(Length::Fill)
            .padding(iced::Padding {
                right: SCROLL_GUTTER,
                ..Default::default()
            }),
    )
    .direction(iced::widget::scrollable::Direction::Vertical(
        iced::widget::scrollable::Scrollbar::new()
            .width(theme::size::SCROLLBAR_W)
            .scroller_width(theme::scroll::THUMB_W)
            .margin(0.0),
    ))
    .style(theme::scrollbar_style)
}

/// 1px hairline. `snap` keeps it on the pixel grid — an unsnapped 1px
/// rect that lands on a half-pixel is antialiased into a soft 2px band.
pub fn hairline<'a, M: 'a>(color: Color) -> Element<'a, M> {
    container(iced::widget::Space::new())
        .width(Length::Fill)
        .height(Length::Fixed(1.0))
        .style(move |_| container::Style {
            background: Some(color.into()),
            snap: true,
            ..Default::default()
        })
        .into()
}

/// 1px vertical divider of given height.
pub fn vdivider<'a, M: 'a>(color: Color, height: f32) -> Element<'a, M> {
    container(iced::widget::Space::new())
        .width(Length::Fixed(1.0))
        .height(Length::Fixed(height))
        .style(move |_| container::Style {
            background: Some(color.into()),
            snap: true,
            ..Default::default()
        })
        .into()
}
