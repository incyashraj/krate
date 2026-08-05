//! Krate Notes — a two-pane notes app, drawn on a canvas.
//!
//! The Apple-Notes shape: a sidebar of note cards on the left (title, date,
//! snippet), the selected note on the right as real type — first line as the
//! title, the rest as body text with a caret. Click a row to switch, type to
//! write, "+ New" to add a note. Everything persists as plain text files in
//! the one directory the user granted (`./notes/`), same paths and format as
//! every earlier version of this app.
//!
//! `#![no_std]` keeps it `krate:*`-only: the SDK owns the allocator and a
//! trapping panic handler, so no path drags in the `wasi:*` import set. All
//! state is fixed-size; no `format!`, `unwrap`, or panicking index.

#![no_std]

extern crate alloc;

extern crate krate as _krate_runtime;

#[allow(warnings)]
mod bindings;

use bindings::krate::fs::files::{self, OpenMode};
use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::ui::{clipboard, events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const W: f32 = 900.0;
const H: f32 = 640.0;
const SIDEBAR_W: f32 = 280.0;

/// Note slots. Fixed so no allocation is needed; the sidebar fits exactly this
/// many rows above the "+ New" button.
const SLOTS: usize = 7;
/// Bytes of text a note holds. Fixed-capacity, same ceiling as always.
const CAP: usize = 512;

/// Same paths and plain-text format as every earlier krate-notes, so notes
/// saved by an older build load unchanged here.
const NOTE_FILES: [&str; SLOTS] = [
    "./notes/first.txt",
    "./notes/second.txt",
    "./notes/third.txt",
    "./notes/fourth.txt",
    "./notes/fifth.txt",
    "./notes/sixth.txt",
    "./notes/seventh.txt",
];

const INITIAL_NOTE_COUNT: usize = 3;

const MAX_WAIT_ROUNDS: u32 = 600_000;
const QUICK_WAIT_ROUNDS: u32 = 30;
const WAIT_ROUND_MILLIS: u32 = 50;
const MAX_IDLE_ROUNDS: u32 = 200;

// ------------------------------------------------------------------
// Palette
// ------------------------------------------------------------------

const BG_TOP: gfx::Color = gfx::Color { r: 0.043, g: 0.055, b: 0.082, a: 1.0 }; // #0B0E15
const BG_BOT: gfx::Color = gfx::Color { r: 0.063, g: 0.078, b: 0.114, a: 1.0 }; // #10141D
const SIDEBAR_BG: gfx::Color = gfx::Color { r: 0.030, g: 0.039, b: 0.059, a: 1.0 }; // darker
const ROW_CARD: gfx::Color = gfx::Color { r: 0.055, g: 0.071, b: 0.104, a: 1.0 };
/// Accent at ~18% over the sidebar ground, pre-blended so the rounded-rect
/// pieces can be laid down at full alpha without doubling up.
const ROW_SELECTED: gfx::Color = gfx::Color { r: 0.079, g: 0.132, b: 0.228, a: 1.0 };
const DIVIDER: gfx::Color = gfx::Color { r: 0.137, g: 0.165, b: 0.220, a: 1.0 }; // #232A38
const INK: gfx::Color = gfx::Color { r: 0.949, g: 0.961, b: 0.980, a: 1.0 }; // #F2F5FA
const INK_BODY: gfx::Color = gfx::Color { r: 0.839, g: 0.867, b: 0.910, a: 1.0 };
const INK_SEC: gfx::Color = gfx::Color { r: 0.604, g: 0.647, b: 0.710, a: 1.0 }; // #9AA5B5
const INK_QUIET: gfx::Color = gfx::Color { r: 0.365, g: 0.408, b: 0.471, a: 1.0 }; // #5D6878
const ACCENT: gfx::Color = gfx::Color { r: 0.298, g: 0.553, b: 1.0, a: 1.0 }; // #4C8DFF
const GHOST_BORDER: gfx::Color = gfx::Color { r: 0.165, g: 0.196, b: 0.251, a: 1.0 };

// ------------------------------------------------------------------
// Sidebar layout
// ------------------------------------------------------------------

const ROW_X: f32 = 12.0;
const ROW_W: f32 = SIDEBAR_W - ROW_X * 2.0;
const ROW_H: f32 = 58.0;
const ROW_GAP: f32 = 6.0;
const ROWS_Y: f32 = 104.0;

const NEW_BTN_H: f32 = 44.0;
const NEW_BTN_Y: f32 = H - 24.0 - NEW_BTN_H;

// Editor layout.
const CONTENT_X: f32 = SIDEBAR_W + 48.0;
const CONTENT_W: f32 = W - CONTENT_X - 44.0; // 528, under the 640 max measure
const TOPBAR_RULE_Y: f32 = 52.0;
const TITLE_BASELINE: f32 = 112.0;
const TITLE_SIZE: f32 = 24.0;
const BODY_SIZE: f32 = 16.0;
const BODY_LH: f32 = 23.0; // ~1.45 line height
const BODY_Y0: f32 = TITLE_BASELINE + 38.0;
const BODY_MAX_LINES: usize = 21;

/// Approximate advance per character for the system face. Measured off a real
/// shot of this app: body text renders at ~0.42-0.44em per char, so 0.44 keeps
/// the caret near the true text end while still wrapping inside the measure.
fn char_w(size: f32) -> f32 {
    size * 0.44
}

/// Rendered width of a string at a given font size, measured by the host with
/// the same font layout `draw_text` draws with.
///
/// This used to be `s.len() * char_w(size)`, byte count times an invented
/// constant. On a proportional face `i` and `W` differ about four times in real
/// width, so a centred label was not centred and the caret sat beside the title
/// rather than after it. `measure_text` is the true answer.
///
/// `char_w` survives below only where it is used as a rough characters-per-line
/// budget, which is a count and not a width.
fn text_width(canvas: u64, s: &str, size: f32) -> f32 {
    match canvas2d::measure_text(canvas, s, size) {
        Ok(m) => m.width,
        Err(_) => 0.0,
    }
}

// ------------------------------------------------------------------
// Note buffers — fixed-capacity, panic-free
// ------------------------------------------------------------------

#[derive(Clone, Copy)]
struct NoteBuf {
    bytes: [u8; CAP],
    len: usize,
}

impl NoteBuf {
    const fn new() -> Self {
        Self { bytes: [0; CAP], len: 0 }
    }

    fn clear(&mut self) {
        self.len = 0;
    }

    fn push(&mut self, byte: u8) {
        if let Some(slot) = self.bytes.get_mut(self.len) {
            *slot = byte;
            self.len += 1;
        }
    }

    /// Append printable ASCII and newlines only; the drawn text path treats a
    /// byte as a character, so anything else is dropped.
    fn push_str(&mut self, text: &str) {
        for byte in text.as_bytes() {
            if byte.is_ascii_graphic() || *byte == b' ' || *byte == b'\n' {
                self.push(*byte);
            }
        }
    }

    fn pop(&mut self) {
        self.len = self.len.saturating_sub(1);
    }

    fn as_str(&self) -> &str {
        let slice = self.bytes.get(..self.len).unwrap_or(&[]);
        core::str::from_utf8(slice).unwrap_or("")
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// The first line of a note is its title.
fn first_line(s: &str) -> &str {
    match s.as_bytes().iter().position(|b| *b == b'\n') {
        Some(pos) => s.get(..pos).unwrap_or(""),
        None => s,
    }
}

/// Everything after the first newline is the body.
fn body_text(s: &str) -> &str {
    match s.as_bytes().iter().position(|b| *b == b'\n') {
        Some(pos) => s.get(pos + 1..).unwrap_or(""),
        None => "",
    }
}

// ------------------------------------------------------------------
// Persistence — same plain-text-per-file scheme as always
// ------------------------------------------------------------------

fn load_note(index: usize, buf: &mut NoteBuf) -> bool {
    buf.clear();
    let Some(path) = NOTE_FILES.get(index) else {
        return false;
    };
    let Ok(file) = files::open(path, OpenMode::Read) else {
        return false;
    };
    while let Ok(chunk) = file.read(CAP as u32) {
        if chunk.is_empty() {
            break;
        }
        for byte in &chunk {
            buf.push(*byte);
        }
    }
    true
}

fn save_note(index: usize, buf: &NoteBuf) -> bool {
    let Some(path) = NOTE_FILES.get(index) else {
        return false;
    };
    let Ok(file) = files::open(path, OpenMode::Write) else {
        return false;
    };
    let bytes = buf.bytes.get(..buf.len).unwrap_or(&[]);
    file.write(bytes).is_ok()
}

// ------------------------------------------------------------------
// Drawing helpers
// ------------------------------------------------------------------

fn fill(canvas: u64, x: f32, y: f32, w: f32, h: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_rect(canvas, gfx::Rect { x, y, width: w, height: h }, c)
}

fn disc(canvas: u64, cx: f32, cy: f32, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_circle(canvas, gfx::Point { x: cx, y: cy }, r, c)
}

/// Opaque rounded rect: cross of two rects plus four corner discs. Pieces
/// overlap, which is fine at full alpha.
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

/// Hairline rounded border: a filled rounded rect in the border color with a
/// slightly smaller rounded rect of the ground punched back over it, so the
/// corners actually curve instead of leaving gaps.
fn stroke_rounded(
    canvas: u64,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
    c: gfx::Color,
    ground: gfx::Color,
) -> Result<(), gfx::GfxError> {
    let t = 1.5;
    rounded_rect(canvas, x, y, w, h, r, c)?;
    rounded_rect(canvas, x + t, y + t, w - t * 2.0, h - t * 2.0, r - t, ground)?;
    Ok(())
}

fn draw_text(canvas: u64, text: &str, x: f32, y: f32, size: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::draw_text(canvas, text, gfx::Point { x, y }, size, c)
}

/// Draw `s` truncated to `max_chars` with a trailing ellipsis when it is cut.
fn draw_text_trunc(
    canvas: u64,
    s: &str,
    max_chars: usize,
    x: f32,
    y: f32,
    size: f32,
    c: gfx::Color,
) -> Result<(), gfx::GfxError> {
    if s.len() <= max_chars {
        return draw_text(canvas, s, x, y, size, c);
    }
    let mut buf = [0u8; 96];
    let mut pos = 0usize;
    let keep = max_chars.saturating_sub(1).min(buf.len().saturating_sub(3));
    for byte in s.as_bytes().iter().take(keep) {
        push_byte(&mut buf, &mut pos, *byte);
    }
    // U+2026 HORIZONTAL ELLIPSIS.
    push_byte(&mut buf, &mut pos, 0xE2);
    push_byte(&mut buf, &mut pos, 0x80);
    push_byte(&mut buf, &mut pos, 0xA6);
    let slice = buf.get(..pos).unwrap_or(&[]);
    if let Ok(txt) = core::str::from_utf8(slice) {
        draw_text(canvas, txt, x, y, size, c)?;
    }
    Ok(())
}

fn push_byte(buf: &mut [u8], pos: &mut usize, byte: u8) {
    if let Some(slot) = buf.get_mut(*pos) {
        *slot = byte;
        *pos += 1;
    }
}

// ------------------------------------------------------------------
// Sidebar snippet — body collapsed onto one line
// ------------------------------------------------------------------

/// Copy the body into `buf` with newlines and runs of spaces collapsed to a
/// single space, capped at `max_chars` with an ellipsis.
fn snippet<'a>(body: &str, buf: &'a mut [u8; 64], max_chars: usize) -> &'a str {
    let mut pos = 0usize;
    let mut last_space = true; // swallow leading whitespace
    let mut truncated = false;
    for byte in body.as_bytes() {
        if pos >= max_chars {
            truncated = true;
            break;
        }
        let b = if *byte == b'\n' { b' ' } else { *byte };
        if b == b' ' {
            if last_space {
                continue;
            }
            last_space = true;
        } else {
            last_space = false;
        }
        push_byte(buf, &mut pos, b);
    }
    if truncated {
        // Drop one char to make room for the ellipsis.
        pos = pos.saturating_sub(1);
        push_byte(buf, &mut pos, 0xE2);
        push_byte(buf, &mut pos, 0x80);
        push_byte(buf, &mut pos, 0xA6);
    }
    let slice = buf.get(..pos).unwrap_or(&[]);
    core::str::from_utf8(slice).unwrap_or("")
}

// ------------------------------------------------------------------
// Body wrap — greedy word wrap over ASCII bytes
// ------------------------------------------------------------------

/// Draw the body word-wrapped. Returns (chars on the last drawn line, baseline
/// of the last drawn line) so the caller can place the caret at the end.
fn draw_body(
    canvas: u64,
    body: &str,
    x: f32,
    y0: f32,
) -> Result<(usize, f32), gfx::GfxError> {
    let budget = (CONTENT_W / char_w(BODY_SIZE)) as usize; // chars per line
    let bytes = body.as_bytes();
    let len = bytes.len();
    let mut start = 0usize;
    let mut line = 0usize;
    let mut last_len = 0usize;
    let mut last_baseline = y0;
    while line < BODY_MAX_LINES {
        let baseline = y0 + (line as f32) * BODY_LH;
        // Find where this line ends.
        let mut end = len;
        let mut next = len + 1; // signals "done" when no break found
        let mut last_space: Option<usize> = None;
        let mut i = start;
        while i < len {
            let b = bytes.get(i).copied().unwrap_or(0);
            if b == b'\n' {
                end = i;
                next = i + 1;
                break;
            }
            if b == b' ' {
                last_space = Some(i);
            }
            if i - start >= budget {
                if let Some(sp) = last_space {
                    if sp > start {
                        end = sp;
                        next = sp + 1;
                        break;
                    }
                }
                end = i;
                next = i;
                break;
            }
            i += 1;
        }
        if let Some(slice) = bytes.get(start..end) {
            if let Ok(txt) = core::str::from_utf8(slice) {
                if !txt.is_empty() {
                    draw_text(canvas, txt, x, baseline, BODY_SIZE, INK_BODY)?;
                }
                last_len = txt.len();
                last_baseline = baseline;
            }
        }
        if next > len {
            break; // consumed the whole body
        }
        start = next;
        line += 1;
        // A trailing newline should still land the caret on the fresh line.
        if start == len {
            last_len = 0;
            last_baseline = y0 + (line as f32) * BODY_LH;
            break;
        }
    }
    Ok((last_len, last_baseline))
}

// ------------------------------------------------------------------
// The frame
// ------------------------------------------------------------------

fn row_top(index: usize) -> f32 {
    ROWS_Y + (index as f32) * (ROW_H + ROW_GAP)
}

fn hit_row(x: f32, y: f32, live: usize) -> Option<usize> {
    if x < ROW_X || x > ROW_X + ROW_W {
        return None;
    }
    let mut i = 0usize;
    while i < live {
        let top = row_top(i);
        if y >= top && y <= top + ROW_H {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn hit_new(x: f32, y: f32) -> bool {
    x >= ROW_X && x <= ROW_X + ROW_W && y >= NEW_BTN_Y && y <= NEW_BTN_Y + NEW_BTN_H
}

fn draw_frame(
    canvas: u64,
    notes: &[NoteBuf; SLOTS],
    dates: &[&str; SLOTS],
    live: usize,
    selected: usize,
) -> Result<(), gfx::GfxError> {
    // Ground: the editor surface gradient, then the darker sidebar over it.
    canvas2d::linear_gradient(
        canvas,
        gfx::Rect { x: 0.0, y: 0.0, width: W, height: H },
        BG_TOP,
        BG_BOT,
    )?;
    fill(canvas, 0.0, 0.0, SIDEBAR_W, H, SIDEBAR_BG)?;
    // Hairline divider between the panes.
    fill(canvas, SIDEBAR_W, 0.0, 1.0, H, DIVIDER)?;

    // ---- sidebar header ----
    draw_text(canvas, "Notes", 24.0, 56.0, 28.0, INK)?;
    let mut cbuf = [0u8; 16];
    let count_txt = count_label(live, &mut cbuf);
    draw_text(canvas, count_txt, 25.0, 80.0, 13.0, INK_QUIET)?;

    // ---- note rows ----
    let title_budget = ((ROW_W - 32.0) / char_w(16.0)) as usize;
    let mut i = 0usize;
    while i < live {
        let top = row_top(i);
        let is_sel = i == selected;
        let card = if is_sel { ROW_SELECTED } else { ROW_CARD };
        rounded_rect(canvas, ROW_X, top, ROW_W, ROW_H, 10.0, card)?;
        if is_sel {
            // The 3px accent bar on the row's left edge.
            rounded_rect(canvas, ROW_X + 4.0, top + 10.0, 3.0, ROW_H - 20.0, 1.5, ACCENT)?;
        }
        let note = notes.get(i).copied().unwrap_or(NoteBuf::new());
        let text = note.as_str();
        let title = first_line(text);
        let tx = ROW_X + 16.0;
        if title.is_empty() {
            draw_text(canvas, "New Note", tx, top + 24.0, 16.0, INK_QUIET)?;
        } else {
            draw_text_trunc(canvas, title, title_budget, tx, top + 24.0, 16.0, INK)?;
        }
        // Second line: date in secondary, snippet in quiet. The date width is
        // over-estimated on purpose (0.55em) so the snippet never crowds it,
        // and the snippet budget under-fills (0.5em) so it never touches the
        // card's right edge.
        let date = dates.get(i).copied().unwrap_or("Today");
        draw_text(canvas, date, tx, top + 45.0, 12.5, INK_SEC)?;
        let date_w = (date.len() as f32) * 12.5 * 0.55;
        let mut sbuf = [0u8; 64];
        let remaining = ROW_W - 32.0 - date_w - 10.0;
        let snip_budget = (remaining / (12.5 * 0.5)) as usize;
        let snip = snippet(body_text(text), &mut sbuf, snip_budget.min(60));
        if snip.is_empty() {
            draw_text(canvas, "No additional text", tx + date_w + 10.0, top + 45.0, 12.5, INK_QUIET)?;
        } else {
            draw_text(canvas, snip, tx + date_w + 10.0, top + 45.0, 12.5, INK_QUIET)?;
        }
        i += 1;
    }

    // ---- "+ New" ghost button ----
    stroke_rounded(canvas, ROW_X, NEW_BTN_Y, ROW_W, NEW_BTN_H, 12.0, GHOST_BORDER, SIDEBAR_BG)?;
    let label = "+ New";
    let lw = text_width(canvas, label, 15.0);
    draw_text(
        canvas,
        label,
        ROW_X + (ROW_W - lw) * 0.5,
        NEW_BTN_Y + NEW_BTN_H * 0.5 + 5.0,
        15.0,
        ACCENT,
    )?;

    // ---- editor top bar: the note's date, centered, with a hairline ----
    let date = dates.get(selected).copied().unwrap_or("Today");
    let mut dbuf = [0u8; 32];
    let date_line = edited_label(date, &mut dbuf);
    let dw = text_width(canvas, date_line, 12.5);
    draw_text(
        canvas,
        date_line,
        SIDEBAR_W + ((W - SIDEBAR_W) - dw) * 0.5,
        32.0,
        12.5,
        INK_QUIET,
    )?;
    fill(
        canvas,
        SIDEBAR_W + 1.0,
        TOPBAR_RULE_Y,
        W - SIDEBAR_W - 1.0,
        1.0,
        gfx::Color { r: 0.137, g: 0.165, b: 0.220, a: 0.6 },
    )?;

    // ---- the selected note ----
    let note = notes.get(selected).copied().unwrap_or(NoteBuf::new());
    let text = note.as_str();
    let title = first_line(text);
    let title_budget = (CONTENT_W / char_w(TITLE_SIZE)) as usize;
    let (caret_x, caret_y);
    if text.is_empty() {
        draw_text(canvas, "New Note", CONTENT_X, TITLE_BASELINE, TITLE_SIZE, INK_QUIET)?;
        caret_x = CONTENT_X;
        caret_y = TITLE_BASELINE;
    } else {
        draw_text_trunc(canvas, title, title_budget, CONTENT_X, TITLE_BASELINE, TITLE_SIZE, INK)?;
        let body = body_text(text);
        let has_break = text.len() > title.len(); // a newline exists
        if !has_break {
            // Still typing the first line: caret rides the title.
            caret_x = CONTENT_X + text_width(canvas, title, TITLE_SIZE);
            caret_y = TITLE_BASELINE;
        } else if body.is_empty() {
            caret_x = CONTENT_X;
            caret_y = BODY_Y0;
        } else {
            let (last_len, last_baseline) = draw_body(canvas, body, CONTENT_X, BODY_Y0)?;
            caret_x = CONTENT_X + (last_len as f32) * char_w(BODY_SIZE);
            caret_y = last_baseline;
        }
    }
    // The caret: a slim accent bar hanging from the last baseline.
    let caret_h = if caret_y <= TITLE_BASELINE + 0.5 { 24.0 } else { 18.0 };
    fill(canvas, caret_x + 1.0, caret_y - caret_h + 3.0, 2.0, caret_h, ACCENT)?;

    canvas2d::present(canvas)?;
    Ok(())
}

/// "3 notes" (or "1 note").
fn count_label<'a>(live: usize, buf: &'a mut [u8; 16]) -> &'a str {
    let mut pos = 0usize;
    let n = live.min(99);
    if n >= 10 {
        push_byte(buf, &mut pos, b'0' + (n / 10) as u8);
    }
    push_byte(buf, &mut pos, b'0' + (n % 10) as u8);
    for byte in if n == 1 { b" note".as_slice() } else { b" notes".as_slice() } {
        push_byte(buf, &mut pos, *byte);
    }
    let slice = buf.get(..pos).unwrap_or(&[]);
    core::str::from_utf8(slice).unwrap_or("notes")
}

/// "Edited Today" and friends for the top bar.
fn edited_label<'a>(date: &str, buf: &'a mut [u8; 32]) -> &'a str {
    let mut pos = 0usize;
    for byte in b"Edited " {
        push_byte(buf, &mut pos, *byte);
    }
    for byte in date.as_bytes().iter().take(20) {
        push_byte(buf, &mut pos, *byte);
    }
    let slice = buf.get(..pos).unwrap_or(&[]);
    core::str::from_utf8(slice).unwrap_or("Edited")
}

// ------------------------------------------------------------------
// Seeds
// ------------------------------------------------------------------

const SEED_TEXTS: [&str; INITIAL_NOTE_COUNT] = [
    "Ship the demo\nRecord the 90-second run-through, tighten the intro, and send the link before Friday standup. Keep the energy up in the first ten seconds - that is where people decide to keep watching.\n\nCut the settings tour. Nobody asked for the settings tour.",
    "Groceries\nOat milk, espresso beans, basil, lemons, arborio rice, parmesan, olive oil, dark chocolate, sourdough.",
    "Ideas\nVoice memos that turn into checklists. A weekly review template. Pixel-art icon pack for the store. Tiny apps that do one thing well.",
];
const SEED_DATES: [&str; INITIAL_NOTE_COUNT] = ["Today", "Yesterday", "Sunday"];

// ------------------------------------------------------------------
// Widget tree — one canvas filling the window
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
// Entry point
// ------------------------------------------------------------------

struct Component;

impl bindings::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize {
            width: W as u32,
            height: H as u32,
        };
        let Ok(win) = window::create("Notes", size) else {
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

        let mut notes = [NoteBuf::new(); SLOTS];
        let mut dates: [&str; SLOTS] = ["Today"; SLOTS];
        let mut live = INITIAL_NOTE_COUNT;
        let mut selected = 0usize;
        let mut saved_any = false;
        let mut dirty = false;
        let mut close_requested = false;

        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");

        if quick {
            // The store screenshot: three realistic notes, first selected,
            // saved to disk to prove the write path.
            let mut i = 0usize;
            while i < INITIAL_NOTE_COUNT {
                if let (Some(buf), Some(text)) = (notes.get_mut(i), SEED_TEXTS.get(i)) {
                    buf.clear();
                    buf.push_str(text);
                    if save_note(i, buf) {
                        saved_any = true;
                    }
                }
                if let (Some(slot), Some(date)) = (dates.get_mut(i), SEED_DATES.get(i)) {
                    *slot = date;
                }
                i += 1;
            }
        } else {
            // Load what exists; reveal any notes beyond the initial three that
            // an earlier session created.
            let mut found_any = false;
            let mut i = 0usize;
            while i < INITIAL_NOTE_COUNT {
                if let Some(buf) = notes.get_mut(i) {
                    if load_note(i, buf) && !buf.is_empty() {
                        found_any = true;
                    }
                }
                i += 1;
            }
            while live < SLOTS {
                let Some(buf) = notes.get_mut(live) else {
                    break;
                };
                if !load_note(live, buf) {
                    break;
                }
                live += 1;
            }
            if !found_any {
                // Fresh install: welcome content instead of three blank rows.
                let mut j = 0usize;
                while j < INITIAL_NOTE_COUNT {
                    if let (Some(buf), Some(text)) = (notes.get_mut(j), SEED_TEXTS.get(j)) {
                        buf.clear();
                        buf.push_str(text);
                        if save_note(j, buf) {
                            saved_any = true;
                        }
                    }
                    if let (Some(slot), Some(date)) = (dates.get_mut(j), SEED_DATES.get(j)) {
                        *slot = date;
                    }
                    j += 1;
                }
            }
        }

        if draw_frame(canvas, &notes, &dates, live, selected).is_err() {
            let _ = window::close(win);
            return 34;
        }

        let rounds = if quick { QUICK_WAIT_ROUNDS } else { MAX_WAIT_ROUNDS };
        let mut idle_rounds = 0u32;
        let mut round = 0u32;
        while round < rounds {
            round += 1;
            let event = events::wait(Some(WAIT_ROUND_MILLIS));
            if event.is_none() {
                if quick {
                    continue;
                }
                idle_rounds += 1;
                if idle_rounds >= MAX_IDLE_ROUNDS {
                    break;
                }
                continue;
            }
            idle_rounds = 0;
            let mut redraw = false;
            match event {
                Some(types::Event::Pointer(p)) if p.pressed => {
                    if let Some(index) = hit_row(p.x, p.y, live) {
                        if index != selected {
                            // Switching saves the note being edited first, so a
                            // click never loses work.
                            if dirty {
                                if save_note(selected, notes.get(selected).unwrap_or(&NoteBuf::new())) {
                                    saved_any = true;
                                }
                                dirty = false;
                            }
                            selected = index;
                            redraw = true;
                        }
                    } else if hit_new(p.x, p.y) && live < SLOTS {
                        if dirty {
                            if save_note(selected, notes.get(selected).unwrap_or(&NoteBuf::new())) {
                                saved_any = true;
                            }
                            dirty = false;
                        }
                        selected = live;
                        live += 1;
                        if let Some(buf) = notes.get_mut(selected) {
                            buf.clear();
                        }
                        if let Some(slot) = dates.get_mut(selected) {
                            *slot = "Today";
                        }
                        redraw = true;
                    }
                }
                Some(types::Event::TextInput(text)) => {
                    if let Some(buf) = notes.get_mut(selected) {
                        buf.push_str(&text);
                        dirty = true;
                        redraw = true;
                    }
                }
                Some(types::Event::Key(key)) if key.pressed => {
                    let chord = key.modifiers.control || key.modifiers.meta;
                    let k = key.key.as_bytes();
                    if chord {
                        match k {
                            b"s" | b"S" => {
                                if save_note(selected, notes.get(selected).unwrap_or(&NoteBuf::new())) {
                                    saved_any = true;
                                    dirty = false;
                                }
                            }
                            b"v" | b"V" => {
                                if let Ok(text) = clipboard::read_text() {
                                    if !text.is_empty() {
                                        if let Some(buf) = notes.get_mut(selected) {
                                            buf.push_str(&text);
                                            dirty = true;
                                            redraw = true;
                                        }
                                    }
                                }
                            }
                            b"c" | b"C" => {
                                if let Some(buf) = notes.get(selected) {
                                    if !buf.is_empty() {
                                        let _ = clipboard::write_text(buf.as_str());
                                    }
                                }
                            }
                            _ => {}
                        }
                    } else {
                        match k {
                            b"Backspace" => {
                                if let Some(buf) = notes.get_mut(selected) {
                                    if !buf.is_empty() {
                                        buf.pop();
                                        dirty = true;
                                        redraw = true;
                                    }
                                }
                            }
                            b"Enter" | b"Return" => {
                                if let Some(buf) = notes.get_mut(selected) {
                                    buf.push(b'\n');
                                    dirty = true;
                                    redraw = true;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Some(types::Event::CloseRequested(_)) => {
                    close_requested = true;
                }
                _ => {}
            }
            if redraw {
                let _ = draw_frame(canvas, &notes, &dates, live, selected);
            }
            if close_requested {
                break;
            }
        }

        // Never lose the note being edited on the way out — but also never
        // erase a file by writing an empty buffer over it.
        if dirty {
            if let Some(buf) = notes.get(selected) {
                if !buf.is_empty() && save_note(selected, buf) {
                    saved_any = true;
                }
            }
        }

        let _ = window::close(win);

        let out = stdio::stdout();
        let _ = out.write(b"note:");
        if let Some(buf) = notes.get(selected) {
            let title = first_line(buf.as_str());
            let cut = title.get(..title.len().min(40)).unwrap_or("");
            let _ = out.write(cut.as_bytes());
        }
        let _ = out.write(b"\n");
        if saved_any {
            let _ = out.write(b"saved:yes\n");
        }

        if close_requested {
            2
        } else {
            0
        }
    }
}

bindings::export!(Component with_types_in bindings);
