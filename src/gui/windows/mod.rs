//! Per-window iced applications. Each window kind runs as its own OS
//! process (`oxdm gui <kind>`), matching the egui app's model.

pub mod about;
pub mod add;
pub mod batch;
pub mod download;
pub mod main;
pub mod main_dialogs;
pub mod power;
pub mod properties;
pub mod queues;
pub mod settings;
