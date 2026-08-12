//! Application state and the bridge to `odl`.
//!
//! This is the only layer permitted to import `odl::*`. Everything outside
//! observes it through `domain` types and `DomainEvent`s. Wiring lives in
//! [`AppState`]; the runner that drives one job through `odl` is
//! [`runner::JobRunner`]; pause/cancel policy is hidden behind
//! [`pause::PauseStrategy`].

pub mod conditions;
pub mod crypto;
mod events;
mod file_watch;
mod hooks;
pub mod idle;
pub mod keyring;
mod mapping;
mod pause;
mod power;
mod queue_scheduler;
mod resolvers;
mod runner;
pub mod space;
pub mod state;
pub mod store;
pub mod update_channel;
mod update_watch;

pub use conditions::{CondSupport, available_conditions};
pub use events::{ConflictKind, DomainEvent, next_event};
pub use file_watch::spawn as spawn_file_watch;
pub use hooks::spawn as spawn_hook_executor;
pub use idle::IdleWatch;
pub use pause::{CancelResumeStrategy, PauseStrategy};
pub use queue_scheduler::spawn as spawn_queue_scheduler;
pub use runner::PartCounters;
pub use state::{
    AppState, JobEntry, ProbeResult, RemoveOpts, decode_pairing_code, encode_pairing_code,
};
pub use update_channel::{
    HttpFeedUpdateChannel, NoopUpdateChannel, UpdateChannel, UpdateInfo, UpdaterEvent,
};
pub use update_watch::spawn as spawn_update_watch;
