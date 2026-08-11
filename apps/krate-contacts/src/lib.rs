//! Krate contacts — a real SQL database behind a modern contact list.
//!
//! The wall it tests: key-value storage is enough for a counter, but a real
//! app has structured data it queries -- rows, columns, ordering, a WHERE.
//! This app keeps contacts in an actual SQLite table through `store.sql`:
//! it creates the table, seeds a few people the first time it runs, then queries
//! them back sorted by name and shows them. If SQL is faked or the query path
//! is broken, the list comes back empty and the header says so.
//!
//! The presentation is drawn entirely on one `gfx.canvas2d`: a considered dark
//! ground, a bold title with a live count, and one rounded card per contact --
//! a colored avatar disc with the initial, the name in bright ink, the email in
//! a muted tone. The host cannot style native widgets, so the whole UI is
//! painted pixel by pixel instead, at the level of the Nova reference.
//!
//! This app must be `#![no_std]`, and that is the whole reason it is a probe.
//! `store.sql`'s `query` returns a nested `list<row<list<value>>>`, and the
//! generated glue that lifts it uses `Vec::with_capacity`. In a `std`-linked
//! guest that call reaches std's allocation-error handler, which routes through
//! std's panic runtime and drags the entire `wasi:*` import set into an
//! otherwise pure component -- so the app fails to instantiate against the Krate
//! linker. Building `#![no_std]` lets the SDK own the allocator and a trapping
//! panic handler, so the same allocation path traps instead of leaking.
#![no_std]
extern crate alloc;

// Linked purely for its `no_std` runtime lang items -- the global allocator, the
// trapping panic handler, and the mem intrinsics -- which apply to the whole
// component. The GUI-world bindings this app calls are the generated `bindings`
// module below; the SDK crate carries only the CLI world, so we take the
// runtime from it here, not the API surface.
extern crate krate as _krate_runtime;

#[allow(warnings)]
mod bindings;

use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::store::sql;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const WIDTH: f32 = 460.0;
const HEIGHT: f32 = 512.0;

/// Up to this many contacts held for drawing. The seed is four; a real table
/// could hold more, and the fixed pool keeps the loop panic-free.
const MAX_CONTACTS: usize = 64;
const NAME_CAP: usize = 48;
const EMAIL_CAP: usize = 64;

const QUICK_ROUNDS: u32 = 4;
const MAX_ROUNDS: u32 = 600;
const ROUND_MILLIS: u32 = 50;

/// The people seeded on first run, so a fresh open shows a real table.
const SEED: [(&str, &str); 4] = [
    ("Ada Lovelace", "ada@analytical.engine"),
    ("Alan Turing", "alan@bombe.uk"),
    ("Grace Hopper", "grace@cobol.navy"),
    ("Katherine Johnson", "katherine@nasa.gov"),
];

/// A palette of avatar tints, chosen by the first letter so each person keeps a
/// stable, distinct color. Warm-to-cool spread, all readable on the dark ground.
const AVATARS: [(f32, f32, f32); 8] = [
    (0.36, 0.72, 1.0),  // sky blue
    (0.98, 0.45, 0.62), // rose
    (0.55, 0.85, 0.55), // green
    (0.98, 0.72, 0.35), // amber
    (0.72, 0.55, 0.98), // violet
    (0.40, 0.85, 0.82), // teal
    (0.98, 0.58, 0.40), // coral
    (0.62, 0.70, 0.85), // slate
];

/// One contact held for drawing: fixed-capacity byte buffers, no growing Vec.
#[derive(Clone, Copy)]
struct Contact {
    name: [u8; NAME_CAP],
    name_len: usize,
    email: [u8; EMAIL_CAP],
    email_len: usize,
}

const ZERO_CONTACT: Contact = Contact {
    name: [0u8; NAME_CAP],
    name_len: 0,
    email: [0u8; EMAIL_CAP],
    email_len: 0,
};

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

        // Query them back, sorted. This is the read path the whole probe rides.
        let result = match sql::query("SELECT name, email FROM contacts ORDER BY name", &[]) {
            Ok(result) => result,
            Err(_) => {
                let _ = out.write(b"sql:query-failed\n");
                return 43;
            }
        };

        // Copy the queried rows into fixed byte buffers for drawing. Done once,
        // up front, so the render loop never touches the SQL result again.
        let mut contacts = [ZERO_CONTACT; MAX_CONTACTS];
        let mut n = 0usize;
        let mut index = 0usize;
        while index < result.rows.len() && n < MAX_CONTACTS {
            if let Some(row) = result.rows.get(index) {
                let mut c = ZERO_CONTACT;
                if let Some(sql::Value::Text(name)) = row.values.first() {
                    c.name_len = copy_into(&mut c.name, name.as_bytes());
                }
                if let Some(sql::Value::Text(email)) = row.values.get(1) {
                    c.email_len = copy_into(&mut c.email, email.as_bytes());
                }
                if let Some(slot) = contacts.get_mut(n) {
                    *slot = c;
                    n += 1;
                }
            }
            index += 1;
        }
        let row_count = result.rows.len();

        // Report the count on stdout so a script can assert the round trip.
        let _ = out.write(b"contacts:");
        let _ = out.write(u64_slice(row_count as u64, &mut [0u8; 20]));
        let _ = out.write(b"\n");

        // ---- window + canvas ----
        let size = types::WindowSize {
            width: WIDTH as u32,
            height: HEIGHT as u32,
        };
        let Ok(win) = window::create("Contacts", size) else {
            return 30;
        };
        if window::show(win).is_err() {
            return 31;
        }
        if tree::set_root(win, &stack_root()).is_err()
            || tree::upsert_node(win, &canvas_node()).is_err()
        {
            let _ = window::close(win);
            return 32;
        }
        let canvas = match canvas2d::bind(win, CANVAS_ID) {
            Ok(c) => c,
            Err(_) => {
                let _ = window::close(win);
                return 33;
            }
        };

        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");

        // Scroll offset in pixels. The seed set fits, so it starts at zero and
        // the list is fully visible.
        let scroll = 0.0f32;
        let _ = draw(canvas, &contacts, n, row_count, scroll);

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
            // Redraw so a resize or expose keeps a clean frame.
            let _ = draw(canvas, &contacts, n, row_count, scroll);
        }

        let _ = window::close(win);
        0
    }
}

// ------------------------------------------------------------------
// Rendering
// ------------------------------------------------------------------

const CARD_X: f32 = 24.0;
const CARD_W: f32 = WIDTH - 48.0;
const CARD_H: f32 = 68.0;
const CARD_GAP: f32 = 12.0;
const LIST_TOP: f32 = 128.0;

fn draw(
    canvas: u64,
    contacts: &[Contact; MAX_CONTACTS],
    n: usize,
    total: usize,
    scroll: f32,
) -> Result<(), gfx::GfxError> {
    // Deep, considered ground: one native vertical gradient from a faint-blue
    // top to a darker floor. No flat black, no widget grey.
    canvas2d::linear_gradient(
        canvas,
        gfx::Rect {
            x: 0.0,
            y: 0.0,
            width: WIDTH,
            height: HEIGHT,
        },
        color(0.078, 0.090, 0.145, 1.0),
        color(0.043, 0.051, 0.086, 1.0),
    )?;

    // A soft glow up top-left gives the header some depth.
    canvas2d::radial_gradient(
        canvas,
        gfx::Point { x: 90.0, y: 40.0 },
        260.0,
        color(0.30, 0.45, 0.90, 0.16),
        color(0.30, 0.45, 0.90, 0.0),
    )?;

    // ---- header ----
    draw_text(canvas, "Contacts", 24.0, 58.0, 34.0, color(0.96, 0.97, 1.0, 1.0))?;

    // Subtitle with a live count and an accent dot.
    let mut buf = [0u8; 40];
    let sub = count_bytes(total as u64, &mut buf);
    if let Ok(txt) = core::str::from_utf8(sub) {
        disc(canvas, 30.0, 80.0, 3.0, color(0.36, 0.72, 1.0, 1.0))?;
        draw_text(canvas, txt, 42.0, 86.0, 15.0, color(0.60, 0.68, 0.85, 1.0))?;
    }

    // A hairline divider under the header.
    canvas2d::fill_rect(
        canvas,
        gfx::Rect {
            x: 24.0,
            y: 104.0,
            width: CARD_W,
            height: 1.0,
        },
        color(1.0, 1.0, 1.0, 0.07),
    )?;

    if n == 0 {
        // Empty state: honest, styled, not a blank pane.
        draw_text(
            canvas,
            "No contacts came back from SQLite.",
            24.0,
            LIST_TOP + 30.0,
            16.0,
            color(0.75, 0.55, 0.55, 1.0),
        )?;
        canvas2d::present(canvas)?;
        return Ok(());
    }

    // ---- contact cards ----
    let mut i = 0usize;
    while i < n {
        if let Some(c) = contacts.get(i) {
            let y = LIST_TOP + (i as f32) * (CARD_H + CARD_GAP) - scroll;
            // Cull cards fully off-screen.
            if y + CARD_H > 96.0 && y < HEIGHT {
                draw_card(canvas, c, y)?;
            }
        }
        i += 1;
    }

    canvas2d::present(canvas)?;
    Ok(())
}

fn draw_card(canvas: u64, c: &Contact, y: f32) -> Result<(), gfx::GfxError> {
    // The card: a rounded panel a touch lighter than the ground, with a very
    // soft shadow beneath so it lifts off the background.
    round_rect(
        canvas,
        CARD_X,
        y + 3.0,
        CARD_W,
        CARD_H,
        16.0,
        color(0.0, 0.0, 0.0, 0.22),
    )?;
    round_rect(
        canvas,
        CARD_X,
        y,
        CARD_W,
        CARD_H,
        16.0,
        color(0.117, 0.133, 0.196, 1.0),
    )?;
    // A hair of top highlight for a lit edge.
    round_rect(canvas, CARD_X, y, CARD_W, 1.5, 16.0, color(1.0, 1.0, 1.0, 0.05))?;

    // Avatar disc: a color chosen by first initial, with a soft ring glow and
    // the initial centered on it.
    let name = c.name.get(..c.name_len).unwrap_or(&[]);
    let initial = first_initial(name);
    let (ar, ag, ab) = avatar_color(initial);
    let ax = CARD_X + 38.0;
    let ay = y + CARD_H * 0.5;
    // Glow halo.
    canvas2d::radial_gradient(
        canvas,
        gfx::Point { x: ax, y: ay },
        30.0,
        color(ar, ag, ab, 0.35),
        color(ar, ag, ab, 0.0),
    )?;
    disc(canvas, ax, ay, 20.0, color(ar, ag, ab, 1.0))?;
    let mut ibuf = [0u8; 1];
    if let Some(slot) = ibuf.get_mut(0) {
        *slot = initial;
    }
    if let Ok(itxt) = core::str::from_utf8(&ibuf) {
        // Single glyph, nudged from center so it sits on the disc.
        draw_text(canvas, itxt, ax - 6.0, ay + 6.0, 20.0, color(0.08, 0.09, 0.14, 1.0))?;
    }

    // Name in bright ink.
    let text_x = CARD_X + 74.0;
    if let Ok(nm) = core::str::from_utf8(name) {
        draw_text(canvas, nm, text_x, y + 30.0, 18.0, color(0.96, 0.97, 1.0, 1.0))?;
    }
    // Email in a muted tone.
    let email = c.email.get(..c.email_len).unwrap_or(&[]);
    if let Ok(em) = core::str::from_utf8(email) {
        draw_text(canvas, em, text_x, y + 52.0, 14.0, color(0.55, 0.62, 0.80, 1.0))?;
    }

    Ok(())
}

// ------------------------------------------------------------------
// Canvas helpers
// ------------------------------------------------------------------

/// A filled rounded rectangle: a center cross of two rects plus four corner
/// discs. One shape, panic-free, no per-pixel work.
fn round_rect(
    canvas: u64,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
    c: gfx::Color,
) -> Result<(), gfx::GfxError> {
    let r = if r * 2.0 > h { h * 0.5 } else { r };
    let r = if r * 2.0 > w { w * 0.5 } else { r };
    // Horizontal band (full width, inset top/bottom by r).
    canvas2d::fill_rect(
        canvas,
        gfx::Rect {
            x,
            y: y + r,
            width: w,
            height: h - 2.0 * r,
        },
        c,
    )?;
    // Vertical band (full height, inset left/right by r).
    canvas2d::fill_rect(
        canvas,
        gfx::Rect {
            x: x + r,
            y,
            width: w - 2.0 * r,
            height: h,
        },
        c,
    )?;
    // Four corner discs.
    disc(canvas, x + r, y + r, r, c)?;
    disc(canvas, x + w - r, y + r, r, c)?;
    disc(canvas, x + r, y + h - r, r, c)?;
    disc(canvas, x + w - r, y + h - r, r, c)?;
    Ok(())
}

fn disc(canvas: u64, cx: f32, cy: f32, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_circle(canvas, gfx::Point { x: cx, y: cy }, r, c)
}

fn draw_text(
    canvas: u64,
    text: &str,
    x: f32,
    y: f32,
    size: f32,
    c: gfx::Color,
) -> Result<(), gfx::GfxError> {
    canvas2d::draw_text(canvas, text, gfx::Point { x, y }, size, c)
}

fn color(r: f32, g: f32, b: f32, a: f32) -> gfx::Color {
    gfx::Color { r, g, b, a }
}

// ------------------------------------------------------------------
// Data helpers
// ------------------------------------------------------------------

/// The first column of the first row as an integer, for COUNT(*).
fn first_integer(result: &sql::QueryResult) -> i64 {
    match result.rows.first().and_then(|row| row.values.first()) {
        Some(sql::Value::Integer(n)) => *n,
        _ => 0,
    }
}

/// Copy `src` into `dst`, up to `dst.len()`, returning bytes written.
fn copy_into(dst: &mut [u8], src: &[u8]) -> usize {
    let mut i = 0usize;
    while i < src.len() && i < dst.len() {
        if let (Some(d), Some(s)) = (dst.get_mut(i), src.get(i)) {
            *d = *s;
        }
        i += 1;
    }
    i
}

/// The first ASCII letter of a name uppercased, or '?' if none.
fn first_initial(name: &[u8]) -> u8 {
    match name.first() {
        Some(&b) if b.is_ascii_lowercase() => b - 32,
        Some(&b) if b.is_ascii_uppercase() => b,
        Some(&b) if b.is_ascii_alphanumeric() => b,
        _ => b'?',
    }
}

/// A stable avatar tint chosen from the initial.
fn avatar_color(initial: u8) -> (f32, f32, f32) {
    let idx = (initial as usize) % AVATARS.len();
    AVATARS.get(idx).copied().unwrap_or((0.5, 0.6, 0.8))
}

/// "4 contacts in SQLite" / "1 contact in SQLite" from a count.
fn count_bytes(count: u64, buf: &mut [u8; 40]) -> &[u8] {
    let mut pos = 0usize;
    let mut num = [0u8; 20];
    for byte in u64_slice(count, &mut num) {
        push(buf, &mut pos, *byte);
    }
    let tail: &[u8] = if count == 1 {
        b" contact in SQLite"
    } else {
        b" contacts in SQLite"
    };
    for byte in tail {
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

// ------------------------------------------------------------------
// Widget builders (root stack + one canvas filling it)
// ------------------------------------------------------------------

fn stack_root() -> types::WidgetNode {
    node(ROOT_ID, None, types::WidgetKind::Stack)
}

fn canvas_node() -> types::WidgetNode {
    node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas)
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
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

// ------------------------------------------------------------------
// pure-alloc string helper (for the bound SQL text parameters)
// ------------------------------------------------------------------

use alloc::string::String;

fn pure_string(text: &str) -> String {
    let bytes = text.as_bytes();
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
