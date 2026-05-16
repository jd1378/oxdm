//! UI infrastructure: icon loading, OS window chrome, edge-resize hit
//! testing, and number/byte/time formatters. These live outside
//! `components/` because they don't render UI directly — they support
//! the components that do.

pub mod chrome;
pub mod dashed;
pub mod format;
pub mod icons;
pub mod modal;
pub mod resize;
