//! Shared host-adapter building blocks for Layer36.
//!
//! This crate is for adapter behavior that must stay identical across Linux,
//! macOS, Windows, and later mobile hosts.

pub mod locale;
pub mod net;
pub mod path;
pub mod time;
