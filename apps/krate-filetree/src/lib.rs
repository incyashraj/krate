//! Krate filetree — the limitation probe for hierarchy.
//!
//! The wall it tests: real apps show nested structure -- a file browser, an
//! outline, a settings tree. The widget set has a TreeView kind, but the drawn
//! painter treats it like a flat list, so the hierarchy has to come from the
//! app: nested rows, indented by depth. This probe builds a folder tree with
//! three levels of nesting and indents each row by its depth, to see whether a
//! believable tree comes out the other side. If nesting or indentation is
//! broken, the rows come out flat or overlapping.
//!
//! Every row is one Text widget whose label is padded with leading spaces for
//! its depth and prefixed with a folder or file glyph. No panic paths, only
//! `krate:*`.

#[allow(warnings)]
mod bindings;

use bindings::krate::io::{args, stdio};
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const TITLE_ID: u64 = 2;
const TREE_ID: u64 = 3;
const ROW_BASE_ID: u64 = 100;

const WIDTH: u32 = 400;
const HEIGHT: u32 = 460;

const QUICK_ROUNDS: u32 = 20;
const MAX_ROUNDS: u32 = 600;
const ROUND_MILLIS: u32 = 50;

/// One tree entry: display name, depth (0 = top), and whether it is a folder.
struct Entry {
    name: &'static str,
    depth: u8,
    folder: bool,
}

const fn e(name: &'static str, depth: u8, folder: bool) -> Entry {
    Entry { name, depth, folder }
}

/// A believable little project tree, three levels deep.
const ENTRIES: [Entry; 14] = [
    e("my-app", 0, true),
    e("src", 1, true),
    e("main.rs", 2, false),
    e("lib.rs", 2, false),
    e("ui", 2, true),
    e("window.rs", 3, false),
    e("widgets.rs", 3, false),
    e("assets", 1, true),
    e("icon.png", 2, false),
    e("logo.svg", 2, false),
    e("Cargo.toml", 1, false),
    e("README.md", 1, false),
    e("tests", 1, true),
    e("smoke.rs", 2, false),
];

struct Component;

impl bindings::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize {
            width: WIDTH,
            height: HEIGHT,
        };
        let Ok(win) = window::create("Files", size) else {
            return 30;
        };
        if window::show(win).is_err() {
            return 31;
        }
        if tree::set_root(win, &stack_root()).is_err()
            || tree::upsert_node(win, &title()).is_err()
            || tree::upsert_node(win, &tree_view()).is_err()
        {
            let _ = window::close(win);
            return 32;
        }

        let mut index = 0usize;
        while index < ENTRIES.len() {
            if let Some(entry) = ENTRIES.get(index) {
                let mut buf = [0u8; 64];
                let text = row_label(entry, &mut buf);
                if tree::upsert_node(win, &row(index, text)).is_err() {
                    break;
                }
            }
            index += 1;
        }

        let out = stdio::stdout();
        let _ = out.write(b"entries:");
        let _ = out.write(u32_slice(ENTRIES.len() as u32, &mut [0u8; 12]));
        let _ = out.write(b"\n");

        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");
        // A real session ends when the person closes the window, never
        // on a round count: 600 rounds x 50 ms quietly shut the window
        // after thirty seconds of use (K-092). `quick` keeps its bound
        // so a headless check can never hang.
        let rounds = if quick { QUICK_ROUNDS } else { u32::MAX };
        for _ in 0..rounds {
            match events::wait(Some(ROUND_MILLIS)) {
                Some(types::Event::CloseRequested(id)) if id == win => break,
                _ => {}
            }
        }

        let _ = window::close(win);
        0
    }
}

/// Build a row label: leading spaces for depth, a glyph, then the name.
fn row_label<'a>(entry: &Entry, buf: &'a mut [u8; 64]) -> &'a [u8] {
    let mut pos = 0usize;
    // Three spaces per level of depth. Space is the one indent the bitmap and
    // vello fonts both advance predictably.
    let indents = (entry.depth as usize) * 3;
    let mut i = 0usize;
    while i < indents {
        push(buf, &mut pos, b' ');
        i += 1;
    }
    // A folder gets a "[+]" marker, a file a "-" bullet, so the two read apart
    // without relying on color.
    let glyph: &[u8] = if entry.folder { b"[+] " } else { b" -  " };
    for byte in glyph {
        push(buf, &mut pos, *byte);
    }
    for byte in entry.name.as_bytes() {
        push(buf, &mut pos, *byte);
    }
    buf.get(..pos).unwrap_or(b"")
}

// ----- widget builders -----

fn stack_root() -> types::WidgetNode {
    node(ROOT_ID, None, types::WidgetKind::Stack, None)
}

fn title() -> types::WidgetNode {
    let mut n = node(
        TITLE_ID,
        Some(ROOT_ID),
        types::WidgetKind::Text,
        Some("Project files, three levels deep"),
    );
    n.role = Some(pure_string("heading"));
    n
}

fn tree_view() -> types::WidgetNode {
    let mut n = node(TREE_ID, Some(ROOT_ID), types::WidgetKind::TreeView, None);
    n.style.grow = 1.0;
    n.role = Some(pure_string("tree"));
    n
}

fn row(index: usize, text: &[u8]) -> types::WidgetNode {
    let mut n = node(
        ROW_BASE_ID + index as u64,
        Some(TREE_ID),
        types::WidgetKind::Text,
        None,
    );
    n.label = Some(pure_string_from_bytes(text));
    n.style.height = Some(26.0);
    n.role = Some(pure_string("treeitem"));
    n
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

// ----- byte helpers -----

fn push(buf: &mut [u8], pos: &mut usize, byte: u8) {
    if let Some(slot) = buf.get_mut(*pos) {
        *slot = byte;
        *pos += 1;
    }
}

fn u32_slice(value: u32, buf: &mut [u8; 12]) -> &[u8] {
    if value == 0 {
        if let Some(slot) = buf.get_mut(0) {
            *slot = b'0';
        }
        return buf.get(..1).unwrap_or(b"0");
    }
    let mut scratch = [0u8; 12];
    let mut n = value;
    let mut count = 0usize;
    while n > 0 && count < scratch.len() {
        if let Some(slot) = scratch.get_mut(count) {
            *slot = b'0' + (n % 10) as u8;
        }
        n /= 10;
        count += 1;
    }
    let mut pos = 0usize;
    let mut i = count;
    while i > 0 {
        i -= 1;
        if let (Some(src), Some(dst)) = (scratch.get(i), buf.get_mut(pos)) {
            *dst = *src;
            pos += 1;
        }
    }
    buf.get(..pos).unwrap_or(b"0")
}

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
