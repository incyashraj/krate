//! Krate clip — the limitation probe for the clipboard.
//!
//! The wall it tests: can an app hand text to the rest of the machine and get
//! it back? Copy and paste is the oldest bridge between programs; if an app
//! cannot reach the system clipboard, it lives on an island. This app writes a
//! known marker string with `clipboard::write-text`, reads it straight back
//! with `clipboard::read-text`, and shows both side by side in a small window.
//!
//! On exit it prints one line so a script can judge the round-trip without
//! looking at pixels: `clip:ok` when the read-back matched the write,
//! `clip:mismatch` when the clipboard returned different bytes, and
//! `clip:error` when either call was denied or unsupported by the host. The
//! comparison is done on raw bytes by hand -- no `format!`, no `==` on owned
//! strings, only `krate:*` imports and no reachable panic.

#[allow(warnings)]
mod bindings;

use bindings::krate::io::{args, stdio};
use bindings::krate::ui::{clipboard, events, tree, types, window};

const ROOT_ID: u64 = 1;
const TITLE_ID: u64 = 2;
const WROTE_ID: u64 = 3;
const READ_ID: u64 = 4;
const VERDICT_ID: u64 = 5;

const MARKER: &str = "krate-clip round-trip 12345";
const WIDTH: u32 = 460;
const HEIGHT: u32 = 240;

const QUICK_ROUNDS: u32 = 20;
const MAX_ROUNDS: u32 = 600;
const ROUND_MILLIS: u32 = 50;

struct Component;

impl bindings::Guest for Component {
    fn run() -> i32 {
        let out = stdio::stdout();

        // Write the marker, then read it back. Either call can be denied or
        // unsupported by the host; treat that as an honest `clip:error` rather
        // than a crash. Nothing here traps.
        if clipboard::write_text(MARKER).is_err() {
            let _ = out.write(b"clip:error\n");
            return draw_and_wait(
                "Clipboard write was refused by the host.",
                MARKER.as_bytes(),
                b"(write failed)",
                b"clip:error",
            );
        }

        let read_back = match clipboard::read_text() {
            Ok(text) => text,
            Err(_) => {
                let _ = out.write(b"clip:error\n");
                return draw_and_wait(
                    "Clipboard read was refused by the host.",
                    MARKER.as_bytes(),
                    b"(read failed)",
                    b"clip:error",
                );
            }
        };

        // Compare on raw bytes -- slice equality never panics.
        let matched = read_back.as_bytes() == MARKER.as_bytes();
        let verdict: &[u8] = if matched { b"clip:ok" } else { b"clip:mismatch" };
        let _ = out.write(verdict);
        let _ = out.write(b"\n");

        let heading: &str = if matched {
            "The clipboard round-trip matched."
        } else {
            "The clipboard returned different text."
        };
        draw_and_wait(heading, MARKER.as_bytes(), read_back.as_bytes(), verdict)
    }
}

// ----- window -----

/// Draw the little report window and pump events until the window closes or the
/// round budget runs out. Returns the process exit code.
fn draw_and_wait(heading: &str, wrote: &[u8], read: &[u8], verdict: &[u8]) -> i32 {
    let size = types::WindowSize {
        width: WIDTH,
        height: HEIGHT,
    };
    let Ok(win) = window::create("Clipboard probe", size) else {
        return 30;
    };
    if window::show(win).is_err() {
        return 31;
    }
    if tree::set_root(win, &stack_root()).is_err()
        || tree::upsert_node(win, &title(heading)).is_err()
        || tree::upsert_node(win, &labelled_line(WROTE_ID, b"Wrote: ", wrote)).is_err()
        || tree::upsert_node(win, &labelled_line(READ_ID, b"Read:  ", read)).is_err()
        || tree::upsert_node(win, &labelled_line(VERDICT_ID, b"Result: ", verdict)).is_err()
    {
        let _ = window::close(win);
        return 32;
    }

    let raw = args::raw();
    let quick = raw
        .as_bytes()
        .split(|byte| *byte == b'\n')
        .next()
        .is_some_and(|first| first == b"quick");
    let rounds = if quick { QUICK_ROUNDS } else { MAX_ROUNDS };
    for _ in 0..rounds {
        match events::wait(Some(ROUND_MILLIS)) {
            Some(types::Event::CloseRequested(id)) if id == win => break,
            _ => {}
        }
    }

    let _ = window::close(win);
    0
}

// ----- widget builders -----

fn stack_root() -> types::WidgetNode {
    node(ROOT_ID, None, types::WidgetKind::Stack, None)
}

fn title(heading: &str) -> types::WidgetNode {
    let mut n = node(TITLE_ID, Some(ROOT_ID), types::WidgetKind::Text, Some(heading));
    n.role = Some(pure_string("heading"));
    n
}

/// A single text line built as `<prefix><value>` by hand, so no `format!`.
fn labelled_line(id: u64, prefix: &[u8], value: &[u8]) -> types::WidgetNode {
    let mut buf = [0u8; 128];
    let text = join(prefix, value, &mut buf);
    let mut n = node(id, Some(ROOT_ID), types::WidgetKind::Text, None);
    n.label = Some(pure_string_from_bytes(text));
    n.style.height = Some(36.0);
    n
}

fn join<'a>(prefix: &[u8], value: &[u8], buf: &'a mut [u8; 128]) -> &'a [u8] {
    let mut pos = 0usize;
    for byte in prefix {
        push(buf, &mut pos, *byte);
    }
    for byte in value {
        push(buf, &mut pos, *byte);
    }
    buf.get(..pos).unwrap_or(b"")
}

fn push(buf: &mut [u8], pos: &mut usize, byte: u8) {
    if let Some(slot) = buf.get_mut(*pos) {
        *slot = byte;
        *pos += 1;
    }
}

fn node(
    id: u64,
    parent: Option<u64>,
    kind: types::WidgetKind,
    label: Option<&str>,
) -> types::WidgetNode {
    types::WidgetNode {
        id,
        parent,
        kind,
        label: label.map(pure_string),
        role: None,
        style: types::Style {
            width: None,
            height: None,
            grow: 0.0,
            padding: 0.0,
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

// ----- pure-alloc string helpers (verbatim from krate-keyvault) -----

fn pure_string(text: &str) -> String {
    pure_string_from_bytes(text.as_bytes())
}

fn pure_string_from_bytes(bytes: &[u8]) -> String {
    let len = bytes.len();
    if len == 0 {
        return String::new();
    }
    unsafe {
        let layout = core::alloc::Layout::from_size_align_unchecked(len, 1);
        let ptr = std::alloc::alloc(layout);
        if ptr.is_null() {
            core::arch::wasm32::unreachable()
        }
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, len);
        String::from_raw_parts(ptr, len, len)
    }
}

bindings::export!(Component with_types_in bindings);
