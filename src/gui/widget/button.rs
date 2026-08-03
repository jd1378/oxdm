//! `Btn` — the design system's button. Mirrors the egui
//! `primitives::button::Btn` builder: six variants × three sizes,
//! optional leading icon / icon-only / accent tint / selected state.

use iced::widget::{button, container, row, text};
use iced::{Alignment, Border, Color, Element, Length, Shadow};
use iced_anim::widget::button as anim_button;

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
    /// Design `.btn` / `.btn.sm` / `.btn.lg` padding.
    fn pad_x(self) -> f32 {
        match self {
            BtnSize::Sm => 8.0,
            BtnSize::Md => 12.0,
            BtnSize::Lg => 14.0,
        }
    }
    /// Design `.btn`: 600 12px, 11px small, 12.5px large.
    fn font_size(self) -> f32 {
        match self {
            BtnSize::Sm => 11.0,
            BtnSize::Md => 12.0,
            BtnSize::Lg => 12.5,
        }
    }
    /// Design `.btn svg { width: 14px }` — one icon size for every
    /// `.btn`; only the toolbar variant scales its glyph up to 16.
    fn icon_size(self) -> f32 {
        14.0
    }
    /// Design gap between glyph and label (`.btn.sm` tightens it).
    fn gap(self) -> f32 {
        match self {
            BtnSize::Sm => 4.0,
            _ => 6.0,
        }
    }
}

/// `.radio-pill .on` colours — `(bg, fg, border)`. tokens.css remaps
/// clay-50/200/700 to warm dark tints under the dark theme so the
/// active pill doesn't punch a bright hole.
fn pill_on(t: &Tokens) -> (Color, Color, Color) {
    match t.theme {
        theme::ResolvedTheme::Dark => (clay::DARK_C50, clay::DARK_C700, clay::DARK_C200),
        _ => (clay::C50, clay::C700, clay::C200),
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
    /// Render the *selected* state as the design's `.radio-pill .on`
    /// clay tint instead of the generic sunken + brand border. Used by
    /// `segmented`, so a one-of-N row reads the same everywhere.
    pill: bool,
    /// Use the toolbar metric set regardless of variant. See [`Btn::tb`].
    tb_metrics: bool,
    /// Ghost/Toolbar button that escalates to the danger tone (rust
    /// text + rust-50 bg) on hover only — borderless/neutral at idle.
    /// Mirrors design `.tb-btn.danger`. Ignored by other variants.
    danger_hover: bool,
    /// Draw a `border_default` outline while hovered, for borderless
    /// variants that otherwise answer the pointer with a fill alone.
    hover_outline: bool,
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
            pill: false,
            tb_metrics: false,
            danger_hover: false,
            hover_outline: false,
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
    /// Outline the button while the pointer is over it.
    pub fn hover_outline(mut self) -> Self {
        self.hover_outline = true;
        self
    }
    pub fn accent(mut self, accent: bool) -> Self {
        self.accent = accent;
        self
    }
    pub fn pill(mut self) -> Self {
        self.pill = true;
        self
    }
    /// Opt a non-`Toolbar` variant into the toolbar's metrics (design
    /// `.toolbar .tb-btn`: 600 12px label, 16px glyph, 6/10 padding).
    /// The main toolbar's primary CTA needs them so it lines up with
    /// the plain toolbar buttons beside it — styles.css has the
    /// `.tb-btn.primary` rule for exactly this pairing.
    pub fn tb(mut self) -> Self {
        self.tb_metrics = true;
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
                    if self.pill {
                        pill_on(t).1
                    } else {
                        t.action_primary
                    }
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
                // Design `.btn.primary`: clay-400 fill, clay-500 border,
                // clay-500 on hover — literal clay, not the themed
                // `action_primary` (which lightens to clay-300 in dark).
                let bg = match status {
                    Hovered => clay::C500,
                    Pressed => clay::C600,
                    _ => clay::C400,
                };
                (Some(bg), Some(clay::C500))
            }
            BtnVariant::Secondary => {
                if self.selected {
                    if self.pill {
                        let (bg, _, border) = pill_on(t);
                        (Some(bg), Some(border))
                    } else {
                        (Some(t.bg_sunken), Some(t.border_brand))
                    }
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
        // Borderless variants get their outline only under the pointer,
        // so the control reads as pressable without carrying a frame at
        // rest. A variant that already has a border keeps its own.
        let border_color = match (self.hover_outline, border_color, status) {
            (true, None, Hovered | Pressed) => Some(t.border_default),
            (_, other, _) => other,
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
            ..Default::default()
        }
    }

    pub fn view(self, t: &Tokens) -> Element<'a, M> {
        let t = *t;
        let height = self.size.height();
        // Design `.toolbar .tb-btn`: 600 12px label, 16px icon, 6/10
        // padding — its own metrics, not `.btn`'s. Every other variant
        // keeps the size scale.
        let toolbar = self.tb_metrics || self.variant == BtnVariant::Toolbar;
        let font_size =
            self.font_size
                .unwrap_or(if toolbar { 12.0 } else { self.size.font_size() });
        let icon_size =
            self.icon_size
                .unwrap_or(if toolbar { 16.0 } else { self.size.icon_size() });
        // Icon inherits the button's per-status `text_color`, exactly
        // like the label beside it — `Btn::style` already resolves it
        // through `fg(t, status)` for hover / press / disabled. The old
        // `icon_dyn` pairing recolored on the icon's OWN hover bounds,
        // so the glyph and its label disagreed whenever the pointer sat
        // on the label or the button's padding.
        let mut parts = row![].spacing(self.size.gap()).align_y(Alignment::Center);
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
        } else if toolbar {
            10.0
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
        let pill = self.pill;
        let danger_hover = self.danger_hover;
        let hover_outline = self.hover_outline;
        let style_proto = Btn::<M> {
            label: String::new(),
            variant,
            size,
            icon: None,
            icon_only: false,
            enabled,
            selected,
            accent,
            pill,
            tb_metrics: false,
            danger_hover,
            hover_outline,
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

        // The filled variants are the only ones whose idle background is
        // `Some(..)` in every status, which is what `iced_anim` needs to
        // interpolate: it treats `None -> Some(color)` as a variant change
        // and snaps. Ghost/toolbar/danger idle to a transparent (`None`)
        // fill, so animating them would only add a widget indirection for
        // a transition that still pops.
        let animated = matches!(self.variant, BtnVariant::Primary) && !t.reduce_motion;
        if animated {
            let mut btn = anim_button(content)
                .height(Length::Fixed(height))
                .width(width)
                .padding([0.0, pad_x])
                .style(move |_theme, status| style_proto.style(&t, status))
                .animation(theme::motion::control());
            if self.enabled {
                btn = btn.on_press_maybe(self.on_press);
            }
            return btn.into();
        }

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
