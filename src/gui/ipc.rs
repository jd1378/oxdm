//! Daemon event bridge: an iced `Subscription` that opens its own
//! subscribe-only connection and forwards `Event`s. Yields `Lost`
//! once the stream ends so windows can exit (matches the egui
//! `daemon_lost` → process exit behavior).

use iced::Subscription;
use iced::futures::{SinkExt, Stream};

use crate::ipc_local::Client;
use crate::ipc_local::protocol::{Event, GuiKind, SubFilter};

#[derive(Debug, Clone)]
pub enum DaemonSignal {
    Event(Event),
    Lost,
}

fn event_stream(filter: SubFilter, kind: GuiKind) -> impl Stream<Item = DaemonSignal> {
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
        // Register THIS connection in the daemon's focus registry:
        // `register_if_ready` requires Hello + Subscribe on the same
        // conn before `try_close` / `try_focus` can reach the window.
        // The window's request/reply connection also says Hello, but
        // it never Subscribes, so it alone can't complete
        // registration — without this, the tray spawn state sticks at
        // `Spawning` and singleton re-triggers (evict + respawn) are
        // silently dropped.
        if client.hello(kind).await.is_err() {
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

/// All daemon events, registered under `kind` for focus/evict.
pub fn all_events(kind: GuiKind) -> Subscription<DaemonSignal> {
    Subscription::run_with(kind, |k| event_stream(SubFilter::All, *k))
}

/// Lifecycle events only — no per-tick counter pumps. For dialog
/// windows that don't render progress.
pub fn lifecycle_events(kind: GuiKind) -> Subscription<DaemonSignal> {
    Subscription::run_with(kind, |k| event_stream(SubFilter::Lifecycle, *k))
}
