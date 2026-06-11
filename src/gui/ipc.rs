//! Daemon event bridge: an iced `Subscription` that opens its own
//! subscribe-only connection and forwards `Event`s. Yields `Lost`
//! once the stream ends so windows can exit (matches the egui
//! `daemon_lost` → process exit behavior).

use iced::Subscription;
use iced::futures::{SinkExt, Stream};

use crate::ipc_local::Client;
use crate::ipc_local::protocol::{Event, SubFilter};

#[derive(Debug, Clone)]
pub enum DaemonSignal {
    Event(Event),
    Lost,
}

fn event_stream(filter: SubFilter) -> impl Stream<Item = DaemonSignal> {
    iced::stream::channel(64, async move |mut out| {
        let client = match Client::connect().await {
            Ok(c) => c,
            Err(_) => {
                let _ = out.send(DaemonSignal::Lost).await;
                return;
            }
        };
        if client.subscribe(filter).await.is_err() {
            let _ = out.send(DaemonSignal::Lost).await;
            return;
        }
        let Some(mut events) = client.take_events().await else {
            let _ = out.send(DaemonSignal::Lost).await;
            return;
        };
        while let Some(ev) = events.recv().await {
            if out.send(DaemonSignal::Event(ev)).await.is_err() {
                return;
            }
        }
        let _ = out.send(DaemonSignal::Lost).await;
    })
}

/// All daemon events (main window).
pub fn all_events() -> Subscription<DaemonSignal> {
    Subscription::run(|| event_stream(SubFilter::All))
}
