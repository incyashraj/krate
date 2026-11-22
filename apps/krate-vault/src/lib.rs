//! Vault -- a Markdown notes app for a folder of notes, made as a Krate app.
//!
//! A vault is one folder of `.md` files and the folders inside it. Vault
//! shows it the way people who keep notes this way expect: the file tree on
//! the left, the notes in tabs, the editor styling Markdown as it is typed
//! (and a reading view), `[[links]]` between notes with completion as you
//! type them, backlinks and an outline on the right, vault-wide search, a
//! quick switcher, a command palette and a status bar with word counts. It
//! is also a measurement instrument: the same work as the well-known
//! Electron notes app, done by a Krate app, at 10,000 notes and in a note of
//! 200,000 lines.
//!
//! Everything that is not drawing or a host call is plain `alloc` code in
//! its own module, so `cargo test` proves it natively:
//!
//! - [`buffer`]: a gap buffer of lines with grouped undo and redo.
//! - [`md`]: Markdown, one line at a time, and the fence index.
//! - [`editor`]: one open note and the edits a Markdown editor makes.
//! - [`vfs`]: the seam to the files, with a folder held in memory.
//! - [`vault`]: the notes, the folders, and the index of links and tags.
//! - [`search`]: vault search as a job that runs between frames.
//! - [`app`]: the whole app's state -- tabs, panes, the switcher, the
//!   palette -- and what every key and click does to it.
//! - [`sample`]: the sample vault and the `quick` run.
//!
//! The guest (`guest`, wasm only) draws that state on one canvas and turns
//! window events into input.

#![cfg_attr(target_arch = "wasm32", no_std)]

extern crate alloc;

#[cfg(test)]
extern crate std;

pub mod app;
pub mod buffer;
pub mod editor;
pub mod md;
pub mod sample;
pub mod search;
pub mod text;
pub mod vault;
pub mod vfs;

#[cfg(target_arch = "wasm32")]
mod guest;
