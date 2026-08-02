//! `Btn` — the design system's button. Mirrors the egui
//! `primitives::button::Btn` builder: six variants × three sizes,
//! optional leading icon / icon-only / accent tint / selected state.

use iced::widget::{button, container, row, text};
use iced::{Alignment, Border, Color, Element, Length, Shadow};

use crate::gui::color::{clay, mix};
use crate::gui::icons;
use crate::gui::theme::{self, Tokens};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BtnVariant {
    Primary,
    Secondary,
    Toolbar,
    Ghost,
    Danger,
    DangerFilled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BtnSize {
    Sm,
    Md,
    Lg,
}

impl BtnSize {
    pub fn height(self) -> f32 {
        match self {
            BtnSize::Sm => theme::control::H_SM,
            BtnSize::Md => theme::control::H_MD,
            BtnSize::Lg => theme::control::H_LG,
        }
    }
    fn pad_x(self) -> f32 {
        match self {
            BtnSize::Sm => 10.0,
            BtnSize::Md => 14.0,
            BtnSize::Lg => 18.0,
        }
    }
    fn font_size(self) -> f32 {
        match self {
            BtnSize::Sm => 12.0,
            BtnSize::Md => 13.0,
            BtnSize::Lg => 14.0,
        }
    }
    fn icon_size(self) -> f32 {
        match self {
            BtnSize::Sm => 16.0,
            BtnSize::Md => 17.0,
            BtnSize::Lg => 20.0,
        }
    }
}

fn darken(c: Color, t: f32) -> Color {
    mix(c, Color::BLACK, t)
}

pub struct Btn<'a, M> {
    label: String,
    variant: BtnVariant,
    size: BtnSize,
    icon: Option<&'a str>,
    icon_only: bool,
    enabled: bool,
    selected: bool,
    accent: bool,
    /// Ghost/Toolbar button that escalates to the danger tone (rust
    /// text + rust-50 bg) on hover only — borderless/neutral at idle.
    /// Mirrors design `.tb-btn.danger`. Ignored by other variants.
    danger_hover: bool,
    min_width: Option<f32>,
    fill_width: bool,
    font_size: Option<f32>,
    icon_size: Option<f32>,
    on_press: Option<M>,
}

impl<'a, M: Clone + 'a> Btn<'a, M> {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            variant: BtnVariant::Secondary,
            size: BtnSize::Md,
            icon: None,
            icon_only: false,
            enabled: true,
            selected: false,
            accent: false,
            danger_hover: false,
            min_width: None,
            fill_width: false,
            font_size: None,
            icon_size: None,
            on_press: None,
        }
    }

    pub fn primary(mut self) -> Self {
        self.variant = BtnVariant::Primary;
        self
    }
    pub fn secondary(mut self) -> Self {
        self.variant = BtnVariant::Secondary;
        self
    }
    pub fn toolbar(mut self) -> Self {
        self.variant = BtnVariant::Toolbar;
        self
    }
    pub fn ghost(mut self) -> Self {
        self.variant = BtnVariant::Ghost;
        self
    }
    pub fn danger(mut self) -> Self {
        self.variant = BtnVariant::Danger;
        self
    }
    pub fn danger_filled(mut self) -> Self {
        self.variant = BtnVariant::DangerFilled;
        self
    }
    pub fn size(mut self, size: BtnSize) -> Self {
        self.size = size;
        self
    }
    pub fn icon(mut self, name: &'a str) -> Self {
        self.icon = Some(name);
        self
    }
    pub fn icon_only(mut self, name: &'a str) -> Self {
        self.icon = Some(name);
        self.icon_only = true;
        self
    }
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
    pub fn accent(mut self, accent: bool) -> Self {
        self.accent = accent;
        self
    }
    /// Borderless ghost/toolbar button that turns rust on hover only
    /// (design `.tb-btn.danger`). Apply on a `.toolbar()`/`.ghost()` button.
    pub fn danger_hover(mut self) -> Self {
        self.danger_hover = true;
        self
    }
    pub fn min_width(mut self, w: f32) -> Self {
        self.min_width = Some(w);
        self
    }
    pub fn fill_width(mut self) -> Self {
        self.fill_width = true;
        self
    }
    pub fn font_size(mut self, size: f32) -> Self {
        self.font_size = Some(size);
        self
    }
    pub fn icon_size(mut self, size: f32) -> Self {
        self.icon_size = Some(size);
        self
    }
    pub fn on_press(mut self, msg: M) -> Self {
        self.on_press = Some(msg);
        self
    }
    pub fn on_press_maybe(mut self, msg: Option<M>) -> Self {
        self.on_press = msg;
        self
    }

    /// Foreground color for a given interaction state. Mirrors the egui
    /// per-variant tables in `findings/iced-port-primitives-spec.md §1`.
    fn fg(&self, t: &Tokens, status: button::Status) -> Color {
        use button::Status::*;
        if !self.enabled || status == Disabled {
            // Filled variants emulate the design's `.btn:disabled
            // { opacity: .5 }`: the label fades by the same 50%
            // page-mix as the fill, keeping their relative contrast —
            // `fg_4` on a still-tinted fill was near-unreadable.
            return match self.variant {
                BtnVariant::Primary => mix(t.action_primary_fg, t.bg_page, 0.5),
                BtnVariant::DangerFilled => mix(Color::WHITE, t.bg_page, 0.5),
                _ => t.fg_4,
            };
        }
        match self.variant {
            BtnVariant::Primary => t.action_primary_fg,
            BtnVariant::Secondary => {
                if self.selected {
                    t.action_primary
                } else {
                    t.fg_1
                }
            }
            BtnVariant::Toolbar | BtnVariant::Ghost => {
                if self.selected {
                    t.action_primary
                } else if self.danger_hover {
                    match status {
                        Hovered | Pressed => t.status_danger,
                        _ => t.fg_2,
                    }
                } else if self.accent {
                    match status {
                        Pressed => darken(t.action_primary, 0.10),
                        _ => t.action_primary,
                    }
                } else {
                    match status {
                        Hovered | Pressed => t.fg_1,
                        _ => t.fg_2,
                    }
                }
            }
            BtnVariant::Danger => t.status_danger,
            BtnVariant::DangerFilled => Color::WHITE,
        }
    }

    fn style(&self, t: &Tokens, status: button::Status) -> button::Style {
        use button::Status::*;
        let disabled = !self.enabled || status == Disabled;
        let (bg, border_color): (Option<Color>, Option<Color>) = match self.variant {
            BtnVariant::Primary => {
                let bg = match status {
                    Hovered => darken(clay::C500, 0.06),
                    Pressed => clay::C600,
                    _ => t.action_primary,
                };
                (Some(bg), Some(clay::C500))
            }
            BtnVariant::Secondary => {
                if self.selected {
                    (Some(t.bg_sunken), Some(t.border_brand))
                } else {
                    let bg = match status {
                        Hovered => mix(t.bg_raised, t.bg_sunken, 0.5),
                        Pressed => t.bg_sunken,
                        _ => t.bg_raised,
                    };
                    (Some(bg), Some(t.border_default))
                }
            }
            BtnVariant::Toolbar => {
                if self.selected {
                    (Some(t.bg_sunken), Some(t.border_brand))
                } else if self.danger_hover {
                    // Borderless idle; rust-50 tint on hover (design `.tb-btn.danger`).
                    let bg = match status {
                        Hovered => Some(t.status_danger_bg),
                        Pressed => Some(mix(t.status_danger_bg, t.bg_page, 0.4)),
                        _ => None,
                    };
                    (bg, None)
                } else {
                    let bg = match status {
                        Hovered => Some(mix(t.bg_page, t.bg_sunken, 0.55)),
                        Pressed => Some(t.bg_sunken),
                        _ => None,
                    };
                    (bg, None)
                }
            }
            BtnVariant::Ghost => {
                if self.selected {
                    (Some(t.bg_sunken), Some(t.border_brand))
                } else {
                    // Design button table: "transparent / fg-2 / hover
                    // sunken + fg-1". Only the text half was here, so a
                    // ghost button read as inert next to a toolbar one.
                    let bg = match status {
                        Hovered => Some(t.bg_sunken),
                        Pressed => Some(darken(t.bg_sunken, 0.06)),
                        _ => None,
                    };
                    (bg, None)
                }
            }
            BtnVariant::Danger => {
                let bg = match status {
                    Hovered => Some(t.status_danger_bg),
                    Pressed => Some(mix(t.status_danger_bg, t.bg_page, 0.4)),
                    _ => None,
                };
                (bg, Some(t.status_danger))
            }
            BtnVariant::DangerFilled => {
                let bg = match status {
                    Hovered => darken(t.status_danger, 0.07),
                    Pressed => darken(t.status_danger, 0.15),
                    _ => t.status_danger,
                };
                (Some(bg), None)
            }
        };
        let (bg, border_color) = if disabled {
            (
                bg.map(|c| mix(c, t.bg_page, 0.5)),
                border_color.map(|c| mix(c, t.bg_page, 0.5)),
            )
        } else {
            (bg, border_color)
        };
        button::Style {
            background: bg.map(Into::into),
            text_color: self.fg(t, status),
            border: Border {
                color: border_color.unwrap_or(Color::TRANSPARENT),
                width: if border_color.is_some() { 1.0 } else { 0.0 },
                radius: theme::control::RADIUS.into(),
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }

    pub fn view(self, t: &Tokens) -> Element<'a, M> {
        let t = *t;
        let height = self.size.height();
        let font_size = self.font_size.unwrap_or(self.size.font_size());
        let icon_size = self.icon_size.unwrap_or(self.size.icon_size());
        // Icon inherits the button's per-status `text_color`, exactly
        // like the label beside it — `Btn::style` already resolves it
        // through `fg(t, status)` for hover / press / disabled. The old
        // `icon_dyn` pairing recolored on the icon's OWN hover bounds,
        // so the glyph and its label disagreed whenever the pointer sat
        // on the label or the button's padding.
        let mut parts = row![].spacing(6.0).align_y(Alignment::Center);
        if let Some(name) = self.icon {
            parts = parts.push(icons::icon_current(name, icon_size));
        }
        if !self.icon_only && !self.label.is_empty() {
            parts = parts.push(
                text(self.label.clone())
                    .font(theme::BODY_BOLD)
                    .size(font_size),
            );
        }

        let pad_x = if self.icon_only {
            ((height - icon_size) * 0.5).max(0.0)
        } else {
            self.size.pad_x()
        };

        let content = container(parts)
            .center_x(Length::Fill)
            .center_y(Length::Fill);

        let variant = self.variant;
        let size = self.size;
        let enabled = self.enabled;
        let selected = self.selected;
        let accent = self.accent;
        let danger_hover = self.danger_hover;
        let style_proto = Btn::<M> {
            label: String::new(),
            variant,
            size,
            icon: None,
            icon_only: false,
            enabled,
            selected,
            accent,
            danger_hover,
            min_width: None,
            fill_width: false,
            font_size: None,
            icon_size: None,
            on_press: None,
        };

        let width = if self.fill_width {
            Length::Fill
        } else if self.icon_only {
            Length::Fixed(height)
        } else {
            // Shrink-wrap; iced sizes the button to content + padding.
            Length::Shrink
        };

        // iced has no min-width; a fixed width is the closest faithful
        // mapping for the design's `min_width` call sites (full-width
        // ghost rows, footer buttons), where content is always narrower.
        let width = match self.min_width {
            Some(w) => Length::Fixed(w),
            None => width,
        };

        let mut btn = button(content)
            .height(Length::Fixed(height))
            .width(width)
            .padding([0.0, pad_x])
            .style(move |_theme, status| style_proto.style(&t, status));
        if self.enabled {
            btn = btn.on_press_maybe(self.on_press);
        }
        btn.into()
    }
}
