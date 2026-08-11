//! Krate Journal — message yourself, WhatsApp-style, drawn on a canvas.
//!
//! An honest local journal: every entry is a chat bubble you sent to yourself.
//! No network, no account — the feed lives in the key-value store and nowhere
//! else. The UI is a familiar messenger layout: an avatar top bar, right-aligned
//! accent bubbles with in-bubble timestamps, date separator pills, and a rounded
//! input field with a paper-plane send button. Type, press Enter (or tap send),
//! and the thought lands in the feed with the current time.
//!
//! `#![no_std]` keeps it `krate:*`-only: the SDK owns the allocator and a
//! trapping panic handler, so no path drags in the `wasi:*` import set. Strings
//! are built by hand (no `format!`), no `unwrap`, no panicking index.

#![no_std]
#![allow(clippy::needless_range_loop)]

extern crate alloc;

extern crate krate as _krate_runtime;

#[allow(warnings)]
mod bindings;

use alloc::string::String;
use alloc::vec::Vec;

use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::store::kv;
use bindings::krate::time::clock;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const WIDTH: f32 = 480.0;
const HEIGHT: f32 = 760.0;

const ENTRIES_KEY: &str = "entries";
const DAY_MS: u64 = 86_400_000;

const MAX_WAIT_ROUNDS: u32 = 600_000;
const WAIT_ROUND_MILLIS: u32 = 33;
const MAX_IDLE_ROUNDS: u32 = 1800;

// ---- layout ----
const BAR_H: f32 = 64.0;
const SIDE: f32 = 16.0;
const INPUT_H: f32 = 76.0;
const INPUT_TOP: f32 = HEIGHT - INPUT_H;
const BUBBLE_MAX_W: f32 = WIDTH * 0.75;
const BUBBLE_PAD_X: f32 = 13.0;
const BUBBLE_PAD_TOP: f32 = 10.0;
const BUBBLE_PAD_BOT: f32 = 10.0;
const BODY_SIZE: f32 = 15.0;
const LINE_H: f32 = 20.0;
const TS_SIZE: f32 = 11.0;
const BUBBLE_GAP: f32 = 8.0;
const SEP_GAP: f32 = 14.0;

// ---- palette ----
// Flat ground: an 8-bit gradient this shallow quantizes into visible bands,
// and a chat feed reads best on a calm, even dark anyway.
const BG: gfx::Color = gfx::Color { r: 0.051, g: 0.063, b: 0.094, a: 1.0 };
const BAR_BG: gfx::Color = gfx::Color { r: 0.078, g: 0.094, b: 0.137, a: 1.0 };
const HAIRLINE: gfx::Color = gfx::Color { r: 0.137, g: 0.165, b: 0.220, a: 1.0 };
const PANEL: gfx::Color = gfx::Color { r: 0.086, g: 0.106, b: 0.149, a: 1.0 };
const INK: gfx::Color = gfx::Color { r: 0.949, g: 0.961, b: 0.980, a: 1.0 };
const INK_DIM: gfx::Color = gfx::Color { r: 0.604, g: 0.647, b: 0.710, a: 1.0 };
const INK_QUIET: gfx::Color = gfx::Color { r: 0.365, g: 0.408, b: 0.471, a: 1.0 };
const ACCENT: gfx::Color = gfx::Color { r: 0.298, g: 0.553, b: 1.0, a: 1.0 };
// Accent darkened toward the ground: the outgoing-bubble fill.
const BUBBLE: gfx::Color = gfx::Color { r: 0.145, g: 0.310, b: 0.647, a: 1.0 };
const TS_INK: gfx::Color = gfx::Color { r: 0.753, g: 0.831, b: 0.980, a: 0.62 };

struct Entry {
    millis: u64,
    text: String,
}

struct Component;

// ------------------------------------------------------------------
// Small string / number helpers, panic-free
// ------------------------------------------------------------------

fn push_u64(out: &mut String, value: u64) {
    let mut scratch = [0u8; 20];
    let mut n = value;
    let mut count = 0usize;
    if n == 0 {
        out.push('0');
        return;
    }
    while n > 0 && count < scratch.len() {
        if let Some(s) = scratch.get_mut(count) {
            *s = b'0' + (n % 10) as u8;
        }
        n /= 10;
        count += 1;
    }
    let mut i = count;
    while i > 0 {
        i -= 1;
        if let Some(d) = scratch.get(i) {
            out.push(*d as char);
        }
    }
}

fn push_two_digits(out: &mut String, value: u64) {
    let v = value % 100;
    out.push((b'0' + (v / 10) as u8) as char);
    out.push((b'0' + (v % 10) as u8) as char);
}

/// "hh:mm" from epoch millis.
fn time_label(millis: u64) -> String {
    let total_min = (millis / 60_000) % 1440;
    let mut s = String::new();
    push_two_digits(&mut s, total_min / 60);
    s.push(':');
    push_two_digits(&mut s, total_min % 60);
    s
}

fn parse_u64(bytes: &[u8]) -> u64 {
    let mut value = 0u64;
    for byte in bytes {
        if byte.is_ascii_digit() {
            value = value.saturating_mul(10).saturating_add(u64::from(byte - b'0'));
        } else {
            break;
        }
    }
    value
}

/// Rendered width of a string at a given font size, measured by the host with
/// the same font layout `draw_text` draws with.
///
/// This used to be character count times an invented constant. On a
/// proportional face `i` and `W` differ about four times in real width, so a
/// bubble was sized for the wrong text and its timestamp tucked beside a line
/// it did not actually fit next to. `measure_text` is the true answer.
fn text_width(canvas: u64, s: &str, size: f32) -> f32 {
    match canvas2d::measure_text(canvas, s, size) {
        Ok(m) => m.width,
        Err(_) => 0.0,
    }
}

// ------------------------------------------------------------------
// Persistence: newline-joined records, each "millis|text".
// ------------------------------------------------------------------

fn load_entries() -> Vec<Entry> {
    let mut out = Vec::new();
    let Ok(Some(bytes)) = kv::get(ENTRIES_KEY) else {
        return out;
    };
    for record in bytes.split(|b| *b == b'\n') {
        if record.is_empty() {
            continue;
        }
        let Some(sep) = record.iter().position(|b| *b == b'|') else {
            continue;
        };
        let millis = parse_u64(record.get(..sep).unwrap_or(b""));
        let Ok(text) = core::str::from_utf8(record.get(sep + 1..).unwrap_or(b"")) else {
            continue;
        };
        if millis > 0 && !text.is_empty() {
            let mut owned = String::new();
            owned.push_str(text);
            out.push(Entry { millis, text: owned });
        }
    }
    out
}

fn save_entries(entries: &[Entry]) {
    let mut blob = String::new();
    for (i, entry) in entries.iter().enumerate() {
        if i > 0 {
            blob.push('\n');
        }
        push_u64(&mut blob, entry.millis);
        blob.push('|');
        for ch in entry.text.chars() {
            // Newline is the record separator; never let it into a record.
            blob.push(if ch == '\n' { ' ' } else { ch });
        }
    }
    let _ = kv::set(ENTRIES_KEY, blob.as_bytes());
}

// ------------------------------------------------------------------
// Word wrap: byte ranges of `text`, each fitting `max_w` at BODY_SIZE.
// ------------------------------------------------------------------

fn wrap_lines(text: &str, max_w: f32) -> Vec<(usize, usize)> {
    let char_w = BODY_SIZE * 0.47;
    let max_chars = ((max_w / char_w) as usize).max(4);

    let mut lines: Vec<(usize, usize)> = Vec::new();
    let mut line_start = 0usize; // byte index
    let mut line_chars = 0usize;
    let mut last_space: Option<usize> = None; // byte index of last space seen

    for (idx, ch) in text.char_indices() {
        if line_chars >= max_chars {
            // Break at the last space if we saw one, else hard-break here.
            if let Some(sp_idx) = last_space {
                if sp_idx > line_start {
                    lines.push((line_start, sp_idx));
                    line_start = sp_idx + 1; // skip the space
                    line_chars = text
                        .get(line_start..idx)
                        .map(|s| s.chars().count())
                        .unwrap_or(0);
                    last_space = None;
                }
            }
            if line_chars >= max_chars {
                lines.push((line_start, idx));
                line_start = idx;
                line_chars = 0;
                last_space = None;
            }
        }
        if ch == ' ' {
            last_space = Some(idx);
        }
        line_chars += 1;
    }
    if line_start < text.len() || lines.is_empty() {
        lines.push((line_start, text.len()));
    }
    lines
}

// ------------------------------------------------------------------
// Feed layout
// ------------------------------------------------------------------

enum Item {
    Separator(&'static str),
    Bubble {
        entry_index: usize,
        width: f32,
        height: f32,
        ts_beside: bool,
    },
}

fn day_of(millis: u64) -> u64 {
    millis / DAY_MS
}

fn day_label(entry_day: u64, today: u64) -> &'static str {
    if entry_day == today {
        "Today"
    } else if entry_day + 1 == today {
        "Yesterday"
    } else {
        "Earlier"
    }
}

/// Measure one entry's bubble. Returns (width, height, timestamp-beside-text).
fn measure_bubble(canvas: u64, text: &str, ts: &str) -> (f32, f32, bool) {
    let max_text_w = BUBBLE_MAX_W - BUBBLE_PAD_X * 2.0;
    let lines = wrap_lines(text, max_text_w);
    let mut widest = 0.0f32;
    for (a, b) in lines.iter() {
        if let Some(line) = text.get(*a..*b) {
            let w = text_width(canvas, line, BODY_SIZE);
            if w > widest {
                widest = w;
            }
        }
    }
    let ts_w = text_width(canvas, ts, TS_SIZE);
    let last_w = lines
        .last()
        .and_then(|(a, b)| text.get(*a..*b))
        .map(|l| text_width(canvas, l, BODY_SIZE))
        .unwrap_or(0.0);
    // WhatsApp tucks the timestamp beside the last line when it fits.
    let beside = last_w + 10.0 + ts_w <= max_text_w;
    let content_w = if beside {
        widest.max(last_w + 10.0 + ts_w)
    } else {
        widest.max(ts_w)
    };
    let mut h = (lines.len() as f32) * LINE_H + BUBBLE_PAD_TOP + BUBBLE_PAD_BOT;
    if !beside {
        h += 14.0;
    }
    (content_w + BUBBLE_PAD_X * 2.0, h, beside)
}

fn build_items(canvas: u64, entries: &[Entry], today: u64) -> Vec<Item> {
    let mut items = Vec::new();
    let mut prev_day: Option<u64> = None;
    for (i, entry) in entries.iter().enumerate() {
        let d = day_of(entry.millis);
        if prev_day != Some(d) {
            items.push(Item::Separator(day_label(d, today)));
            prev_day = Some(d);
        }
        let ts = time_label(entry.millis);
        let (w, h, beside) = measure_bubble(canvas, &entry.text, &ts);
        items.push(Item::Bubble {
            entry_index: i,
            width: w,
            height: h,
            ts_beside: beside,
        });
    }
    items
}

// ------------------------------------------------------------------
// Rendering
// ------------------------------------------------------------------

fn draw(canvas: u64, entries: &[Entry], draft: &str, now: u64) -> Result<(), gfx::GfxError> {
    // Ground.
    canvas2d::clear(canvas, BG)?;

    draw_feed(canvas, entries, now)?;
    draw_top_bar(canvas, entries.len())?;
    draw_input_bar(canvas, draft)?;

    canvas2d::present(canvas)?;
    Ok(())
}

fn draw_feed(canvas: u64, entries: &[Entry], now: u64) -> Result<(), gfx::GfxError> {
    let items = build_items(canvas, entries, day_of(now));

    // Anchor the newest item just above the input bar and stack upward,
    // like a real chat: history scrolls away under the top bar.
    let mut y_end = INPUT_TOP - 14.0;
    let mut tops: Vec<f32> = Vec::new();
    tops.resize(items.len(), 0.0);
    let mut i = items.len();
    while i > 0 {
        i -= 1;
        let (h, gap_above) = match items.get(i) {
            Some(Item::Separator(_)) => (22.0, SEP_GAP),
            Some(Item::Bubble { height, .. }) => (*height, BUBBLE_GAP),
            None => (0.0, 0.0),
        };
        // Extra room above a bubble that follows a separator.
        let extra = match items.get(i) {
            Some(Item::Bubble { .. }) => {
                if i > 0 && matches!(items.get(i - 1), Some(Item::Separator(_))) {
                    SEP_GAP - BUBBLE_GAP
                } else {
                    0.0
                }
            }
            _ => 0.0,
        };
        let top = y_end - h;
        if let Some(slot) = tops.get_mut(i) {
            *slot = top;
        }
        y_end = top - gap_above - extra;
    }

    // When the whole history fits with headroom, float a quiet privacy note
    // in the empty space — the honest-local version of WhatsApp's E2E banner.
    let first_top = tops.first().copied().unwrap_or(INPUT_TOP);
    if first_top > BAR_H + 120.0 {
        let note = "Only you can see this — entries stay on this device";
        let size = 11.5;
        let tw = text_width(canvas, note, size);
        let pw = tw + 28.0;
        let ph = 24.0;
        let px = (WIDTH - pw) * 0.5;
        let py = BAR_H + (first_top - BAR_H - ph) * 0.5;
        rounded_rect(canvas, px, py, pw, ph, 8.0, PANEL)?;
        draw_text(canvas, note, px + 14.0, py + 16.0, size, INK_QUIET)?;
    }

    for (i, item) in items.iter().enumerate() {
        let top = tops.get(i).copied().unwrap_or(0.0);
        if top + 60.0 < 0.0 {
            continue;
        }
        match item {
            Item::Separator(label) => draw_separator(canvas, label, top)?,
            Item::Bubble { entry_index, width, height, ts_beside } => {
                if let Some(entry) = entries.get(*entry_index) {
                    draw_bubble(canvas, entry, top, *width, *height, *ts_beside)?;
                }
            }
        }
    }
    Ok(())
}

fn draw_separator(canvas: u64, label: &str, top: f32) -> Result<(), gfx::GfxError> {
    let size = 11.5;
    let tw = text_width(canvas, label, size);
    let pill_w = tw + 26.0;
    let pill_h = 22.0;
    let x = (WIDTH - pill_w) * 0.5;
    rounded_rect(canvas, x, top, pill_w, pill_h, pill_h * 0.5, PANEL)?;
    draw_text(canvas, label, x + (pill_w - tw) * 0.5, top + 15.0, size, INK_DIM)?;
    Ok(())
}

fn draw_bubble(
    canvas: u64,
    entry: &Entry,
    top: f32,
    w: f32,
    h: f32,
    ts_beside: bool,
) -> Result<(), gfx::GfxError> {
    let x = WIDTH - SIDE - w;
    // Chat-tail feel: every corner 16 except a tight 4 at the bottom-right.
    bubble_shape(canvas, x, top, w, h, 16.0, 4.0, BUBBLE)?;

    let max_text_w = BUBBLE_MAX_W - BUBBLE_PAD_X * 2.0;
    let lines = wrap_lines(&entry.text, max_text_w);
    let mut baseline = top + BUBBLE_PAD_TOP + 14.0;
    for (a, b) in lines.iter() {
        if let Some(line) = entry.text.get(*a..*b) {
            draw_text(canvas, line, x + BUBBLE_PAD_X, baseline, BODY_SIZE, INK)?;
        }
        baseline += LINE_H;
    }

    let ts = time_label(entry.millis);
    let ts_w = text_width(canvas, &ts, TS_SIZE);
    let ts_x = x + w - BUBBLE_PAD_X - ts_w;
    let ts_y = if ts_beside {
        // Sit on (just under) the last text baseline, WhatsApp-style.
        baseline - LINE_H + 1.0
    } else {
        top + h - BUBBLE_PAD_BOT
    };
    draw_text(canvas, &ts, ts_x, ts_y, TS_SIZE, TS_INK)?;
    Ok(())
}

fn draw_top_bar(canvas: u64, count: usize) -> Result<(), gfx::GfxError> {
    fill(canvas, 0.0, 0.0, WIDTH, BAR_H, BAR_BG)?;
    fill(canvas, 0.0, BAR_H - 1.0, WIDTH, 1.0, HAIRLINE)?;

    // Avatar disc: accent-tinted, "M" initial in accent.
    let cx = 24.0 + 20.0;
    let cy = BAR_H * 0.5;
    disc(canvas, cx, cy, 20.0, color(0.298, 0.553, 1.0, 0.16))?;
    let iw = text_width(canvas, "M", 17.0);
    draw_text(canvas, "M", cx - iw * 0.5 - 1.0, cy + 6.0, 17.0, ACCENT)?;

    draw_text(canvas, "My Journal", 78.0, 28.0, 17.0, INK)?;
    let mut sub = String::new();
    push_u64(&mut sub, count as u64);
    sub.push_str(if count == 1 { " entry" } else { " entries" });
    draw_text(canvas, &sub, 78.0, 47.0, 12.0, INK_DIM)?;
    Ok(())
}

fn draw_input_bar(canvas: u64, draft: &str) -> Result<(), gfx::GfxError> {
    fill(canvas, 0.0, INPUT_TOP, WIDTH, INPUT_H, BAR_BG)?;
    fill(canvas, 0.0, INPUT_TOP, WIDTH, 1.0, HAIRLINE)?;

    // The rounded field.
    let fx = SIDE;
    let fy = INPUT_TOP + 16.0;
    let fh = 44.0;
    let (bx, _by, br) = send_button();
    let fw = bx - br - 10.0 - fx;
    rounded_rect(canvas, fx, fy, fw, fh, fh * 0.5, PANEL)?;
    stroke_rounded(canvas, fx, fy, fw, fh, fh * 0.5, HAIRLINE)?;

    let tx = fx + 18.0;
    let ty = fy + fh * 0.5 + 5.0;
    if draft.is_empty() {
        draw_text(canvas, "Message yourself", tx, ty, BODY_SIZE, INK_QUIET)?;
        fill(canvas, tx, fy + 12.0, 2.0, fh - 24.0, ACCENT)?;
    } else {
        // Show the tail of a long draft so the caret stays visible.
        let fit = ((fw - 36.0 - 8.0) / (BODY_SIZE * 0.47)) as usize;
        let total = draft.chars().count();
        let skip = total.saturating_sub(fit);
        let start = draft
            .char_indices()
            .nth(skip)
            .map(|(i, _)| i)
            .unwrap_or(0);
        let shown = draft.get(start..).unwrap_or(draft);
        draw_text(canvas, shown, tx, ty, BODY_SIZE, INK)?;
        let cw = text_width(canvas, shown, BODY_SIZE);
        fill(canvas, tx + cw + 3.0, fy + 12.0, 2.0, fh - 24.0, ACCENT)?;
    }

    // Send button: accent disc + paper-plane built from filled shapes.
    let (bx, by, br) = send_button();
    disc(canvas, bx, by, br, ACCENT)?;
    paper_plane(canvas, bx, by)?;
    Ok(())
}

fn send_button() -> (f32, f32, f32) {
    let r = 22.0;
    (WIDTH - SIDE - r, INPUT_TOP + 16.0 + r, r)
}

/// A right-pointing paper plane: the classic send glyph, a solid triangle with
/// a notch bitten out of its tail. Rasterized into a supersampled RGBA buffer
/// so the diagonal edges come out properly antialiased, then blitted once.
fn paper_plane(canvas: u64, cx: f32, cy: f32) -> Result<(), gfx::GfxError> {
    const GLYPH: f32 = 22.0; // canvas-unit square the glyph occupies
    const RES: usize = 88; // buffer resolution: 4x the glyph square
    const SS: usize = 2; // 2x2 coverage subsamples per buffer pixel

    // Glyph-space (0..22) vertices: tip, back-top, back-bottom, notch point.
    let ax = 18.5f32;
    let ay = 11.0f32;
    let bx = 4.0f32;
    let by = 3.5f32;
    let cx2 = 4.0f32;
    let cy2 = 18.5f32;
    let dx = 9.0f32;
    let dy = 11.0f32;

    fn side(px: f32, py: f32, x0: f32, y0: f32, x1: f32, y1: f32) -> f32 {
        (x1 - x0) * (py - y0) - (y1 - y0) * (px - x0)
    }
    fn in_tri(px: f32, py: f32, x0: f32, y0: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> bool {
        let s0 = side(px, py, x0, y0, x1, y1);
        let s1 = side(px, py, x1, y1, x2, y2);
        let s2 = side(px, py, x2, y2, x0, y0);
        (s0 >= 0.0 && s1 >= 0.0 && s2 >= 0.0) || (s0 <= 0.0 && s1 <= 0.0 && s2 <= 0.0)
    }

    let mut rgba: Vec<u8> = Vec::new();
    rgba.resize(RES * RES * 4, 0);
    let scale = GLYPH / RES as f32;
    for row in 0..RES {
        for col in 0..RES {
            let mut cover = 0u32;
            for sy in 0..SS {
                for sx in 0..SS {
                    let px = (col as f32 + (sx as f32 + 0.5) / SS as f32) * scale;
                    let py = (row as f32 + (sy as f32 + 0.5) / SS as f32) * scale;
                    let inside = in_tri(px, py, ax, ay, bx, by, cx2, cy2)
                        && !in_tri(px, py, bx, by, cx2, cy2, dx, dy);
                    if inside {
                        cover += 1;
                    }
                }
            }
            if cover > 0 {
                let a = (cover * 255 / (SS * SS) as u32) as u8;
                let at = (row * RES + col) * 4;
                if let Some(px) = rgba.get_mut(at..at + 4) {
                    px.copy_from_slice(&[255, 255, 255, a]);
                }
            }
        }
    }

    canvas2d::draw_pixels(
        canvas,
        gfx::Rect {
            x: cx - GLYPH * 0.5,
            y: cy - GLYPH * 0.5,
            width: GLYPH,
            height: GLYPH,
        },
        RES as u32,
        RES as u32,
        &rgba,
    )
}

// ------------------------------------------------------------------
// Shape helpers
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

/// Rounded rect with radius `r` everywhere except a small `s` at bottom-right —
/// the chat-bubble tail corner. Full-alpha color only (pieces overlap).
fn bubble_shape(canvas: u64, x: f32, y: f32, w: f32, h: f32, r: f32, s: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    let r = r.min(w * 0.5).min(h * 0.5);
    // Middle column, full height.
    fill(canvas, x + r, y, w - r * 2.0, h, c)?;
    // Left column between the two big corners.
    fill(canvas, x, y + r, r, h - r * 2.0, c)?;
    // Right column from below the top-right corner down to the small corner.
    fill(canvas, x + w - r, y + r, r, h - r - s, c)?;
    // Bottom-right filler strip left of the small corner disc.
    fill(canvas, x + w - r, y + h - s, r - s, s, c)?;
    disc(canvas, x + r, y + r, r, c)?;
    disc(canvas, x + w - r, y + r, r, c)?;
    disc(canvas, x + r, y + h - r, r, c)?;
    disc(canvas, x + w - s, y + h - s, s, c)?;
    Ok(())
}

fn stroke_rounded(canvas: u64, x: f32, y: f32, w: f32, h: f32, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    let t = 1.0;
    let r = r.min(w * 0.5).min(h * 0.5);
    fill(canvas, x + r, y, w - r * 2.0, t, c)?;
    fill(canvas, x + r, y + h - t, w - r * 2.0, t, c)?;
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

// ------------------------------------------------------------------
// Seed data for the quick shot
// ------------------------------------------------------------------

fn seed_entries(now: u64) -> Vec<Entry> {
    let today_start = now - now % DAY_MS;
    let yesterday = today_start.saturating_sub(DAY_MS);
    let mut out = Vec::new();
    let mk = |base: u64, h: u64, m: u64, text: &str| -> Entry {
        let mut s = String::new();
        s.push_str(text);
        Entry { millis: base + h * 3_600_000 + m * 60_000, text: s }
    };
    out.push(mk(yesterday, 18, 42, "Shipped the store redesign. Two weeks of late nights but it finally feels right."));
    out.push(mk(yesterday, 21, 7, "Note to self: call the bank about the card limit before Friday."));
    out.push(mk(today_start, 8, 54, "Slept properly for once. Keeping the no-phone-in-bed rule."));
    out.push(mk(today_start, 12, 26, "Lunch with Priya. She said the pitch reads clearer without the roadmap slide. She's right."));
    out
}

// ------------------------------------------------------------------
// Entry point
// ------------------------------------------------------------------

impl bindings::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize {
            width: WIDTH as u32,
            height: HEIGHT as u32,
        };
        let Ok(win) = window::create("Journal", size) else {
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

        let now = clock::now_millis();
        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");

        if quick {
            // The store screenshot: a believable two-day feed and a draft mid-type.
            let entries = seed_entries(now);
            save_entries(&entries);
            let draft = "Booked the Lisbon flights";
            let code = if draw(canvas, &entries, draft, now).is_ok() { 0 } else { 34 };
            let out = stdio::stdout();
            let _ = out.write(if code == 0 { b"journal:ok\n" } else { b"journal:draw-failed\n" });
            let _ = window::close(win);
            return code;
        }

        let mut entries = load_entries();
        let mut draft = String::new();

        if draw(canvas, &entries, &draft, now).is_err() {
            let _ = window::close(win);
            return 34;
        }

        let mut idle_rounds = 0u32;
        let mut round = 0u32;
        while round < MAX_WAIT_ROUNDS {
            round += 1;
            let event = events::wait(Some(WAIT_ROUND_MILLIS));
            if event.is_none() {
                idle_rounds += 1;
                // Only a headless check gives up on silence. A person who
                // opens this and thinks for a moment must not watch the
                // window close itself.
                if quick && idle_rounds >= MAX_IDLE_ROUNDS {
                    break;
                }
                continue;
            }
            idle_rounds = 0;
            let mut dirty = false;
            let mut done = false;
            let mut send = false;
            match event {
                Some(types::Event::Pointer(p)) if p.pressed => {
                    let (bx, by, br) = send_button();
                    let dx = p.x - bx;
                    let dy = p.y - by;
                    if dx * dx + dy * dy <= br * br {
                        send = true;
                    }
                }
                Some(types::Event::TextInput(text)) => {
                    for ch in text.chars() {
                        if !ch.is_control() {
                            draft.push(ch);
                        }
                    }
                    dirty = true;
                }
                Some(types::Event::Key(key)) if key.pressed => {
                    let k = key.key.as_bytes();
                    if k == b"Backspace" {
                        let _ = draft.pop();
                        dirty = true;
                    } else if k == b"Enter" || k == b"Return" {
                        send = true;
                    }
                }
                Some(types::Event::CloseRequested(_)) => {
                    done = true;
                }
                _ => {}
            }
            if send {
                let trimmed = draft.trim();
                if !trimmed.is_empty() {
                    let mut text = String::new();
                    text.push_str(trimmed);
                    entries.push(Entry { millis: clock::now_millis(), text });
                    save_entries(&entries);
                }
                draft.clear();
                dirty = true;
            }
            if dirty {
                let _ = draw(canvas, &entries, &draft, clock::now_millis());
            }
            if done {
                break;
            }
        }

        let out = stdio::stdout();
        let _ = out.write(b"entries:");
        let mut buf = String::new();
        push_u64(&mut buf, entries.len() as u64);
        let _ = out.write(buf.as_bytes());
        let _ = out.write(b"\n");
        let _ = window::close(win);
        0
    }
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
