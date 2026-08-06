//! Text-entry primitives: text input, search field, password input,
//! file input. All sized to `control::H_MD` (28) with `control::RADIUS`.

use iced::widget::{container, mouse_area, row, text_input};
use iced::{Alignment, Border, Color, Element, Length};

use crate::gui::icons;
use crate::gui::theme::{self, Tokens};
use crate::gui::widget::button::{Btn, BtnSize};

fn base_style(
    t: &Tokens,
    status: text_input::Status,
    border_override: Option<Color>,
) -> text_input::Style {
    let focused = matches!(status, text_input::Status::Focused { .. });
    text_input::Style {
        background: t.bg_raised.into(),
        border: Border {
            color: border_override.unwrap_or(if focused {
                t.border_brand
            } else {
                t.border_default
            }),
            width: t.border_width,
            radius: theme::control::RADIUS.into(),
        },
        icon: t.fg_3,
        placeholder: t.fg_4,
        value: if status == text_input::Status::Disabled {
            t.fg_4
        } else {
            t.fg_1
        },
        selection: t.selection_bg(),
    }
}

pub struct TextInput<'a, M> {
    value: &'a str,
    hint: String,
    width: Length,
    font: iced::Font,
    font_size: f32,
    secure: bool,
    enabled: bool,
    read_only: Option<M>,
    border: Option<Color>,
    on_input: Option<Box<dyn Fn(String) -> M + 'a>>,
    on_submit: Option<M>,
}

impl<'a, M: Clone + 'a> TextInput<'a, M> {
    pub fn new(value: &'a str) -> Self {
        Self {
            value,
            hint: String::new(),
            width: Length::Fill,
            // Design `.input` value text is 500-weight (medium).
            font: theme::BODY_MEDIUM,
            font_size: 13.0,
            secure: false,
            enabled: true,
            read_only: None,
            border: None,
            on_input: None,
            on_submit: None,
        }
    }
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }
    pub fn font(mut self, font: iced::Font, size: f32) -> Self {
        self.font = font;
        self.font_size = size;
        self
    }
    pub fn mono(self) -> Self {
        self.font(theme::MONO, 12.0)
    }
    pub fn secure(mut self, secure: bool) -> Self {
        self.secure = secure;
        self
    }
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
    /// Show a value the user can select and copy but not change.
    ///
    /// Not the same as `enabled(false)`: iced refuses focus to a
    /// disabled input, so its text cannot be selected either, and a
    /// field whose whole purpose is "here is the path, take it" has to
    /// be selectable. The input stays live and swallows every edit by
    /// mapping input to `noop` — the caller's do-nothing message.
    pub fn read_only(mut self, noop: M) -> Self {
        self.read_only = Some(noop);
        self
    }
    /// Override idle border color (e.g. brand border once URL filled).
    pub fn border(mut self, color: Color) -> Self {
        self.border = Some(color);
        self
    }
    pub fn on_input(mut self, f: impl Fn(String) -> M + 'a) -> Self {
        self.on_input = Some(Box::new(f));
        self
    }
    pub fn on_submit(mut self, msg: M) -> Self {
        self.on_submit = Some(msg);
        self
    }

    pub fn view(self, t: &Tokens) -> Element<'a, M> {
        let t = *t;
        let border = self.border;
        let mut input = text_input(&self.hint, self.value)
            .font(self.font)
            .size(self.font_size)
            .secure(self.secure)
            .width(self.width)
            .padding([
                (theme::control::H_MD - self.font_size * 1.3) / 2.0,
                theme::control::INPUT_PAD_X,
            ])
            .style(move |_th, status| base_style(&t, status, border));
        if let Some(noop) = self.read_only {
            input = input.on_input(move |_| noop.clone());
        } else if self.enabled {
            if let Some(f) = self.on_input {
                input = input.on_input(move |s| f(s));
            }
            if let Some(m) = self.on_submit {
                input = input.on_submit(m);
            }
        }
        input.into()
    }
}

/// Search field: 26px sunken pill-ish box, magnifier 13px, no border.
pub fn search_field<'a, M: Clone + 'a>(
    t: &Tokens,
    value: &'a str,
    placeholder: &'a str,
    width: f32,
    on_input: impl Fn(String) -> M + 'a,
) -> Element<'a, M> {
    let t = *t;
    let input = text_input(placeholder, value)
        .font(theme::BODY)
        .size(13.0)
        .padding([0.0, 0.0])
        .width(Length::Fill)
        .style(move |_th, status| text_input::Style {
            background: Color::TRANSPARENT.into(),
            border: Border::default(),
            icon: t.fg_3,
            placeholder: t.fg_4,
            value: if status == text_input::Status::Disabled {
                t.fg_4
            } else {
                t.fg_1
            },
            selection: t.selection_bg(),
        })
        .on_input(on_input);

    container(
        row![icons::icon("search", 13.0, t.fg_3), input]
            .spacing(theme::space::S2)
            .align_y(Alignment::Center),
    )
    .width(Length::Fixed(width.max(60.0)))
    .height(Length::Fixed(theme::control::H_SM))
    .padding([0.0, theme::space::S2])
    .align_y(Alignment::Center)
    .style(move |_| container::Style {
        background: Some(t.bg_sunken.into()),
        border: Border {
            radius: theme::control::RADIUS.into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .into()
}

/// Password input with hold-to-reveal eye. The reveal flag lives in
/// window state; callers wire `on_reveal(true)` on press and
/// `on_reveal(false)` on release.
pub struct PasswordInput<'a, M> {
    value: &'a str,
    hint: String,
    width: Length,
    enabled: bool,
    revealed: bool,
    on_input: Option<Box<dyn Fn(String) -> M + 'a>>,
    on_reveal: Option<Box<dyn Fn(bool) -> M + 'a>>,
}

impl<'a, M: Clone + 'a> PasswordInput<'a, M> {
    pub fn new(value: &'a str) -> Self {
        Self {
            value,
            hint: String::new(),
            width: Length::Fill,
            enabled: true,
            revealed: false,
            on_input: None,
            on_reveal: None,
        }
    }
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
    pub fn revealed(mut self, revealed: bool) -> Self {
        self.revealed = revealed;
        self
    }
    pub fn on_input(mut self, f: impl Fn(String) -> M + 'a) -> Self {
        self.on_input = Some(Box::new(f));
        self
    }
    pub fn on_reveal(mut self, f: impl Fn(bool) -> M + 'a) -> Self {
        self.on_reveal = Some(Box::new(f));
        self
    }

    pub fn view(self, t: &Tokens) -> Element<'a, M> {
        let t = *t;
        let mut input = text_input(&self.hint, self.value)
            .font(theme::BODY_MEDIUM)
            .size(13.0)
            .secure(!self.revealed)
            .width(Length::Fill)
            .padding([
                (theme::control::H_MD - 13.0 * 1.3) / 2.0,
                theme::control::INPUT_PAD_X,
            ])
            .style(move |_th, status| base_style(&t, status, None));
        if self.enabled
            && let Some(f) = self.on_input
        {
            input = input.on_input(move |s| f(s));
        }

        let mut parts = row![input].spacing(0.0).align_y(Alignment::Center);
        if !self.value.is_empty()
            && self.enabled
            && let Some(on_reveal) = self.on_reveal
        {
            let icon_name = if self.revealed { "eye-off" } else { "eye" };
            let eye = mouse_area(
                container(icons::icon_dyn(icon_name, 16.0, t.fg_3, t.fg_2))
                    .width(Length::Fixed(theme::control::H_MD))
                    .height(Length::Fixed(theme::control::H_MD))
                    .align_x(Alignment::Center)
                    .align_y(Alignment::Center),
            )
            .on_press(on_reveal(true))
            .on_release(on_reveal(false))
            .interaction(iced::mouse::Interaction::Pointer);
            parts = parts.push(eye);
        }
        container(parts).width(self.width).into()
    }
}

/// File input: text field + square folder button, 6px gap.
pub struct FileInput<'a, M> {
    value: &'a str,
    hint: String,
    width: Length,
    font: iced::Font,
    font_size: f32,
    icon: &'a str,
    enabled: bool,
    on_input: Option<Box<dyn Fn(String) -> M + 'a>>,
    on_browse: Option<M>,
}

impl<'a, M: Clone + 'a> FileInput<'a, M> {
    pub fn new(value: &'a str) -> Self {
        Self {
            value,
            hint: String::new(),
            width: Length::Fill,
            // Design `.input` value text is 500-weight (medium).
            font: theme::BODY_MEDIUM,
            font_size: 13.0,
            icon: "folder",
            enabled: true,
            on_input: None,
            on_browse: None,
        }
    }
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }
    pub fn mono(mut self) -> Self {
        self.font = theme::MONO;
        self.font_size = 12.0;
        self
    }
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
    pub fn on_input(mut self, f: impl Fn(String) -> M + 'a) -> Self {
        self.on_input = Some(Box::new(f));
        self
    }
    pub fn on_browse(mut self, msg: M) -> Self {
        self.on_browse = Some(msg);
        self
    }

    pub fn view(self, t: &Tokens) -> Element<'a, M> {
        let tt = *t;
        let mut input = text_input(&self.hint, self.value)
            .font(self.font)
            .size(self.font_size)
            .width(Length::Fill)
            .padding([
                (theme::control::H_MD - self.font_size * 1.3) / 2.0,
                theme::control::INPUT_PAD_X,
            ])
            .style(move |_th, status| base_style(&tt, status, None));
        if self.enabled
            && let Some(f) = self.on_input
        {
            input = input.on_input(move |s| f(s));
        }
        let browse = Btn::new("")
            .secondary()
            .icon_only(self.icon)
            .size(BtnSize::Md)
            .enabled(self.enabled)
            .on_press_maybe(self.on_browse)
            .view(t);
        container(row![input, browse].spacing(6.0).align_y(Alignment::Center))
            .width(self.width)
            .into()
    }
}
