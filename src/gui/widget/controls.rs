//! Toggle (pill switch), checkbox, combo (pick_list), segmented
//! group, number stepper — styled to the design system.

use iced::widget::{button, canvas, checkbox as iced_checkbox, container, pick_list, row, text};
use iced::{Alignment, Border, Color, Element, Length, Point, Rectangle, Shadow, Size};
use iced_anim::animation_builder;

use crate::gui::color::{clay, mix, with_alpha};
use crate::gui::icons;
use crate::gui::theme::{self, Tokens};
use crate::gui::widget::button::{Btn, BtnSize};

/// Design `.s-toggle`: 36×20 track, 16px thumb inset 2px (1px border +
/// `left: 1px`), so it travels `36 - 2*2 - 16 = 16px`. The `toggler`
/// this replaced was locked to `2 * size` by `size`, i.e. 40×20.
const TRACK_W: f32 = 36.0;
const TRACK_H: f32 = 20.0;
const KNOB_PAD: f32 = 2.0;
/// `box-shadow: 0 1px …` — the thumb's shadow sits one pixel below it.
const SHADOW_OFFSET_Y: f32 = 1.0;

/// Pill switch 36×20, white knob 16, clay track when on. The knob
/// slides and the track cross-fades over `motion::FAST`.
///
/// Hand-rolled rather than `iced::widget::toggler` because `toggler`
/// draws the knob at one of two fixed offsets keyed off `is_toggled`,
/// with no interpolation hook. Drawn on a `canvas` rather than as
/// nested containers so the throw is a *paint* change: a laid-out knob
/// offset would force `AnimationBuilder::animates_layout`, and iced
/// warns ("More than 3 consecutive RedrawRequested events produced
/// layout invalidation") once per frame for the whole throw.
pub fn toggle<'a, M: Clone + 'a>(
    t: &Tokens,
    on: bool,
    enabled: bool,
    on_toggle: impl Fn(bool) -> M + 'a,
) -> Element<'a, M> {
    let t = *t;
    // Disabled mutes by mixing toward the surface, not by going
    // translucent. A half-alpha knob lets the track through it and
    // stops reading as a knob at all — the switch turns into one washed
    // blob. Mixed, every part keeps its own shape and just loses
    // contrast, which is what "disabled" should look like.
    let mute = move |c: Color| {
        if enabled {
            c
        } else {
            mix(t.bg_surface, c, 0.45)
        }
    };
    // Resolved once: the builder closure re-runs per animation frame,
    // and `on_toggle` is only `Fn(bool) -> M`, not `Copy`.
    let press = enabled.then(|| on_toggle(!on));

    animation_builder(if on { 1.0f32 } else { 0.0 }, move |p| {
        // Design `.s-toggle.on`: literal clay-400 fill / clay-500
        // border, like `.btn.primary` — not the themed `action_primary`
        // pair, which lightens to clay-300 under the dark theme.
        let pill = canvas(Switch {
            progress: p,
            track: mute(mix(t.bg_sunken, clay::C400, p)),
            track_border: mute(mix(t.border_default, clay::C500, p)),
            knob: mute(Color::WHITE),
            // The thumb's lift is a cue about pressing it. Nothing to
            // press here.
            shadow: enabled,
        })
        .width(Length::Fixed(TRACK_W))
        .height(Length::Fixed(TRACK_H));

        button(pill)
            .padding(0)
            .on_press_maybe(press.clone())
            .style(|_th, _status| button::Style {
                background: None,
                text_color: Color::TRANSPARENT,
                border: Border::default(),
                shadow: Shadow::default(),
                ..Default::default()
            })
            .into()
    })
    .animation(theme::motion::control())
    .disabled(t.reduce_motion)
    .into()
}

/// The switch face at a given throw `progress` in `[0, 1]`.
struct Switch {
    progress: f32,
    track: Color,
    track_border: Color,
    knob: Color,
    shadow: bool,
}

impl<M> canvas::Program<M> for Switch {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let size = bounds.size();
        let radius = size.height / 2.0;

        frame.fill(
            &canvas::Path::rounded_rectangle(Point::ORIGIN, size, radius.into()),
            self.track,
        );
        // Inset by half the stroke width so the 1px border lands inside
        // the track instead of straddling its edge.
        frame.stroke(
            &canvas::Path::rounded_rectangle(
                Point::new(0.5, 0.5),
                Size::new(size.width - 1.0, size.height - 1.0),
                (radius - 0.5).into(),
            ),
            canvas::Stroke::default()
                .with_color(self.track_border)
                .with_width(1.0),
        );

        let knob = size.height - 2.0 * KNOB_PAD;
        let travel = size.width - size.height;
        let center = Point::new(
            KNOB_PAD + knob / 2.0 + self.progress * travel,
            size.height / 2.0,
        );

        // Design `.s-toggle .thumb`: `box-shadow: 0 1px 2px rgba(0,0,0,.25)`.
        // `canvas::Frame` has no blur, so this is the knob re-filled one
        // pixel lower — the opaque thumb then covers all of it but a 1px
        // crescent along the bottom. The literal 2px blur radius reads as
        // a smudge at this size; the crescent is the part that carries
        // the depth cue.
        if self.shadow {
            frame.fill(
                &canvas::Path::circle(Point::new(center.x, center.y + SHADOW_OFFSET_Y), knob / 2.0),
                with_alpha(Color::BLACK, 0.25),
            );
        }

        frame.fill(&canvas::Path::circle(center, knob / 2.0), self.knob);

        vec![frame.into_geometry()]
    }
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

/// Rows an open dropdown shows before it starts scrolling.
const MENU_MAX_ROWS: usize = 8;

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
    // Nothing to choose from: render the value the way a disabled input
    // reads, rather than a menu that opens onto a single row.
    if options.len() < 2 {
        return locked_combo(
            &t,
            selected
                .or_else(|| options.first().cloned())
                .map(|v| v.to_string()),
            width,
        );
    }
    // Rows in the open menu, at most. Left to itself the menu takes
    // whatever space is above or below the field, which is rarely a
    // whole number of rows — so the list ended on a row sliced through
    // the middle, spilling over the menu's own border and onto the
    // field. A height counted in whole rows cannot end mid-row.
    //
    // Each row is exactly `control::H_MD` tall: the menu inherits the
    // field's padding, and `line height + padding.y()` is how the menu
    // measures one.
    let rows = options.len().min(MENU_MAX_ROWS) as f32;
    let list = pick_list(options, selected, on_select)
        .menu_height(Length::Fixed(rows * theme::control::H_MD))
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
                    // Same idle/active border as `TextInput`: a combo is
                    // a field, and `border_subtle` all but vanished on
                    // the raised fill.
                    color: if matches!(status, pick_list::Status::Opened { .. }) {
                        t.border_brand
                    } else {
                        t.border_default
                    },
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

/// A `combo` with nothing to pick: the field, its value and a dimmed
/// chevron, minus the menu.
/// A combo that cannot be opened: the current value, dimmed, in the
/// field's own shape. Used where the choice is real but not available
/// right now — a setting the running download has already read.
pub fn locked_combo<'a, M: 'a>(t: &Tokens, value: Option<String>, width: Length) -> Element<'a, M> {
    let t = *t;
    let label = value.unwrap_or_default();
    let field = container(
        text(label)
            .font(theme::BODY)
            .size(13.0)
            .color(with_alpha(t.fg_1, 0.5)),
    )
    .width(Length::Fill)
    .height(Length::Fixed(theme::control::H_MD))
    .align_y(Alignment::Center)
    .padding([0.0, theme::control::INPUT_PAD_X])
    .style(move |_| container::Style {
        background: Some(t.bg_raised.into()),
        border: Border {
            color: t.border_default,
            width: t.border_width,
            radius: theme::control::RADIUS.into(),
        },
        ..Default::default()
    });
    let chevron = container(
        container(icons::icon("chevron-down", 14.0, with_alpha(t.fg_3, 0.5))).padding(
            iced::Padding {
                right: theme::control::INPUT_PAD_X,
                ..Default::default()
            },
        ),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .align_x(iced::alignment::Horizontal::Right)
    .align_y(Alignment::Center);
    container(iced::widget::stack![field, chevron])
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

/// A settings row inside a `labeled_section`: title, description, and
/// a switch on the right, in the padding the Properties tabs use.
pub fn toggle_row<'a, M: Clone + 'a>(
    t: &Tokens,
    title: &'a str,
    desc: &'a str,
    on: bool,
    enabled: bool,
    msg: impl Fn(bool) -> M + 'a,
) -> Element<'a, M> {
    container(
        row![
            iced::widget::column![
                text(title)
                    .font(theme::BODY_MEDIUM)
                    .size(12.0)
                    .color(t.fg_1),
                text(desc).font(theme::BODY).size(11.0).color(t.fg_3),
            ]
            .spacing(2.0)
            .width(Length::Fill),
            toggle(t, on, enabled, msg),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center),
    )
    .padding([10.0, theme::space::S3])
    .into()
}

/// `[-] value [+]` stepper, 88px default, mono value.
/// Stepper value: mono 11px in a 12px line box, nudged down a pixel so
/// the digits sit dead centre in the field.
const STEPPER_FONT: f32 = 11.0;
const STEPPER_LINE: f32 = 12.0;
const STEPPER_INK_NUDGE: f32 = 2.0;

/// `selected` marks the stepper as *the* control holding the current
/// choice — true when the value is one no sibling preset pill covers.
/// Without it a custom value leaves every pill unlit and nothing lit in
/// their place, so the row reads as if nothing were chosen.
pub fn number_stepper<'a, M: Clone + 'a>(
    t: &Tokens,
    value: i64,
    min: i64,
    max: i64,
    enabled: bool,
    selected: bool,
    msg: impl Fn(i64) -> M,
) -> Element<'a, M> {
    let t2 = *t;
    let (_, sel_fg, sel_border) = t.pill_selected();
    let seg = |el: Element<'a, M>| el;
    let arrow = |name: &'static str, target: Option<i64>, msg: Option<M>| {
        let enabled_btn = enabled && target.is_some();
        Btn::new("")
            .toolbar()
            .icon_only(name)
            .size(BtnSize::Md)
            .icon_size(14.0)
            .hover_outline()
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
                .size(STEPPER_FONT)
                // Even line box, so centring it in the even-height field
                // lands on whole pixels.
                .line_height(text::LineHeight::Absolute(STEPPER_LINE.into()))
                .color(match (enabled, selected) {
                    (false, _) => t.fg_4,
                    (true, true) => sel_fg,
                    (true, false) => t.fg_1,
                })
        )
        .width(Length::Fixed(32.0))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .height(Length::Fill)
        // The digits' ink sits a row above the line box's centre; two
        // pixels of top padding are halved by the centring into the one
        // that fixes it.
        .padding(iced::Padding {
            top: STEPPER_INK_NUDGE,
            ..iced::Padding::ZERO
        }),
        seg(arrow("plus", (value < max).then_some(value + 1), inc)),
    ]
    .align_y(Alignment::Center);

    container(content)
        .height(Length::Fixed(theme::control::H_MD))
        .style(move |_| container::Style {
            background: Some(t2.bg_raised.into()),
            border: Border {
                color: if selected {
                    sel_border
                } else {
                    t2.border_subtle
                },
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
