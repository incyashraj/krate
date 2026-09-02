//! Bigtable -- a hundred thousand rows, and an honest answer about what that
//! costs.
//!
//! This is a LIMITATION PROBE, not a showcase. Every other example is built
//! to look good; this one is built to find where the canvas gives up, and to
//! say so on its own face. It holds 100,000 rows, draws only the window that
//! fits, and times its own frame with `time::monotonic-nanos` -- so the
//! number on screen is measured by the app, in the app, rather than claimed
//! in a README.
//!
//! What it is actually testing:
//!
//! * **Draw calls per frame.** A row is five strings, so a 30-row window is
//!   150 `draw-text` calls plus the rules and stripes. If the canvas has a
//!   per-frame ceiling this is where a developer meets it.
//! * **Virtualised scrolling.** Only visible rows are drawn. Naively drawing
//!   100,000 rows is the mistake this app exists to make unnecessary, and
//!   the row budget is on screen so the technique is visible rather than
//!   implied.
//! * **Text measurement under load.** Every column is right- or left-aligned
//!   by measuring, not by guessing character widths -- the same thing that
//!   made check-layout wrong about punctuation until it started measuring.
//!
//! The verdict line reports the frame time and what it implies, including
//! when the answer is bad. An example that hides its own ceiling is worth
//! less than one that names it.
//!
//! `#![no_std]` keeps it `krate:*`-only: the SDK owns the allocator and a
//! trapping panic handler. Nothing here allocates per frame.

#![no_std]
#![allow(clippy::needless_range_loop)]

extern crate alloc;

extern crate krate as _krate_runtime;

#[allow(warnings)]
mod bindings;

use alloc::string::String;
use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::time::clock;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const WIDTH: f32 = 1240.0;
const HEIGHT: f32 = 780.0;
const MARGIN: f32 = 24.0;

const MAX_WAIT_ROUNDS: u32 = 600_000;
const WAIT_ROUND_MILLIS: u32 = 33;

/// The point of the app. Not a round number for show: it is the size at
/// which "just draw them all" stops being survivable, which is the lesson.
const TOTAL_ROWS: u32 = 100_000;

// ---- palette ----------------------------------------------------------------

const BG_TOP: gfx::Color = rgb(0.043, 0.055, 0.082);
const BG_BOT: gfx::Color = rgb(0.063, 0.078, 0.114);
const CARD: gfx::Color = rgb(0.086, 0.106, 0.149);
const CARD_EDGE: gfx::Color = rgb(0.137, 0.165, 0.220);
const INK: gfx::Color = rgb(0.949, 0.961, 0.980);
const INK_DIM: gfx::Color = rgb(0.604, 0.647, 0.710);
const INK_QUIET: gfx::Color = rgb(0.365, 0.408, 0.471);
const ZEBRA: gfx::Color = rgb(0.098, 0.118, 0.165);
const ACCENT: gfx::Color = rgb(0.486, 0.424, 1.0);
const GOOD: gfx::Color = rgb(0.424, 0.957, 0.843);
const WARN: gfx::Color = rgb(0.976, 0.694, 0.267);
const BAD: gfx::Color = rgb(0.965, 0.353, 0.373);
const HAIRLINE: gfx::Color = gfx::Color { r: 1.0, g: 1.0, b: 1.0, a: 0.05 };

const fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
}

// ---- the data ---------------------------------------------------------------

/// Row content is DERIVED, never stored: a hundred thousand rows of real
/// strings would be megabytes of bundle for no gain, and the point is the
/// drawing cost, not the storage. Each row's values come from its index, so
/// the table is deterministic and the memory is a handful of bytes.
const HOSTS: [&str; 8] = [
    "api-gateway", "orders", "payments", "auth", "search", "mailer", "images", "reports",
];
const STATUSES: [(&str, gfx::Color); 4] =
    [("200", GOOD), ("201", GOOD), ("404", WARN), ("500", BAD)];
const METHODS: [&str; 4] = ["GET", "POST", "PATCH", "DELETE"];

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

/// Unsigned integer into `buf`, returning the used slice.
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

/// With thousands separators: "100,000".
fn commas(buf: &mut [u8; 16], mut value: u32) -> &str {
    if value == 0 {
        buf[0] = b'0';
        return core::str::from_utf8(&buf[..1]).unwrap_or("0");
    }
    let mut tmp = [0u8; 16];
    let mut n = 0usize;
    let mut group = 0u8;
    while value > 0 && n < tmp.len() {
        if group == 3 {
            tmp[n] = b',';
            n += 1;
            group = 0;
            continue;
        }
        tmp[n] = b'0' + (value % 10) as u8;
        value /= 10;
        n += 1;
        group += 1;
    }
    for i in 0..n {
        buf[i] = tmp[n - 1 - i];
    }
    core::str::from_utf8(&buf[..n]).unwrap_or("0")
}

/// One decimal place: "3.4".
fn one_dp(buf: &mut [u8; 16], whole: u32, tenth: u32) -> &str {
    let mut a = [0u8; 16];
    let w = uint(&mut a, whole);
    let wl = w.len();
    buf[..wl].copy_from_slice(&a[..wl]);
    buf[wl] = b'.';
    buf[wl + 1] = b'0' + (tenth % 10) as u8;
    core::str::from_utf8(&buf[..wl + 2]).unwrap_or("0.0")
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

const ROW_H: f32 = 26.0;
const HEAD_H: f32 = 38.0;

/// Draw the table and return how long it took, in microseconds.
///
/// The measurement wraps the drawing and NOT the present: the app can only
/// account for its own work, and saying "the frame took N" when N includes
/// the compositor would be a claim it cannot support.
fn draw(canvas: u64, first_row: u32) -> (u32, u32) {
    let t0 = clock::monotonic_nanos();

    let _ = canvas2d::linear_gradient(
        canvas,
        gfx::Rect { x: 0.0, y: 0.0, width: WIDTH, height: HEIGHT },
        BG_TOP,
        BG_BOT,
    );

    // ---- the head -----------------------------------------------------------
    let mut cb = [0u8; 16];
    text(canvas, "Request log", MARGIN, 42.0, 18.0, INK);
    let total = commas(&mut cb, TOTAL_ROWS);
    let tw = width_of(canvas, total, 18.0);
    text(canvas, total, MARGIN + width_of(canvas, "Request log", 18.0) + 14.0, 42.0, 18.0, ACCENT);
    text(
        canvas,
        "rows",
        MARGIN + width_of(canvas, "Request log", 18.0) + 14.0 + tw + 7.0,
        42.0,
        18.0,
        INK_QUIET,
    );

    // ---- the table ----------------------------------------------------------
    let top = 70.0;
    let table_h = HEIGHT - top - MARGIN - 44.0;
    rounded(canvas, MARGIN, top, WIDTH - MARGIN * 2.0, table_h, 14.0, CARD).ok();
    let _ = canvas2d::stroke_round_rect(
        canvas,
        gfx::Rect { x: MARGIN, y: top, width: WIDTH - MARGIN * 2.0, height: table_h },
        gfx::CornerRadii { top_left: 14.0, top_right: 14.0, bottom_right: 14.0, bottom_left: 14.0 },
        1.0,
        CARD_EDGE,
    );

    // Columns, measured from the right so numbers stay in their own lane
    // whatever their width.
    let x_id = MARGIN + 18.0;
    let x_time = MARGIN + 96.0;
    let x_method = MARGIN + 196.0;
    let x_host = MARGIN + 272.0;
    let x_path = MARGIN + 400.0;
    let r_ms = WIDTH - MARGIN - 22.0;
    let r_status = r_ms - 92.0;

    text(canvas, "#", x_id, top + 25.0, 10.5, INK_QUIET);
    text(canvas, "TIME", x_time, top + 25.0, 10.5, INK_QUIET);
    text(canvas, "METHOD", x_method, top + 25.0, 10.5, INK_QUIET);
    text(canvas, "SERVICE", x_host, top + 25.0, 10.5, INK_QUIET);
    text(canvas, "PATH", x_path, top + 25.0, 10.5, INK_QUIET);
    text_right(canvas, "STATUS", r_status, top + 25.0, 10.5, INK_QUIET);
    text_right(canvas, "MS", r_ms, top + 25.0, 10.5, INK_QUIET);
    let _ = fill(canvas, MARGIN + 1.0, top + HEAD_H, WIDTH - MARGIN * 2.0 - 2.0, 1.0, HAIRLINE);

    // Only the rows that fit. This is the whole technique: the table is a
    // hundred thousand rows and the frame costs the same as thirty.
    let body_top = top + HEAD_H;
    let room = table_h - HEAD_H - 8.0;
    let visible = if room > 0.0 { (room / ROW_H) as u32 } else { 0 };

    for i in 0..visible {
        let n = first_row + i;
        if n >= TOTAL_ROWS {
            break;
        }
        let y = body_top + i as f32 * ROW_H;
        if i % 2 == 1 {
            let _ = fill(canvas, MARGIN + 1.0, y, WIDTH - MARGIN * 2.0 - 2.0, ROW_H, ZEBRA);
        }
        let base = y + 17.0;

        let mut nb = [0u8; 16];
        text_right(canvas, commas(&mut nb, n + 1), x_id + 52.0, base, 11.0, INK_QUIET);

        // A clock derived from the row, so the column is monotonic and looks
        // like a log rather than random noise.
        let secs = n % 60;
        let mins = (n / 60) % 60;
        let mut tb = [0u8; 16];
        let mut hh = [0u8; 16];
        let mm = uint(&mut tb, mins);
        let mml = mm.len();
        hh[0] = b'1';
        hh[1] = b'4';
        hh[2] = b':';
        if mml == 1 {
            hh[3] = b'0';
            hh[4] = tb[0];
        } else {
            hh[3] = tb[0];
            hh[4] = tb[1];
        }
        hh[5] = b':';
        let mut sb = [0u8; 16];
        let ss = uint(&mut sb, secs);
        if ss.len() == 1 {
            hh[6] = b'0';
            hh[7] = sb[0];
        } else {
            hh[6] = sb[0];
            hh[7] = sb[1];
        }
        if let Ok(s) = core::str::from_utf8(&hh[..8]) {
            text(canvas, s, x_time, base, 11.5, INK_QUIET);
        }

        let m = METHODS[(n % 4) as usize];
        text(canvas, m, x_method, base, 11.5, INK_DIM);

        let h = HOSTS[(n % 8) as usize];
        text(canvas, h, x_host, base, 12.0, INK_DIM);

        // The path carries the row number so a reader can see the table is
        // really scrolling and not redrawing the same thirty rows.
        let mut pb = [0u8; 16];
        let idn = uint(&mut pb, n * 7 % 99991);
        let px = x_path;
        text(canvas, "/v1/orders/", px, base, 12.0, INK);
        text(canvas, idn, px + width_of(canvas, "/v1/orders/", 12.0), base, 12.0, INK);

        let (code, col) = STATUSES[(n % 4) as usize];
        text_right(canvas, code, r_status, base, 11.5, col);

        let ms = 8 + (n * 13) % 340;
        let mut mb = [0u8; 16];
        text_right(canvas, uint(&mut mb, ms), r_ms, base, 11.5, INK_DIM);
    }

    let elapsed_ns = clock::monotonic_nanos().saturating_sub(t0);
    let micros = (elapsed_ns / 1_000) as u32;
    (micros, visible)
}

/// The verdict strip: what the number means, said plainly, including when it
/// is bad.
fn verdict(canvas: u64, micros: u32, visible: u32) {
    let y = HEIGHT - MARGIN - 8.0;
    let _ = fill(canvas, MARGIN, y - 26.0, WIDTH - MARGIN * 2.0, 1.0, HAIRLINE);

    let mut vb = [0u8; 16];
    let mut x = MARGIN;
    text(canvas, "drew", x, y, 11.5, INK_QUIET);
    x += width_of(canvas, "drew", 11.5) + 6.0;
    let v = uint(&mut vb, visible);
    text(canvas, v, x, y, 11.5, INK);
    x += width_of(canvas, v, 11.5) + 6.0;
    let mut tb = [0u8; 16];
    let of = commas(&mut tb, TOTAL_ROWS);
    text(canvas, "of", x, y, 11.5, INK_QUIET);
    x += width_of(canvas, "of", 11.5) + 6.0;
    text(canvas, of, x, y, 11.5, INK);
    x += width_of(canvas, of, 11.5) + 6.0;
    text(canvas, "rows in", x, y, 11.5, INK_QUIET);
    x += width_of(canvas, "rows in", 11.5) + 6.0;

    // Milliseconds to one decimal, from the microseconds we measured.
    let mut ms = [0u8; 16];
    let shown = one_dp(&mut ms, micros / 1000, (micros % 1000) / 100);
    // 16ms is a 60fps frame; past it the window will visibly stutter, and
    // saying so is the whole point of a probe.
    let col = if micros < 8_000 {
        GOOD
    } else if micros < 16_000 {
        WARN
    } else {
        BAD
    };
    text(canvas, shown, x, y, 11.5, col);
    x += width_of(canvas, shown, 11.5) + 3.0;
    text(canvas, "ms", x, y, 11.5, col);

    let note = if micros < 8_000 {
        "comfortably inside a 60fps frame -- the ceiling is not here"
    } else if micros < 16_000 {
        "inside a 60fps frame, with little room left"
    } else {
        "over a 60fps frame: this window would visibly stutter"
    };
    text_right(canvas, note, WIDTH - MARGIN, y, 11.5, col);
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
        let Ok(win) = window::create("Bigtable", size) else {
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

        // Somewhere in the middle, so the row numbers on the first frame make
        // the point that this is row 40,000 of a hundred thousand.
        let mut first_row: u32 = 40_000;
        let (micros, visible) = draw(canvas, first_row);
        verdict(canvas, micros, visible);
        let _ = canvas2d::present(canvas);

        if quick {
            let out = stdio::stdout();
            let _ = out.write(b"bigtable:ok\n");
            let _ = out.flush();
            let _ = window::close(win);
            return 0;
        }

        for _ in 0..MAX_WAIT_ROUNDS {
            match events::wait(Some(WAIT_ROUND_MILLIS)) {
                Some(types::Event::CloseRequested(id)) if id == win => break,
                Some(types::Event::Wheel(w)) => {
                    // dy is in logical pixels and POSITIVE SCROLLS DOWN, so
                    // the row offset moves the same way -- inverting it here
                    // is the classic way to ship a table that scrolls
                    // backwards.
                    //
                    // Clamped, because scrolling past the end of a hundred
                    // thousand rows into empty space is the ugliest thing a
                    // table can do. Ctrl means zoom on every desktop, so a
                    // ctrl-wheel is left alone rather than silently scrolling.
                    if w.modifiers.control {
                        continue;
                    }
                    let rows = (w.dy / ROW_H) as i64;
                    let step = if rows == 0 {
                        if w.dy > 0.0 { 1 } else if w.dy < 0.0 { -1 } else { 0 }
                    } else {
                        rows
                    };
                    let next = first_row as i64 + step;
                    first_row = next.clamp(0, (TOTAL_ROWS - 1) as i64) as u32;
                    let (m, v) = draw(canvas, first_row);
                    verdict(canvas, m, v);
                    let _ = canvas2d::present(canvas);
                }
                Some(types::Event::Resized(_)) => {
                    let (m, v) = draw(canvas, first_row);
                    verdict(canvas, m, v);
                    let _ = canvas2d::present(canvas);
                }
                _ => {}
            }
        }

        let _ = window::close(win);
        let out = stdio::stdout();
        let _ = out.write(b"bigtable:ok\n");
        let _ = out.flush();
        0
    }
}

bindings::export!(Component with_types_in bindings);
