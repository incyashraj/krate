//! Krate weather — a modern weather card, drawn on a canvas.
//!
//! A new user is not impressed by a column of plain labels. So the whole card
//! is painted into a canvas: a soft gradient sky, a big temperature, a simple
//! sun-and-cloud mark, a condition line, and a row of forecast pills along the
//! bottom. It is mock data -- no network -- but it should look like an app
//! someone would actually want to open.
//!
//! The drawing is rectangle fills, discs approximated by scanned rows, and the
//! canvas text call. No panic paths, only `krate:*`.

#[allow(warnings)]
mod bindings;

use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const WIDTH: u32 = 400;
const HEIGHT: u32 = 520;

const CITY: &str = "San Francisco";
const CONDITION: &str = "Partly cloudy";
const TEMP: i32 = 64;
const HIGH: i32 = 68;
const LOW: i32 = 57;

/// Five days: label, high, and a condition code (0 sun, 1 partly, 2 cloud, 3 rain).
const FORECAST: [(&str, i32, u8); 5] = [
    ("Mon", 66, 0),
    ("Tue", 61, 2),
    ("Wed", 68, 1),
    ("Thu", 70, 0),
    ("Fri", 63, 3),
];

const QUICK_ROUNDS: u32 = 20;
const MAX_ROUNDS: u32 = 600;
const ROUND_MILLIS: u32 = 50;

struct Component;

impl bindings::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize {
            width: WIDTH,
            height: HEIGHT,
        };
        let Ok(win) = window::create("Weather", size) else {
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
        if draw_card(canvas).is_err() {
            let _ = window::close(win);
            return 34;
        }

        let out = stdio::stdout();
        let _ = out.write(b"weather:ok\n");

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

fn draw_card(canvas: u64) -> Result<(), gfx::GfxError> {
    let size = canvas2d::canvas_size(canvas)?;
    let w = size.width;
    let h = size.height;

    // A vertical gradient sky: deep blue at the top easing to a lighter blue,
    // painted as a stack of thin horizontal bands.
    let bands = 60u32;
    let mut i = 0u32;
    while i < bands {
        let t = i as f32 / bands as f32;
        let r = 0.16 + t * 0.22;
        let g = 0.34 + t * 0.28;
        let b = 0.64 + t * 0.18;
        let band_h = h / bands as f32 + 1.0;
        fill(canvas, 0.0, t * h, w, band_h, r, g, b)?;
        i += 1;
    }

    // City name, top-left.
    text(canvas, CITY, 24.0, 46.0, 22.0, 0.92, 0.95, 1.0)?;

    // A sun-and-cloud mark, top-right.
    draw_sun_cloud(canvas, w - 92.0, 30.0)?;

    // The big temperature, large.
    let mut tbuf = [0u8; 8];
    let temp_text = degree(TEMP, &mut tbuf);
    text(canvas, temp_text, 22.0, 168.0, 88.0, 1.0, 1.0, 1.0)?;

    // Condition and high/low under the number.
    text(canvas, CONDITION, 26.0, 208.0, 19.0, 0.88, 0.92, 0.99)?;
    let mut hlbuf = [0u8; 24];
    let hl = high_low(HIGH, LOW, &mut hlbuf);
    text(canvas, hl, 26.0, 236.0, 16.0, 0.80, 0.86, 0.96)?;

    // A divider line: 1px tall, white.
    fill(canvas, 24.0, 272.0, w - 48.0, 1.0, 1.0, 1.0, 1.0)?;

    // Five forecast pills across the bottom.
    let count = FORECAST.len() as f32;
    let margin = 20.0;
    let gap = 10.0;
    let usable = w - margin * 2.0 - gap * (count - 1.0);
    let pill_w = usable / count;
    let pill_top = 310.0;
    let pill_h = 168.0;

    let mut j = 0usize;
    while j < FORECAST.len() {
        if let Some(&(day, high, code)) = FORECAST.get(j) {
            let x = margin + (j as f32) * (pill_w + gap);
            // Pill background: a lighter panel over the sky.
            fill(canvas, x, pill_top, pill_w, pill_h, 0.30, 0.44, 0.68)?;
            // Day label, centered-ish.
            text(canvas, day, x + 10.0, pill_top + 28.0, 15.0, 0.92, 0.95, 1.0)?;
            // A small weather glyph for the day.
            draw_glyph(canvas, x + pill_w / 2.0 - 14.0, pill_top + 58.0, code)?;
            // High temperature near the bottom of the pill.
            let mut dbuf = [0u8; 8];
            let d = degree(high, &mut dbuf);
            text(canvas, d, x + 10.0, pill_top + pill_h - 20.0, 20.0, 1.0, 1.0, 1.0)?;
        }
        j += 1;
    }

    canvas2d::present(canvas)?;
    Ok(())
}

/// A sun with a small cloud overlapping it, for the header.
fn draw_sun_cloud(canvas: u64, x: f32, y: f32) -> Result<(), gfx::GfxError> {
    disc(canvas, x + 22.0, y + 20.0, 17.0, 1.0, 0.85, 0.40)?;
    disc(canvas, x + 6.0, y + 36.0, 12.0, 0.96, 0.98, 1.0)?;
    disc(canvas, x + 24.0, y + 32.0, 15.0, 0.96, 0.98, 1.0)?;
    disc(canvas, x + 42.0, y + 36.0, 12.0, 0.96, 0.98, 1.0)?;
    fill(canvas, x + 6.0, y + 42.0, 48.0, 10.0, 0.96, 0.98, 1.0)?;
    Ok(())
}

/// A small forecast glyph by condition code.
fn draw_glyph(canvas: u64, x: f32, y: f32, code: u8) -> Result<(), gfx::GfxError> {
    match code {
        0 => {
            disc(canvas, x + 14.0, y + 12.0, 12.0, 1.0, 0.85, 0.40)?;
        }
        1 => {
            disc(canvas, x + 8.0, y + 8.0, 8.0, 1.0, 0.85, 0.40)?;
            disc(canvas, x + 18.0, y + 15.0, 10.0, 0.96, 0.98, 1.0)?;
        }
        2 => {
            disc(canvas, x + 9.0, y + 13.0, 10.0, 0.92, 0.95, 1.0)?;
            disc(canvas, x + 21.0, y + 13.0, 10.0, 0.92, 0.95, 1.0)?;
            fill(canvas, x + 9.0, y + 15.0, 20.0, 8.0, 0.92, 0.95, 1.0)?;
        }
        _ => {
            disc(canvas, x + 14.0, y + 9.0, 10.0, 0.86, 0.89, 0.95)?;
            fill(canvas, x + 8.0, y + 22.0, 3.0, 8.0, 0.6, 0.80, 1.0)?;
            fill(canvas, x + 16.0, y + 22.0, 3.0, 8.0, 0.6, 0.80, 1.0)?;
            fill(canvas, x + 24.0, y + 22.0, 3.0, 8.0, 0.6, 0.80, 1.0)?;
        }
    }
    Ok(())
}

/// A filled disc, approximated by scanning rows and filling the chord width.
fn disc(
    canvas: u64,
    cx: f32,
    cy: f32,
    radius: f32,
    r: f32,
    g: f32,
    b: f32,
) -> Result<(), gfx::GfxError> {
    let steps = (radius * 2.0) as i32;
    let mut i = 0i32;
    while i < steps {
        let dy = (i as f32) - radius;
        let half = radius * radius - dy * dy;
        if half > 0.0 {
            let chord = sqrt(half);
            fill(canvas, cx - chord, cy + dy, chord * 2.0, 1.5, r, g, b)?;
        }
        i += 1;
    }
    Ok(())
}

/// Square root by Newton's method, no std math import.
fn sqrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut guess = x;
    let mut i = 0;
    while i < 8 {
        guess = 0.5 * (guess + x / guess);
        i += 1;
    }
    guess
}

fn fill(
    canvas: u64,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
    g: f32,
    b: f32,
) -> Result<(), gfx::GfxError> {
    canvas2d::fill_rect(
        canvas,
        gfx::Rect {
            x,
            y,
            width: w,
            height: h,
        },
        gfx::Color { r, g, b, a: 1.0 },
    )
}

fn text(
    canvas: u64,
    s: &str,
    x: f32,
    y: f32,
    size: f32,
    r: f32,
    g: f32,
    b: f32,
) -> Result<(), gfx::GfxError> {
    canvas2d::draw_text(
        canvas,
        s,
        gfx::Point { x, y },
        size,
        gfx::Color { r, g, b, a: 1.0 },
    )
}

// ----- widget builders -----

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

// ----- text helpers, panic-free -----

/// "64°" as a str. The degree sign is UTF-8 0xC2 0xB0.
fn degree(value: i32, buf: &mut [u8; 8]) -> &str {
    let mut pos = 0usize;
    let mut num = [0u8; 6];
    for byte in i32_bytes(value, &mut num) {
        push(buf, &mut pos, *byte);
    }
    push(buf, &mut pos, 0xC2);
    push(buf, &mut pos, 0xB0);
    // SAFETY: digits, an optional minus, and the two-byte degree sign are UTF-8.
    unsafe { core::str::from_utf8_unchecked(buf.get(..pos).unwrap_or(b"")) }
}

/// "H:68   L:57".
fn high_low(high: i32, low: i32, buf: &mut [u8; 24]) -> &str {
    let mut pos = 0usize;
    for byte in b"H:" {
        push(buf, &mut pos, *byte);
    }
    let mut num = [0u8; 6];
    for byte in i32_bytes(high, &mut num) {
        push(buf, &mut pos, *byte);
    }
    for byte in b"   L:" {
        push(buf, &mut pos, *byte);
    }
    for byte in i32_bytes(low, &mut num) {
        push(buf, &mut pos, *byte);
    }
    unsafe { core::str::from_utf8_unchecked(buf.get(..pos).unwrap_or(b"")) }
}

fn push(buf: &mut [u8], pos: &mut usize, byte: u8) {
    if let Some(slot) = buf.get_mut(*pos) {
        *slot = byte;
        *pos += 1;
    }
}

fn i32_bytes(value: i32, buf: &mut [u8; 6]) -> &[u8] {
    let mut pos = 0usize;
    let mut mag = if value < 0 {
        push(buf, &mut pos, b'-');
        i64::from(value).unsigned_abs()
    } else {
        value as u64
    };
    if mag == 0 {
        push(buf, &mut pos, b'0');
        return buf.get(..pos).unwrap_or(b"0");
    }
    let mut scratch = [0u8; 12];
    let mut count = 0usize;
    while mag > 0 && count < scratch.len() {
        if let Some(slot) = scratch.get_mut(count) {
            *slot = b'0' + (mag % 10) as u8;
        }
        mag /= 10;
        count += 1;
    }
    let mut i = count;
    while i > 0 {
        i -= 1;
        if let Some(digit) = scratch.get(i) {
            push(buf, &mut pos, *digit);
        }
    }
    buf.get(..pos).unwrap_or(b"0")
}

bindings::export!(Component with_types_in bindings);
