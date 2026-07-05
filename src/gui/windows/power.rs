//! Grace-countdown window (`oxdm gui power`): surfaced by the daemon
//! whenever a destructive power action (shutdown / restart / sleep /
//! hibernate) arms behind the grace timer. Offers an instant **Cancel**
//! and an instant **Confirm now** — the main window no longer shows a
//! countdown banner. Closing the window without choosing dismisses the
//! prompt only; the countdown keeps running daemon-side.

use std::sync::Arc;
use std::time::Duration;

use iced::widget::{column, container, row, text};
use iced::{Alignment, Element, Length, Subscription, Task};

use crate::domain::PowerAction;
use crate::gui::chrome::{self, WindowControl, titlebar};
use crate::gui::icons;
use crate::gui::ipc::DaemonSignal;
use crate::gui::shot::Shot;
use crate::gui::theme::{self, Tokens};
use crate::gui::widget::{Btn, hairline};
use crate::ipc_local::Client;
use crate::ipc_local::protocol::Event;

const WIN_W: f32 = 440.0;
const WIN_H: f32 = 228.0;
/// Icon tile mirrors the queues delete-confirm card (40px, radius 8).
const ICO_TILE: f32 = 40.0;
/// Countdown redraw rate; remaining time is derived from the deadline
/// on every redraw, never accumulated locally.
const TICK: Duration = Duration::from_millis(250);

/// Boot payload: `(client, settings, action, deadline_ms)`.
type BootData = Box<(Arc<Client>, crate::domain::Settings, PowerAction, i64)>;

#[derive(Clone)]
pub enum Msg {
    Connected(Result<BootData, String>),
    Daemon(DaemonSignal),
    Window(WindowControl),
    Tick,
    Cancel,
    ConfirmNow,
    /// Cancel/confirm request finished — either way the prompt is done.
    Requested(Result<(), String>),
    KeyPressed(iced::keyboard::Key),
    ShotTick,
    Shot(iced::window::Screenshot),
    Noop,
}

pub enum App {
    Connecting,
    Failed(String),
    Ready(Box<State>),
}

pub struct State {
    client: Arc<Client>,
    tokens: Tokens,
    action: PowerAction,
    deadline_ms: i64,
    shot: Option<Shot>,
}

fn action_words(action: PowerAction) -> (&'static str, &'static str, &'static str) {
    // (verb for copy, "…now" button label, icon)
    match action {
        PowerAction::ShutDown => ("shut down", "Shut down now", "power"),
        PowerAction::Restart => ("restart", "Restart now", "rotate-cw"),
        PowerAction::Sleep => ("sleep", "Sleep now", "moon"),
        PowerAction::Hibernate => ("hibernate", "Hibernate now", "moon"),
    }
}

pub fn boot() -> (App, Task<Msg>) {
    (
        App::Connecting,
        Task::perform(
            async {
                let client = Client::connect_retry(Duration::from_secs(8))
                    .await
                    .map_err(|e| e.to_string())?;
                client
                    .hello(crate::ipc_local::protocol::GuiKind::Power)
                    .await?;
                let snap = client.snapshot().await?;
                let (action, deadline_ms) = snap
                    .pending_shutdown
                    .ok_or_else(|| "no pending power action".to_owned())?;
                Ok(Box::new((client, snap.settings, action, deadline_ms)))
            },
            Msg::Connected,
        ),
    )
}

pub fn update(app: &mut App, msg: Msg) -> Task<Msg> {
    match msg {
        Msg::Connected(Ok(boxed)) => {
            let (client, settings, action, deadline_ms) = *boxed;
            *app = App::Ready(Box::new(State {
                tokens: Tokens::from_settings(&settings),
                client,
                action,
                deadline_ms,
                shot: Shot::from_env(),
            }));
            Task::none()
        }
        Msg::Connected(Err(_)) => iced::exit(), // raced a cancel; nothing to prompt
        Msg::Window(ctl) => chrome::window_task(ctl),
        msg => {
            let App::Ready(st) = app else {
                return Task::none();
            };
            update_ready(st, msg)
        }
    }
}

fn update_ready(st: &mut State, msg: Msg) -> Task<Msg> {
    match msg {
        Msg::Daemon(DaemonSignal::Lost) => iced::exit(),
        Msg::Daemon(DaemonSignal::Event(ev)) => match ev {
            Event::ShutdownCancelled => iced::exit(),
            Event::ShutdownPending {
                action,
                deadline_ms,
            } => {
                st.action = action;
                st.deadline_ms = deadline_ms;
                Task::none()
            }
            Event::Close => iced::exit(),
            Event::Focus => iced::window::latest().and_then(iced::window::gain_focus),
            _ => Task::none(),
        },
        Msg::Tick => {
            // The daemon fires the action at the deadline; nothing left
            // to prompt for once it passes.
            if chrono::Utc::now().timestamp_millis() >= st.deadline_ms {
                return iced::exit();
            }
            Task::none()
        }
        Msg::Cancel => {
            let client = st.client.clone();
            Task::perform(
                async move { client.cancel_pending_shutdown().await },
                Msg::Requested,
            )
        }
        Msg::ConfirmNow => {
            let client = st.client.clone();
            Task::perform(
                async move { client.confirm_pending_shutdown().await },
                Msg::Requested,
            )
        }
        Msg::Requested(_) => iced::exit(),
        Msg::KeyPressed(key) => {
            // Escape = cancel the pending action (the safe choice);
            // confirming stays click-only to avoid accidental Enter.
            if matches!(
                key.as_ref(),
                iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape)
            ) {
                return update_ready(st, Msg::Cancel);
            }
            Task::none()
        }
        Msg::ShotTick => {
            if let Some(shot) = &mut st.shot
                && let Some(task) = shot.tick()
            {
                return task.map(Msg::Shot);
            }
            Task::none()
        }
        Msg::Shot(s) => match &st.shot {
            Some(shot) => shot.save_and_exit(s),
            None => Task::none(),
        },
        Msg::Connected(_) | Msg::Window(_) | Msg::Noop => Task::none(),
    }
}

pub fn subscription(app: &App) -> Subscription<Msg> {
    let App::Ready(st) = app else {
        return Subscription::none();
    };
    let mut subs = vec![
        iced::event::listen_with(|event, _status, _id| match event {
            iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { key, .. }) => {
                Some(Msg::KeyPressed(key))
            }
            _ => None,
        }),
        crate::gui::ipc::lifecycle_events(crate::ipc_local::protocol::GuiKind::Power)
            .map(Msg::Daemon),
        iced::time::every(TICK).map(|_| Msg::Tick),
    ];
    if st.shot.is_some() {
        subs.push(Shot::frames().map(|_| Msg::ShotTick));
    }
    Subscription::batch(subs)
}

pub fn view(app: &App) -> Element<'_, Msg> {
    chrome::framed(match app {
        App::Connecting => splash("Connecting…".to_owned()),
        App::Failed(e) => splash(e.clone()),
        App::Ready(st) => ready_view(st),
    })
}

fn splash<'a>(msg: String) -> Element<'a, Msg> {
    let t = Tokens::dark();
    container(text(msg).color(t.fg_2))
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(move |_| container::Style {
            background: Some(t.bg_page.into()),
            ..Default::default()
        })
        .into()
}

fn ready_view(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let t2 = *t;
    let (verb, now_label, icon_name) = action_words(st.action);
    // Ceil so the prompt never reads "0s" while still pending.
    let remaining_s =
        ((st.deadline_ms - chrono::Utc::now().timestamp_millis()).max(0) + 999) / 1000;

    let tile = container(icons::icon(icon_name, 20.0, t.status_danger))
        .width(Length::Fixed(ICO_TILE))
        .height(Length::Fixed(ICO_TILE))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(move |_| container::Style {
            background: Some(t2.status_danger_bg.into()),
            border: iced::Border {
                color: t2.status_danger,
                width: 1.0,
                radius: theme::radius::SM.into(),
            },
            ..Default::default()
        });

    let head = row![
        tile,
        column![
            text(format!("System will {verb} in {remaining_s}s"))
                .font(theme::DISPLAY)
                .size(16.0)
                .color(t.fg_1),
            text("All downloads finished. Cancel to keep the system running, or skip the wait.")
                .font(theme::BODY)
                .size(12.0)
                .color(t.fg_3),
        ]
        .spacing(theme::space::S1)
        .width(Length::Fill),
    ]
    .spacing(theme::space::S3)
    .align_y(Alignment::Center);

    let buttons = row![
        iced::widget::Space::new().width(Length::Fill),
        Btn::new("Cancel")
            .secondary()
            .icon("x")
            .on_press(Msg::Cancel)
            .view(t),
        Btn::new(now_label)
            .danger_filled()
            .icon(icon_name)
            .on_press(Msg::ConfirmNow)
            .view(t),
    ]
    .spacing(theme::space::S2)
    .align_y(Alignment::Center);

    let body = container(
        column![
            head,
            iced::widget::Space::new().height(Length::Fill),
            buttons
        ]
        .spacing(theme::space::S3),
    )
    .padding(theme::space::S4)
    .width(Length::Fill)
    .height(Length::Fill);

    let content = container(column![
        titlebar::titlebar(t, "oxdm — Power action pending", false, Msg::Window),
        hairline(t.border_subtle),
        body,
    ])
    .width(Length::Fill)
    .height(Length::Fill)
    .style(move |_| container::Style {
        background: Some(t2.bg_page.into()),
        ..Default::default()
    });
    content.into()
}

pub fn launch_power() {
    let mut app = iced::application(boot, update, view)
        .title(|_: &App| "oxdm — Power action pending".to_owned())
        .theme(|app: &App| match app {
            App::Ready(st) => st.tokens.iced_theme(),
            _ => Tokens::dark().iced_theme(),
        })
        .subscription(subscription)
        .default_font(theme::BODY)
        .antialiasing(true)
        .window(chrome::window_settings(
            iced::Size::new(WIN_W, WIN_H),
            iced::Size::new(WIN_W, WIN_H),
        ));
    for f in theme::fonts::ALL {
        app = app.font(*f);
    }
    if let Err(e) = app.run() {
        eprintln!("gui error: {e}");
        std::process::exit(1);
    }
}
