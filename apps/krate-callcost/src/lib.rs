//! Callcost -- what does ONE host call cost, and where does the time go?
//!
//! K-227 measured ~138us per canvas call from `krate-bigtable` and divided a
//! whole frame by the number of calls in it. That number is right and it does
//! not say WHERE the time goes: it could be the drawing, the boundary
//! crossing, or the argument marshalling.
//!
//! The host's own timers say it is not the drawing. Over 60 frames of
//! bigtable, `KRATE_FRAME_STATS=1` reports fills 62.9ms, text 171.2ms and
//! measure 7.2ms -- about 6ms per frame of actual raster work against a frame
//! the guest measures at 40ms. Roughly 34ms per frame is spent somewhere
//! other than drawing.
//!
//! This app isolates it. Each band times a tight loop of ONE kind of call:
//!
//!   * `canvas-size`  -- reads a number, draws nothing. Pure boundary.
//!   * `fill-rect`    -- one tiny rect, 4x4. Boundary plus trivial raster.
//!   * `measure-text` -- short string, no drawing. Boundary plus shaping.
//!   * `draw-text`    -- the same short string, drawn.
//!
//! A `fill-rect` of 4x4 pixels cannot cost what a full-width one does, so the
//! difference between `canvas-size` and `fill-rect` is what a trivial draw
//! adds, and `canvas-size` on its own is the floor nothing can go under.
//!
//! `#![no_std]`, so this stays `krate:*`-only: fixed-size state, hand-rolled
//! number formatting, no `format!` and no panicking index.

#![no_std]

extern crate alloc;

use alloc::string::String;
use krate::bindings::krate::io::stdio;
use krate::gfx::{canvas2d, types as gfx};
use krate::ui::{events, tree, types, window};
use krate::bindings::krate::time::clock;

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;
const WIDTH: f32 = 900.0;
const HEIGHT: f32 = 560.0;

/// Calls per band. Large enough that the timer's own resolution is noise and
/// small enough that the window still draws at a sane rate.
const REPS: u32 = 400;

const BG: gfx::Color = rgb(0.055, 0.063, 0.086);
const INK: gfx::Color = rgb(0.90, 0.92, 0.96);
const QUIET: gfx::Color = rgb(0.45, 0.49, 0.58);
const ACCENT: gfx::Color = rgb(0.42, 0.55, 0.98);
const WARN: gfx::Color = rgb(0.98, 0.45, 0.42);

const fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
}

fn text(canvas: u64, s: &str, x: f32, y: f32, size: f32, ink: gfx::Color) {
    let _ = canvas2d::draw_text(canvas, s, gfx::Point { x, y }, size, ink);
}

/// Append `n` to `out` without `format!`.
fn push_u32(mut n: u32, out: &mut String) {
    if n == 0 {
        out.push('0');
        return;
    }
    let mut digits = [0u8; 10];
    let mut len = 0usize;
    while n > 0 && len < digits.len() {
        digits[len] = b'0' + (n % 10) as u8;
        n /= 10;
        len += 1;
    }
    while len > 0 {
        len -= 1;
        out.push(digits[len] as char);
    }
}

/// Append `n` with one decimal place, from a value in tenths.
fn push_tenths(tenths: u32, out: &mut String) {
    push_u32(tenths / 10, out);
    out.push('.');
    push_u32(tenths % 10, out);
}

/// Nanoseconds for `REPS` calls, as nanoseconds PER call.
fn per_call_ns(total_ns: u64) -> u32 {
    (total_ns / REPS as u64) as u32
}

struct Band {
    name: &'static str,
    /// Nanoseconds for one call.
    ns: u32,
}

/// Time `REPS` calls of each kind. Returns the four bands.
///
/// Every loop body uses its result, so nothing here can be optimised away as
/// dead: a host call has a side effect the compiler cannot see through, but
/// the arithmetic around it could otherwise vanish.
fn measure(canvas: u64) -> [Band; 5] {
    // 1. canvas-size: reads two numbers, draws nothing.
    let t0 = clock::monotonic_nanos();
    let mut sink = 0f32;
    for _ in 0..REPS {
        if let Ok(size) = canvas2d::canvas_size(canvas) {
            sink += size.width;
        }
    }
    let size_ns = per_call_ns(clock::monotonic_nanos().saturating_sub(t0));

    // 2. fill-rect, 4x4 pixels: the smallest real draw there is.
    let t0 = clock::monotonic_nanos();
    for i in 0..REPS {
        let x = (i % 200) as f32;
        let _ = canvas2d::fill_rect(
            canvas,
            gfx::Rect { x, y: HEIGHT - 6.0, width: 4.0, height: 4.0 },
            BG,
        );
    }
    let fill_ns = per_call_ns(clock::monotonic_nanos().saturating_sub(t0));

    // 3. measure-text: shaping, no raster.
    let t0 = clock::monotonic_nanos();
    for _ in 0..REPS {
        if let Ok(m) = canvas2d::measure_text(canvas, "sample", 12.0) {
            sink += m.width;
        }
    }
    let measure_ns = per_call_ns(clock::monotonic_nanos().saturating_sub(t0));

    // 4. draw-text: the same string, rastered off the visible area.
    let t0 = clock::monotonic_nanos();
    for _ in 0..REPS {
        let _ = canvas2d::draw_text(
            canvas,
            "sample",
            gfx::Point { x: 4.0, y: HEIGHT - 2.0 },
            12.0,
            BG,
        );
    }
    let text_ns = per_call_ns(clock::monotonic_nanos().saturating_sub(t0));

    // Keep `sink` alive so the reads above cannot be treated as dead.
    if sink < 0.0 {
        let out = stdio::stdout();
        let _ = out.write(b"unreachable\n");
    }

    // 5. draw-text with a DIFFERENT string each time: the cache-miss path.
    //
    // Band 4 draws one repeated string, so every call after the first is a
    // blend of cached coverage. A real table draws a different string in every
    // cell. The gap between the two is what rasterising actually costs, and it
    // is the number that decides whether batching the CALLS would help.
    let t0 = clock::monotonic_nanos();
    let mut buf = String::new();
    for i in 0..REPS {
        buf.clear();
        buf.push_str("row ");
        push_u32(i, &mut buf);
        let _ = canvas2d::draw_text(
            canvas,
            &buf,
            gfx::Point { x: 4.0, y: HEIGHT - 2.0 },
            12.0,
            BG,
        );
    }
    let fresh_ns = per_call_ns(clock::monotonic_nanos().saturating_sub(t0));

    [
        Band { name: "canvas-size  (no drawing at all)", ns: size_ns },
        Band { name: "fill-rect    (4x4 pixels)", ns: fill_ns },
        Band { name: "measure-text (\"sample\", no raster)", ns: measure_ns },
        Band { name: "draw-text    (same string, cached)", ns: text_ns },
        Band { name: "draw-text    (a NEW string each call)", ns: fresh_ns },
    ]
}

fn draw(canvas: u64, bands: &[Band; 5]) {
    let _ = canvas2d::clear(canvas, BG);
    text(canvas, "What does one host call cost?", 40.0, 56.0, 22.0, INK);

    let mut sub = String::new();
    push_u32(REPS, &mut sub);
    sub.push_str(" calls per band, timed by the guest");
    text(canvas, &sub, 40.0, 84.0, 13.0, QUIET);

    // The floor: whatever canvas-size costs is what no call can go under.
    let floor = bands[0].ns;

    let mut y = 150.0;
    for band in bands {
        text(canvas, band.name, 40.0, y, 14.0, INK);

        let mut line = String::new();
        push_tenths(band.ns / 100, &mut line);
        line.push_str(" us");
        text(canvas, &line, 470.0, y, 14.0, ACCENT);

        // How much of this call is NOT the boundary.
        let mut work = String::new();
        if band.ns > floor {
            work.push_str("boundary + ");
            push_tenths((band.ns - floor) / 100, &mut work);
            work.push_str(" us of work");
        } else {
            work.push_str("the floor: boundary only");
        }
        text(canvas, &work, 580.0, y, 12.0, QUIET);

        // A bar, so the shape is readable without reading the numbers.
        let widest = bands.iter().map(|b| b.ns).max().unwrap_or(1).max(1);
        let w = 380.0 * (band.ns as f32 / widest as f32);
        let _ = canvas2d::fill_round_rect(
            canvas,
            gfx::Rect { x: 40.0, y: y + 10.0, width: w, height: 8.0 },
            gfx::CornerRadii { top_left: 4.0, top_right: 4.0, bottom_right: 4.0, bottom_left: 4.0 },
            ACCENT,
        );
        y += 62.0;
    }

    // What it means for a frame.
    let mut budget = String::new();
    budget.push_str("a 60fps frame is 16,666 us, so it affords about ");
    // Use the text call, the one every text-heavy app is made of.
    let per = bands[4].ns.max(1);
    push_u32(16_666_000 / per, &mut budget);
    budget.push_str(" draw-text calls");
    text(canvas, &budget, 40.0, y + 16.0, 14.0, WARN);

    let mut floor_line = String::new();
    floor_line.push_str("and the floor under every one of them is ");
    push_tenths(floor / 100, &mut floor_line);
    floor_line.push_str(" us of boundary, drawing nothing");
    text(canvas, &floor_line, 40.0, y + 44.0, 13.0, QUIET);

    let _ = canvas2d::present(canvas);
}

fn say(line: &str) {
    let out = stdio::stdout();
    let _ = out.write(line.as_bytes());
    let _ = out.write(b"\n");
    let _ = out.flush();
}

fn report(bands: &[Band; 5]) {
    for band in bands {
        let mut line = String::new();
        line.push_str("callcost: ");
        line.push_str(band.name);
        line.push_str(" = ");
        push_tenths(band.ns / 100, &mut line);
        line.push_str(" us");
        say(&line);
    }
    let mut line = String::new();
    line.push_str("callcost: floor ");
    push_tenths(bands[0].ns / 100, &mut line);
    line.push_str(" us; draw-text calls per 60fps frame ");
    push_u32(16_666_000 / bands[4].ns.max(1), &mut line);
    say(&line);
}

struct Component;

impl krate::Guest for Component {
    fn run() -> i32 {
        let win = match window::create(
            "Call cost",
            types::WindowSize { width: WIDTH as u32, height: HEIGHT as u32 },
        ) {
            Ok(win) => win,
            Err(_) => return 1,
        };

        let root = types::WidgetNode {
            id: ROOT_ID,
            parent: None,
            kind: types::WidgetKind::Stack,
            label: None,
            role: None,
            style: types::Style {
                width: None,
                height: None,
                grow: 1.0,
                padding: 0.0,
                text: None,
                place: None,
                box_: None,
            },
            checked: None,
            value: None,
            selected: None,
            text_cursor: None,
        };
        if tree::set_root(win, &root).is_err() {
            return 1;
        }
        let mut canvas_node = root.clone();
        canvas_node.id = CANVAS_ID;
        canvas_node.parent = Some(ROOT_ID);
        canvas_node.kind = types::WidgetKind::Canvas;
        if tree::upsert_node(win, &canvas_node).is_err() {
            return 1;
        }
        let canvas = match canvas2d::bind(win, CANVAS_ID) {
            Ok(canvas) => canvas,
            Err(_) => return 1,
        };

        // Measure once, on the first frame, then hold the result on screen.
        // Re-measuring every frame would make the numbers jitter for no gain:
        // the question is what a call costs, not how it varies.
        let bands = measure(canvas);
        report(&bands);
        draw(canvas, &bands);

        loop {
            match events::wait(Some(80)) {
                Some(types::Event::CloseRequested(_)) => break,
                Some(types::Event::RedrawRequested(_)) | Some(types::Event::Resized(_)) => {
                    draw(canvas, &bands);
                }
                _ => {}
            }
        }
        let _ = window::close(win);
        0
    }
}

krate::export!(Component);
