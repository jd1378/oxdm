//! Queues & scheduling window (`oxdm gui queues`): queue list on the
//! left, editor (name + color, concurrency presets, schedule, on-
//! finish hooks) on the right, Cancel / Save footer, delete-confirm
//! overlay.

use std::sync::Arc;
use std::time::Duration;

use iced::widget::{column, container, mouse_area, row, scrollable, text};
use iced::{Alignment, Element, Length, Subscription, Task};

use crate::domain::{Queue, QueueHook, QueueId, QueueSchedule, ShutdownAction, WeekDayMask};
use crate::gui::chrome::{self, WindowControl, titlebar};
use crate::gui::color;
use crate::gui::ipc::DaemonSignal;
use crate::gui::shot::Shot;
use crate::gui::theme::{self, Tokens};
use crate::gui::widget::{Btn, TextInput, checkbox, hairline, section_card};
use crate::ipc_local::Client;
use crate::ipc_local::protocol::Event;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedKind {
    Manual,
    Recurring,
    OneOff,
    Condition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishKind {
    Nothing,
    Notify,
    Sleep,
    Shutdown,
    Disconnect,
    RunCommand,
}

#[derive(Clone)]
pub enum Msg {
    Connected(Result<Box<(Arc<Client>, Vec<Queue>, crate::domain::Settings)>, String>),
    Queues(Vec<Queue>),
    Daemon(DaemonSignal),
    Window(WindowControl),
    Select(QueueId),
    AddQueue,
    Name(String),
    Concurrency(Option<usize>),
    Sched(SchedKind),
    SchedStart(String),
    SchedDay(u8, bool),
    Finish(FinishKind),
    FinishCommand(String),
    DeleteAsk,
    DeleteConfirm,
    DeleteCancel,
    Save,
    Saved(Result<(), String>),
    Cancel,
    WinResized(f32, f32),
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
    queues: Vec<Queue>,
    selected: Option<QueueId>,

    name: String,
    max_concurrent: Option<usize>,
    sched: SchedKind,
    sched_start: String,
    sched_days: WeekDayMask,
    finish: FinishKind,
    finish_cmd: String,

    confirm_delete: bool,
    shot: Option<Shot>,
}

impl State {
    fn selected_queue(&self) -> Option<&Queue> {
        self.queues.iter().find(|q| Some(q.id) == self.selected)
    }

    fn hydrate(&mut self) {
        let Some(q) = self.selected_queue().cloned() else {
            return;
        };
        self.name = q.name;
        self.max_concurrent = q.max_concurrent;
        self.sched = match q.schedule {
            QueueSchedule::Manual => SchedKind::Manual,
            QueueSchedule::Daily { .. } => SchedKind::Recurring,
            QueueSchedule::Once { .. } => SchedKind::OneOff,
        };
        self.sched_start = match q.schedule {
            QueueSchedule::Daily { start, .. } => start.format("%H:%M").to_string(),
            QueueSchedule::Once { start, .. } => start.format("%Y-%m-%d %H:%M").to_string(),
            _ => String::new(),
        };
        self.sched_days = match q.schedule {
            QueueSchedule::Daily { days, .. } => days,
            _ => WeekDayMask(0x7F),
        };
        self.finish = q
            .on_finish
            .first()
            .map(|h| match h {
                QueueHook::Shutdown(_) => FinishKind::Shutdown,
                QueueHook::Sleep | QueueHook::Hibernate => FinishKind::Sleep,
                QueueHook::ExitOxdm => FinishKind::Nothing,
                QueueHook::RunCommand { .. } => FinishKind::RunCommand,
                QueueHook::Notify { .. } => FinishKind::Notify,
            })
            .unwrap_or(FinishKind::Nothing);
        self.finish_cmd = q
            .on_finish
            .iter()
            .find_map(|h| match h {
                QueueHook::RunCommand { cmd, .. } => Some(cmd.clone()),
                _ => None,
            })
            .unwrap_or_default();
    }

    fn build_queue(&self) -> Option<Queue> {
        let mut q = self.selected_queue()?.clone();
        q.name = self.name.trim().to_owned();
        q.max_concurrent = self.max_concurrent;
        q.schedule = match self.sched {
            SchedKind::Manual | SchedKind::Condition => QueueSchedule::Manual,
            SchedKind::Recurring => QueueSchedule::Daily {
                start: chrono::NaiveTime::parse_from_str(self.sched_start.trim(), "%H:%M")
                    .unwrap_or_else(|_| chrono::NaiveTime::from_hms_opt(9, 0, 0).unwrap()),
                stop: None,
                days: self.sched_days,
            },
            SchedKind::OneOff => QueueSchedule::Once {
                start: chrono::NaiveDateTime::parse_from_str(
                    self.sched_start.trim(),
                    "%Y-%m-%d %H:%M",
                )
                .ok()
                .and_then(|n| n.and_local_timezone(chrono::Local).single())
                .unwrap_or_else(chrono::Local::now),
                stop: None,
            },
        };
        q.on_finish = match self.finish {
            FinishKind::Nothing | FinishKind::Disconnect => vec![],
            FinishKind::Notify => vec![QueueHook::Notify {
                title: "Queue finished".to_owned(),
                body: q.name.clone(),
            }],
            FinishKind::Sleep => vec![QueueHook::Sleep],
            FinishKind::Shutdown => vec![QueueHook::Shutdown(ShutdownAction::ShutDown)],
            FinishKind::RunCommand => vec![QueueHook::RunCommand {
                cmd: self.finish_cmd.trim().to_owned(),
                args: vec![],
            }],
        };
        Some(q)
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
                    .hello(crate::ipc_local::protocol::GuiKind::Queues)
                    .await?;
                let snap = client.snapshot().await?;
                Ok(Box::new((client, snap.queues, snap.settings)))
            },
            Msg::Connected,
        ),
    )
}

pub fn update(app: &mut App, msg: Msg) -> Task<Msg> {
    match msg {
        Msg::Connected(Ok(boxed)) => {
            let (client, queues, settings) = *boxed;
            let mut st = State {
                tokens: Tokens::from_settings(&settings),
                selected: queues.first().map(|q| q.id),
                queues,
                name: String::new(),
                max_concurrent: None,
                sched: SchedKind::Manual,
                sched_start: String::new(),
                sched_days: WeekDayMask(0x7F),
                finish: FinishKind::Nothing,
                finish_cmd: String::new(),
                confirm_delete: false,
                shot: Shot::from_env(),
                client,
            };
            st.hydrate();
            *app = App::Ready(Box::new(st));
            Task::none()
        }
        Msg::Connected(Err(e)) => {
            *app = App::Failed(e);
            Task::none()
        }
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
        Msg::Queues(qs) => {
            st.queues = qs;
            if st.selected_queue().is_none() {
                st.selected = st.queues.first().map(|q| q.id);
                st.hydrate();
            }
            Task::none()
        }
        Msg::Daemon(DaemonSignal::Lost) => iced::exit(),
        Msg::Daemon(DaemonSignal::Event(ev)) => match ev {
            Event::QueuesChanged => {
                let client = st.client.clone();
                Task::perform(async move { client.snapshot().await }, |r| match r {
                    Ok(s) => Msg::Queues(s.queues),
                    Err(_) => Msg::Noop,
                })
            }
            Event::Close => iced::exit(),
            Event::Focus => iced::window::latest().and_then(iced::window::gain_focus),
            _ => Task::none(),
        },
        Msg::Select(id) => {
            st.selected = Some(id);
            st.confirm_delete = false;
            st.hydrate();
            Task::none()
        }
        Msg::AddQueue => {
            let client = st.client.clone();
            let n = st.queues.len();
            Task::perform(
                async move {
                    let mut q = Queue::new_main();
                    q.builtin = false;
                    q.name = format!("Queue {}", n + 1);
                    q.color = Some(crate::domain::random_vivid_color());
                    client.upsert_queue(q).await
                },
                |_| Msg::Noop,
            )
        }
        Msg::Name(v) => {
            st.name = v;
            Task::none()
        }
        Msg::Concurrency(v) => {
            st.max_concurrent = v;
            Task::none()
        }
        Msg::Sched(k) => {
            st.sched = k;
            Task::none()
        }
        Msg::SchedStart(v) => {
            st.sched_start = v;
            Task::none()
        }
        Msg::SchedDay(bit, on) => {
            if on {
                st.sched_days.0 |= 1 << bit;
            } else {
                st.sched_days.0 &= !(1 << bit);
            }
            Task::none()
        }
        Msg::Finish(k) => {
            st.finish = k;
            Task::none()
        }
        Msg::FinishCommand(v) => {
            st.finish_cmd = v;
            Task::none()
        }
        Msg::DeleteAsk => {
            st.confirm_delete = true;
            Task::none()
        }
        Msg::DeleteCancel => {
            st.confirm_delete = false;
            Task::none()
        }
        Msg::DeleteConfirm => {
            st.confirm_delete = false;
            let Some(id) = st.selected else {
                return Task::none();
            };
            let client = st.client.clone();
            Task::perform(async move { client.delete_queue(id).await }, |_| Msg::Noop)
        }
        Msg::Save => {
            let Some(q) = st.build_queue() else {
                return Task::none();
            };
            let client = st.client.clone();
            Task::perform(async move { client.upsert_queue(q).await }, Msg::Saved)
        }
        Msg::Saved(_) => iced::exit(),
        Msg::Cancel => iced::exit(),
        Msg::WinResized(w, h) => {
            chrome::enforce_min_size(iced::Size::new(w, h), iced::Size::new(640.0, 480.0))
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
            iced::Event::Window(iced::window::Event::Resized(size)) => {
                Some(Msg::WinResized(size.width, size.height))
            }
            _ => None,
        }),
        crate::gui::ipc::all_events().map(Msg::Daemon),
    ];
    if st.shot.is_some() {
        subs.push(Shot::frames().map(|_| Msg::ShotTick));
    }
    Subscription::batch(subs)
}

// ---------------------------------------------------------------- view

pub fn view(app: &App) -> Element<'_, Msg> {
    match app {
        App::Connecting => splash("Connecting…".to_owned()),
        App::Failed(e) => splash(e.clone()),
        App::Ready(st) => ready_view(st),
    }
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

fn queue_color(t: &Tokens, q: &Queue) -> iced::Color {
    if let Some([r, g, b]) = q.color {
        return iced::Color::from_rgb8(r, g, b);
    }
    if q.builtin {
        return t.action_primary;
    }
    let palette = [
        t.cat_music,
        t.cat_programs,
        t.cat_pictures,
        t.cat_videos,
        t.cat_documents,
        t.cat_compressed,
        t.status_info,
        t.status_success,
    ];
    let mut h: u32 = 0;
    for b in q.name.bytes() {
        h = h.wrapping_mul(131).wrapping_add(b as u32);
    }
    palette[(h as usize) % palette.len()]
}

fn seg_btn<'a>(
    t: &Tokens,
    label: &'a str,
    icon: Option<&'a str>,
    selected: bool,
    msg: Msg,
) -> Element<'a, Msg> {
    let mut b = Btn::new(label).secondary().selected(selected).on_press(msg);
    if let Some(icon) = icon {
        b = b.icon(icon);
    }
    b.view(t)
}

fn ready_view(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let t2 = *t;

    // Left: queue list.
    let mut list = column![]
        .spacing(theme::space::S2)
        .padding(theme::space::S3);
    for q in &st.queues {
        let active = Some(q.id) == st.selected;
        let count = q.max_concurrent.unwrap_or(0);
        list = list.push(
            mouse_area(
                container(
                    row![
                        crate::gui::widget::dot(8.0, queue_color(t, q)),
                        text(q.name.clone())
                            .font(theme::BODY_MEDIUM)
                            .size(13.0)
                            .color(t.fg_1),
                        iced::widget::Space::new().width(Length::Fill),
                        text(format!("{count}\u{00d7}"))
                            .font(theme::MONO)
                            .size(11.0)
                            .color(t.fg_3),
                    ]
                    .spacing(theme::space::S2)
                    .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .height(Length::Fixed(44.0))
                .align_y(Alignment::Center)
                .padding([0.0, theme::space::S3])
                .style(move |_| container::Style {
                    background: Some(t2.bg_surface.into()),
                    border: iced::Border {
                        color: if active {
                            t2.border_brand
                        } else {
                            t2.border_subtle
                        },
                        width: 1.0,
                        radius: theme::radius::SM.into(),
                    },
                    ..Default::default()
                }),
            )
            .on_press(Msg::Select(q.id))
            .interaction(iced::mouse::Interaction::Pointer),
        );
    }
    list = list.push(
        Btn::new("Add queue")
            .ghost()
            .icon("plus")
            .on_press(Msg::AddQueue)
            .view(t),
    );
    let sidebar = container(scrollable(list).height(Length::Fill))
        .width(Length::Fixed(240.0))
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(t2.bg_sidebar.into()),
            ..Default::default()
        });

    // Right: editor.
    let is_main = st.selected_queue().is_some_and(|q| q.builtin);
    let head = row![
        container(crate::gui::widget::swatch(
            18.0,
            6.0,
            st.selected_queue()
                .map(|q| queue_color(t, q))
                .unwrap_or(t.action_primary)
        ))
        .width(Length::Fixed(32.0))
        .height(Length::Fixed(32.0))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(move |_| container::Style {
            background: Some(t2.bg_raised.into()),
            border: iced::Border {
                color: t2.border_subtle,
                width: 1.0,
                radius: theme::control::RADIUS.into(),
            },
            ..Default::default()
        }),
        TextInput::new(&st.name).on_input(Msg::Name).view(t),
        Btn::new("Delete")
            .danger_filled()
            .icon("trash-2")
            .enabled(!is_main)
            .on_press(Msg::DeleteAsk)
            .view(t),
    ]
    .spacing(theme::space::S2)
    .align_y(Alignment::Center);

    let conc = st.max_concurrent;
    let concurrency = section_card(
        t,
        "layers",
        "Concurrency",
        column![
            text("How many downloads from this queue can run in parallel.")
                .font(theme::BODY)
                .size(12.0)
                .color(t.fg_3),
            row![
                seg_btn(t, "1x", None, conc == Some(1), Msg::Concurrency(Some(1))),
                seg_btn(t, "2x", None, conc == Some(2), Msg::Concurrency(Some(2))),
                seg_btn(t, "3x", None, conc == Some(3), Msg::Concurrency(Some(3))),
                seg_btn(t, "5x", None, conc == Some(5), Msg::Concurrency(Some(5))),
                seg_btn(t, "8x", None, conc == Some(8), Msg::Concurrency(Some(8))),
                Btn::new("Custom")
                    .ghost()
                    .on_press(Msg::Concurrency(None))
                    .view(t),
            ]
            .spacing(4.0)
            .align_y(Alignment::Center),
        ]
        .spacing(theme::space::S3)
        .into(),
    );

    let mut sched_col = column![
        row![
            seg_btn(
                t,
                "Manual",
                Some("calendar"),
                st.sched == SchedKind::Manual,
                Msg::Sched(SchedKind::Manual)
            ),
            seg_btn(
                t,
                "Recurring",
                Some("refresh-cw"),
                st.sched == SchedKind::Recurring,
                Msg::Sched(SchedKind::Recurring)
            ),
            seg_btn(
                t,
                "One-off",
                Some("zap"),
                st.sched == SchedKind::OneOff,
                Msg::Sched(SchedKind::OneOff)
            ),
            seg_btn(
                t,
                "Condition",
                Some("wifi"),
                st.sched == SchedKind::Condition,
                Msg::Sched(SchedKind::Condition)
            ),
        ]
        .spacing(4.0),
    ]
    .spacing(theme::space::S3);
    match st.sched {
        SchedKind::Recurring => {
            let mut days = row![].spacing(theme::space::S2);
            for (bit, label) in [
                (0u8, "Mon"),
                (1, "Tue"),
                (2, "Wed"),
                (3, "Thu"),
                (4, "Fri"),
                (5, "Sat"),
                (6, "Sun"),
            ] {
                let on = st.sched_days.0 & (1 << bit) != 0;
                days = days.push(checkbox(t, label, on, true, move |v| Msg::SchedDay(bit, v)));
            }
            sched_col = sched_col
                .push(
                    row![
                        text("Start time")
                            .font(theme::BODY)
                            .size(13.0)
                            .color(t.fg_2),
                        TextInput::new(&st.sched_start)
                            .hint("09:00")
                            .mono()
                            .width(Length::Fixed(90.0))
                            .on_input(Msg::SchedStart)
                            .view(t),
                    ]
                    .spacing(theme::space::S2)
                    .align_y(Alignment::Center),
                )
                .push(days);
        }
        SchedKind::OneOff => {
            sched_col = sched_col.push(
                row![
                    text("Start at").font(theme::BODY).size(13.0).color(t.fg_2),
                    TextInput::new(&st.sched_start)
                        .hint("2026-06-11 09:00")
                        .mono()
                        .width(Length::Fixed(170.0))
                        .on_input(Msg::SchedStart)
                        .view(t),
                ]
                .spacing(theme::space::S2)
                .align_y(Alignment::Center),
            );
        }
        SchedKind::Condition => {
            sched_col = sched_col.push(
                text("Run while a condition holds (e.g. on Wi-Fi). Coming soon.")
                    .font(theme::BODY)
                    .size(12.0)
                    .color(t.fg_3),
            );
        }
        SchedKind::Manual => {}
    }
    let schedule = section_card(t, "calendar", "Schedule", sched_col.into());

    let mut finish_col = column![
        row![
            seg_btn(
                t,
                "Nothing",
                None,
                st.finish == FinishKind::Nothing,
                Msg::Finish(FinishKind::Nothing)
            ),
            seg_btn(
                t,
                "Notify",
                Some("bell"),
                st.finish == FinishKind::Notify,
                Msg::Finish(FinishKind::Notify)
            ),
            seg_btn(
                t,
                "Sleep",
                Some("clock"),
                st.finish == FinishKind::Sleep,
                Msg::Finish(FinishKind::Sleep)
            ),
            seg_btn(
                t,
                "Shutdown",
                Some("power"),
                st.finish == FinishKind::Shutdown,
                Msg::Finish(FinishKind::Shutdown)
            ),
        ]
        .spacing(4.0),
        row![
            seg_btn(
                t,
                "Disconnect",
                Some("unplug"),
                st.finish == FinishKind::Disconnect,
                Msg::Finish(FinishKind::Disconnect)
            ),
            seg_btn(
                t,
                "Run command",
                Some("terminal"),
                st.finish == FinishKind::RunCommand,
                Msg::Finish(FinishKind::RunCommand)
            ),
        ]
        .spacing(4.0),
    ]
    .spacing(theme::space::S2);
    if st.finish == FinishKind::RunCommand {
        finish_col = finish_col.push(
            TextInput::new(&st.finish_cmd)
                .hint("notify-send 'queue done'")
                .mono()
                .on_input(Msg::FinishCommand)
                .view(t),
        );
    }
    let on_finish = section_card(t, "clock", "When the queue finishes", finish_col.into());

    let editor = scrollable(
        container(column![head, concurrency, schedule, on_finish].spacing(theme::space::S3))
            .padding(theme::space::S4)
            .width(Length::Fill),
    )
    .height(Length::Fill);

    let footer_el = crate::gui::windows::add::footer(
        t,
        Btn::new("Cancel").ghost().on_press(Msg::Cancel).view(t),
        Btn::new("Save")
            .primary()
            .icon("check")
            .on_press(Msg::Save)
            .view(t),
    );

    let body: Element<'_, Msg> = column![
        row![sidebar, editor].height(Length::Fill),
        hairline(t.border_subtle),
        footer_el,
    ]
    .into();

    let overlaid: Element<'_, Msg> = if st.confirm_delete {
        delete_overlay(st, body)
    } else {
        body
    };

    let content = container(column![
        titlebar::titlebar(t, "oxdm — Queues & scheduling", false, Msg::Window),
        hairline(t.border_subtle),
        overlaid,
    ])
    .width(Length::Fill)
    .height(Length::Fill)
    .style(move |_| container::Style {
        background: Some(t2.bg_page.into()),
        text_color: Some(t2.fg_1),
        ..Default::default()
    });
    chrome::resize::resizable(t, content.into(), true, Msg::Window)
}

fn delete_overlay<'a>(st: &'a State, base: Element<'a, Msg>) -> Element<'a, Msg> {
    let t = &st.tokens;
    let t2 = *t;
    let name = st
        .selected_queue()
        .map(|q| q.name.clone())
        .unwrap_or_default();
    let n_jobs = st.selected_queue().map(|q| q.job_ids.len()).unwrap_or(0);
    let card = container(
        column![
            text(format!("Delete queue \"{name}\"?"))
                .font(theme::BODY_BOLD)
                .size(14.0)
                .color(t.fg_1),
            text(format!(
                "{n_jobs} job(s) will become queueless. Files on disk are not touched."
            ))
            .font(theme::BODY)
            .size(12.0)
            .color(t.fg_2),
            row![
                iced::widget::Space::new().width(Length::Fill),
                Btn::new("Cancel")
                    .ghost()
                    .on_press(Msg::DeleteCancel)
                    .view(t),
                Btn::new("Delete")
                    .danger_filled()
                    .icon("trash-2")
                    .on_press(Msg::DeleteConfirm)
                    .view(t),
            ]
            .spacing(theme::space::S2)
            .align_y(Alignment::Center),
        ]
        .spacing(theme::space::S3),
    )
    .width(Length::Fixed(380.0))
    .padding(theme::space::S4)
    .style(move |_| container::Style {
        background: Some(t2.bg_surface.into()),
        border: iced::Border {
            color: t2.border_default,
            width: 1.0,
            radius: theme::surface::RADIUS.into(),
        },
        shadow: iced::Shadow {
            color: color::with_alpha(iced::Color::BLACK, 80.0 / 255.0),
            offset: iced::Vector::new(0.0, 4.0),
            blur_radius: 16.0,
        },
        ..Default::default()
    });

    let scrim = iced::widget::opaque(
        mouse_area(
            container(iced::widget::Space::new())
                .width(Length::Fill)
                .height(Length::Fill)
                .style(|_| container::Style {
                    background: Some(color::with_alpha(iced::Color::BLACK, 120.0 / 255.0).into()),
                    ..Default::default()
                }),
        )
        .on_press(Msg::DeleteCancel),
    );

    iced::widget::stack![
        base,
        scrim,
        container(iced::widget::opaque(card))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center),
    ]
    .into()
}

pub fn launch_queues() {
    let mut app = iced::application(boot, update, view)
        .title(|_: &App| "oxdm — Queues & scheduling".to_owned())
        .theme(|app: &App| match app {
            App::Ready(st) => st.tokens.iced_theme(),
            _ => Tokens::dark().iced_theme(),
        })
        .subscription(subscription)
        .default_font(theme::BODY)
        .antialiasing(true)
        .window(chrome::window_settings(
            iced::Size::new(820.0, 620.0),
            iced::Size::new(640.0, 480.0),
        ));
    for f in theme::fonts::ALL {
        app = app.font(*f);
    }
    if let Err(e) = app.run() {
        eprintln!("gui error: {e}");
        std::process::exit(1);
    }
}
