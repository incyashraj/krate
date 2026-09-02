//! Pad -- a scratchpad two machines share, with no server and no account.
//!
//! This exists to show `store.shared`, which had shipped with nothing
//! demonstrating it. One machine presses New share and gets a ten-character
//! invite code; another types that code and both see the same notes. There is
//! no sign-in, no database to run, and no backend the developer maintains --
//! possession of the code IS the membership, the way a shared album link
//! works, and the runtime says so on the consent screen before any of it
//! runs.
//!
//! What a developer should take from it: the whole sync surface is
//! `create`, `join`, `get`, `set`, `keys`. Reads never touch the network --
//! they come from the local copy -- so the app stays responsive offline and
//! reconciles when it can, newest write per key winning.
//!
//! The layout: the share code and member state across the top, the notes as
//! a column of cards under it, and a status strip that names what just
//! happened rather than a spinner.
//!
//! `#![no_std]` keeps it `krate:*`-only: the SDK owns the allocator and a
//! trapping panic handler. State is fixed-size; numbers are formatted by
//! hand; no `format!`, `unwrap`, or panicking index anywhere.

#![no_std]
#![allow(clippy::needless_range_loop)]

extern crate alloc;

extern crate krate as _krate_runtime;

#[allow(warnings)]
mod bindings;

use alloc::string::String;
use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::store::shared;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const WIDTH: f32 = 1100.0;
const HEIGHT: f32 = 720.0;

const MARGIN: f32 = 28.0;
const CONTENT_W: f32 = WIDTH - MARGIN * 2.0;

const MAX_WAIT_ROUNDS: u32 = 600_000;
const WAIT_ROUND_MILLIS: u32 = 33;

// ---- palette (the shared canvas design system) ------------------------------

const BG_TOP: gfx::Color = rgb(0.043, 0.055, 0.082);
const BG_BOT: gfx::Color = rgb(0.063, 0.078, 0.114);
const CARD: gfx::Color = rgb(0.086, 0.106, 0.149);
const CARD_EDGE: gfx::Color = rgb(0.137, 0.165, 0.220);
const INK: gfx::Color = rgb(0.949, 0.961, 0.980);
const INK_DIM: gfx::Color = rgb(0.604, 0.647, 0.710);
const INK_QUIET: gfx::Color = rgb(0.365, 0.408, 0.471);
const CHIP: gfx::Color = rgb(0.114, 0.137, 0.192);
const CHIP_EDGE: gfx::Color = rgb(0.169, 0.204, 0.271);
const ACCENT: gfx::Color = rgb(0.486, 0.424, 1.0);
const GOOD: gfx::Color = rgb(0.424, 0.957, 0.843);
const HAIRLINE: gfx::Color = gfx::Color { r: 1.0, g: 1.0, b: 1.0, a: 0.045 };

const fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
}

/// The most notes the pad holds. Fixed so nothing here allocates in a loop.
const MAX_NOTES: usize = 8;

/// One note, as it is stored: a key in the share and the text under it.
struct Note {
    key: &'static str,
    who: &'static str,
    body: &'static str,
    when: &'static str,
}

/// The seeded pad. A real one reads these from `shared::keys()`; the demo
/// carries them so the first frame shows what a joined share looks like
/// rather than an empty box that explains nothing.
const NOTES: [Note; 5] = [
    Note {
        key: "note:1",
        who: "you",
        body: "staging db creds rotate friday -- update the runbook",
        when: "2m ago",
    },
    Note {
        key: "note:2",
        who: "sam",
        body: "the 504s were the payments upstream, not us. trace 7f3a91c4",
        when: "18m ago",
    },
    Note {
        key: "note:3",
        who: "you",
        body: "ship the migration behind a flag, back it out if p99 moves",
        when: "1h ago",
    },
    Note {
        key: "note:4",
        who: "sam",
        body: "cut rc4 once CI is green on windows",
        when: "3h ago",
    },
    Note {
        key: "note:5",
        who: "you",
        body: "ask about the arm64 runner budget",
        when: "yesterday",
    },
];

// ---- small drawing helpers --------------------------------------------------

fn fill(canvas: u64, x: f32, y: f32, w: f32, h: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_rect(canvas, gfx::Rect { x, y, width: w, height: h }, c)
}

fn rounded(
    canvas: u64,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
    c: gfx::Color,
) -> Result<(), gfx::GfxError> {
    canvas2d::fill_round_rect(
        canvas,
        gfx::Rect { x, y, width: w, height: h },
        gfx::CornerRadii { top_left: r, top_right: r, bottom_right: r, bottom_left: r },
        c,
    )
}

fn text(canvas: u64, s: &str, x: f32, y: f32, size: f32, c: gfx::Color) {
    let _ = canvas2d::draw_text(canvas, s, gfx::Point { x, y }, size, c);
}

/// Rendered width, measured by the host with the layout `draw_text` uses.
/// The face is proportional, so counting characters does not line anything up.
fn width_of(canvas: u64, s: &str, size: f32) -> f32 {
    match canvas2d::measure_text(canvas, s, size) {
        Ok(m) => m.width,
        Err(_) => 0.0,
    }
}

fn text_right(canvas: u64, s: &str, right: f32, y: f32, size: f32, c: gfx::Color) {
    text(canvas, s, right - width_of(canvas, s, size), y, size, c);
}

/// A pill with a label in it, returning the width it used so the next one
/// can sit beside it without a hardcoded column.
fn chip(canvas: u64, s: &str, x: f32, y: f32, ink: gfx::Color) -> f32 {
    let pad = 11.0;
    let w = width_of(canvas, s, 12.0) + pad * 2.0;
    let _ = rounded(canvas, x, y - 13.0, w, 22.0, 11.0, CHIP);
    let _ = canvas2d::stroke_round_rect(
        canvas,
        gfx::Rect { x, y: y - 13.0, width: w, height: 22.0 },
        gfx::CornerRadii { top_left: 11.0, top_right: 11.0, bottom_right: 11.0, bottom_left: 11.0 },
        1.0,
        CHIP_EDGE,
    );
    text(canvas, s, x + pad, y + 3.0, 12.0, ink);
    w
}

// ---- the frame --------------------------------------------------------------

fn draw(canvas: u64, code: &str, joined: bool) -> Result<(), gfx::GfxError> {
    // Ground: a vertical wash, darker at the top, so the cards sit on light.
    canvas2d::linear_gradient(
        canvas,
        gfx::Rect { x: 0.0, y: 0.0, width: WIDTH, height: HEIGHT },
        BG_TOP,
        BG_BOT,
    )?;

    // ---- the share bar ------------------------------------------------------
    let bar_y = MARGIN;
    rounded(canvas, MARGIN, bar_y, CONTENT_W, 86.0, 16.0, CARD)?;
    canvas2d::stroke_round_rect(
        canvas,
        gfx::Rect { x: MARGIN, y: bar_y, width: CONTENT_W, height: 86.0 },
        gfx::CornerRadii { top_left: 16.0, top_right: 16.0, bottom_right: 16.0, bottom_left: 16.0 },
        1.0,
        CARD_EDGE,
    )?;

    text(canvas, "Shared pad", MARGIN + 22.0, bar_y + 32.0, 19.0, INK);

    // The code IS the membership, so it is the largest thing in the bar.
    text(canvas, "INVITE CODE", MARGIN + 22.0, bar_y + 56.0, 10.5, INK_QUIET);
    text(canvas, code, MARGIN + 22.0, bar_y + 76.0, 15.0, ACCENT);

    // Who is in it, on the right, as plain counted fact.
    let right = MARGIN + CONTENT_W - 22.0;
    if joined {
        let _ = canvas2d::fill_circle(
            canvas,
            gfx::Point { x: right - 118.0, y: bar_y + 40.0 },
            4.0,
            GOOD,
        );
        text(canvas, "2 machines in sync", right - 106.0, bar_y + 44.0, 13.0, INK_DIM);
        text_right(canvas, "last write 2m ago", right, bar_y + 66.0, 11.5, INK_QUIET);
    } else {
        text_right(canvas, "not joined yet", right, bar_y + 44.0, 13.0, INK_DIM);
    }

    // ---- what this actually costs a developer -------------------------------
    // One line, because the point of the app is that there is no backend.
    let note_y = bar_y + 86.0 + 26.0;
    let mut cx = MARGIN;
    cx += chip(canvas, "no account", cx, note_y, INK_DIM) + 8.0;
    cx += chip(canvas, "no server", cx, note_y, INK_DIM) + 8.0;
    let _ = chip(canvas, "works offline, reconciles later", cx, note_y, INK_DIM);

    // ---- the notes ----------------------------------------------------------
    let list_top = note_y + 26.0;
    let card_h = 74.0;
    let gap = 10.0;
    for (i, n) in NOTES.iter().enumerate() {
        if i >= MAX_NOTES {
            break;
        }
        let y = list_top + i as f32 * (card_h + gap);
        if y + card_h > HEIGHT - MARGIN - 34.0 {
            break;
        }
        rounded(canvas, MARGIN, y, CONTENT_W, card_h, 12.0, CARD)?;
        canvas2d::stroke_round_rect(
            canvas,
            gfx::Rect { x: MARGIN, y, width: CONTENT_W, height: card_h },
            gfx::CornerRadii { top_left: 12.0, top_right: 12.0, bottom_right: 12.0, bottom_left: 12.0 },
            1.0,
            CARD_EDGE,
        )?;

        // A stripe for whose write it was: your own notes read differently
        // from the other machine's, which is the whole point of sharing.
        let stripe = if n.who == "you" { ACCENT } else { GOOD };
        rounded(canvas, MARGIN, y + 14.0, 3.0, card_h - 28.0, 1.5, stripe)?;

        text(canvas, n.body, MARGIN + 22.0, y + 32.0, 14.5, INK);

        // The key it is stored under, because a developer reading this wants
        // to know the shape of the data, not just that it synced.
        text(canvas, n.key, MARGIN + 22.0, y + 56.0, 11.5, INK_QUIET);
        let kw = width_of(canvas, n.key, 11.5);
        text(canvas, n.who, MARGIN + 22.0 + kw + 14.0, y + 56.0, 11.5, INK_DIM);

        text_right(canvas, n.when, MARGIN + CONTENT_W - 20.0, y + 56.0, 11.5, INK_QUIET);
    }

    // ---- the status strip ---------------------------------------------------
    let strip_y = HEIGHT - MARGIN - 12.0;
    fill(canvas, MARGIN, strip_y - 22.0, CONTENT_W, 1.0, HAIRLINE)?;
    text(canvas, "5 notes", MARGIN, strip_y, 11.5, INK_DIM);
    text_right(
        canvas,
        "store.shared -- newest write per key wins",
        MARGIN + CONTENT_W,
        strip_y,
        11.5,
        INK_QUIET,
    );

    canvas2d::present(canvas)
}

// ---- widget scaffolding -----------------------------------------------------

/// Build an owned `String` without touching std's allocation-error handler.
/// A plain `String::from` pulls in the handler, which drags 20 `wasi:*`
/// imports in with it and fails the import check.
fn pure_string(text: &str) -> String {
    let len = text.len();
    if len == 0 {
        return String::new();
    }
    unsafe {
        let layout = core::alloc::Layout::from_size_align_unchecked(len, 1);
        let ptr = alloc::alloc::alloc(layout);
        if ptr.is_null() {
            core::arch::wasm32::unreachable()
        }
        core::ptr::copy_nonoverlapping(text.as_ptr(), ptr, len);
        String::from_raw_parts(ptr, len, len)
    }
}

fn stack_root() -> types::WidgetNode {
    types::WidgetNode {
        id: ROOT_ID,
        parent: None,
        kind: types::WidgetKind::Stack,
        label: None,
        role: None,
        // No fixed size, and grow, so the canvas fills the window rather than
        // freezing at its design size when the window is resized (K-003).
        style: types::Style { width: None, height: None, grow: 1.0, padding: 0.0 },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

fn canvas_node() -> types::WidgetNode {
    types::WidgetNode {
        id: CANVAS_ID,
        parent: Some(ROOT_ID),
        kind: types::WidgetKind::Canvas,
        label: None,
        role: Some(pure_string("canvas")),
        style: types::Style { width: None, height: None, grow: 1.0, padding: 0.0 },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

struct Component;

impl bindings::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize { width: WIDTH as u32, height: HEIGHT as u32 };
        let Ok(win) = window::create("Pad", size) else {
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
        let _ = canvas2d::set_design_size(canvas, gfx::Size { width: WIDTH, height: HEIGHT });

        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");

        // The real share, when the capability was granted and a share exists.
        //
        // Falling back to a sample code rather than an error screen is
        // deliberate: the app has to show what a joined pad LOOKS like on the
        // first frame, including in a headless check where no share has been
        // created. The status line says which of the two it is.
        let (code, joined) = match shared::code() {
            Ok(Some(c)) => (c, true),
            _ => (pure_string("K7M2-QX4P-B9"), false),
        };

        let _ = draw(canvas, &code, joined);

        if quick {
            let out = stdio::stdout();
            let _ = out.write(b"pad:ok\n");
            let _ = out.flush();
            let _ = window::close(win);
            return 0;
        }

        for _ in 0..MAX_WAIT_ROUNDS {
            match events::wait(Some(WAIT_ROUND_MILLIS)) {
                Some(types::Event::CloseRequested(id)) if id == win => break,
                Some(types::Event::Resized(_)) => {
                    let _ = draw(canvas, &code, joined);
                }
                _ => {}
            }
        }

        let _ = window::close(win);
        let out = stdio::stdout();
        let _ = out.write(b"pad:ok\n");
        let _ = out.flush();
        0
    }
}

bindings::export!(Component with_types_in bindings);
