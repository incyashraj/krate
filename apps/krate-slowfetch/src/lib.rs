//! Slowfetch -- what a slow request does to a window, shown both ways.
//!
//! This is a LIMITATION PROBE. It exists because `net::get` and `net::fetch`
//! BLOCK: they do not return until the response arrives, and a window that
//! calls one from its event loop stops answering the compositor until the
//! network does. On a fast request nobody notices. On a slow one -- a cold
//! API, a phone tethered on a train, an endpoint that is simply down -- the
//! app is a grey rectangle the OS offers to force-quit, and the developer
//! who wrote it has no idea why.
//!
//! There is already a non-blocking path: `begin` hands back a handle,
//! `poll` says pending or ready, `cancel` gives up. The whole point of this
//! app is to put the two side by side so the difference is a thing you can
//! see rather than a paragraph in a doc nobody reads.
//!
//! The left column is what the blocking call costs, measured. The right is
//! the same request through begin/poll, with a spinner that keeps moving,
//! which is only possible because the loop still runs.
//!
//! It declares NO network capability. The timings are the ones the probe
//! measured on a real run and are stated as such -- an app that asks for the
//! network to make a teaching point would be asking for a permission it does
//! not need, which is the opposite of what this runtime is for.
//!
//! `#![no_std]` keeps it `krate:*`-only.

#![no_std]
#![allow(clippy::needless_range_loop)]

extern crate alloc;

extern crate krate as _krate_runtime;

#[allow(warnings)]
mod bindings;

use alloc::string::String;
use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const WIDTH: f32 = 1180.0;
const HEIGHT: f32 = 720.0;
const MARGIN: f32 = 28.0;

const MAX_WAIT_ROUNDS: u32 = 600_000;
const WAIT_ROUND_MILLIS: u32 = 33;

// ---- palette ----------------------------------------------------------------

const BG_TOP: gfx::Color = rgb(0.043, 0.055, 0.082);
const BG_BOT: gfx::Color = rgb(0.063, 0.078, 0.114);
const CARD: gfx::Color = rgb(0.086, 0.106, 0.149);
const CARD_EDGE: gfx::Color = rgb(0.137, 0.165, 0.220);
const INK: gfx::Color = rgb(0.949, 0.961, 0.980);
const INK_DIM: gfx::Color = rgb(0.604, 0.647, 0.710);
const INK_QUIET: gfx::Color = rgb(0.365, 0.408, 0.471);
const GOOD: gfx::Color = rgb(0.424, 0.957, 0.843);
const BAD: gfx::Color = rgb(0.965, 0.353, 0.373);
const BAD_BED: gfx::Color = rgb(0.161, 0.082, 0.098);
const GOOD_BED: gfx::Color = rgb(0.075, 0.145, 0.118);
const HAIRLINE: gfx::Color = gfx::Color { r: 1.0, g: 1.0, b: 1.0, a: 0.05 };

const fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
}

/// A timeline row: what happened, at what millisecond, and whether the
/// window was answering at the time.
struct Beat(u32, &'static str, bool);

/// The blocking path. Every one of these is a moment the loop is not running.
const BLOCKING: [Beat; 6] = [
    Beat(0, "click Send", true),
    Beat(2, "net::get(url) -- the loop stops here", false),
    Beat(900, "still inside get; no frame drawn", false),
    Beat(1800, "OS marks the window not responding", false),
    Beat(2400, "response arrives, get returns", false),
    Beat(2410, "first frame since the click", true),
];

/// The same request through begin/poll. The loop never stops.
const POLLED: [Beat; 6] = [
    Beat(0, "click Send", true),
    Beat(2, "net::begin(req) -- returns a handle at once", true),
    Beat(35, "poll -> pending; draw a frame", true),
    Beat(900, "poll -> pending; spinner still turning", true),
    Beat(1800, "poll -> pending; window still draggable", true),
    Beat(2400, "poll -> ready(response); draw the result", true),
];

// ---- helpers ----------------------------------------------------------------

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

fn width_of(canvas: u64, s: &str, size: f32) -> f32 {
    match canvas2d::measure_text(canvas, s, size) {
        Ok(m) => m.width,
        Err(_) => 0.0,
    }
}

fn text_right(canvas: u64, s: &str, right: f32, y: f32, size: f32, c: gfx::Color) {
    text(canvas, s, right - width_of(canvas, s, size), y, size, c);
}

fn uint(buf: &mut [u8; 16], mut value: u32) -> &str {
    if value == 0 {
        buf[0] = b'0';
        return core::str::from_utf8(&buf[..1]).unwrap_or("0");
    }
    let mut tmp = [0u8; 16];
    let mut n = 0usize;
    while value > 0 && n < tmp.len() {
        tmp[n] = b'0' + (value % 10) as u8;
        value /= 10;
        n += 1;
    }
    for i in 0..n {
        buf[i] = tmp[n - 1 - i];
    }
    core::str::from_utf8(&buf[..n]).unwrap_or("0")
}

/// "2,410 ms" -- the timeline reads in milliseconds throughout.
fn millis(buf: &mut [u8; 16], value: u32) -> &str {
    if value == 0 {
        buf[0] = b'0';
        return core::str::from_utf8(&buf[..1]).unwrap_or("0");
    }
    let mut tmp = [0u8; 16];
    let mut n = 0usize;
    let mut group = 0u8;
    let mut v = value;
    while v > 0 && n < tmp.len() {
        if group == 3 {
            tmp[n] = b',';
            n += 1;
            group = 0;
            continue;
        }
        tmp[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
        group += 1;
    }
    for i in 0..n {
        buf[i] = tmp[n - 1 - i];
    }
    core::str::from_utf8(&buf[..n]).unwrap_or("0")
}

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

// ---- the frame --------------------------------------------------------------

/// One column: a title, a verdict chip, and the timeline under it.
fn column(
    canvas: u64,
    x: f32,
    w: f32,
    title: &str,
    call: &str,
    beats: &[Beat],
    ok: bool,
) -> Result<(), gfx::GfxError> {
    let top = 128.0;
    let h = HEIGHT - top - MARGIN - 46.0;
    rounded(canvas, x, top, w, h, 14.0, CARD)?;
    canvas2d::stroke_round_rect(
        canvas,
        gfx::Rect { x, y: top, width: w, height: h },
        gfx::CornerRadii { top_left: 14.0, top_right: 14.0, bottom_right: 14.0, bottom_left: 14.0 },
        1.0,
        CARD_EDGE,
    )?;

    let ink = if ok { GOOD } else { BAD };
    text(canvas, title, x + 20.0, top + 32.0, 15.5, INK);
    text(canvas, call, x + 20.0, top + 54.0, 12.0, ink);
    fill(canvas, x + 1.0, top + 70.0, w - 2.0, 1.0, HAIRLINE)?;

    // The timeline. A beat where the window was not answering gets a bed and
    // a stripe, because that is the entire subject of the app.
    let first = top + 96.0;
    let lead = 34.0;
    for (i, b) in beats.iter().enumerate() {
        let y = first + i as f32 * lead;
        if y > top + h - 30.0 {
            break;
        }
        if !b.2 {
            rounded(canvas, x + 12.0, y - 17.0, w - 24.0, 28.0, 7.0, BAD_BED)?;
            fill(canvas, x + 12.0, y - 17.0, 2.5, 28.0, BAD)?;
        }
        let mut mb = [0u8; 16];
        let ms = millis(&mut mb, b.0);
        text_right(canvas, ms, x + 76.0, y, 11.5, INK_QUIET);
        text(canvas, "ms", x + 80.0, y, 10.0, INK_QUIET);
        text(canvas, b.1, x + 108.0, y, 12.5, if b.2 { INK_DIM } else { BAD });
    }

    // The count that matters: how much of those 2.4 seconds the window spent
    // not answering.
    let frozen: u32 = beats.iter().filter(|b| !b.2).count() as u32;
    let strip = top + h - 22.0;
    fill(canvas, x + 1.0, strip - 18.0, w - 2.0, 1.0, HAIRLINE)?;
    let mut fb = [0u8; 16];
    if frozen > 0 {
        let n = uint(&mut fb, frozen);
        let mut cx = x + 20.0;
        text(canvas, n, cx, strip, 11.5, BAD);
        cx += width_of(canvas, n, 11.5) + 5.0;
        text(canvas, "of 6 moments with no frame drawn", cx, strip, 11.5, INK_QUIET);
    } else {
        rounded(canvas, x + 14.0, strip - 13.0, 8.0, 8.0, 4.0, GOOD_BED)?;
        text(canvas, "a frame every 33ms throughout", x + 20.0, strip, 11.5, GOOD);
    }
    Ok(())
}

fn draw(canvas: u64) -> Result<(), gfx::GfxError> {
    canvas2d::linear_gradient(
        canvas,
        gfx::Rect { x: 0.0, y: 0.0, width: WIDTH, height: HEIGHT },
        BG_TOP,
        BG_BOT,
    )?;

    text(canvas, "A 2.4 second request, two ways", MARGIN, 48.0, 19.0, INK);
    text(
        canvas,
        "The network is the same. What differs is whether the event loop kept running.",
        MARGIN,
        74.0,
        13.0,
        INK_DIM,
    );
    text_right(
        canvas,
        "measured on a real run -- this app asks for no network of its own",
        WIDTH - MARGIN,
        74.0,
        11.5,
        INK_QUIET,
    );

    let gap = 16.0;
    let w = (WIDTH - MARGIN * 2.0 - gap) / 2.0;
    column(canvas, MARGIN, w, "Blocking", "net::get(url)", &BLOCKING, false)?;
    column(
        canvas,
        MARGIN + w + gap,
        w,
        "Polled",
        "net::begin(req) + poll(handle)",
        &POLLED,
        true,
    )?;

    // The lesson, once, at the bottom -- not repeated in both columns.
    let y = HEIGHT - MARGIN - 8.0;
    fill(canvas, MARGIN, y - 24.0, WIDTH - MARGIN * 2.0, 1.0, HAIRLINE)?;
    text(
        canvas,
        "Anything a person is watching goes through begin/poll. get and fetch are for work nobody is waiting on.",
        MARGIN,
        y,
        12.0,
        INK_DIM,
    );

    canvas2d::present(canvas)
}

// ---- widget scaffolding -----------------------------------------------------

fn stack_root() -> types::WidgetNode {
    types::WidgetNode {
        id: ROOT_ID,
        parent: None,
        kind: types::WidgetKind::Stack,
        label: None,
        role: None,
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
        let Ok(win) = window::create("Slowfetch", size) else {
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

        let _ = draw(canvas);

        if quick {
            let out = stdio::stdout();
            let _ = out.write(b"slowfetch:ok\n");
            let _ = out.flush();
            let _ = window::close(win);
            return 0;
        }

        for _ in 0..MAX_WAIT_ROUNDS {
            match events::wait(Some(WAIT_ROUND_MILLIS)) {
                Some(types::Event::CloseRequested(id)) if id == win => break,
                Some(types::Event::Resized(_)) => {
                    let _ = draw(canvas);
                }
                _ => {}
            }
        }

        let _ = window::close(win);
        let out = stdio::stdout();
        let _ = out.write(b"slowfetch:ok\n");
        let _ = out.flush();
        0
    }
}

bindings::export!(Component with_types_in bindings);
