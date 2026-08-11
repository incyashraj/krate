//! Krate keyvault — the limitation probe for persistence, drawn on a canvas.
//!
//! The wall it tests: can an app remember anything between runs? This app keeps
//! a run counter in the key-value store: it reads the count, adds one, saves it,
//! and shows it. Run it three times and it must read 1, then 2, then 3. If
//! persistence is fake, the count never moves off 1.
//!
//! The presentation is deliberately minimal: one large, glowing count centered
//! as the focal point on a considered dark ground, a small label above, and a
//! quiet subtitle below. The whole thing is painted into one `gfx.canvas2d`.
//!
//! `#![no_std]` keeps it `krate:*`-only: the SDK owns the allocator and a
//! trapping panic handler, so no path drags in the `wasi:*` import set. The
//! counter is stored as decimal text and parsed by hand; numbers are formatted
//! into fixed byte buffers. No `Vec`, `format!`, `unwrap`, or panicking index.

#![no_std]
#![allow(clippy::needless_range_loop)]

extern crate alloc;

extern crate krate as _krate_runtime;

#[allow(warnings)]
mod bindings;

use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::store::kv;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const COUNT_KEY: &str = "run-count";
const WIDTH: f32 = 420.0;
const HEIGHT: f32 = 320.0;

const QUICK_ROUNDS: u32 = 1;
const MAX_ROUNDS: u32 = 600;
const ROUND_MILLIS: u32 = 50;

// Palette.
const BG_TOP: gfx::Color = gfx::Color { r: 0.078, g: 0.090, b: 0.133, a: 1.0 };
const BG_BOT: gfx::Color = gfx::Color { r: 0.039, g: 0.047, b: 0.075, a: 1.0 };
const INK: gfx::Color = gfx::Color { r: 0.933, g: 0.953, b: 1.0, a: 1.0 };
const INK_DIM: gfx::Color = gfx::Color { r: 0.451, g: 0.502, b: 0.612, a: 1.0 };
const ACCENT: gfx::Color = gfx::Color { r: 0.42, g: 0.62, b: 1.0, a: 1.0 };

struct Component;

impl bindings::Guest for Component {
    fn run() -> i32 {
        // Read the saved count, or zero on the very first run. A missing key is
        // `Ok(None)`, the normal fresh-start case, not an error.
        let previous = match kv::get(COUNT_KEY) {
            Ok(Some(bytes)) => parse_u64(&bytes),
            Ok(None) => 0,
            Err(_) => {
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
            width: WIDTH as u32,
            height: HEIGHT as u32,
        };
        let Ok(win) = window::create("Run counter", size) else {
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
        // The app's own coordinate system: keep drawing in these numbers
        // and the host scales them to any window, centred, never stretched
        // out of proportion (K-096).
        let _ = canvas2d::set_design_size(
            canvas,
            gfx::Size {
                width: WIDTH,
                height: HEIGHT,
            },
        );

        if draw(canvas, count).is_err() {
            let _ = window::close(win);
            return 34;
        }

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

// ------------------------------------------------------------------
// Rendering
// ------------------------------------------------------------------

fn draw(canvas: u64, count: u64) -> Result<(), gfx::GfxError> {
    // Considered dark ground: a soft vertical gradient.
    canvas2d::linear_gradient(
        canvas,
        gfx::Rect { x: 0.0, y: 0.0, width: WIDTH, height: HEIGHT },
        BG_TOP,
        BG_BOT,
    )?;

    let cx = WIDTH * 0.5;

    // A soft accent glow pooled behind the number, so the focal point sits in
    // its own light rather than floating on flat dark.
    canvas2d::radial_gradient(
        canvas,
        gfx::Point { x: cx, y: HEIGHT * 0.5 },
        150.0,
        color(0.42, 0.62, 1.0, 0.16),
        color(0.42, 0.62, 1.0, 0.0),
    )?;

    // ---- small uppercase label above ----
    let label = "TIMES OPENED";
    let lsize = 14.0;
    let lw = text_width(canvas, label, lsize);
    draw_text(canvas, label, cx - lw * 0.5, 74.0, lsize, INK_DIM)?;

    // ---- the large count, centered, the focal point ----
    let mut buf = [0u8; 20];
    let digits = u64_to_bytes(count, &mut buf);
    if let Ok(txt) = core::str::from_utf8(digits) {
        let nsize = 92.0;
        let nw = text_width(canvas, txt, nsize);
        let nx = cx - nw * 0.5;
        // Baseline placed so the glyph body sits centered in the window's middle
        // band, clear of the label above and the subtitle below.
        let ny = 190.0;
        // A faint drop shadow for weight, then the number on top.
        draw_text(canvas, txt, nx + 2.0, ny + 3.0, nsize, color(0.0, 0.0, 0.0, 0.35))?;
        draw_text(canvas, txt, nx, ny, nsize, INK)?;
    }

    // ---- quiet subtitle below ----
    let sub = "Reopen it — this keeps climbing.";
    let ssize = 14.0;
    let sw = text_width(canvas, sub, ssize);
    draw_text(canvas, sub, cx - sw * 0.5, HEIGHT - 46.0, ssize, INK_DIM)?;

    // A short accent underline centered beneath the subtitle for a finished feel.
    let uw = 40.0;
    rounded_rect(canvas, cx - uw * 0.5, HEIGHT - 30.0, uw, 3.0, 1.5, ACCENT)?;

    canvas2d::present(canvas)?;
    Ok(())
}

// ------------------------------------------------------------------
// Drawing helpers
// ------------------------------------------------------------------

fn fill(canvas: u64, x: f32, y: f32, w: f32, h: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_rect(canvas, gfx::Rect { x, y, width: w, height: h }, c)
}

fn rounded_rect(canvas: u64, x: f32, y: f32, w: f32, h: f32, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    let r = r.min(w * 0.5).min(h * 0.5);
    fill(canvas, x + r, y, w - r * 2.0, h, c)?;
    fill(canvas, x, y + r, w, h - r * 2.0, c)?;
    disc(canvas, x + r, y + r, r, c)?;
    disc(canvas, x + w - r, y + r, r, c)?;
    disc(canvas, x + r, y + h - r, r, c)?;
    disc(canvas, x + w - r, y + h - r, r, c)?;
    Ok(())
}

fn disc(canvas: u64, cx: f32, cy: f32, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_circle(canvas, gfx::Point { x: cx, y: cy }, r, c)
}

fn draw_text(canvas: u64, text: &str, x: f32, y: f32, size: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::draw_text(canvas, text, gfx::Point { x, y }, size, c)
}

fn color(r: f32, g: f32, b: f32, a: f32) -> gfx::Color {
    gfx::Color { r, g, b, a }
}

/// Rendered width of a string at a given font size, measured by the host with
/// the same font layout `draw_text` draws with.
///
/// This used to be character count times an invented constant. On a
/// proportional face `i` and `W` differ about four times in real width, so a
/// centred label was not centred and a caret sat beside its text rather than
/// after it. `measure_text` is the true answer.
fn text_width(canvas: u64, s: &str, size: f32) -> f32 {
    match canvas2d::measure_text(canvas, s, size) {
        Ok(m) => m.width,
        Err(_) => 0.0,
    }
}

// ------------------------------------------------------------------
// byte <-> number, panic-free
// ------------------------------------------------------------------

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

// ----- widget builders (one canvas filling the window) -----

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

bindings::export!(Component with_types_in bindings);
