//! Headless render probe for the iced UI foundation.
//!
//! Renders the design-token gallery (fonts, palette swatches, icons)
//! on the tiny-skia backend, self-screenshots after a few frames, and
//! exits. Run under Xvfb:
//!
//! ```sh
//! xvfb-run -a cargo run --example gui_probe -- /tmp/probe.png [light|dark|warm]
//! ```

use iced::widget::{column, container, row, text};
use iced::window;
use iced::{Alignment, Color, Element, Length, Subscription, Task};
use oxdm::gui::widget::{Btn, BtnSize, TabBtn, TextInput, combo, search_field, segmented, toggle};
use oxdm::gui::{color, icons, theme, widget};

#[derive(Debug, Clone)]
enum Msg {
    Frame,
    Shot(window::Screenshot),
}

struct Probe {
    tokens: theme::Tokens,
    frames: u32,
    out: String,
    page: String,
}

fn boot() -> (Probe, Task<Msg>) {
    let theme_arg = std::env::args().nth(2).unwrap_or_default();
    let tokens = match theme_arg.as_str() {
        "light" => theme::Tokens::light(),
        "warm" => theme::Tokens::warm(),
        _ => theme::Tokens::dark(),
    };
    (
        Probe {
            tokens,
            frames: 0,
            out: std::env::args()
                .nth(1)
                .unwrap_or_else(|| "/tmp/gui_probe.png".into()),
            page: std::env::args().nth(3).unwrap_or_else(|| "tokens".into()),
        },
        Task::none(),
    )
}

fn update(state: &mut Probe, msg: Msg) -> Task<Msg> {
    match msg {
        Msg::Frame => {
            state.frames += 1;
            let shot_at: u32 = std::env::var("PROBE_FRAMES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(20);
            if state.frames == shot_at {
                window::latest().and_then(window::screenshot).map(Msg::Shot)
            } else {
                Task::none()
            }
        }
        Msg::Shot(shot) => {
            let img =
                image::RgbaImage::from_raw(shot.size.width, shot.size.height, shot.rgba.to_vec())
                    .expect("screenshot buffer size");
            img.save(&state.out).expect("save png");
            println!("saved {}", state.out);
            iced::exit()
        }
    }
}

fn swatch<'a>(c: Color, label: &'a str, fg: Color) -> Element<'a, Msg> {
    container(text(label).size(10.0).color(fg))
        .width(Length::Fixed(86.0))
        .height(Length::Fixed(40.0))
        .padding(6)
        .style(move |_| container::Style {
            background: Some(c.into()),
            ..Default::default()
        })
        .into()
}

fn eyebrow_label<'a>(t: &theme::Tokens, s: &str) -> Element<'a, Msg> {
    text(s.to_uppercase())
        .font(theme::BODY_BOLD)
        .size(11.0)
        .color(t.fg_3)
        .into()
}

fn buttons_page(t: &theme::Tokens) -> Element<'static, Msg> {
    let tone_row = |label: &'static str, mk: fn(Btn<'static, Msg>) -> Btn<'static, Msg>| {
        row![
            container(text(label).size(12.0).color(t.fg_2)).width(Length::Fixed(90.0)),
            mk(Btn::new("default")).on_press(Msg::Frame).view(t),
            mk(Btn::new("disabled")).enabled(false).view(t),
            mk(Btn::new("selected"))
                .selected(true)
                .on_press(Msg::Frame)
                .view(t),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center)
    };
    column![
        eyebrow_label(t, "variants × default state"),
        row![
            Btn::new("Primary").primary().on_press(Msg::Frame).view(t),
            Btn::new("Secondary")
                .secondary()
                .on_press(Msg::Frame)
                .view(t),
            Btn::new("Ghost").ghost().on_press(Msg::Frame).view(t),
            Btn::new("Danger").danger().on_press(Msg::Frame).view(t),
            Btn::new("DangerFilled")
                .danger_filled()
                .on_press(Msg::Frame)
                .view(t),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center),
        eyebrow_label(t, "sizes (primary)"),
        row![
            Btn::new("Sm")
                .primary()
                .size(BtnSize::Sm)
                .on_press(Msg::Frame)
                .view(t),
            Btn::new("Md")
                .primary()
                .size(BtnSize::Md)
                .on_press(Msg::Frame)
                .view(t),
            Btn::new("Lg")
                .primary()
                .size(BtnSize::Lg)
                .on_press(Msg::Frame)
                .view(t),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center),
        eyebrow_label(t, "with icon + icon-only"),
        row![
            Btn::new("Add")
                .primary()
                .icon("plus")
                .on_press(Msg::Frame)
                .view(t),
            Btn::new("Pause")
                .secondary()
                .icon("pause")
                .on_press(Msg::Frame)
                .view(t),
            Btn::new("")
                .toolbar()
                .icon_only("x")
                .on_press(Msg::Frame)
                .view(t),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center),
        eyebrow_label(t, "disabled vs selected"),
        row![
            Btn::new("Disabled Primary")
                .primary()
                .enabled(false)
                .view(t),
            Btn::new("Disabled Secondary")
                .secondary()
                .enabled(false)
                .view(t),
            Btn::new("Selected")
                .secondary()
                .selected(true)
                .on_press(Msg::Frame)
                .view(t),
            Btn::new("Selected Ghost")
                .ghost()
                .selected(true)
                .on_press(Msg::Frame)
                .view(t),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center),
        eyebrow_label(t, "tone variants — all six"),
        tone_row("Primary", |b| b.primary()),
        tone_row("Secondary", |b| b.secondary()),
        tone_row("Toolbar", |b| b.toolbar()),
        tone_row("Danger", |b| b.danger()),
        tone_row("DangerFilled", |b| b.danger_filled()),
    ]
    .spacing(theme::space::S3)
    .padding(theme::space::S2)
    .into()
}

fn inputs_page(t: &theme::Tokens) -> Element<'static, Msg> {
    column![
        eyebrow_label(t, "TextInput vs Combo (same width)"),
        row![
            TextInput::new("127.0.0.1:1080")
                .width(Length::Fixed(240.0))
                .view(t),
            combo(
                t,
                vec!["None (direct)".to_owned(), "HTTP".to_owned()],
                Some("None (direct)".to_owned()),
                |_| Msg::Frame,
                Length::Fixed(240.0),
            ),
        ]
        .spacing(theme::space::S3),
        eyebrow_label(t, "stacked — verify identical right edge + height"),
        TextInput::new("127.0.0.1:1080")
            .width(Length::Fixed(240.0))
            .view(t),
        combo(
            t,
            vec!["Compressed".to_owned()],
            Some("Compressed".to_owned()),
            |_| Msg::Frame,
            Length::Fixed(240.0),
        ),
        eyebrow_label(t, "disabled state (cursor + tint)"),
        row![
            TextInput::new("")
                .hint("127.0.0.1:1080")
                .enabled(false)
                .width(Length::Fixed(240.0))
                .view(t),
        ],
        eyebrow_label(t, "search + segmented + toggle + tabs"),
        search_field(t, "", "Search...", 200.0, |_| Msg::Frame),
        segmented(
            t,
            &[
                ("1x", None),
                ("2x", None),
                ("3x", None),
                ("5x", None),
                ("8x", None)
            ],
            0,
            BtnSize::Md,
            |_| Msg::Frame,
        ),
        row![
            toggle(t, true, true, |_| Msg::Frame),
            toggle(t, false, true, |_| Msg::Frame),
            widget::checkbox(t, "Use speed limiter", true, true, |_| Msg::Frame),
            widget::checkbox(t, "Disabled", false, false, |_| Msg::Frame),
        ]
        .spacing(theme::space::S3)
        .align_y(Alignment::Center),
        row![
            TabBtn::new("All")
                .count(3)
                .active(true)
                .on_press(Msg::Frame)
                .view(t),
            TabBtn::new("Active").count(1).on_press(Msg::Frame).view(t),
            TabBtn::new("Finished")
                .count(0)
                .on_press(Msg::Frame)
                .view(t),
        ],
        eyebrow_label(t, "progress"),
        container(widget::rate_chart(
            widget::RateChart {
                samples: vec![0.2, 0.5, 1.4, 2.5, 2.2, 2.5, 1.9],
                max: 2.5,
                avg: 1.6,
                accent: t.action_primary,
                grid: color::with_alpha(t.fg_4, 170.0 / 255.0),
                label_color: t.fg_3,
            },
            124.0,
        ))
        .width(Length::Fixed(400.0)),
        widget::pill_progress(
            0.63,
            Length::Fixed(260.0),
            8.0,
            t.progress_track,
            t.progress_fill
        ),
        widget::inline_progress(
            t,
            0.63,
            "Downloading".into(),
            false,
            widget::ProgressTone::Active,
            Length::Fixed(260.0),
            22.0
        ),
        // Stalled tones: a paused / failed row keeps its bar, muted so
        // it cannot be mistaken for a live transfer.
        widget::inline_progress(
            t,
            0.38,
            "Paused".into(),
            false,
            widget::ProgressTone::Paused,
            Length::Fixed(260.0),
            22.0
        ),
        widget::inline_progress(
            t,
            0.23,
            "Failed".into(),
            false,
            widget::ProgressTone::Failed,
            Length::Fixed(260.0),
            22.0
        ),
        widget::striped_progress(
            0.49,
            Length::Fixed(260.0),
            10.0,
            t.progress_track,
            t.progress_fill,
            Some((color::clay::C400, color::clay::C300)),
            true,
            0.6,
        ),
        row![
            widget::status_dot(t.status_success, "Complete", 12.0),
            widget::pill_count(8, t.pill_active_fg, t.pill_active_bg),
        ]
        .spacing(theme::space::S3)
        .align_y(Alignment::Center),
    ]
    .spacing(theme::space::S3)
    .padding(theme::space::S2)
    .into()
}

fn view(state: &Probe) -> Element<'_, Msg> {
    let t = state.tokens;
    match state.page.as_str() {
        "buttons" => return page_bg(&t, buttons_page(&t)),
        "inputs" => return page_bg(&t, inputs_page(&t)),
        "canvas" => {
            return iced::widget::canvas(DebugCanvas)
                .width(Length::Fill)
                .height(Length::Fill)
                .into();
        }
        "canvas2" => {
            return container(
                iced::widget::canvas(DebugCanvas)
                    .width(Length::Fixed(300.0))
                    .height(Length::Fixed(130.0)),
            )
            .padding(iced::Padding {
                top: 50.0,
                left: 30.0,
                ..Default::default()
            })
            .into();
        }
        _ => {}
    }
    let icon_row = row![
        icons::icon("download", 20.0, t.fg_1),
        icons::icon("play", 20.0, t.action_primary),
        icons::icon("pause", 20.0, t.status_warning),
        icons::icon("check", 20.0, t.status_success),
        icons::icon("x", 20.0, t.status_danger),
        icons::icon("settings", 20.0, t.fg_2),
        icons::icon("folder", 20.0, t.fg_3),
        icons::icon("search", 14.0, t.fg_2),
        icons::icon("plus", 14.0, t.fg_1),
        icons::icon("trash-2", 14.0, t.status_danger),
    ]
    .spacing(12);

    let content = column![
        text("Display Fraunces 22 — No downloads yet")
            .font(theme::DISPLAY)
            .size(22.0)
            .color(t.fg_1),
        text("Body Jakarta Regular 13 — quick brown fox 0123456789")
            .font(theme::BODY)
            .size(13.0)
            .color(t.fg_1),
        text("Body Medium 13 — quick brown fox")
            .font(theme::BODY_MEDIUM)
            .size(13.0)
            .color(t.fg_2),
        text("BODY BOLD 11 EYEBROW")
            .font(theme::BODY_BOLD)
            .size(11.0)
            .color(t.fg_3),
        text("Mono Medium 12 — 100.0 MB · 2.5 MB/s")
            .font(theme::MONO)
            .size(12.0)
            .color(t.fg_1),
        icon_row,
        row![
            swatch(t.bg_page, "bg_page", t.fg_1),
            swatch(t.bg_surface, "bg_surface", t.fg_1),
            swatch(t.bg_sunken, "bg_sunken", t.fg_1),
            swatch(t.bg_raised, "bg_raised", t.fg_1),
            swatch(t.bg_sidebar, "bg_sidebar", t.fg_1),
        ]
        .spacing(4),
        row![
            swatch(t.action_primary, "primary", t.action_primary_fg),
            swatch(t.status_success, "success", color::WHITE),
            swatch(t.status_warning, "warning", color::BLACK),
            swatch(t.status_danger, "danger", color::WHITE),
            swatch(t.pill_active_bg, "pill", t.pill_active_fg),
        ]
        .spacing(4),
    ]
    .spacing(10)
    .padding(20);

    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(t.bg_page.into()),
            text_color: Some(t.fg_1),
            ..Default::default()
        })
        .into()
}

struct DebugCanvas;
impl iced::widget::canvas::Program<Msg> for DebugCanvas {
    type State = ();
    fn draw(
        &self,
        _s: &(),
        renderer: &iced::Renderer,
        _t: &iced::Theme,
        bounds: iced::Rectangle,
        _c: iced::mouse::Cursor,
    ) -> Vec<iced::widget::canvas::Geometry> {
        use iced::widget::canvas;
        let cache = canvas::Cache::new();
        let geom = cache.draw(renderer, bounds.size(), |f| {
            f.fill_rectangle(
                iced::Point::new(0.0, 0.0),
                iced::Size::new(100.0, 20.0),
                Color::from_rgb(1.0, 0.0, 0.0),
            );
            let p =
                canvas::Path::rectangle(iced::Point::new(0.0, 30.0), iced::Size::new(100.0, 20.0));
            f.fill(&p, Color::from_rgb(0.0, 1.0, 0.0));
            let p = canvas::Path::rounded_rectangle(
                iced::Point::new(0.0, 60.0),
                iced::Size::new(100.0, 20.0),
                10.0.into(),
            );
            f.fill(&p, Color::from_rgb(0.2, 0.5, 1.0));
            f.with_clip(
                iced::Rectangle::new(iced::Point::new(0.0, 90.0), iced::Size::new(50.0, 20.0)),
                |f| {
                    let p = canvas::Path::rounded_rectangle(
                        iced::Point::new(0.0, 0.0),
                        iced::Size::new(100.0, 20.0),
                        10.0.into(),
                    );
                    f.fill(&p, Color::from_rgb(1.0, 0.5, 0.0));
                },
            );
        });
        vec![geom]
    }
}

fn page_bg<'a>(t: &theme::Tokens, content: Element<'a, Msg>) -> Element<'a, Msg> {
    let t = *t;
    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(t.bg_page.into()),
            text_color: Some(t.fg_1),
            ..Default::default()
        })
        .into()
}

fn subscription(_state: &Probe) -> Subscription<Msg> {
    window::frames().map(|_| Msg::Frame)
}

fn main() -> iced::Result {
    let mut app = iced::application(boot, update, view)
        .title(|_s: &Probe| "gui-probe".to_owned())
        .theme(|s: &Probe| s.tokens.iced_theme())
        .subscription(subscription)
        .default_font(theme::BODY)
        .window(window::Settings {
            size: iced::Size::new(960.0, 760.0),
            decorations: false,
            ..Default::default()
        });
    for f in theme::fonts::ALL {
        app = app.font(*f);
    }
    app.run()
}
