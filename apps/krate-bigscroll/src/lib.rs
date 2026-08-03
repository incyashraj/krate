//! Krate bigscroll — the limitation probe for large widget trees.
//!
//! The wall it tests: real apps have long lists. A file browser, a chat, a log
//! viewer -- hundreds of rows, scrolled, clipped. If the layout engine chokes
//! on a big tree, or scroll clipping only works for a handful of rows, this is
//! where it breaks. Five hundred rows in one Scroll container, each a real
//! widget the host lays out and clips.
//!
//! It also proves the count is honest: the app prints `rows:500` so a script
//! can assert the whole tree was built, not just that something scrolled into
//! view.

#[allow(warnings)]
mod bindings;

use bindings::krate::io::{args, stdio};
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const TITLE_ID: u64 = 2;
const SCROLL_ID: u64 = 3;
/// Rows start here so their ids never collide with the chrome above.
const ROW_BASE_ID: u64 = 100;
/// How many rows the list holds. Far past what fits, so most are clipped and
/// only scrolling reveals them.
const ROW_COUNT: u64 = 500;

const WIDTH: u32 = 360;
const HEIGHT: u32 = 520;

const QUICK_ROUNDS: u32 = 30;
const MAX_ROUNDS: u32 = 1200;
const ROUND_MILLIS: u32 = 40;

struct Component;

impl bindings::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize {
            width: WIDTH,
            height: HEIGHT,
        };
        let Ok(win) = window::create("500 rows", size) else {
            return 30;
        };
        if window::show(win).is_err() {
            return 31;
        }

        if tree::set_root(win, &stack_root()).is_err()
            || tree::upsert_node(win, &title()).is_err()
            || tree::upsert_node(win, &scroll_area()).is_err()
        {
            let _ = window::close(win);
            return 32;
        }

        // Build every row. The whole point of the probe is that this loop runs
        // five hundred times and the host lays out and clips all of it.
        let mut i = 0u64;
        while i < ROW_COUNT {
            if tree::upsert_node(win, &row(i)).is_err() {
                let _ = window::close(win);
                return 32;
            }
            i += 1;
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

        // Report the row count so a script can assert the whole tree existed.
        let out = stdio::stdout();
        let _ = out.write(b"rows:");
        let mut buf = [0u8; 20];
        let text = u64_to_bytes(ROW_COUNT, &mut buf);
        let _ = out.write(text);
        let _ = out.write(b"\n");

        let _ = window::close(win);
        0
    }
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
        Some("500 rows, scroll to see them all"),
    );
    n.role = Some(pure_string("heading"));
    n
}

fn scroll_area() -> types::WidgetNode {
    let mut n = node(SCROLL_ID, Some(ROOT_ID), types::WidgetKind::Scroll, None);
    n.style.grow = 1.0;
    n.role = Some(pure_string("scrollarea"));
    n
}

fn row(index: u64) -> types::WidgetNode {
    let mut buf = [0u8; 32];
    let text = row_label(index, &mut buf);
    let mut n = node(ROW_BASE_ID + index, Some(SCROLL_ID), types::WidgetKind::Text, None);
    n.label = Some(pure_string_from_bytes(text));
    n.style.height = Some(28.0);
    n.role = Some(pure_string("text"));
    n
}

/// "Row 42" without `format!`.
fn row_label(index: u64, buf: &mut [u8; 32]) -> &[u8] {
    let mut pos = 0usize;
    for byte in b"Row " {
        if let Some(slot) = buf.get_mut(pos) {
            *slot = *byte;
            pos += 1;
        }
    }
    let mut num = [0u8; 20];
    let digits = u64_to_bytes(index + 1, &mut num);
    for byte in digits {
        if let Some(slot) = buf.get_mut(pos) {
            *slot = *byte;
            pos += 1;
        }
    }
    buf.get(..pos).unwrap_or(b"Row")
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

fn u64_to_bytes(value: u64, buf: &mut [u8; 20]) -> &[u8] {
    if value == 0 {
        if let Some(slot) = buf.get_mut(0) {
            *slot = b'0';
        }
        return buf.get(..1).unwrap_or(b"0");
    }
    let mut scratch = [0u8; 20];
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
