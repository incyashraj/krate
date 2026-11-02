//! File Check -- the browser Player's file dialog proof (IC-974,
//! crates/adapter-web/player/file-proof.mjs).
//!
//! Every path of krate:ui/dialog and picked files an editor or a converter
//! uses, in one window. Each outcome is a line on stdout (the proof reads
//! it) and on the canvas:
//!
//! - on start, seven seconds after the last tap so that nobody's tap is
//!   behind it, the open-file dialog; the file
//!   is read whole and by seeking, its size taken from the handle, and a
//!   write to it (opened for reading) is refused;
//! - on the first tap, the open-file dialog again, from a tap;
//! - on the second tap, the open-file dialog, which the proof cancels;
//! - on the third tap, the save-file dialog, a note written to it and let
//!   go of, then read back through the same token;
//! - on the fourth and fifth taps, a yes/no question; on the sixth, a
//!   message;
//! - a file dropped on the window, read through its token;
//! - a token nobody was handed, refused as not found.
//!
//! Without the dialog capabilities (manifest-ungranted.toml) the dialogs on
//! start answer permission-denied, as on desktop.

#![no_std]
extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use krate::bindings::krate::fs::files as fs;
use krate::fs::{FileExt, FsError, OpenMode};
use krate::gfx::{canvas2d, types as gfx};
use krate::io::stdio;
use krate::ui::dialog::{self, UiError};
use krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;
const WIDTH: f32 = 360.0;
const HEIGHT: f32 = 300.0;

struct Component;

fn color(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
}

fn node(id: u64, parent: Option<u64>, kind: types::WidgetKind) -> types::WidgetNode {
    types::WidgetNode {
        id,
        parent,
        kind,
        label: None,
        role: None,
        style: types::Style {
            width: None,
            height: None,
            grow: 1.0,
            padding: 0.0,
            text: None,
            place: None,
            box_: None,
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

/// FNV-1a, 64 bits: the proof computes the same over the bytes it gave.
fn fnv(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn ui_error(e: &UiError) -> String {
    match e {
        UiError::PermissionDenied => String::from("permission-denied"),
        UiError::InvalidWindow => String::from("invalid-window"),
        UiError::InvalidWidget => String::from("invalid-widget"),
        UiError::Unsupported(w) => format!("unsupported: {w}"),
        UiError::Platform(w) => format!("platform: {w}"),
    }
}

fn fs_error(e: &FsError) -> String {
    match e {
        FsError::NotFound => String::from("not-found"),
        FsError::PermissionDenied => String::from("permission-denied"),
        FsError::AlreadyExists => String::from("already-exists"),
        FsError::InvalidPath => String::from("invalid-path"),
        FsError::NotADirectory => String::from("not-a-directory"),
        FsError::IsADirectory => String::from("is-a-directory"),
        FsError::Io(w) => format!("io: {w}"),
    }
}

fn say(lines: &mut Vec<String>, text: String) {
    let _ = stdio::println(&text);
    lines.push(text);
    if lines.len() > 14 {
        lines.remove(0);
    }
}

/// Everything about one picked file, in one line.
fn describe(token: &str, name: &str) -> String {
    let file = match fs::open_chosen(token, OpenMode::Read) {
        Ok(f) => f,
        Err(e) => return format!("{name} open {}", fs_error(&e)),
    };
    let size = file.stat().map(|s| s.size).unwrap_or(u64::MAX);
    let all = match file.read_to_end() {
        Ok(b) => b,
        Err(e) => return format!("{name} read {}", fs_error(&e)),
    };
    let at = file
        .seek_set(100)
        .map(|p| format!("{p}"))
        .unwrap_or_else(|e| fs_error(&e));
    let four = file
        .read(4)
        .map(|b: Vec<u8>| hex(&b))
        .unwrap_or_else(|e| fs_error(&e));
    let end = file
        .seek_end()
        .map(|p| format!("{p}"))
        .unwrap_or_else(|e| fs_error(&e));
    let past = file
        .read(16)
        .map(|b| format!("{}", b.len()))
        .unwrap_or_else(|e| fs_error(&e));
    let write = match file.write(b"x") {
        Ok(n) => format!("wrote {n}"),
        Err(e) => fs_error(&e),
    };
    format!(
        "{name} size={size} len={} fnv={} seek={at} at100={four} end={end} past-end={past} write={write}",
        all.len(),
        fnv(&all)
    )
}

fn open_line(win: u64, title: &str) -> String {
    match dialog::open_file(win, title, "bin,txt") {
        Ok(Some(chosen)) => describe(&chosen.token, &chosen.name),
        Ok(None) => String::from("none"),
        Err(e) => ui_error(&e),
    }
}

fn note() -> Vec<u8> {
    let mut bytes = Vec::from(&b"Saved from a tab\n"[..]);
    bytes.extend(0u8..=255);
    bytes
}

fn save_line(win: u64) -> String {
    let chosen = match dialog::save_file(win, "Save the note", "note", "txt") {
        Ok(Some(c)) => c,
        Ok(None) => return String::from("none"),
        Err(e) => return ui_error(&e),
    };
    let bytes = note();
    {
        let file = match fs::open_chosen(&chosen.token, OpenMode::Write) {
            Ok(f) => f,
            Err(e) => return format!("{} open {}", chosen.name, fs_error(&e)),
        };
        if let Err(e) = file.write_all(&bytes[..10]) {
            return format!("{} write {}", chosen.name, fs_error(&e));
        }
        if let Err(e) = file.write_all(&bytes[10..]) {
            return format!("{} write {}", chosen.name, fs_error(&e));
        }
        // Let go of it here: that is the save.
    }
    let back = match fs::open_chosen(&chosen.token, OpenMode::Read).and_then(|f| f.read_to_end()) {
        Ok(b) => fnv(b.as_slice()),
        Err(e) => fs_error(&e),
    };
    format!(
        "{} wrote={} fnv={} reread={back}",
        chosen.name,
        bytes.len(),
        fnv(&bytes)
    )
}

fn yes_no(win: u64, n: u32) -> String {
    match dialog::confirm(win, "Keep going?", &format!("Question {n}: answer it.")) {
        Ok(true) => String::from("yes"),
        Ok(false) => String::from("no"),
        Err(e) => ui_error(&e),
    }
}

impl krate::Guest for Component {
    fn run() -> i32 {
        let Ok(win) = window::create(
            "File Check",
            types::WindowSize {
                width: WIDTH as u32,
                height: HEIGHT as u32,
            },
        ) else {
            return 1;
        };
        if tree::set_root(win, &node(ROOT_ID, None, types::WidgetKind::Stack)).is_err() {
            return 2;
        }
        let _ = tree::upsert_node(
            win,
            &node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas),
        );
        let Ok(canvas) = canvas2d::bind(win, CANVAS_ID) else {
            return 3;
        };
        let _ = canvas2d::set_design_size(
            canvas,
            gfx::Size {
                width: WIDTH,
                height: HEIGHT,
            },
        );

        let mut lines: Vec<String> = Vec::new();
        let draw = |lines: &Vec<String>| {
            let _ = canvas2d::clear(canvas, color(0.07, 0.09, 0.14));
            let ink = color(0.92, 0.94, 0.98);
            let _ = canvas2d::draw_text(
                canvas,
                "File Check",
                gfx::Point { x: 16.0, y: 30.0 },
                20.0,
                ink,
            );
            for (i, l) in lines.iter().enumerate() {
                let _ = canvas2d::draw_text(
                    canvas,
                    l,
                    gfx::Point {
                        x: 16.0,
                        y: 56.0 + 16.0 * i as f32,
                    },
                    10.0,
                    ink,
                );
            }
            let _ = canvas2d::present(canvas);
        };
        draw(&lines);

        // On start, and long enough after the last tap (Allow on the consent
        // card) that the browser no longer counts it: nobody's tap is behind
        // this picker. Browsers keep a tap for about five seconds.
        let mut quiet = 0;
        while quiet < 70 {
            if events::wait(Some(100)).is_none() {
                quiet += 1;
            }
        }
        let first = open_line(win, "Choose the test file");
        let refused = first == "permission-denied";
        say(&mut lines, format!("start-open: {first}"));
        if refused {
            say(&mut lines, format!("start-save: {}", save_line(win)));
        }
        let forged = match fs::open_chosen("chosen-999", OpenMode::Read) {
            Ok(_) => String::from("opened"),
            Err(e) => fs_error(&e),
        };
        say(&mut lines, format!("forged-token: {forged}"));
        say(&mut lines, String::from("ready"));

        let mut taps = 0u32;
        loop {
            draw(&lines);
            let mut next = events::wait(Some(50));
            let mut closed = false;
            while let Some(event) = next {
                match event {
                    types::Event::CloseRequested(_) => closed = true,
                    types::Event::Pointer(p) if p.pressed && !refused => {
                        taps += 1;
                        let line = match taps {
                            1 => format!("tap-open: {}", open_line(win, "Choose a file")),
                            2 => format!("tap-cancel: {}", open_line(win, "Choose nothing")),
                            3 => format!("save: {}", save_line(win)),
                            4 | 5 => format!("confirm: {}", yes_no(win, taps - 3)),
                            6 => match dialog::message(win, "All done", "That was the last one.") {
                                Ok(()) => String::from("message: ok"),
                                Err(e) => format!("message: {}", ui_error(&e)),
                            },
                            _ => format!("tap: {taps}"),
                        };
                        say(&mut lines, line);
                    }
                    types::Event::FileDropped(chosen) => {
                        say(
                            &mut lines,
                            format!("dropped: {}", describe(&chosen.token, &chosen.name)),
                        );
                    }
                    types::Event::FileHovering(over) => {
                        say(&mut lines, format!("hovering: {over}"));
                    }
                    _ => {}
                }
                next = events::poll();
            }
            if closed {
                break;
            }
        }
        let _ = window::close(win);
        0
    }
}

krate::export!(Component);
