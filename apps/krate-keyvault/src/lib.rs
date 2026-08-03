//! Krate keyvault — the limitation probe for persistence.
//!
//! The wall it tests: can an app remember anything between runs? A note app, a
//! game with a high score, settings -- all of it is worthless if the data does
//! not survive the process. This app keeps a run counter in the key-value
//! store: it reads the count, adds one, saves it, and shows it. Run it three
//! times and it must read 1, then 2, then 3. If persistence is fake, the count
//! never moves off 1.
//!
//! The store holds bytes, so the counter is stored as its decimal text and
//! parsed back by hand -- no serialization library, no panic paths, only
//! `krate:*` imports.

#[allow(warnings)]
mod bindings;

use bindings::krate::io::{args, stdio};
use bindings::krate::store::kv;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const TITLE_ID: u64 = 2;
const COUNT_ID: u64 = 3;
const HINT_ID: u64 = 4;

const COUNT_KEY: &str = "run-count";
const WIDTH: u32 = 380;
const HEIGHT: u32 = 220;

const QUICK_ROUNDS: u32 = 20;
const MAX_ROUNDS: u32 = 600;
const ROUND_MILLIS: u32 = 50;

struct Component;

impl bindings::Guest for Component {
    fn run() -> i32 {
        // Read the saved count, or zero on the very first run. A missing key is
        // `Ok(None)`, which is the normal fresh-start case, not an error.
        let previous = match kv::get(COUNT_KEY) {
            Ok(Some(bytes)) => parse_u64(&bytes),
            Ok(None) => 0,
            Err(_) => {
                // Denied or unreadable store: report it and exit non-zero so a
                // test sees the failure rather than a silent zero.
                let out = stdio::stdout();
                let _ = out.write(b"store:unavailable\n");
                return 40;
            }
        };
        let count = previous + 1;

        // Save the new count before drawing, so the number on screen is the
        // number that persisted, not one the app only meant to save.
        let mut buf = [0u8; 20];
        let text = u64_to_bytes(count, &mut buf);
        if kv::set(COUNT_KEY, text).is_err() {
            let out = stdio::stdout();
            let _ = out.write(b"store:write-failed\n");
            return 41;
        }

        // Report the count so a script can assert it climbs across runs.
        let out = stdio::stdout();
        let _ = out.write(b"count:");
        let _ = out.write(text);
        let _ = out.write(b"\n");

        let size = types::WindowSize {
            width: WIDTH,
            height: HEIGHT,
        };
        let Ok(win) = window::create("Run counter", size) else {
            return 30;
        };
        if window::show(win).is_err() {
            return 31;
        }
        if tree::set_root(win, &stack_root()).is_err()
            || tree::upsert_node(win, &title()).is_err()
            || tree::upsert_node(win, &count_line(count)).is_err()
            || tree::upsert_node(win, &hint()).is_err()
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
        Some("This app remembers how many times you have opened it."),
    );
    n.role = Some(pure_string("heading"));
    n
}

fn count_line(count: u64) -> types::WidgetNode {
    // "Opened 3 times" built by hand.
    let mut buf = [0u8; 40];
    let text = count_label(count, &mut buf);
    let mut n = node(COUNT_ID, Some(ROOT_ID), types::WidgetKind::Text, None);
    n.label = Some(pure_string_from_bytes(text));
    n.style.height = Some(48.0);
    n.role = Some(pure_string("status"));
    n
}

fn hint() -> types::WidgetNode {
    node(
        HINT_ID,
        Some(ROOT_ID),
        types::WidgetKind::Text,
        Some("Close and open it again -- the number goes up and stays up."),
    )
}

fn count_label(count: u64, buf: &mut [u8; 40]) -> &[u8] {
    let mut pos = 0usize;
    for byte in b"Opened " {
        push(buf, &mut pos, *byte);
    }
    let mut num = [0u8; 20];
    for byte in u64_to_bytes(count, &mut num) {
        push(buf, &mut pos, *byte);
    }
    let tail: &[u8] = if count == 1 { b" time" } else { b" times" };
    for byte in tail {
        push(buf, &mut pos, *byte);
    }
    buf.get(..pos).unwrap_or(b"Opened")
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

// ----- byte<->number, panic-free -----

/// Parse a decimal byte string to a number, ignoring anything that is not a
/// digit. A corrupt value reads as zero rather than trapping.
fn parse_u64(bytes: &[u8]) -> u64 {
    let mut value = 0u64;
    for byte in bytes {
        if byte.is_ascii_digit() {
            value = value.saturating_mul(10).saturating_add(u64::from(byte - b'0'));
        }
    }
    value
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
