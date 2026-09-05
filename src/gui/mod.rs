//! iced (tiny-skia) presentation layer. Replaces `src/ui` (egui).
//!
//! Window/process model matches the egui app: each window kind is a
//! separate OS process launched via `oxdm gui <kind>`; in-window
//! dialogs are overlay layers, not extra OS windows.

pub mod app_icon;
pub mod chrome;
pub mod clipboard;
pub mod color;
pub mod diff;
pub mod format;
pub mod icons;
pub mod ipc;
pub mod save_path;
pub mod shot;
pub mod theme;
pub mod ui_prefs;
pub mod widget;
pub mod window_size;
pub mod windows;
