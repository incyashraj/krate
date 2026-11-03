//! Krate Code -- a keyboard-first Rust editor, made as a Krate app.
//!
//! The editor half of IC-978: a modal editor in the spirit of Neovim
//! (normal, insert, visual and command modes, counts, motions, operators,
//! `.` repeat, search, `:` commands), Rust highlighting, a project folder with
//! a file tree, tabs and vertical splits, a command palette, and a bottom
//! panel kept for Problems, Output and a Terminal.
//!
//! Everything that is not drawing or a host call is plain `alloc` code in its
//! own module, so `cargo test` proves it natively:
//!
//! - [`buffer`]: a gap buffer of lines with grouped undo and redo.
//! - [`vim`]: the modal engine -- keys in, edits and effects out.
//! - [`syntax`]: the Rust highlighter, one line at a time with carried state.
//! - [`theme`]: the two looks, dark and light.
//! - [`project`]: the folder, its tree, and the file system seam.
//! - [`palette`]: every action, for the command palette.
//! - [`tools`]: the seam where running programs (IC-977) plugs in.
//! - [`workspace`]: open files, panes, panels -- the whole editor's state.
//!
//! The guest (`guest`, wasm only) draws that state on one canvas and turns
//! window events into editor input.

#![cfg_attr(target_arch = "wasm32", no_std)]

extern crate alloc;

#[cfg(test)]
extern crate std;

pub mod buffer;
pub mod palette;
pub mod project;
pub mod syntax;
pub mod text;
pub mod theme;
pub mod tools;
pub mod vim;
pub mod workspace;

#[cfg(test)]
mod vim_tests;

#[cfg(target_arch = "wasm32")]
mod guest;
