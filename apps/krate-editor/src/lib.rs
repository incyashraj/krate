//! Editor -- a code editor window, drawn on one canvas.
//!
//! The layout every developer already knows: a file tree down the left with
//! the open file marked, two tabs across the top, the file itself in the middle
//! with line numbers, Rust syntax colours, a current-line wash, a selection and
//! a blinking caret, a minimap down the right edge, and a status bar that says
//! where the caret is.
//!
//! It is alive on the clock. Every frame is a pure function of the time since
//! the app started: the caret blinks on a 530 ms cycle, and from half a second
//! in the editor types three new lines at a human cadence, auto-indenting after
//! each Enter and scrolling one line so the caret stays put. A headless shoot
//! at any `KRATE_SHOOT_AFTER_MS` therefore shows exactly that moment, and a
//! sequence of shoots is footage.
//!
//! Text is drawn one colour run at a time, so a line of six tokens costs six
//! host calls (K-227). Only the lines inside the viewport are drawn, and
//! whitespace and punctuation merge into the run beside them, which keeps a
//! full screen to roughly 250 calls.
//!
//! `#![no_std]`: the SDK owns the allocator and a trapping panic handler, all
//! state is fixed-size, and nothing indexes with `[]`.

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

const WIDTH: f32 = 1180.0;
const HEIGHT: f32 = 760.0;

const QUICK_FRAMES: u32 = 8;

// ---- palette (shared with Query and Trace) ---------------------------------

const BG: gfx::Color = rgb(0.055, 0.063, 0.078);
const RAIL: gfx::Color = rgb(0.075, 0.086, 0.106);
const PANE: gfx::Color = rgb(0.086, 0.098, 0.122);
const EDITOR: gfx::Color = rgb(0.067, 0.078, 0.098);
const LINE: gfx::Color = rgb(0.141, 0.161, 0.196);
const LINE_SOFT: gfx::Color = rgb(0.110, 0.126, 0.157);
/// The line the caret is on, one step up from the editor ground.
const CUR_LINE: gfx::Color = rgb(0.090, 0.102, 0.128);

const INK: gfx::Color = rgb(0.902, 0.925, 0.957);
const INK_DIM: gfx::Color = rgb(0.573, 0.616, 0.686);
const INK_QUIET: gfx::Color = rgb(0.373, 0.412, 0.478);

const ACCENT: gfx::Color = rgb(0.353, 0.596, 1.0);
/// Pre-blended: a translucent rounded shape doubles alpha where the rect and
/// its corner discs overlap, so selections use flattened opaque tints.
const SEL_WASH: gfx::Color = rgb(0.129, 0.180, 0.286);
const GREEN: gfx::Color = rgb(0.310, 0.827, 0.549);
const VIEWPORT: gfx::Color = gfx::Color {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 0.055,
};

// Syntax colours. Keywords take the same purple as Query's SQL keywords so
// the two apps read as one family; the rest are the roles an editor needs to
// tell apart at a glance.
const C_KEYWORD: gfx::Color = rgb(0.780, 0.545, 0.980);
const C_TYPE: gfx::Color = rgb(0.910, 0.760, 0.400);
const C_STRING: gfx::Color = rgb(0.596, 0.812, 0.475);
const C_NUMBER: gfx::Color = rgb(0.949, 0.620, 0.420);
const C_MACRO: gfx::Color = ACCENT;
const C_FN: gfx::Color = rgb(0.470, 0.730, 0.980);
const C_COMMENT: gfx::Color = rgb(0.420, 0.470, 0.550);
const C_ATTR: gfx::Color = rgb(0.560, 0.640, 0.760);
const C_IDENT: gfx::Color = rgb(0.847, 0.882, 0.925);
const C_PUNCT: gfx::Color = INK_DIM;

const fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
}

// ---- the open file ---------------------------------------------------------

/// `src/lib.rs` as it stands when the window opens. Line 42 (index 41) is the
/// one the developer is in the middle of; the typing script below continues
/// it. Everything after is what a small Krate canvas app looks like.
const LINES: [&str; 115] = [
    "//! Pulse -- a live metrics card, drawn on one canvas.",
    "//!",
    "//! Every frame is a pure function of the clock. Nothing on screen is",
    "//! animated by state that could drift, so a window that sat covered for",
    "//! a minute resumes exactly where the clock says it should be.",
    "",
    "#![no_std]",
    "",
    "extern crate alloc;",
    "",
    "#[allow(warnings)]",
    "mod bindings;",
    "",
    "use bindings::krate::gfx::{canvas2d, types as gfx};",
    "use bindings::krate::time::clock;",
    "use bindings::krate::ui::{events, tree, types, window};",
    "use krate::motion::{ease_out, Spring};",
    "",
    "const WIDTH: f32 = 1180.0;",
    "const HEIGHT: f32 = 760.0;",
    "const FRAME_NANOS: u64 = 1_000_000_000 / 60;",
    "",
    "/// One ground, one accent, one ink. Everything else is a tint of these.",
    "const BG: gfx::Color = rgb(0.055, 0.063, 0.078);",
    "const ACCENT: gfx::Color = rgb(0.231, 0.459, 1.0);",
    "const INK: gfx::Color = rgb(0.902, 0.925, 0.957);",
    "",
    "const fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {",
    "    gfx::Color { r, g, b, a: 1.0 }",
    "}",
    "",
    "pub struct Card {",
    "    samples: [f32; 240],",
    "    head: usize,",
    "    level: Spring,",
    "    frames: u32,",
    "}",
    "",
    "impl Card {",
    "    /// Advance one frame. A stall must not teleport the spring.",
    "    pub fn tick(&mut self, target: f32, dt: f32) {",
    "        let dt =",
    "    }",
    "",
    "    /// Push one reading; the ring overwrites the oldest sample.",
    "    pub fn push(&mut self, value: f32) {",
    "        if let Some(slot) = self.samples.get_mut(self.head) {",
    "            *slot = value;",
    "        }",
    "        self.head = (self.head + 1) % self.samples.len();",
    "    }",
    "",
    "    pub fn new() -> Self {",
    "        Card {",
    "            samples: [0.0; 240],",
    "            head: 0,",
    "            level: Spring::rest_at(0.0, 20.0),",
    "            frames: 0,",
    "        }",
    "    }",
    "}",
    "",
    "fn draw(canvas: u64, card: &Card, t: f32) -> Result<(), gfx::GfxError> {",
    "    canvas2d::clear(canvas, BG)?;",
    "",
    "    // The chart: one bar per sample, newest at the right edge.",
    "    let bar_w = WIDTH / card.samples.len() as f32;",
    "    for i in 0..card.samples.len() {",
    "        let index = (card.head + i) % card.samples.len();",
    "        let Some(value) = card.samples.get(index) else { break };",
    "        let h = (value * 220.0).max(2.0);",
    "        let x = i as f32 * bar_w;",
    "        let area = gfx::Rect { x, y: HEIGHT - 80.0 - h, width: bar_w - 1.0, height: h };",
    "        canvas2d::fill_rect(canvas, area, ACCENT)?;",
    "    }",
    "",
    "    // The headline eases in over the first half second.",
    "    let reveal = ease_out((t * 2.0).min(1.0));",
    "    let y = 96.0 + (1.0 - reveal) * 12.0;",
    "    canvas2d::draw_text(canvas, \"p99 latency\", gfx::Point { x: 48.0, y }, 13.0, INK)?;",
    "    canvas2d::present(canvas)",
    "}",
    "",
    "struct Component;",
    "",
    "impl bindings::Guest for Component {",
    "    fn run() -> i32 {",
    "        let size = types::WindowSize { width: WIDTH as u32, height: HEIGHT as u32 };",
    "        let Ok(win) = window::create(\"Pulse\", size) else { return 30 };",
    "        let _ = window::show(win);",
    "        let Ok(canvas) = canvas2d::bind(win, 2) else { return 33 };",
    "",
    "        let mut card = Card::new();",
    "        let started = clock::monotonic_nanos();",
    "        let mut next_frame = started;",
    "        loop {",
    "            let now = clock::monotonic_nanos();",
    "            let t = now.saturating_sub(started) as f32 / 1e9;",
    "            card.push(sample_at(t));",
    "            card.tick(0.72, 1.0 / 60.0);",
    "            if draw(canvas, &card, t).is_err() {",
    "                return 1;",
    "            }",
    "            next_frame = next_frame.saturating_add(FRAME_NANOS);",
    "            let wait = next_frame.saturating_sub(clock::monotonic_nanos()) / 1_000_000;",
    "            if let Some(types::Event::CloseRequested(_)) = events::wait(Some(wait as u32)) {",
    "                break;",
    "            }",
    "        }",
    "        let _ = window::close(win);",
    "        0",
    "    }",
    "}",
    "",
    "bindings::export!(Component with_types_in bindings);",
];

/// Index of the line being typed into. It is 16 characters long, so the caret
/// starts at column 17.
const EDIT_LINE: usize = 41;
/// The selection when the window opens: from line 40 column 5 to the caret.
const SEL_START_LINE: usize = 39;
const SEL_START_COL: usize = 4;

/// What gets typed, one byte per keystroke. `\x01` is a Right-arrow that
/// collapses the selection; `\n` is Enter, after which the editor auto-indents
/// the new line to eight spaces.
const TYPED: &str = "\x01 dt.min(0.05);\nself.level.tick(target, dt);\nself.frames += 1;";
const AUTO_INDENT: usize = 8;

/// Typing starts here, in milliseconds after the app started.
const TYPE_START_MS: u64 = 450;
/// The caret's blink half-period.
const BLINK_MS: u64 = 530;
/// A typewriter scroll: one line per Enter, eased over this long.
const SCROLL_MS: u64 = 200;

/// Where the caret line sits on screen -- row 19 of about 29 visible rows, so
/// there is code both above and below it.
const CARET_ROW: usize = 19;

/// Words the highlighter colours as keywords.
const KEYWORDS: [&str; 38] = [
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where",
    "while", "dyn", "async", "await", "Self",
];

// ---- layout ----------------------------------------------------------------

const TITLEBAR_H: f32 = 44.0;
const RAIL_W: f32 = 220.0;
const TABS_H: f32 = 38.0;
const STATUS_H: f32 = 30.0;
const MINIMAP_W: f32 = 96.0;
const GUTTER_W: f32 = 64.0;

const EDITOR_X: f32 = RAIL_W;
const EDITOR_Y: f32 = TITLEBAR_H + TABS_H;
const EDITOR_W: f32 = WIDTH - RAIL_W - MINIMAP_W;
const STATUS_Y: f32 = HEIGHT - STATUS_H;
const EDITOR_H: f32 = STATUS_Y - EDITOR_Y;
const TEXT_X: f32 = EDITOR_X + GUTTER_W + 8.0;
const MINIMAP_X: f32 = WIDTH - MINIMAP_W;

const LINE_H: f32 = 22.0;
const CODE_SIZE: f32 = 13.0;
const MM_LINE_H: f32 = 3.0;
const MM_CHAR_W: f32 = 1.05;

// ---- tiny drawing + text helpers -------------------------------------------

fn fill(canvas: u64, x: f32, y: f32, w: f32, h: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_rect(
        canvas,
        gfx::Rect {
            x,
            y,
            width: w,
            height: h,
        },
        c,
    )
}

fn disc(canvas: u64, x: f32, y: f32, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_circle(canvas, gfx::Point { x, y }, r, c)
}

fn text(canvas: u64, s: &str, x: f32, y: f32, size: f32, c: gfx::Color) {
    let _ = canvas2d::draw_text(canvas, s, gfx::Point { x, y }, size, c);
}

fn mono_style(italic: bool) -> gfx::TextStyle {
    gfx::TextStyle {
        weight: 400,
        italic,
        letter_spacing: 0.0,
        family: gfx::FontFamily::Mono,
    }
}

fn code(canvas: u64, s: &str, x: f32, y: f32, c: gfx::Color, italic: bool) {
    let _ = canvas2d::draw_text_styled(
        canvas,
        s,
        gfx::Point { x, y },
        CODE_SIZE,
        c,
        mono_style(italic),
    );
}

/// Rendered width, measured by the host with the same layout `draw_text` uses.
fn est_width(canvas: u64, s: &str, size: f32) -> f32 {
    match canvas2d::measure_text(canvas, s, size) {
        Ok(m) => m.width,
        Err(_) => 0.0,
    }
}

fn text_right(canvas: u64, s: &str, right: f32, y: f32, size: f32, c: gfx::Color) {
    text(canvas, s, right - est_width(canvas, s, size), y, size, c);
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
        gfx::Rect {
            x,
            y,
            width: w,
            height: h,
        },
        gfx::CornerRadii {
            top_left: r,
            top_right: r,
            bottom_right: r,
            bottom_left: r,
        },
        c,
    )
}

fn stroke_rounded(canvas: u64, x: f32, y: f32, w: f32, h: f32, r: f32, c: gfx::Color) {
    let _ = canvas2d::stroke_round_rect(
        canvas,
        gfx::Rect {
            x,
            y,
            width: w,
            height: h,
        },
        gfx::CornerRadii {
            top_left: r,
            top_right: r,
            bottom_right: r,
            bottom_left: r,
        },
        1.0,
        c,
    );
}

/// Build an owned `String` without touching std's allocation-error handler.
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

/// Format an unsigned integer into `buf`, returning the used slice.
fn digits(buf: &mut [u8; 24], mut n: u64) -> &str {
    let mut tmp = [0u8; 20];
    let mut count = 0usize;
    loop {
        if let Some(slot) = tmp.get_mut(count) {
            *slot = b'0' + (n % 10) as u8;
            count += 1;
        }
        n /= 10;
        if n == 0 {
            break;
        }
    }
    let mut out = 0usize;
    let mut i = count;
    while i > 0 {
        i -= 1;
        if let (Some(src), Some(dst)) = (tmp.get(i), buf.get_mut(out)) {
            *dst = *src;
            out += 1;
        }
    }
    core::str::from_utf8(buf.get(..out).unwrap_or(b"0")).unwrap_or("0")
}

fn as_str(bytes: &[u8], len: usize) -> &str {
    core::str::from_utf8(bytes.get(..len).unwrap_or(&[])).unwrap_or("")
}

fn out(line: &str) {
    let stdout = stdio::stdout();
    let _ = stdout.write(line.as_bytes());
    let _ = stdout.write(b"\n");
}

fn out_number(key: &str, n: u64) {
    let stdout = stdio::stdout();
    let mut buf = [0u8; 24];
    let _ = stdout.write(key.as_bytes());
    let _ = stdout.write(digits(&mut buf, n).as_bytes());
    let _ = stdout.write(b"\n");
}

/// Cubic ease-out: fast start, soft landing.
fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let u = 1.0 - t;
    1.0 - u * u * u
}

/// A stable pseudo-random 0..n for keystroke jitter, so every run types with
/// the same rhythm and a frame sequence is reproducible.
fn jitter(i: usize, n: u64) -> u64 {
    let mut x = (i as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(0x1234_5678);
    x ^= x >> 29;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 32;
    x % n
}

// ---- the moment: everything the clock decides -------------------------------

const MAX_TYPED_LINES: usize = 4;
const TYPED_CAP: usize = 96;
const MAX_ENTERS: usize = 4;

/// The document and caret at one instant. Built fresh every frame from the
/// elapsed time alone, which is what makes a headless shoot at any delay show
/// exactly that moment.
struct Moment {
    /// The line being edited and those typed after it, first one included.
    typed: [[u8; TYPED_CAP]; MAX_TYPED_LINES],
    lens: [usize; MAX_TYPED_LINES],
    count: usize,
    /// Keystrokes that produced a character, and when the last landed.
    chars: usize,
    last_key_ms: Option<u64>,
    /// Enter presses so far, and when each landed (for the scroll).
    enters: usize,
    enter_at: [u64; MAX_ENTERS],
    /// Whether the opening selection is still there.
    selecting: bool,
    caret_on: bool,
    scroll_px: f32,
}

fn key_delay(i: usize, byte: u8, prev: u8) -> u64 {
    let mut delay = 44 + jitter(i, 34);
    if byte == b' ' {
        delay += 30;
    }
    if byte == b'\n' {
        delay = 190 + jitter(i, 40);
    }
    if prev == b'.' || prev == b'(' || prev == b',' {
        delay += 22;
    }
    delay
}

fn moment(t_ms: u64) -> Moment {
    let mut m = Moment {
        typed: [[0u8; TYPED_CAP]; MAX_TYPED_LINES],
        lens: [0usize; MAX_TYPED_LINES],
        count: 1,
        chars: 0,
        last_key_ms: None,
        enters: 0,
        enter_at: [0u64; MAX_ENTERS],
        selecting: true,
        caret_on: true,
        scroll_px: 0.0,
    };

    // The edited line starts as the file has it.
    let base = LINES.get(EDIT_LINE).copied().unwrap_or("");
    let mut n = 0usize;
    for b in base.bytes() {
        if let Some(slot) = m.typed.get_mut(0).and_then(|row| row.get_mut(n)) {
            *slot = b;
            n += 1;
        }
    }
    if let Some(len) = m.lens.get_mut(0) {
        *len = n;
    }

    // Replay the script up to now.
    let mut elapsed = TYPE_START_MS;
    let mut prev = 0u8;
    for (i, byte) in TYPED.bytes().enumerate() {
        elapsed += key_delay(i, byte, prev);
        prev = byte;
        if elapsed > t_ms {
            break;
        }
        m.last_key_ms = Some(elapsed);
        match byte {
            0x01 => m.selecting = false,
            b'\n' => {
                if m.count < MAX_TYPED_LINES {
                    if let Some(slot) = m.enter_at.get_mut(m.enters) {
                        *slot = elapsed;
                        m.enters += 1;
                    }
                    let line = m.count;
                    m.count += 1;
                    let mut k = 0usize;
                    while k < AUTO_INDENT {
                        if let Some(slot) = m.typed.get_mut(line).and_then(|row| row.get_mut(k)) {
                            *slot = b' ';
                        }
                        k += 1;
                    }
                    if let Some(len) = m.lens.get_mut(line) {
                        *len = AUTO_INDENT;
                    }
                }
            }
            _ => {
                let line = m.count - 1;
                let len = m.lens.get(line).copied().unwrap_or(0);
                if let Some(slot) = m.typed.get_mut(line).and_then(|row| row.get_mut(len)) {
                    *slot = byte;
                    if let Some(l) = m.lens.get_mut(line) {
                        *l = len + 1;
                    }
                    m.chars += 1;
                }
            }
        }
    }

    // The caret is solid while typing and blinks when idle.
    m.caret_on = match m.last_key_ms {
        Some(at) if t_ms.saturating_sub(at) < 500 => true,
        _ => (t_ms / BLINK_MS) % 2 == 0,
    };

    // Typewriter scroll: each Enter eases the view down one line so the caret
    // row never moves.
    let mut scroll = (EDIT_LINE - CARET_ROW) as f32 * LINE_H;
    for i in 0..m.enters {
        let at = m.enter_at.get(i).copied().unwrap_or(0);
        let f = t_ms.saturating_sub(at) as f32 / SCROLL_MS as f32;
        scroll += LINE_H * ease_out(f);
    }
    m.scroll_px = scroll;
    m
}

impl Moment {
    fn total_lines(&self) -> usize {
        LINES.len() + self.count - 1
    }

    /// The composite document: the file with the typed lines spliced in.
    fn line(&self, i: usize) -> &str {
        if i < EDIT_LINE {
            return LINES.get(i).copied().unwrap_or("");
        }
        let k = i - EDIT_LINE;
        if k < self.count {
            let len = self.lens.get(k).copied().unwrap_or(0);
            return self.typed.get(k).map(|row| as_str(row, len)).unwrap_or("");
        }
        LINES.get(i - self.count + 1).copied().unwrap_or("")
    }

    fn caret_line(&self) -> usize {
        EDIT_LINE + self.count - 1
    }

    fn caret_col(&self) -> usize {
        self.lens.get(self.count - 1).copied().unwrap_or(0)
    }
}

// ---- syntax --------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Cls {
    Ws,
    Punct,
    Ident,
    Keyword,
    Type,
    Fn,
    Macro,
    Str,
    Num,
    Comment,
    Attr,
}

fn cls_colour(c: Cls) -> gfx::Color {
    match c {
        Cls::Keyword => C_KEYWORD,
        Cls::Type => C_TYPE,
        Cls::Fn => C_FN,
        Cls::Macro => C_MACRO,
        Cls::Str => C_STRING,
        Cls::Num => C_NUMBER,
        Cls::Comment => C_COMMENT,
        Cls::Attr => C_ATTR,
        Cls::Ident => C_IDENT,
        Cls::Punct | Cls::Ws => C_PUNCT,
    }
}

#[derive(Clone, Copy)]
struct Run {
    start: usize,
    end: usize,
    cls: Cls,
}

const MAX_RUNS: usize = 48;
const NO_RUN: Run = Run {
    start: 0,
    end: 0,
    cls: Cls::Ws,
};

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_keyword(word: &str) -> bool {
    for k in KEYWORDS.iter() {
        if *k == word {
            return true;
        }
    }
    false
}

/// Split one line into coloured runs. Adjacent runs of one colour are merged,
/// and whitespace and punctuation join whatever run precedes them, so a line
/// costs as few `draw-text` calls as its colour changes.
fn tokenize(line: &str, runs: &mut [Run; MAX_RUNS]) -> usize {
    let bytes = line.as_bytes();
    let n = bytes.len();
    let mut count = 0usize;
    let mut i = 0usize;

    let at = |k: usize| bytes.get(k).copied().unwrap_or(0);

    while i < n {
        let b = at(i);
        let start = i;
        let cls;

        if b == b'/' && at(i + 1) == b'/' {
            i = n;
            cls = Cls::Comment;
        } else if b == b'"' {
            i += 1;
            while i < n {
                let c = at(i);
                i += 1;
                if c == b'\\' {
                    i += 1;
                } else if c == b'"' {
                    break;
                }
            }
            cls = Cls::Str;
        } else if b == b'#' && at(i + 1) == b'[' || b == b'#' && at(i + 1) == b'!' {
            while i < n && at(i) != b']' {
                i += 1;
            }
            i += 1;
            cls = Cls::Attr;
        } else if b == b'\'' && is_ident_start(at(i + 1)) && at(i + 2) != b'\'' {
            i += 1;
            while i < n && is_ident_byte(at(i)) {
                i += 1;
            }
            cls = Cls::Type;
        } else if b == b'\'' {
            i += 1;
            while i < n && at(i) != b'\'' {
                i += 1;
            }
            i += 1;
            cls = Cls::Str;
        } else if b.is_ascii_digit() {
            while i < n && (is_ident_byte(at(i)) || (at(i) == b'.' && at(i + 1).is_ascii_digit())) {
                i += 1;
            }
            cls = Cls::Num;
        } else if is_ident_start(b) {
            while i < n && is_ident_byte(at(i)) {
                i += 1;
            }
            let word = line.get(start..i).unwrap_or("");
            if at(i) == b'!' {
                i += 1;
                cls = Cls::Macro;
            } else if is_keyword(word) {
                cls = Cls::Keyword;
            } else if b.is_ascii_uppercase() {
                cls = Cls::Type;
            } else if at(i) == b'(' {
                cls = Cls::Fn;
            } else {
                cls = Cls::Ident;
            }
        } else if b == b' ' || b == b'\t' {
            while i < n && (at(i) == b' ' || at(i) == b'\t') {
                i += 1;
            }
            cls = Cls::Ws;
        } else {
            i += 1;
            cls = Cls::Punct;
        }

        let end = i.min(n).max(start + 1);
        i = end;

        // Merge into the previous run when the colour would not change.
        let merged = match count.checked_sub(1).and_then(|k| runs.get_mut(k)) {
            Some(prev) => {
                let joinable =
                    prev.cls == cls || cls == Cls::Ws || cls == Cls::Punct || prev.cls == Cls::Ws;
                if joinable {
                    if prev.cls == Cls::Ws {
                        prev.cls = cls;
                    }
                    prev.end = end;
                    true
                } else {
                    false
                }
            }
            None => false,
        };
        if !merged {
            if let Some(slot) = runs.get_mut(count) {
                *slot = Run { start, end, cls };
                count += 1;
            } else {
                break;
            }
        }
    }
    count
}

/// Leading spaces of a line, in characters.
fn indent_of(line: &str) -> usize {
    let mut n = 0usize;
    for b in line.bytes() {
        if b == b' ' {
            n += 1;
        } else {
            break;
        }
    }
    n
}

// ---- drawing ---------------------------------------------------------------

/// Metrics of the code face, measured once per frame. The face is monospace,
/// so one advance width places every character on the line.
struct Mono {
    cw: f32,
    ascent: f32,
    height: f32,
}

fn measure_mono(canvas: u64) -> Mono {
    match canvas2d::measure_text_styled(canvas, "0000000000", CODE_SIZE, mono_style(false)) {
        Ok(m) => Mono {
            cw: m.width / 10.0,
            ascent: m.ascent,
            height: m.height,
        },
        Err(_) => Mono {
            cw: CODE_SIZE * 0.6,
            ascent: CODE_SIZE * 0.8,
            height: CODE_SIZE * 1.2,
        },
    }
}

fn draw(canvas: u64, m: &Moment) -> Result<(), gfx::GfxError> {
    fill(canvas, 0.0, 0.0, WIDTH, HEIGHT, BG)?;
    let mono = measure_mono(canvas);

    draw_titlebar(canvas);
    draw_rail(canvas, m)?;
    draw_tabs(canvas, m)?;
    draw_editor(canvas, m, &mono)?;
    draw_minimap(canvas, m)?;
    draw_status(canvas, m)?;

    canvas2d::present(canvas)
}

/// The window chrome: app mark, the file this window is on, and Run.
fn draw_titlebar(canvas: u64) {
    let _ = fill(canvas, 0.0, 0.0, WIDTH, TITLEBAR_H, RAIL);
    let _ = fill(canvas, 0.0, TITLEBAR_H - 1.0, WIDTH, 1.0, LINE);

    let _ = rounded(canvas, 20.0, 13.0, 18.0, 18.0, 5.0, ACCENT);
    // Three hairlines of uneven length: the "code" glyph at this size.
    let _ = fill(canvas, 24.0, 18.0, 10.0, 1.6, BG);
    let _ = fill(canvas, 26.0, 21.5, 6.0, 1.6, BG);
    let _ = fill(canvas, 24.0, 25.0, 8.0, 1.6, BG);
    text(canvas, "Editor", 48.0, 28.0, 15.0, INK);

    let label = "krate-editor  \u{b7}  src/lib.rs";
    let chip_w = est_width(canvas, label, 12.5) + 40.0;
    let chip_x = (WIDTH - chip_w) * 0.5;
    let _ = rounded(canvas, chip_x, 10.0, chip_w, 24.0, 6.0, PANE);
    let _ = disc(canvas, chip_x + 15.0, 22.0, 3.5, GREEN);
    text(canvas, label, chip_x + 26.0, 26.5, 12.5, INK_DIM);

    let run_w = 74.0;
    let run_x = WIDTH - 20.0 - run_w;
    let _ = rounded(canvas, run_x, 10.0, run_w, 24.0, 6.0, SEL_WASH);
    text(canvas, "Run", run_x + 30.0, 26.5, 12.5, ACCENT);
    let mut i = 0.0f32;
    while i < 5.0 {
        let _ = fill(
            canvas,
            run_x + 14.0 + i,
            17.0 + i,
            1.2,
            10.0 - i * 2.0,
            ACCENT,
        );
        i += 1.0;
    }
}

struct Entry {
    name: &'static str,
    depth: f32,
    folder: bool,
}

const TREE: [Entry; 7] = [
    Entry {
        name: "src",
        depth: 0.0,
        folder: true,
    },
    Entry {
        name: "main.rs",
        depth: 1.0,
        folder: false,
    },
    Entry {
        name: "lib.rs",
        depth: 1.0,
        folder: false,
    },
    Entry {
        name: "ui",
        depth: 1.0,
        folder: true,
    },
    Entry {
        name: "window.rs",
        depth: 2.0,
        folder: false,
    },
    Entry {
        name: "Cargo.toml",
        depth: 0.0,
        folder: false,
    },
    Entry {
        name: "README.md",
        depth: 0.0,
        folder: false,
    },
];
const OPEN_ENTRY: usize = 2;
const OTHER_TAB_ENTRY: usize = 4;

/// A small page glyph coloured by file type, the way every explorer does it.
fn file_ink(name: &str) -> gfx::Color {
    let b = name.as_bytes();
    let n = b.len();
    if n >= 3 && b.get(n - 3..) == Some(b".rs") {
        C_NUMBER
    } else if n >= 5 && b.get(n - 5..) == Some(b".toml") {
        C_TYPE
    } else {
        C_FN
    }
}

/// A small downward chevron for an open folder, built from shrinking rows.
fn chevron_down(canvas: u64, x: f32, y: f32, c: gfx::Color) {
    let mut i = 0.0f32;
    while i < 4.0 {
        let _ = fill(canvas, x + i, y + i, 8.0 - i * 2.0, 1.2, c);
        i += 1.0;
    }
}

fn draw_rail(canvas: u64, m: &Moment) -> Result<(), gfx::GfxError> {
    fill(canvas, 0.0, TITLEBAR_H, RAIL_W, HEIGHT - TITLEBAR_H, RAIL)?;
    fill(
        canvas,
        RAIL_W - 1.0,
        TITLEBAR_H,
        1.0,
        HEIGHT - TITLEBAR_H,
        LINE,
    )?;

    text(canvas, "EXPLORER", 20.0, TITLEBAR_H + 34.0, 10.5, INK_QUIET);

    // The project row, then its entries.
    let row_h = 28.0;
    let mut y = TITLEBAR_H + 50.0;
    chevron_down(canvas, 18.0, y + 11.0, INK_DIM);
    text(canvas, "KRATE-EDITOR", 34.0, y + 19.0, 11.5, INK_DIM);
    y += row_h;

    for i in 0..TREE.len() {
        let Some(entry) = TREE.get(i) else { break };
        let open = i == OPEN_ENTRY;
        let x = 34.0 + entry.depth * 16.0;
        if open {
            rounded(
                canvas,
                8.0,
                y + 1.0,
                RAIL_W - 20.0,
                row_h - 2.0,
                6.0,
                SEL_WASH,
            )?;
            rounded(canvas, 0.0, y + 6.0, 3.0, row_h - 12.0, 1.5, ACCENT)?;
        }
        let ink = if open {
            INK
        } else if i == OTHER_TAB_ENTRY {
            INK_DIM
        } else {
            INK_QUIET
        };
        if entry.folder {
            chevron_down(canvas, x - 16.0, y + 11.0, INK_QUIET);
            // Folder glyph: a tab over a body.
            fill(canvas, x, y + 9.0, 5.0, 2.0, INK_QUIET)?;
            fill(canvas, x, y + 11.0, 11.0, 7.0, INK_QUIET)?;
        } else {
            stroke_rounded(
                canvas,
                x + 1.0,
                y + 8.0,
                8.0,
                11.0,
                1.5,
                file_ink(entry.name),
            );
            fill(canvas, x + 3.0, y + 12.0, 4.0, 1.0, file_ink(entry.name))?;
            fill(canvas, x + 3.0, y + 14.5, 4.0, 1.0, file_ink(entry.name))?;
        }
        text(canvas, entry.name, x + 18.0, y + 19.0, 13.0, ink);
        // The file with unsaved edits carries a dot, once there are edits.
        if open && m.chars > 0 {
            disc(canvas, RAIL_W - 24.0, y + row_h * 0.5, 3.0, ACCENT)?;
        }
        y += row_h;
    }
    Ok(())
}

fn draw_tabs(canvas: u64, m: &Moment) -> Result<(), gfx::GfxError> {
    let x0 = EDITOR_X;
    let w = WIDTH - EDITOR_X;
    fill(canvas, x0, TITLEBAR_H, w, TABS_H, RAIL)?;
    fill(canvas, x0, EDITOR_Y - 1.0, w, 1.0, LINE)?;

    let base = TITLEBAR_H + 24.0;
    let mut x = x0;

    // Active tab: the editor's own colour so it fuses with the pane below,
    // an accent rule across its top, and a dot once the file is dirty.
    let name = "lib.rs";
    let tab_w = est_width(canvas, name, 12.5) + 64.0;
    fill(canvas, x, TITLEBAR_H, tab_w, TABS_H, EDITOR)?;
    fill(canvas, x, TITLEBAR_H, tab_w, 2.0, ACCENT)?;
    fill(canvas, x + tab_w - 1.0, TITLEBAR_H, 1.0, TABS_H, LINE)?;
    stroke_rounded(
        canvas,
        x + 17.0,
        TITLEBAR_H + 13.0,
        8.0,
        11.0,
        1.5,
        C_NUMBER,
    );
    text(canvas, name, x + 34.0, base, 12.5, INK);
    if m.chars > 0 {
        disc(
            canvas,
            x + tab_w - 18.0,
            TITLEBAR_H + TABS_H * 0.5,
            3.0,
            ACCENT,
        )?;
    } else {
        text(canvas, "\u{d7}", x + tab_w - 22.0, base, 12.5, INK_QUIET);
    }
    x += tab_w;

    let name = "window.rs";
    let tab_w = est_width(canvas, name, 12.5) + 64.0;
    fill(
        canvas,
        x + tab_w - 1.0,
        TITLEBAR_H + 8.0,
        1.0,
        TABS_H - 16.0,
        LINE_SOFT,
    )?;
    stroke_rounded(
        canvas,
        x + 17.0,
        TITLEBAR_H + 13.0,
        8.0,
        11.0,
        1.5,
        INK_QUIET,
    );
    text(canvas, name, x + 34.0, base, 12.5, INK_DIM);

    text_right(
        canvas,
        "src  \u{203a}  lib.rs  \u{203a}  Card  \u{203a}  tick",
        WIDTH - 20.0,
        base,
        11.5,
        INK_QUIET,
    );
    Ok(())
}

fn draw_editor(canvas: u64, m: &Moment, mono: &Mono) -> Result<(), gfx::GfxError> {
    fill(canvas, EDITOR_X, EDITOR_Y, EDITOR_W, EDITOR_H, EDITOR)?;
    canvas2d::set_clip(canvas, EDITOR_X, EDITOR_Y, EDITOR_W, EDITOR_H)?;

    let cw = mono.cw;
    let text_baseline = (LINE_H - mono.height) * 0.5 + mono.ascent;
    let total = m.total_lines();
    let caret_line = m.caret_line();
    let caret_col = m.caret_col();

    // Pixel scrolling: the first line may start above the top edge.
    let first = (m.scroll_px / LINE_H) as usize;
    let within = m.scroll_px - first as f32 * LINE_H;
    let mut y = EDITOR_Y - within;
    let mut i = first;

    let mut runs = [NO_RUN; MAX_RUNS];
    let mut nbuf = [0u8; 24];
    let text_right_edge = EDITOR_X + EDITOR_W;

    while i < total && y < EDITOR_Y + EDITOR_H {
        let line = m.line(i);

        // Current line wash, under everything.
        if i == caret_line {
            fill(
                canvas,
                EDITOR_X + GUTTER_W,
                y,
                EDITOR_W - GUTTER_W,
                LINE_H,
                CUR_LINE,
            )?;
        }

        // Selection: from its anchor to the caret, per line.
        if m.selecting && i >= SEL_START_LINE && i <= caret_line {
            let from = if i == SEL_START_LINE {
                SEL_START_COL
            } else {
                0
            };
            let to = if i == caret_line {
                caret_col
            } else {
                line.len()
            };
            let sx = TEXT_X + from as f32 * cw;
            let sw = if i == caret_line {
                (to.saturating_sub(from)) as f32 * cw
            } else {
                (to.saturating_sub(from)) as f32 * cw + cw * 0.6
            };
            fill(canvas, sx, y, sw.max(cw * 0.6), LINE_H, SEL_WASH)?;
        }

        // Indent guides. A blank line borrows the depth of the next code line
        // so a guide does not break inside a block.
        let mut indent = indent_of(line);
        if line.is_empty() {
            let mut k = i + 1;
            let mut below = 0usize;
            while k < total && k < i + 4 {
                let next = m.line(k);
                if !next.is_empty() {
                    below = indent_of(next);
                    break;
                }
                k += 1;
            }
            let above = if i > 0 { indent_of(m.line(i - 1)) } else { 0 };
            indent = below.min(above);
        }
        let mut level = 4usize;
        while level <= indent {
            let gx = TEXT_X + (level - 4) as f32 * cw;
            fill(canvas, gx, y, 1.0, LINE_H, LINE_SOFT)?;
            level += 4;
        }

        // Line number, right-aligned in the gutter.
        let n = digits(&mut nbuf, i as u64 + 1);
        let nx = EDITOR_X + GUTTER_W - 14.0 - n.len() as f32 * cw;
        let n_ink = if i == caret_line { INK_DIM } else { INK_QUIET };
        code(canvas, n, nx, y + text_baseline, n_ink, false);

        // The line's coloured runs.
        let count = tokenize(line, &mut runs);
        for r in 0..count {
            let Some(run) = runs.get(r) else { break };
            let Some(slice) = line.get(run.start..run.end) else {
                continue;
            };
            let x = TEXT_X + run.start as f32 * cw;
            if x > text_right_edge {
                break;
            }
            code(
                canvas,
                slice,
                x,
                y + text_baseline,
                cls_colour(run.cls),
                run.cls == Cls::Comment,
            );
        }

        if i == caret_line && m.caret_on {
            let cx = TEXT_X + caret_col as f32 * cw;
            fill(canvas, cx, y + 2.0, 2.0, LINE_H - 4.0, ACCENT)?;
        }

        y += LINE_H;
        i += 1;
    }

    canvas2d::clear_clip(canvas)?;
    Ok(())
}

fn draw_minimap(canvas: u64, m: &Moment) -> Result<(), gfx::GfxError> {
    fill(canvas, MINIMAP_X, EDITOR_Y, MINIMAP_W, EDITOR_H, RAIL)?;
    fill(canvas, MINIMAP_X, EDITOR_Y, 1.0, EDITOR_H, LINE)?;

    let top = EDITOR_Y + 8.0;
    let x0 = MINIMAP_X + 10.0;
    let max_w = MINIMAP_W - 20.0;

    // The viewport slab, under the text so the lines stay crisp.
    let first = m.scroll_px / LINE_H;
    let rows = EDITOR_H / LINE_H;
    fill(
        canvas,
        MINIMAP_X + 1.0,
        top + first * MM_LINE_H,
        MINIMAP_W - 1.0,
        rows * MM_LINE_H,
        VIEWPORT,
    )?;

    let mut runs = [NO_RUN; MAX_RUNS];
    let total = m.total_lines();
    let mut i = 0usize;
    while i < total {
        let y = top + i as f32 * MM_LINE_H;
        if y > STATUS_Y - 8.0 {
            break;
        }
        let line = m.line(i);
        let count = tokenize(line, &mut runs);
        for r in 0..count {
            let Some(run) = runs.get(r) else { break };
            // Skip the leading whitespace of the run so indentation shows.
            let slice = line.get(run.start..run.end).unwrap_or("");
            let lead = indent_of(slice);
            let start = run.start + lead;
            if start >= run.end {
                continue;
            }
            let x = x0 + start as f32 * MM_CHAR_W;
            if x > x0 + max_w {
                break;
            }
            let w = ((run.end - start) as f32 * MM_CHAR_W).min(x0 + max_w - x);
            let mut c = cls_colour(run.cls);
            c.a = 0.62;
            fill(canvas, x, y, w, 2.0, c)?;
        }
        i += 1;
    }
    Ok(())
}

fn draw_status(canvas: u64, m: &Moment) -> Result<(), gfx::GfxError> {
    fill(canvas, 0.0, STATUS_Y, WIDTH, STATUS_H, RAIL)?;
    fill(canvas, 0.0, STATUS_Y, WIDTH, 1.0, LINE)?;

    let base = STATUS_Y + 20.0;

    // Branch glyph: two nodes and the line between them.
    disc(canvas, 22.0, STATUS_Y + 10.0, 2.2, INK_QUIET)?;
    disc(canvas, 22.0, STATUS_Y + 21.0, 2.2, INK_QUIET)?;
    fill(canvas, 21.4, STATUS_Y + 11.0, 1.2, 9.0, INK_QUIET)?;
    text(canvas, "main", 32.0, base, 11.5, INK_DIM);

    let mut pen = 32.0 + est_width(canvas, "main", 11.5) + 22.0;
    let unsaved = m.chars > 0;
    disc(
        canvas,
        pen + 3.0,
        STATUS_Y + 15.0,
        3.0,
        if unsaved { ACCENT } else { GREEN },
    )?;
    let state = if unsaved {
        "1 unsaved change"
    } else {
        "no problems"
    };
    text(canvas, state, pen + 13.0, base, 11.5, INK_QUIET);
    pen += 13.0 + est_width(canvas, state, 11.5) + 22.0;
    text(canvas, "rust-analyzer", pen, base, 11.5, INK_QUIET);

    // Right side, laid right to left so the caret position sits innermost.
    let mut right = WIDTH - 20.0;
    for item in ["Rust", "LF", "UTF-8", "Spaces: 4"] {
        text_right(canvas, item, right, base, 11.5, INK_QUIET);
        right -= est_width(canvas, item, 11.5) + 24.0;
    }

    // "Ln 42, Col 17", from the real caret.
    let mut lbuf = [0u8; 24];
    let mut cbuf = [0u8; 24];
    let ln = digits(&mut lbuf, m.caret_line() as u64 + 1);
    let col = digits(&mut cbuf, m.caret_col() as u64 + 1);
    let w_col = est_width(canvas, col, 11.5);
    right -= w_col;
    text(canvas, col, right, base, 11.5, INK_DIM);
    let w = est_width(canvas, ", Col ", 11.5);
    right -= w;
    text(canvas, ", Col ", right, base, 11.5, INK_QUIET);
    let w_ln = est_width(canvas, ln, 11.5);
    right -= w_ln;
    text(canvas, ln, right, base, 11.5, INK_DIM);
    let w = est_width(canvas, "Ln ", 11.5);
    right -= w;
    text(canvas, "Ln ", right, base, 11.5, INK_QUIET);
    Ok(())
}

// ---- widget scaffolding -----------------------------------------------------

fn node(
    id: u64,
    parent: Option<u64>,
    kind: types::WidgetKind,
    role: Option<&str>,
) -> types::WidgetNode {
    types::WidgetNode {
        id,
        parent,
        kind,
        label: None,
        role: role.map(pure_string),
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
    }
}

// ---- app --------------------------------------------------------------------

struct Component;

impl bindings::Guest for Component {
    fn run() -> i32 {
        let raw = args::raw();
        let quick = raw.split_whitespace().any(|word| word == "quick");

        let size = types::WindowSize {
            width: WIDTH as u32,
            height: HEIGHT as u32,
        };
        let Ok(win) = window::create("Editor", size) else {
            out("window:no");
            return 30;
        };
        if tree::set_root(win, &node(ROOT_ID, None, types::WidgetKind::Stack, None)).is_err() {
            out("tree:no");
            return 32;
        }
        let _ = tree::upsert_node(
            win,
            &node(
                CANVAS_ID,
                Some(ROOT_ID),
                types::WidgetKind::Canvas,
                Some("canvas"),
            ),
        );
        let _ = window::show(win);

        let canvas = match canvas2d::bind(win, CANVAS_ID) {
            Ok(c) => c,
            Err(_) => {
                out("bind:no");
                let _ = window::close(win);
                return 33;
            }
        };
        // Draw in design coordinates; the host scales them to any window,
        // centred and in proportion (K-096).
        let _ = canvas2d::set_design_size(
            canvas,
            gfx::Size {
                width: WIDTH,
                height: HEIGHT,
            },
        );

        let started = clock::monotonic_nanos();
        let mut frames: u32 = 0;
        let mut draw_nanos: u64 = 0;

        const FRAME_NANOS: u64 = 1_000_000_000 / 60;
        let mut next_frame = clock::monotonic_nanos();

        loop {
            let now = clock::monotonic_nanos();
            // The quick run steps synthetic time, half a second a frame, so
            // the whole typing script is exercised in eight frames.
            let t_ms = if quick {
                frames as u64 * 500
            } else {
                now.saturating_sub(started) / 1_000_000
            };

            let m = moment(t_ms);
            let before = clock::monotonic_nanos();
            if draw(canvas, &m).is_err() {
                out("draw:no");
                return 1;
            }
            draw_nanos = draw_nanos.saturating_add(clock::monotonic_nanos().saturating_sub(before));
            frames += 1;

            if quick {
                if frames >= QUICK_FRAMES {
                    // Read every value out of the last moment drawn.
                    out_number("lines:", m.total_lines() as u64);
                    out_number("typed:", m.chars as u64);
                    out_number("enters:", m.enters as u64);
                    out_number("line:", m.caret_line() as u64 + 1);
                    out_number("col:", m.caret_col() as u64 + 1);
                    out_number("scroll:", m.scroll_px as u64);
                    out(if m.caret_on { "caret:on" } else { "caret:off" });
                    out(if m.selecting {
                        "selection:yes"
                    } else {
                        "selection:no"
                    });
                    let stdout = stdio::stdout();
                    let _ = stdout.write(b"current:");
                    let _ = stdout.write(m.line(m.caret_line()).trim().as_bytes());
                    let _ = stdout.write(b"\n");
                    out_number("frame_us:", draw_nanos / 1_000 / frames.max(1) as u64);
                    let _ = stdout.flush();
                    break;
                }
                continue;
            }

            // No `request-redraw`: this loop draws on its own schedule and
            // paces against the clock, spending the rest of each frame
            // blocked in `wait` draining input.
            next_frame = next_frame.saturating_add(FRAME_NANOS);
            let after_draw = clock::monotonic_nanos();
            if next_frame < after_draw {
                next_frame = after_draw;
            }
            let mut closing = false;
            loop {
                let now = clock::monotonic_nanos();
                let remaining = next_frame.saturating_sub(now);
                if remaining == 0 {
                    break;
                }
                let millis = (remaining / 1_000_000) as u32;
                match events::wait(Some(millis.max(1))) {
                    Some(types::Event::CloseRequested(_)) => {
                        closing = true;
                        break;
                    }
                    Some(_) | None => {}
                }
            }
            if closing {
                break;
            }
        }

        if !quick {
            let elapsed = clock::monotonic_nanos().saturating_sub(started);
            out_number("frames:", frames as u64);
            if elapsed > 0 {
                out_number("fps-centi:", frames as u64 * 100 * 1_000_000_000 / elapsed);
            }
            out_number("frame_us:", draw_nanos / 1_000 / frames.max(1) as u64);
        }
        let _ = window::close(win);
        0
    }
}

bindings::export!(Component with_types_in bindings);
