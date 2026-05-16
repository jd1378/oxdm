//! oxdm — cross-platform download manager built on `odl`.
//!
//! Layered:
//! - [`domain`]: pure types — no I/O, no UI.
//! - [`data`]: state, persistence, and the `odl` runner.
//! - [`ipc`]: browser-extension bridge (capture API).
//! - [`app`]: egui/eframe presentation layer.

pub mod daemon;
pub mod data;
pub mod domain;
pub mod ipc;
pub mod ipc_local;
pub mod single_instance;
pub mod ui;
