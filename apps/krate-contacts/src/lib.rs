//! Krate contacts — the limitation probe for a real database.
//!
//! The wall it tests: key-value storage is enough for a counter, but a real
//! app has structured data it queries -- rows, columns, ordering, a WHERE.
//! This app keeps contacts in an actual SQLite table through `store.sql`:
//! it creates the table, seeds a few people the first time it runs, then queries
//! them back sorted by name and shows them. If SQL is faked or the query path
//! is broken, the list comes back empty and the status line says so.
//!
//! Parameters are bound, never pasted into the statement, which is the point
//! of the typed value set. Text out of the rows is turned into widget labels
//! without a panic path, so the component imports only `krate:*`.
//!
//! This app must be `#![no_std]`, and that is the whole reason it is a probe.
//! `store.sql`'s `query` returns a nested `list<row<list<value>>>`, and the
//! generated glue that lifts it uses `Vec::with_capacity`. In a `std`-linked
//! guest that call reaches std's allocation-error handler, which routes through
//! std's panic runtime and drags the entire `wasi:*` import set into an
//! otherwise pure component -- so the app fails to instantiate against the Krate
//! linker. Building `#![no_std]` lets the SDK own the allocator and a trapping
//! panic handler, so the same allocation path traps instead of leaking. This is
//! the same fix every other Krate guest uses; the database result type is just
//! the first one whose *generated* lift forces the growable-`Vec` path an app
//! cannot hand-write its way around.
#![no_std]
extern crate alloc;

// Linked purely for its `no_std` runtime lang items -- the global allocator, the
// trapping panic handler, and the mem intrinsics -- which apply to the whole
// component. The GUI-world bindings this app calls are the generated `bindings`
// module below; the SDK crate carries only the CLI world, so we take the
// runtime from it here, not the API surface.
extern crate krate as _krate_runtime;

use alloc::string::String;

#[allow(warnings)]
mod bindings;

use bindings::krate::io::{args, stdio};
use bindings::krate::store::sql;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const TITLE_ID: u64 = 2;
const STATUS_ID: u64 = 3;
const LIST_ID: u64 = 4;
/// Contact rows start here.
const ROW_BASE_ID: u64 = 100;

const WIDTH: u32 = 420;
const HEIGHT: u32 = 380;

const QUICK_ROUNDS: u32 = 20;
const MAX_ROUNDS: u32 = 600;
const ROUND_MILLIS: u32 = 50;

/// The people seeded on first run, so a fresh open shows a real table.
const SEED: [(&str, &str); 4] = [
    ("Ada Lovelace", "ada@analytical.engine"),
    ("Alan Turing", "alan@bombe.uk"),
    ("Grace Hopper", "grace@cobol.navy"),
    ("Katherine Johnson", "katherine@nasa.gov"),
];

struct Component;

impl bindings::Guest for Component {
    fn run() -> i32 {
        let out = stdio::stdout();

        // Create the table if it is not there yet. execute returns the rows
        // affected; a schema statement affects none, which is fine.
        if sql::execute(
            "CREATE TABLE IF NOT EXISTS contacts (id INTEGER PRIMARY KEY, name TEXT NOT NULL, email TEXT NOT NULL)",
            &[],
        )
        .is_err()
        {
            let _ = out.write(b"sql:create-failed\n");
            return 40;
        }

        // Seed once: only insert when the table is empty, so reopening does not
        // pile up duplicates. A COUNT(*) query proves the read path too.
        let count = match sql::query("SELECT COUNT(*) FROM contacts", &[]) {
            Ok(result) => first_integer(&result),
            Err(_) => {
                let _ = out.write(b"sql:count-failed\n");
                return 41;
            }
        };
        if count == 0 {
            // Insert the seed people one per statement, bound parameters.
            let mut i = 0usize;
            while i < SEED.len() {
                let Some(&(name, email)) = SEED.get(i) else {
                    break;
                };
                if sql::execute(
                    "INSERT INTO contacts (name, email) VALUES (?, ?)",
                    &[
                        sql::Value::Text(pure_string(name)),
                        sql::Value::Text(pure_string(email)),
                    ],
                )
                .is_err()
                {
                    let _ = out.write(b"sql:insert-failed\n");
                    return 42;
                }
                i += 1;
            }
        }

        // Query them back, sorted, and build the list.
        let result = match sql::query(
            "SELECT name, email FROM contacts ORDER BY name",
            &[],
        ) {
            Ok(result) => result,
            Err(_) => {
                let _ = out.write(b"sql:query-failed\n");
                return 43;
            }
        };

        let size = types::WindowSize {
            width: WIDTH,
            height: HEIGHT,
        };
        let Ok(win) = window::create("Contacts", size) else {
            return 30;
        };
        if window::show(win).is_err() {
            return 31;
        }
        if tree::set_root(win, &stack_root()).is_err()
            || tree::upsert_node(win, &title()).is_err()
        {
            let _ = window::close(win);
            return 32;
        }

        // Status line reports the row count queried back.
        let row_count = result.rows.len();
        let mut sbuf = [0u8; 40];
        let status_text = status_bytes(row_count as u64, &mut sbuf);
        if tree::upsert_node(win, &status_from_bytes(status_text)).is_err()
            || tree::upsert_node(win, &contact_list()).is_err()
        {
            let _ = window::close(win);
            return 32;
        }

        // One row per contact: "Name  <email>".
        let mut index = 0usize;
        while index < result.rows.len() {
            if let Some(row) = result.rows.get(index) {
                let mut buf = [0u8; 128];
                let text = row_label(row, &mut buf);
                if tree::upsert_node(win, &contact_row(index, text)).is_err() {
                    break;
                }
            }
            index += 1;
        }

        // Report the count on stdout so a script can assert the round trip.
        let _ = out.write(b"contacts:");
        let _ = out.write(u64_slice(row_count as u64, &mut [0u8; 20]));
        let _ = out.write(b"\n");

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

/// The first column of the first row as an integer, for COUNT(*).
fn first_integer(result: &sql::QueryResult) -> i64 {
    match result.rows.first().and_then(|row| row.values.first()) {
        Some(sql::Value::Integer(n)) => *n,
        _ => 0,
    }
}

/// Build "Name  <email>" from a two-column row.
fn row_label<'a>(row: &sql::Row, buf: &'a mut [u8; 128]) -> &'a [u8] {
    let mut pos = 0usize;
    if let Some(sql::Value::Text(name)) = row.values.first() {
        for byte in name.as_bytes() {
            push(buf, &mut pos, *byte);
        }
    }
    for byte in b"   " {
        push(buf, &mut pos, *byte);
    }
    if let Some(sql::Value::Text(email)) = row.values.get(1) {
        for byte in email.as_bytes() {
            push(buf, &mut pos, *byte);
        }
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
        Some("Contacts, kept in a real SQL table"),
    );
    n.role = Some(pure_string("heading"));
    n
}

fn status_from_bytes(text: &[u8]) -> types::WidgetNode {
    let mut n = node(STATUS_ID, Some(ROOT_ID), types::WidgetKind::Text, None);
    n.label = Some(pure_string_from_bytes(text));
    n.role = Some(pure_string("status"));
    n
}

fn contact_list() -> types::WidgetNode {
    let mut n = node(LIST_ID, Some(ROOT_ID), types::WidgetKind::Scroll, None);
    n.style.grow = 1.0;
    n.role = Some(pure_string("scrollarea"));
    n
}

fn contact_row(index: usize, text: &[u8]) -> types::WidgetNode {
    let mut n = node(
        ROW_BASE_ID + index as u64,
        Some(LIST_ID),
        types::WidgetKind::Text,
        None,
    );
    n.label = Some(pure_string_from_bytes(text));
    n.style.height = Some(28.0);
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

// ----- byte helpers -----

fn status_bytes(count: u64, buf: &mut [u8; 40]) -> &[u8] {
    let mut pos = 0usize;
    for byte in b"Queried " {
        push(buf, &mut pos, *byte);
    }
    let mut num = [0u8; 20];
    for byte in u64_slice(count, &mut num) {
        push(buf, &mut pos, *byte);
    }
    let tail: &[u8] = if count == 1 {
        b" row back from SQLite"
    } else {
        b" rows back from SQLite"
    };
    for byte in tail {
        push(buf, &mut pos, *byte);
    }
    buf.get(..pos).unwrap_or(b"Queried")
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
        let ptr = alloc::alloc::alloc(layout);
        if ptr.is_null() {
            #[cfg(target_arch = "wasm32")]
            core::arch::wasm32::unreachable();
            #[cfg(not(target_arch = "wasm32"))]
            unreachable!();
        }
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, len);
        String::from_raw_parts(ptr, len, len)
    }
}

bindings::export!(Component with_types_in bindings);
