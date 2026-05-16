//! Local IPC between the oxdm daemon and its GUI subprocesses.
//!
//! Transport: `interprocess::local_socket` (Unix domain socket on
//! Linux/mac, named pipe on Windows). Per-user namespace.
//!
//! Frames: 4-byte big-endian length prefix + JSON body. Each
//! connection is bidirectional: clients send `Frame::Request`s + a
//! single `Subscribe`; the server replies with `Frame::Reply`s and
//! pushes `Frame::Event`s for the lifetime of the subscription.
//!
//! See `protocol.rs` for the wire types. The wire surface intentionally
//! mirrors `data::AppState` method signatures so the GUI client can
//! offer the same Rust-level API a direct `Arc<AppState>` once did.

pub mod client;
pub mod codec;
pub mod protocol;
pub mod server;

pub use client::Client;
pub use protocol::{Event, Frame, Reply, Request, SubFilter};
pub use server::serve;

/// Per-user socket name shared by daemon and GUI subprocesses.
/// Matches `single_instance::socket_name()`'s structure but uses a
/// dedicated suffix so the two namespaces never collide.
///
/// `OXDM_INSTANCE_SUFFIX` extends the per-user suffix so a sandboxed
/// secondary instance (e.g. the visual-test harness) doesn't talk to
/// the host daemon's GUI subprocesses.
pub fn socket_name() -> String {
    #[cfg(unix)]
    let base = unsafe { libc::getuid() }.to_string();
    #[cfg(not(unix))]
    let base = "user".to_string();
    let suffix = match std::env::var("OXDM_INSTANCE_SUFFIX")
        .ok()
        .filter(|s| !s.is_empty())
    {
        Some(s) => format!("{base}-{s}"),
        None => base,
    };
    format!("oxdm-gui-{suffix}.sock")
}
