//! Batch-capture triage window (`oxdm gui batch <staged-json-path>`):
//! row per captured link with probe status, queue selector, select
//! all, Start-now toggle, "Add N URLs" footer.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use iced::widget::{column, container, row, text};
use iced::{Alignment, Element, Length, Subscription, Task};

use crate::data::ProbeResult;
use crate::domain::{CaptureRequest, QueueId};
use crate::gui::chrome::{self, WindowControl, titlebar};
use crate::gui::format::format_bytes;
use crate::gui::icons;
use crate::gui::shot::Shot;
use crate::gui::theme::{self, Tokens};
use crate::gui::widget::{Btn, checkbox, combo, hairline, sibling};
use crate::ipc_local::Client;
use crate::ipc_local::protocol::AddJobReq;

#[derive(Clone)]
pub struct Row {
    req: CaptureRequest,
    selected: bool,
    /// The probe's whole answer, not a projection of it: every field
    /// the row shows or forwards to `AddJobReq` comes from here.
    probe: Option<Result<Box<ProbeResult>, String>>,
}

/// Everything the window needs to open, in one named piece. It was a
/// four-element tuple, which is why the rows had to be smuggled past
/// it through a global.
#[derive(Clone)]
pub struct Boot {
    pub client: Arc<Client>,
    pub queues: Vec<(QueueId, String)>,
    pub settings: crate::domain::Settings,
    pub items: Vec<CaptureRequest>,
    pub taken_names: Vec<String>,
}

#[derive(Clone)]
pub enum Msg {
    Connected(Result<Box<Boot>, String>),
    Window(WindowControl),
    Daemon(crate::gui::ipc::DaemonSignal),
    Probed(usize, Result<Box<ProbeResult>, String>),
    SelectAll(bool),
    Select(usize, bool),
    SetQueue(String),
    StartNow(bool),
    Send,
    Sent(Result<(), String>),
    Cancel,
    WinResized(f32, f32),
    ShotTick,
    Shot(iced::window::Screenshot),
    Themed(Box<Tokens>),
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
    rows: Vec<Row>,
    queues: Vec<(QueueId, String)>,
    queue: Option<QueueId>,
    start_now: bool,
    save_dir: PathBuf,
    shot: Option<Shot>,
    /// Name keys the download list already holds, as comparison keys.
    /// A batch has nobody to ask, so the daemon numbers a name rather
    /// than refusing it; the window carries the list so a row can say
    /// that before the send instead of after.
    taken_names: Vec<String>,
    /// A send is in flight, or the last one came back with something
    /// to say. The window stays open on a failure: a batch that closed
    /// on error is a batch that lost downloads silently.
    sending: bool,
    error: Option<String>,
}

fn staged_path() -> Option<PathBuf> {
    std::env::args().nth(3).map(PathBuf::from)
}

pub fn boot() -> (App, Task<Msg>) {
    let Some(path) = staged_path() else {
        return (App::Failed("missing staged path".into()), Task::none());
    };
    (
        App::Connecting,
        Task::perform(
            async move {
                let client = Client::connect_retry(Duration::from_secs(8))
                    .await
                    .map_err(|e| e.to_string())?;
                client
                    .hello(crate::ipc_local::protocol::GuiKind::Batch)
                    .await?;
                let snap = client.snapshot().await?;
                let items =
                    crate::ipc::batch::load_and_consume(&path).map_err(|e| e.to_string())?;
                let taken_names = snap
                    .jobs
                    .iter()
                    .filter_map(|j| j.filename.as_deref())
                    .map(crate::domain::name_key)
                    .collect::<Vec<_>>();
                Ok(Box::new(Boot {
                    client,
                    queues: snap.queues.iter().map(|q| (q.id, q.name.clone())).collect(),
                    settings: snap.settings,
                    items,
                    taken_names,
                }))
            },
            Msg::Connected,
        )
        // Started by the daemon, not by the user's own click, so the
        // window has to ask to be in front. See `focus_on_open`.
        .chain(chrome::focus_on_open()),
    )
}

pub fn update(app: &mut App, msg: Msg) -> Task<Msg> {
    match msg {
        Msg::Connected(Ok(boot)) => {
            let Boot {
                client,
                queues,
                settings,
                items,
                taken_names,
            } = *boot;
            let rows: Vec<Row> = items
                .into_iter()
                .map(|req| Row {
                    req,
                    selected: true,
                    probe: None,
                })
                .collect();
            let mut probes = Vec::new();
            for (i, r) in rows.iter().enumerate() {
                let client = client.clone();
                let url = r.req.url.clone();
                probes.push(Task::perform(
                    async move {
                        match client.probe(url).await {
                            // Batch rows render a flat message; flatten
                            // the structured `JobError` here.
                            Ok(inner) => inner.map(Box::new).map_err(|e| e.to_string()),
                            Err(e) => Err(e),
                        }
                    },
                    move |res| Msg::Probed(i, res),
                ));
            }
            let main_queue = queues.first().map(|(id, _)| *id);
            *app = App::Ready(Box::new(State {
                tokens: Tokens::from_settings(&settings),
                rows,
                queue: main_queue,
                queues,
                start_now: false,
                save_dir: settings.fallback_dir(),
                shot: Shot::from_env(),
                client,
                taken_names,
                sending: false,
                error: None,
            }));
            Task::batch(probes)
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
        Msg::Probed(i, res) => {
            if let Some(r) = st.rows.get_mut(i) {
                r.probe = Some(res);
            }
            Task::none()
        }
        Msg::SelectAll(v) => {
            for r in &mut st.rows {
                r.selected = v;
            }
            Task::none()
        }
        Msg::Select(i, v) => {
            if let Some(r) = st.rows.get_mut(i) {
                r.selected = v;
            }
            Task::none()
        }
        Msg::SetQueue(name) => {
            if let Some((id, _)) = st.queues.iter().find(|(_, n)| *n == name) {
                st.queue = Some(*id);
            }
            Task::none()
        }
        Msg::StartNow(v) => {
            st.start_now = v;
            Task::none()
        }
        Msg::Send => {
            let client = st.client.clone();
            let queue = st.queue;
            let start_now = st.start_now;
            let save_dir = st.save_dir.clone();
            // Everything the probe learned rides along: a queued row
            // already knows how big it is and what it will be checked
            // against, and throwing that away leaves the list and the
            // download window with nothing to show until the transfer
            // starts.
            let reqs: Vec<(CaptureRequest, Option<Box<ProbeResult>>)> = st
                .rows
                .iter()
                .filter(|r| r.selected)
                .map(|r| {
                    let probed = r.probe.as_ref().and_then(|p| p.as_ref().ok());
                    (r.req.clone(), probed.cloned())
                })
                .collect();
            st.sending = true;
            st.error = None;
            Task::perform(
                async move {
                    let mut added = Vec::new();
                    let mut failed: Vec<String> = Vec::new();
                    for (req, probed) in reqs {
                        let add = AddJobReq {
                            url: req.url.clone(),
                            queue,
                            save_dir: save_dir.clone(),
                            filename: req
                                .filename
                                .clone()
                                .or_else(|| probed.as_ref().map(|p| p.filename.clone())),
                            referrer: req.referrer.clone(),
                            headers: req.headers.clone(),
                            max_connections: None,
                            // A captured batch carries no credentials
                            // of its own; the extension's headers, if
                            // any, are already above.
                            creds: Default::default(),
                            cookies: req.cookies.clone(),
                            category: None,
                            size: probed.as_ref().and_then(|p| p.size),
                            checksums: probed.map(|p| p.checksums.clone()).unwrap_or_default(),
                        };
                        match client.add_job(add).await {
                            Ok(id) => added.push(id),
                            // One row the daemon refused must not take
                            // the rest of the list with it: the user
                            // picked these together and expects them
                            // all, or to be told which one did not go.
                            Err(e) => failed.push(e),
                        }
                    }
                    if start_now {
                        // Started by hand, one gesture, after every job
                        // exists: this is not a queue run, so the
                        // global cap does not get to defer them. Bulk
                        // starts are silent about failures — a triage
                        // list can start dozens, and one window each
                        // would bury the screen — so they are collected
                        // and reported here instead.
                        for id in added {
                            if let Err(e) = client.start_job(id).await {
                                failed.push(e);
                            }
                        }
                    }
                    if failed.is_empty() {
                        return Ok(());
                    }
                    Err(failed.join("; "))
                },
                Msg::Sent,
            )
        }
        Msg::Sent(Ok(())) => iced::exit(),
        // The window stays open on a failure and says what happened.
        // Exiting either way is how a batch could lose downloads
        // without anyone finding out.
        Msg::Sent(Err(e)) => {
            st.error = Some(e);
            st.sending = false;
            Task::none()
        }
        Msg::Cancel => iced::exit(),
        Msg::Daemon(crate::gui::ipc::DaemonSignal::Lost) => iced::exit(),
        Msg::Daemon(crate::gui::ipc::DaemonSignal::Event(ev)) => match ev {
            crate::ipc_local::protocol::Event::SettingsChanged => {
                crate::gui::theme::refresh_tokens(
                    st.client.clone(),
                    |t| Msg::Themed(Box::new(t)),
                    Msg::Noop,
                )
            }
            crate::ipc_local::protocol::Event::Close => iced::exit(),
            crate::ipc_local::protocol::Event::Focus => {
                iced::window::latest().and_then(iced::window::gain_focus)
            }
            _ => Task::none(),
        },
        Msg::Themed(t) => {
            st.tokens = *t;
            Task::none()
        }
        Msg::WinResized(w, h) => {
            chrome::enforce_min_size(iced::Size::new(w, h), iced::Size::new(520.0, 360.0))
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
    let resize = iced::event::listen_with(|event, _status, _id| match event {
        iced::Event::Window(iced::window::Event::Resized(size)) => {
            Some(Msg::WinResized(size.width, size.height))
        }
        _ => None,
    });
    let events = crate::gui::ipc::lifecycle_events(crate::ipc_local::protocol::GuiKind::Batch)
        .map(Msg::Daemon);
    match app {
        App::Ready(st) if st.shot.is_some() => {
            Subscription::batch([resize, events, Shot::frames().map(|_| Msg::ShotTick)])
        }
        _ => Subscription::batch([resize, events]),
    }
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

/// The name each row will actually be added under.
///
/// Walked in send order, and only over the selected rows, because that
/// is what the daemon will see: a batch of three `clip.mkv` adds one
/// name and numbers the next two, and unticking a row gives its name
/// back to the row below.
///
/// `None` where nothing needs saying: an unselected row, a row whose
/// name is not known yet, or one whose name is free.
fn planned_names(st: &State) -> Vec<Option<String>> {
    let wanted: Vec<Option<String>> = st
        .rows
        .iter()
        .map(|r| r.selected.then(|| row_name(r)).flatten())
        .collect();
    plan(&st.taken_names, &wanted)
}

/// The renames a list of wanted names implies, given what is taken.
/// Split out from the rows so it can be tested without a window.
fn plan(taken: &[String], wanted: &[Option<String>]) -> Vec<Option<String>> {
    let mut taken: Vec<String> = taken.to_vec();
    wanted
        .iter()
        .map(|w| {
            let raw = w.as_deref()?;
            let planned =
                crate::domain::unique_name(raw, |c| taken.contains(&crate::domain::name_key(c)));
            taken.push(crate::domain::name_key(&planned));
            (planned != raw).then_some(planned)
        })
        .collect()
}

/// What this row would be saved as, as far as the window knows: the
/// captured name, else what the probe found.
fn row_name(r: &Row) -> Option<String> {
    r.req
        .filename
        .clone()
        .or_else(|| match &r.probe {
            Some(Ok(p)) => Some(p.filename.clone()),
            _ => None,
        })
        .filter(|n| !n.trim().is_empty())
}

fn ready_view(st: &State) -> Element<'_, Msg> {
    let t = &st.tokens;
    let t2 = *t;
    let n_total = st.rows.len();
    let n_sel = st.rows.iter().filter(|r| r.selected).count();
    let all = n_sel == n_total && n_total > 0;

    let header = row![
        // Not "send to oxdm": whoever the links came from, they are in
        // oxdm now and this window is oxdm asking which of them to
        // keep. "URLs" rather than "links" to match the toolbar button
        // that opens its single-item sibling.
        text(format!(
            "Add {n_sel} of {n_total} URL{}",
            if n_total == 1 { "" } else { "s" }
        ))
        .font(theme::BODY_BOLD)
        .size(14.0)
        .color(t.fg_1),
        iced::widget::Space::new().width(Length::Fill),
        text("Queue").font(theme::BODY).size(12.0).color(t.fg_2),
        combo(
            t,
            st.queues.iter().map(|(_, n)| n.clone()).collect(),
            st.queue.and_then(|q| st
                .queues
                .iter()
                .find(|(id, _)| *id == q)
                .map(|(_, n)| n.clone())),
            Msg::SetQueue,
            Length::Fixed(160.0),
        ),
    ]
    .spacing(theme::space::S2)
    .align_y(Alignment::Center);

    let planned = planned_names(st);
    let mut list = column![].spacing(2.0);
    list = list.push(checkbox(t, "Select all", all, true, Msg::SelectAll));
    list = list.push(hairline(t.border_subtle));
    for (i, r) in st.rows.iter().enumerate() {
        let detail: Element<'_, Msg> = match &r.probe {
            // Each row is really probed — the same HEAD the Add dialog
            // makes — and on a slow host that takes a while. A bare
            // ellipsis left the row looking empty rather than busy.
            None => row![
                crate::gui::icons::icon("ellipsis", 11.0, t.fg_3),
                text("checking the link\u{2026}")
                    .font(theme::BODY)
                    .size(11.0)
                    .color(t.fg_3),
            ]
            .spacing(theme::space::S1)
            .align_y(Alignment::Center)
            .into(),
            Some(Ok(p)) => row![
                text(format!(
                    "{}  ·  {}  ·  ",
                    p.filename,
                    // A dash says nothing: the server did not give a
                    // length, which is worth a word rather than a mark
                    // the reader has to interpret.
                    p.size
                        .map(format_bytes)
                        .unwrap_or_else(|| "unknown size".into()),
                ))
                .font(theme::BODY)
                .size(11.0)
                .color(t.fg_3),
                // The one fact in the line that costs the user
                // something later: a dropped connection starts this
                // one over. Coloured, not shouted.
                text(if p.is_resumable {
                    "resumable"
                } else {
                    "no resume"
                })
                .font(theme::BODY)
                .size(11.0)
                .color(if p.is_resumable {
                    t.fg_3
                } else {
                    t.status_warning
                }),
            ]
            .into(),
            Some(Err(e)) => text(format!("probe failed: {e}"))
                .font(theme::BODY)
                .size(11.0)
                .color(t.status_danger)
                .into(),
        };
        let mut lines = column![
            text(r.req.url.to_string())
                .font(theme::MONO)
                .size(12.0)
                .color(t.fg_1),
            detail,
        ]
        .spacing(2.0);
        // One name, one download: the daemon numbers a name the list
        // already holds, and this is where that stops being a surprise.
        if let Some(name) = planned.get(i).and_then(|n| n.clone()) {
            lines = lines.push(
                text(format!("will be added as {name}"))
                    .font(theme::MONO)
                    .size(11.0)
                    .color(t.status_warning),
            );
        }
        list = list.push(
            container(
                row![
                    checkbox(t, "", r.selected, true, move |v| Msg::Select(i, v)),
                    lines,
                ]
                .spacing(theme::space::S2)
                .align_y(Alignment::Center),
            )
            .padding([6.0, 0.0]),
        );
        if i + 1 < st.rows.len() {
            list = list.push(hairline(t.border_subtle));
        }
    }

    let footer_el = crate::gui::windows::add::footer(
        t,
        row![
            Btn::new("Cancel").ghost().on_press(Msg::Cancel).view(t),
            checkbox(t, "Start now", st.start_now, true, Msg::StartNow),
        ]
        .spacing(theme::space::S3)
        .align_y(Alignment::Center)
        .into(),
        row![
            match &st.error {
                Some(e) => row![
                    icons::icon("triangle-alert", 12.0, t.status_danger),
                    text(e.clone())
                        .font(theme::BODY)
                        .size(11.0)
                        .color(t.status_danger),
                ]
                .spacing(4.0)
                .align_y(Alignment::Center)
                .into(),
                None => Element::from(iced::widget::Space::new()),
            },
            Btn::new(format!(
                "Add {n_sel} URL{}",
                if n_sel == 1 { "" } else { "s" }
            ))
            .primary()
            .icon("download")
            .enabled(n_sel > 0 && !st.sending)
            .on_press(Msg::Send)
            .view(t),
        ]
        .spacing(theme::space::S3)
        .align_y(Alignment::Center)
        .into(),
    );

    let page = column![
        titlebar::titlebar(t, "Add URLs", false, Msg::Window),
        container(
            column![
                sibling(header.into()),
                crate::gui::widget::vscroll(list).height(Length::Fill)
            ]
            .spacing(theme::space::S2)
        )
        .padding(iced::Padding {
            top: theme::space::S4,
            bottom: theme::space::S4,
            left: theme::space::S4,
            right: theme::space::S4 - crate::gui::widget::SCROLL_GUTTER,
        })
        .height(Length::Fill),
        hairline(t.border_subtle),
        footer_el,
    ];

    let content = container(page)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(t2.bg_page.into()),
            text_color: Some(t2.fg_1),
            ..Default::default()
        });
    chrome::resize::resizable(t, content.into(), true, Msg::Window)
}

pub fn launch_batch(_path: PathBuf) {
    let mut app = iced::application(boot, update, view)
        .title(|_: &App| "oxdm - Add URLs".to_owned())
        .theme(|app: &App| match app {
            App::Ready(st) => st.tokens.iced_theme(),
            _ => Tokens::dark().iced_theme(),
        })
        .subscription(subscription)
        .default_font(theme::BODY)
        .antialiasing(true)
        .window(chrome::window_settings(
            iced::Size::new(760.0, 520.0),
            iced::Size::new(520.0, 360.0),
        ));
    for f in theme::fonts::ALL {
        app = app.font(*f);
    }
    if let Err(e) = app.run() {
        eprintln!("gui error: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| crate::domain::name_key(n)).collect()
    }

    fn wanted(names: &[Option<&str>]) -> Vec<Option<String>> {
        names.iter().map(|n| n.map(str::to_owned)).collect()
    }

    /// The first row keeps the name; the rest of the batch numbers
    /// around it, the same way the daemon will when they arrive.
    #[test]
    fn a_batch_numbers_within_itself() {
        let out = plan(
            &[],
            &wanted(&[Some("clip.mkv"), Some("clip.mkv"), Some("clip.mkv")]),
        );
        assert_eq!(out[0], None, "nothing to say about the first");
        assert_eq!(out[1].as_deref(), Some("clip_1.mkv"));
        assert_eq!(out[2].as_deref(), Some("clip_2.mkv"));
    }

    #[test]
    fn what_the_list_already_holds_counts_too() {
        let out = plan(&keys(&["clip.mkv"]), &wanted(&[Some("clip.mkv")]));
        assert_eq!(out[0].as_deref(), Some("clip_1.mkv"));
    }

    /// An unticked row is not being added, so it neither takes a name
    /// nor needs a hint.
    #[test]
    fn a_row_that_is_not_going_takes_no_name() {
        let out = plan(&[], &wanted(&[None, Some("clip.mkv")]));
        assert_eq!(out, vec![None, None]);
    }

    #[test]
    fn a_free_name_says_nothing() {
        let out = plan(&keys(&["other.mkv"]), &wanted(&[Some("clip.mkv")]));
        assert_eq!(out, vec![None]);
    }
}
