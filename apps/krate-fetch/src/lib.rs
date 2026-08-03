//! Krate fetch — the limitation probe for networking.
//!
//! The wall it tests: can an app reach the network and show what it got? A
//! weather app, a feed reader, an API client -- all of it needs a real HTTP
//! round trip, not a stub. This app performs a GET and renders the response
//! body in a scrollable pane, plus a status line that reports success or the
//! exact error. If networking is faked or blocked, the status line says so
//! rather than the app pretending.
//!
//! The URL comes from the first app argument so a test can point it at a
//! localhost server and assert the bytes came back. Everything is byte work on
//! the response, no parsing library, only `krate:*` imports.

#[allow(warnings)]
mod bindings;

use bindings::krate::io::{args, stdio};
use bindings::krate::net::http_client;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const TITLE_ID: u64 = 2;
const STATUS_ID: u64 = 3;
const BODY_SCROLL_ID: u64 = 4;
/// Body lines start here.
const LINE_BASE_ID: u64 = 100;
/// How many lines of the response to show. Enough to prove real content
/// arrived without turning the probe into a full text viewer.
const MAX_LINES: usize = 12;

const WIDTH: u32 = 520;
const HEIGHT: u32 = 420;

const QUICK_ROUNDS: u32 = 20;
const MAX_ROUNDS: u32 = 600;
const ROUND_MILLIS: u32 = 50;

struct Component;

impl bindings::Guest for Component {
    fn run() -> i32 {
        // The URL is the first argument; without one there is nothing to
        // fetch, which is a usage error, not a crash.
        let raw = args::raw();
        let mut lines = raw.as_bytes().split(|byte| *byte == b'\n');
        let url_bytes = lines.next().unwrap_or(b"");
        let quick = lines.next().is_some_and(|second| second == b"quick");

        let size = types::WindowSize {
            width: WIDTH,
            height: HEIGHT,
        };
        let Ok(win) = window::create("Fetch", size) else {
            return 30;
        };
        if window::show(win).is_err() {
            return 31;
        }
        if tree::set_root(win, &stack_root()).is_err()
            || tree::upsert_node(win, &title()).is_err()
            || tree::upsert_node(win, &status("Fetching...")).is_err()
            || tree::upsert_node(win, &body_scroll()).is_err()
        {
            let _ = window::close(win);
            return 32;
        }

        let url = pure_string_from_bytes(url_bytes);
        let out = stdio::stdout();
        match http_client::get(&url) {
            Ok(body) => {
                let byte_count = body.len();
                // Show the first lines of the body.
                render_body(win, &body);
                let mut buf = [0u8; 40];
                let text = status_bytes(b"OK, ", byte_count as u64, b" bytes", &mut buf);
                let _ = tree::upsert_node(win, &status_from_bytes(text));
                let _ = out.write(b"fetch:ok:");
                let _ = out.write(u64_slice(byte_count as u64, &mut [0u8; 20]));
                let _ = out.write(b"\n");
            }
            Err(_) => {
                let _ = tree::upsert_node(win, &status("Fetch failed"));
                let _ = out.write(b"fetch:error\n");
            }
        }

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
}

/// Split the body into lines and lower the first `MAX_LINES` of them.
fn render_body(win: u64, body: &[u8]) {
    let mut line_index = 0usize;
    let mut start = 0usize;
    let mut i = 0usize;
    while i <= body.len() && line_index < MAX_LINES {
        let at_end = i == body.len();
        let is_newline = !at_end && body.get(i) == Some(&b'\n');
        if at_end || is_newline {
            let slice = body.get(start..i).unwrap_or(&[]);
            // Skip empty lines so the pane shows content, not gaps.
            if !slice.is_empty() {
                let _ = tree::upsert_node(win, &body_line(line_index, slice));
                line_index += 1;
            }
            start = i + 1;
        }
        i += 1;
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
        Some("Fetched over HTTP"),
    );
    n.role = Some(pure_string("heading"));
    n
}

fn status(text: &str) -> types::WidgetNode {
    let mut n = node(STATUS_ID, Some(ROOT_ID), types::WidgetKind::Text, Some(text));
    n.role = Some(pure_string("status"));
    n
}

fn status_from_bytes(text: &[u8]) -> types::WidgetNode {
    let mut n = status("");
    n.label = Some(pure_string_from_bytes(text));
    n
}

fn body_scroll() -> types::WidgetNode {
    let mut n = node(BODY_SCROLL_ID, Some(ROOT_ID), types::WidgetKind::Scroll, None);
    n.style.grow = 1.0;
    n.role = Some(pure_string("scrollarea"));
    n
}

fn body_line(index: usize, bytes: &[u8]) -> types::WidgetNode {
    // Clamp a very long line so one giant line does not blow the layout; a
    // viewer would wrap, but this probe only needs to prove content arrived.
    let shown = bytes.get(..bytes.len().min(120)).unwrap_or(bytes);
    let mut n = node(
        LINE_BASE_ID + index as u64,
        Some(BODY_SCROLL_ID),
        types::WidgetKind::Text,
        None,
    );
    n.label = Some(pure_string_from_bytes(shown));
    n.style.height = Some(24.0);
    n.role = Some(pure_string("text"));
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

// ----- byte helpers, panic-free -----

/// "OK, 1256 bytes" assembled from a prefix, a number, and a suffix.
fn status_bytes<'a>(
    prefix: &[u8],
    number: u64,
    suffix: &[u8],
    buf: &'a mut [u8; 40],
) -> &'a [u8] {
    let mut pos = 0usize;
    for byte in prefix {
        push(buf, &mut pos, *byte);
    }
    let mut num = [0u8; 20];
    for byte in u64_slice(number, &mut num) {
        push(buf, &mut pos, *byte);
    }
    for byte in suffix {
        push(buf, &mut pos, *byte);
    }
    buf.get(..pos).unwrap_or(b"OK")
}

fn push(buf: &mut [u8], pos: &mut usize, byte: u8) {
    if let Some(slot) = buf.get_mut(*pos) {
        *slot = byte;
        *pos += 1;
    }
}

fn u64_slice(value: u64, buf: &mut [u8; 20]) -> &[u8] {
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
