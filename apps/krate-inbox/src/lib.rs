//! Inbox -- drop a file on the window and it reads it.
//!
//! This app exists to prove `ui.dropzone` is real. The capability was
//! declarable, validated by the manifest, worded on the consent screen
//! ("accept files you drag onto it"), and actively RECOMMENDED by the port
//! analyzer -- with no WIT interface, no host function and no adapter handling
//! behind any of it. An app that declared it put a promise on the consent
//! sheet that the runtime could not keep (K-175).
//!
//! The permission model is `open-file`'s, because a drop is the same act: the
//! person deliberately handed this file to this window, which is a clearer
//! consent than any dialog. So the app is given a NAME and an opaque TOKEN and
//! never a path. It cannot learn where the file lives, cannot walk to its
//! folder, and cannot come back for it on a later run.
//!
//! What it shows: the drop target lights while a file is over the window, and
//! once dropped, the file's name, its size, and its first few lines.
//!
//! `#![no_std]`, so it stays `krate:*`-only.

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use krate::bindings::krate::fs::files;
use krate::bindings::krate::io::stdio;
use krate::ui::{events, tree, types, window};

const WIDTH: u32 = 820;
const HEIGHT: u32 = 560;

const ROOT: u64 = 1;
const TITLE: u64 = 2;
const HINT: u64 = 3;
const TARGET: u64 = 4;
const TARGET_TEXT: u64 = 5;
const NAME: u64 = 6;
const SIZE: u64 = 7;
const BODY_BASE: u64 = 20;
/// Lines of the file shown. Enough to prove it was really read.
const BODY_LINES: usize = 8;

const fn rgb(r: u8, g: u8, b: u8) -> types::Color {
    types::Color { r, g, b, a: 255 }
}

const PAGE: types::Color = rgb(14, 16, 22);
const INK: types::Color = rgb(237, 240, 247);
const QUIET: types::Color = rgb(148, 158, 178);
const ACCENT: types::Color = rgb(122, 162, 247);
const TARGET_IDLE: types::Color = rgb(30, 34, 44);
const TARGET_LIVE: types::Color = rgb(30, 52, 86);
const EDGE_IDLE: types::Color = rgb(48, 54, 66);

fn owned(s: &str) -> String {
    let mut out = String::new();
    out.push_str(s);
    out
}

fn push_u64(mut n: u64, out: &mut String) {
    if n == 0 {
        out.push('0');
        return;
    }
    let mut d = [0u8; 20];
    let mut l = 0usize;
    while n > 0 && l < d.len() {
        d[l] = b'0' + (n % 10) as u8;
        n /= 10;
        l += 1;
    }
    while l > 0 {
        l -= 1;
        out.push(d[l] as char);
    }
}

fn say(line: &str) {
    let out = stdio::stdout();
    let _ = out.write(line.as_bytes());
    let _ = out.write(b"\n");
    let _ = out.flush();
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
            grow: 0.0,
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

fn ink(color: types::Color, size: f32, bold: bool) -> types::TextStyle {
    types::TextStyle {
        color: Some(color),
        outline: None,
        outline_width: 0.0,
        size: Some(size),
        bold,
    }
}

fn label(id: u64, parent: u64, text: &str, style: types::TextStyle) -> types::WidgetNode {
    let mut n = node(id, Some(parent), types::WidgetKind::Text);
    n.label = Some(owned(text));
    n.style.text = Some(style);
    n
}

/// What the app knows right now.
struct State {
    hovering: bool,
    name: String,
    size: u64,
    lines: Vec<String>,
}

impl State {
    fn new() -> Self {
        Self {
            hovering: false,
            name: String::new(),
            size: 0,
            lines: Vec::new(),
        }
    }
}

fn build(win: u64) -> bool {
    let mut root = node(ROOT, None, types::WidgetKind::Stack);
    root.style.grow = 1.0;
    root.style.padding = 28.0;
    root.style.box_ = Some(types::BoxStyle {
        background: Some(PAGE),
        border: None,
        border_width: 0.0,
        corner_radius: 0.0,
    });
    if tree::set_root(win, &root).is_err() {
        return false;
    }

    let title = label(TITLE, ROOT, "Inbox", ink(INK, 28.0, true));
    let hint = label(
        HINT,
        ROOT,
        "Drag a file onto this window.",
        ink(QUIET, 14.0, false),
    );
    if tree::upsert_node(win, &title).is_err() || tree::upsert_node(win, &hint).is_err() {
        return false;
    }

    let mut target = node(TARGET, Some(ROOT), types::WidgetKind::Stack);
    target.style.padding = 30.0;
    target.style.box_ = Some(types::BoxStyle {
        background: Some(TARGET_IDLE),
        border: Some(EDGE_IDLE),
        border_width: 2.0,
        corner_radius: 12.0,
    });
    if tree::upsert_node(win, &target).is_err() {
        return false;
    }
    let target_text = label(
        TARGET_TEXT,
        TARGET,
        "drop here",
        ink(QUIET, 15.0, false),
    );
    if tree::upsert_node(win, &target_text).is_err() {
        return false;
    }

    let name = label(NAME, ROOT, "", ink(INK, 17.0, true));
    let size = label(SIZE, ROOT, "", ink(QUIET, 13.0, false));
    if tree::upsert_node(win, &name).is_err() || tree::upsert_node(win, &size).is_err() {
        return false;
    }
    for i in 0..BODY_LINES {
        let line = label(BODY_BASE + i as u64, ROOT, "", ink(QUIET, 12.0, false));
        if tree::upsert_node(win, &line).is_err() {
            return false;
        }
    }
    true
}

fn refresh(win: u64, state: &State) {
    // The drop target, lit while something is over the window.
    let mut target = node(TARGET, Some(ROOT), types::WidgetKind::Stack);
    target.style.padding = 30.0;
    target.style.box_ = Some(types::BoxStyle {
        background: Some(if state.hovering {
            TARGET_LIVE
        } else {
            TARGET_IDLE
        }),
        border: Some(if state.hovering { ACCENT } else { EDGE_IDLE }),
        border_width: 2.0,
        corner_radius: 12.0,
    });
    let _ = tree::upsert_node(win, &target);

    let _ = tree::upsert_node(
        win,
        &label(
            TARGET_TEXT,
            TARGET,
            if state.hovering {
                "let go to open it"
            } else {
                "drop here"
            },
            ink(if state.hovering { ACCENT } else { QUIET }, 15.0, false),
        ),
    );

    let _ = tree::upsert_node(win, &label(NAME, ROOT, &state.name, ink(INK, 17.0, true)));

    let mut size_line = String::new();
    if !state.name.is_empty() {
        push_u64(state.size, &mut size_line);
        size_line.push_str(" bytes, read through the token the drop handed over");
    }
    let _ = tree::upsert_node(win, &label(SIZE, ROOT, &size_line, ink(QUIET, 13.0, false)));

    for i in 0..BODY_LINES {
        let text = state.lines.get(i).map(|s| s.as_str()).unwrap_or("");
        let _ = tree::upsert_node(
            win,
            &label(BODY_BASE + i as u64, ROOT, text, ink(QUIET, 12.0, false)),
        );
    }
}

/// Open the dropped file through its token and keep the first few lines.
///
/// The token is the whole permission: it names one file the person handed
/// over, and `open-chosen` is the only thing that can turn it into bytes.
fn read_dropped(state: &mut State, name: String, token: &str) {
    state.name = name;
    state.size = 0;
    state.lines.clear();

    let file = match files::open_chosen(token, files::OpenMode::Read) {
        Ok(file) => file,
        Err(_) => {
            state.lines.push(owned("could not open it"));
            return;
        }
    };
    // A head, not the whole file: this is a demonstration, and a drop of
    // something enormous should not become an allocation of the same size.
    let bytes = match file.read(4096) {
        Ok(bytes) => bytes,
        Err(err) => {
            let mut why = String::new();
            why.push_str("could not read it: ");
            why.push_str(match err {
                files::FsError::NotFound => "not found",
                files::FsError::PermissionDenied => "permission denied",
                files::FsError::AlreadyExists => "already exists",
                files::FsError::InvalidPath => "invalid path",
                files::FsError::NotADirectory => "not a directory",
                files::FsError::IsADirectory => "is a directory",
                _ => "other",
            });
            state.lines.push(why);
            return;
        }
    };
    state.size = file.stat().map(|s| s.size).unwrap_or(bytes.len() as u64);

    // Show it as text when it is text, and say so plainly when it is not,
    // rather than printing replacement characters at somebody.
    match core::str::from_utf8(&bytes) {
        Ok(text) => {
            for line in text.split('\n').take(BODY_LINES) {
                let mut kept = String::new();
                for (i, ch) in line.chars().enumerate() {
                    if i >= 96 {
                        kept.push_str(" ...");
                        break;
                    }
                    kept.push(ch);
                }
                state.lines.push(kept);
            }
        }
        Err(_) => {
            state.lines.push(owned("not text -- showing nothing of it"));
        }
    }
}

struct Component;

impl krate::Guest for Component {
    fn run() -> i32 {
        let win = match window::create(
            "Inbox",
            types::WindowSize {
                width: WIDTH,
                height: HEIGHT,
            },
        ) {
            Ok(win) => win,
            Err(_) => return 1,
        };
        if !build(win) {
            return 1;
        }
        let mut state = State::new();
        refresh(win, &state);
        say("inbox: ready -- drag a file onto the window");

        loop {
            match events::wait(Some(120)) {
                Some(types::Event::CloseRequested(_)) => break,
                Some(types::Event::FileHovering(over)) => {
                    state.hovering = over;
                    refresh(win, &state);
                }
                Some(types::Event::FileDropped(file)) => {
                    let mut m = String::new();
                    m.push_str("inbox: dropped ");
                    m.push_str(&file.name);
                    say(&m);
                    state.hovering = false;
                    read_dropped(&mut state, file.name, &file.token);
                    refresh(win, &state);
                }
                _ => {}
            }
        }
        let _ = window::close(win);
        0
    }
}

krate::export!(Component);
