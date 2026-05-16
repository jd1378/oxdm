//! Top-level eframe viewports. Each window owns its own subprocess
//! entry point; one per executable invocation kind. Window-specific
//! state lives in the window's directory.

pub mod add;
pub mod batch;
pub mod download;
pub mod properties;
pub mod queues;
pub mod settings;
