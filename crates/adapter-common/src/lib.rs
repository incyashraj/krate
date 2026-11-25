//! Shared host-adapter building blocks for Krate.
//!
//! This crate is for adapter behavior that must stay identical across Linux,
//! macOS, Windows, and later mobile hosts.

pub mod appview;
pub mod canvas_damage;
pub mod canvas_list;
pub mod drawtext;
pub mod kv;
pub mod locale;
pub mod menus;
pub mod mime;
pub mod net;
pub mod overlay;
pub mod painter;
pub mod path;
pub mod time;
pub mod ui;
#[cfg(feature = "vector-text")]
pub mod vector_text;
