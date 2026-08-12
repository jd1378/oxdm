//! oxdm — cross-platform download manager built on `odl`.
//!
//! Layered:
//! - [`domain`]: pure types — no I/O, no UI.
//! - [`data`]: state, persistence, and the `odl` runner.
//! - [`ipc`]: browser-extension bridge (capture API).
//! - [`gui`]: iced (tiny-skia) presentation layer.

pub mod daemon;
pub mod data;
pub mod domain;
pub mod gui;
pub mod ipc;
pub mod ipc_local;
pub mod platform;
pub mod single_instance;
pub mod update_install;
