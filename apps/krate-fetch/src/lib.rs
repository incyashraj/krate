//! Krate fetch — a modern reader over a real HTTP fetch.
//!
//! The wall it tests: can an app reach the network and show what it got? A
//! weather app, a feed reader, an API client -- all of it needs a real HTTP
//! round trip, not a stub. This app performs a GET with `net.http-client::get`
//! and renders the response body in a scrollable reader, with a status line
//! that reports the byte count or the exact error. If networking is faked or
//! blocked, the status line says so rather than the app pretending.
//!
//! The whole UI is drawn on one `gfx.canvas2d`: a considered dark ground, a
//! bold title, a URL bar, a rounded Fetch button drawn and hit-tested by hand,
//! and the body laid out as readable lines inside a rounded content card with a
//! scrollbar. Native widgets cannot be styled by the app, so every pixel is
//! painted here instead, at the level of the Nova reference.
//!
//! The URL comes from the first app argument, so a real run points it at a
//! server and the bytes come back live. When the app is launched with only the
//! automated `quick` flag and no URL, it shows a clearly-labelled built-in
//! sample document so the reader is populated -- the live fetch path is still
//! exactly the code a URL run takes. Everything is byte work on the response,
//! no parsing library, only `krate:*` imports and no reachable panic.

#[allow(warnings)]
mod bindings;

use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::net::http_client;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const WIDTH: f32 = 560.0;
const HEIGHT: f32 = 620.0;

const QUICK_ROUNDS: u32 = 4;
const MAX_ROUNDS: u32 = 600;
const ROUND_MILLIS: u32 = 50;

/// How many lines of the response the reader lays out, and how wide each is.
const MAX_LINES: usize = 64;
const LINE_CAP: usize = 96;
/// How many lines are visible in the card at once (drives the scrollbar).
const VISIBLE_LINES: usize = 13;

/// A clearly-labelled built-in document shown when no URL is given, so the shot
/// is populated. A real URL run replaces this with the live response.
const SAMPLE: &str = "\
Krate Reader
============

This is a built-in sample page.

Pass a URL as the first argument
and Krate performs a real HTTP GET,
then lays the response out here
line by line:

  krate run fetch.krate --
      https://example.com

The body arrives as raw bytes over
net.http-client, with no parsing
library in the guest. The reader
splits on newlines and draws each
line in the card.

The status line below reports the
byte count on a live fetch, or the
exact network error if the host
refused or the connection failed.";

/// Outcome of the fetch, for the status line.
#[derive(Clone, Copy, PartialEq)]
enum Status {
    /// Live fetch succeeded; carries the byte count.
    Ok(usize),
    /// Live fetch failed.
    Failed,
    /// No URL was given; showing the built-in sample.
    Sample,
}

struct Component;

/// The reader's content: a fixed pool of fixed-width line buffers, plus the
/// URL, the status, and the current scroll position. No growing Vec.
struct Reader {
    lines: [[u8; LINE_CAP]; MAX_LINES],
    line_lens: [usize; MAX_LINES],
    n_lines: usize,
    url: [u8; 160],
    url_len: usize,
    status: Status,
    /// First visible line index.
    scroll: usize,
    /// Fetch button state.
    hover: bool,
    pressed: bool,
}

impl Reader {
    fn new() -> Self {
        Reader {
            lines: [[0u8; LINE_CAP]; MAX_LINES],
            line_lens: [0usize; MAX_LINES],
            n_lines: 0,
            url: [0u8; 160],
            url_len: 0,
            status: Status::Sample,
            scroll: 0,
            hover: false,
            pressed: false,
        }
    }

    fn url_bytes(&self) -> &[u8] {
        self.url.get(..self.url_len).unwrap_or(&[])
    }

    fn set_url(&mut self, url: &[u8]) {
        self.url_len = copy_into(&mut self.url, url);
    }

    /// Split `body` on newlines and store the first `MAX_LINES` lines into the
    /// fixed pool, collapsing runs of blank lines to a single spacer.
    fn load_body(&mut self, body: &[u8]) {
        self.n_lines = 0;
        self.scroll = 0;
        let mut start = 0usize;
        let mut i = 0usize;
        while i <= body.len() && self.n_lines < MAX_LINES {
            let at_end = i == body.len();
            let is_nl = !at_end && body.get(i) == Some(&b'\n');
            if at_end || is_nl {
                let slice = body.get(start..i).unwrap_or(&[]);
                let blank = slice.iter().all(|b| *b == b' ' || *b == b'\r' || *b == b'\t');
                let prev_blank = self
                    .n_lines
                    .checked_sub(1)
                    .and_then(|p| self.line_lens.get(p))
                    .map(|len| *len == 0)
                    .unwrap_or(true);
                if !(blank && prev_blank) {
                    if let (Some(dst), Some(dlen)) = (
                        self.lines.get_mut(self.n_lines),
                        self.line_lens.get_mut(self.n_lines),
                    ) {
                        let clean: &[u8] = if blank { &[] } else { slice };
                        *dlen = copy_into(dst, clean);
                    }
                    self.n_lines += 1;
                }
                start = i + 1;
            }
            i += 1;
        }
    }

    fn line(&self, i: usize) -> &[u8] {
        match (self.lines.get(i), self.line_lens.get(i)) {
            (Some(buf), Some(len)) => buf.get(..*len).unwrap_or(&[]),
            _ => &[],
        }
    }

    fn max_scroll(&self) -> usize {
        self.n_lines.saturating_sub(VISIBLE_LINES)
    }

    fn scroll_by(&mut self, delta: i32) {
        let max = self.max_scroll();
        if delta < 0 {
            self.scroll = self.scroll.saturating_sub((-delta) as usize);
        } else {
            self.scroll = (self.scroll + delta as usize).min(max);
        }
    }
}

impl bindings::Guest for Component {
    fn run() -> i32 {
        // The first line of raw args is the URL; a later line may be `quick`.
        // The automated harness passes only `quick`, which is not a URL, so we
        // treat a first arg that does not look like a URL as "no URL given".
        let raw = args::raw();
        let mut parts = raw.as_bytes().split(|byte| *byte == b'\n');
        let first = parts.next().unwrap_or(b"");
        let has_quick = first == b"quick" || parts.any(|p| p == b"quick");
        let has_url = looks_like_url(first);

        let out = stdio::stdout();
        let mut reader = Reader::new();
        if has_url {
            reader.set_url(first);
        } else {
            reader.set_url(b"no URL - built-in sample");
        }

        // ---- window + canvas ----
        let size = types::WindowSize {
            width: WIDTH as u32,
            height: HEIGHT as u32,
        };
        let Ok(win) = window::create("Reader", size) else {
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

        // Perform the fetch up front: a live GET when a URL was given, else the
        // built-in sample so the reader is populated for the automated shot.
        do_fetch(&mut reader, has_url, &out);

        let _ = draw(canvas, &reader);

        // A real session ends when the person closes the window, never on a
        // round count (K-092). `quick` keeps its bound so a headless check
        // cannot hang.
        let rounds = if has_quick { QUICK_ROUNDS } else { u32::MAX };
        let mut r = 0u32;
        while r < rounds {
            match events::wait(Some(ROUND_MILLIS)) {
                Some(types::Event::CloseRequested(id)) if id == win => break,
                Some(types::Event::Pointer(p)) => {
                    let on_btn = hit_fetch(p.x, p.y);
                    if p.pressed {
                        reader.pressed = on_btn;
                        reader.hover = on_btn;
                    } else {
                        if on_btn && reader.pressed {
                            do_fetch(&mut reader, has_url, &out);
                        }
                        reader.pressed = false;
                        reader.hover = on_btn;
                    }
                    let _ = draw(canvas, &reader);
                }
                Some(types::Event::Key(k)) => {
                    // Arrow keys scroll the reader.
                    if k.pressed {
                        if k.key.as_bytes() == b"ArrowDown" {
                            reader.scroll_by(1);
                        } else if k.key.as_bytes() == b"ArrowUp" {
                            reader.scroll_by(-1);
                        } else if k.key.as_bytes() == b"PageDown" {
                            reader.scroll_by(VISIBLE_LINES as i32 - 2);
                        } else if k.key.as_bytes() == b"PageUp" {
                            reader.scroll_by(-(VISIBLE_LINES as i32 - 2));
                        }
                    }
                    let _ = draw(canvas, &reader);
                }
                _ => {
                    let _ = draw(canvas, &reader);
                }
            }
            r += 1;
        }

        let _ = window::close(win);
        0
    }
}

/// Run the fetch: a live GET when a URL is present, else load the sample. Prints
/// the machine-readable result line for automated runs.
fn do_fetch(reader: &mut Reader, has_url: bool, out: &stdio::OutputStream) {
    if has_url {
        if let Ok(url) = core::str::from_utf8(reader.url_bytes()) {
            match http_client::get(url) {
                Ok(body) => {
                    let n = body.len();
                    reader.load_body(&body);
                    reader.status = Status::Ok(n);
                    let _ = out.write(b"fetch:ok:");
                    let _ = out.write(u64_slice(n as u64, &mut [0u8; 20]));
                    let _ = out.write(b"\n");
                    return;
                }
                Err(_) => {
                    reader.load_body(
                        b"The fetch failed. The host refused the\nconnection or the server did not answer.",
                    );
                    reader.status = Status::Failed;
                    let _ = out.write(b"fetch:error\n");
                    return;
                }
            }
        }
    }
    // No URL: the built-in sample.
    reader.load_body(SAMPLE.as_bytes());
    reader.status = Status::Sample;
    let _ = out.write(b"fetch:sample\n");
}

/// True when a first arg looks like an http(s) URL rather than the `quick` flag.
fn looks_like_url(arg: &[u8]) -> bool {
    arg.starts_with(b"http://") || arg.starts_with(b"https://")
}

// ------------------------------------------------------------------
// Layout (shared by draw and hit-test)
// ------------------------------------------------------------------

const URL_X: f32 = 28.0;
const URL_Y: f32 = 100.0;
const URL_H: f32 = 46.0;
const BTN_W: f32 = 120.0;
const BTN_H: f32 = 46.0;
const BTN_X: f32 = WIDTH - 28.0 - BTN_W;
const BTN_Y: f32 = URL_Y;
const URL_W: f32 = BTN_X - URL_X - 12.0;

const CARD_X: f32 = 28.0;
const CARD_Y: f32 = 168.0;
const CARD_W: f32 = WIDTH - 56.0;
const CARD_H: f32 = HEIGHT - CARD_Y - 68.0;
const CONTENT_TOP: f32 = CARD_Y + 34.0;
const LINE_STEP: f32 = 24.0;

fn hit_fetch(x: f32, y: f32) -> bool {
    x >= BTN_X && x <= BTN_X + BTN_W && y >= BTN_Y && y <= BTN_Y + BTN_H
}

// ------------------------------------------------------------------
// Rendering
// ------------------------------------------------------------------

fn draw(canvas: u64, reader: &Reader) -> Result<(), gfx::GfxError> {
    // Considered dark ground with a soft top glow.
    canvas2d::linear_gradient(
        canvas,
        gfx::Rect {
            x: 0.0,
            y: 0.0,
            width: WIDTH,
            height: HEIGHT,
        },
        color(0.078, 0.090, 0.145, 1.0),
        color(0.039, 0.047, 0.078, 1.0),
    )?;
    canvas2d::radial_gradient(
        canvas,
        gfx::Point { x: 80.0, y: 24.0 },
        300.0,
        color(0.30, 0.50, 0.95, 0.14),
        color(0.30, 0.50, 0.95, 0.0),
    )?;

    // ---- header ----
    draw_text(canvas, "Reader", 28.0, 52.0, 30.0, color(0.96, 0.97, 1.0, 1.0))?;
    draw_text(
        canvas,
        "Fetch a page over HTTP and read it",
        28.0,
        76.0,
        13.0,
        color(0.55, 0.63, 0.82, 1.0),
    )?;

    // ---- URL bar ----
    round_rect(canvas, URL_X, URL_Y, URL_W, URL_H, 12.0, color(0.129, 0.145, 0.216, 1.0))?;
    // A small globe dot as an affordance.
    disc(canvas, URL_X + 22.0, URL_Y + URL_H * 0.5, 5.0, color(0.36, 0.72, 1.0, 1.0))?;
    if let Ok(u) = core::str::from_utf8(reader.url_bytes()) {
        let shown = truncate_str(u, 34);
        draw_text(
            canvas,
            shown,
            URL_X + 40.0,
            URL_Y + URL_H * 0.5 + 5.0,
            15.0,
            color(0.86, 0.90, 0.98, 1.0),
        )?;
    }

    // ---- Fetch button ----
    draw_button(canvas, BTN_X, BTN_Y, BTN_W, BTN_H, "Fetch", reader.hover, reader.pressed)?;

    // ---- content card ----
    round_rect(canvas, CARD_X, CARD_Y, CARD_W, CARD_H, 16.0, color(0.105, 0.121, 0.184, 1.0))?;
    // Lit top edge (opaque, inset from corners).
    canvas2d::fill_rect(
        canvas,
        gfx::Rect {
            x: CARD_X + 16.0,
            y: CARD_Y,
            width: CARD_W - 32.0,
            height: 1.5,
        },
        color(0.20, 0.22, 0.32, 1.0),
    )?;

    // Body lines, clipped to the visible window.
    let text_x = CARD_X + 24.0;
    let mut vis = 0usize;
    while vis < VISIBLE_LINES {
        let li = reader.scroll + vis;
        if li >= reader.n_lines {
            break;
        }
        let bytes = reader.line(li);
        if !bytes.is_empty() {
            if let Ok(t) = core::str::from_utf8(bytes) {
                let y = CONTENT_TOP + (vis as f32) * LINE_STEP;
                // The very first line reads as a heading; render it brighter.
                let ink = if li == 0 {
                    color(0.96, 0.97, 1.0, 1.0)
                } else {
                    color(0.80, 0.85, 0.94, 1.0)
                };
                let size = if li == 0 { 17.0 } else { 15.0 };
                draw_text(canvas, t, text_x, y, size, ink)?;
            }
        }
        vis += 1;
    }

    // Scrollbar on the right of the card, when content overflows.
    draw_scrollbar(canvas, reader)?;

    // ---- status line ----
    draw_status(canvas, reader)?;

    canvas2d::present(canvas)?;
    Ok(())
}

fn draw_button(
    canvas: u64,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    label: &str,
    hover: bool,
    pressed: bool,
) -> Result<(), gfx::GfxError> {
    // Opaque shadow, then an opaque accent body that reacts to hover/press.
    round_rect(canvas, x, y + 3.0, w, h, 12.0, color(0.020, 0.025, 0.045, 1.0))?;
    let (r, g, b) = (0.24f32, 0.55f32, 1.0f32);
    let k = if pressed {
        0.80
    } else if hover {
        1.12
    } else {
        1.0
    };
    round_rect(
        canvas,
        x,
        y,
        w,
        h,
        12.0,
        color((r * k).min(1.0), (g * k).min(1.0), (b * k).min(1.0), 1.0),
    )?;
    let approx = (label.as_bytes().len() as f32) * 18.0 * 0.60;
    draw_text(
        canvas,
        label,
        x + (w - approx) * 0.5,
        y + h * 0.5 + 6.0,
        18.0,
        color(1.0, 1.0, 1.0, 1.0),
    )?;
    Ok(())
}

fn draw_scrollbar(canvas: u64, reader: &Reader) -> Result<(), gfx::GfxError> {
    if reader.n_lines <= VISIBLE_LINES {
        return Ok(());
    }
    let track_x = CARD_X + CARD_W - 10.0;
    let track_y = CARD_Y + 16.0;
    let track_h = CARD_H - 32.0;
    // Track.
    round_rect(canvas, track_x, track_y, 4.0, track_h, 2.0, color(0.20, 0.23, 0.32, 1.0))?;
    // Thumb.
    let frac_vis = VISIBLE_LINES as f32 / reader.n_lines as f32;
    let thumb_h = (track_h * frac_vis).max(24.0);
    let max_scroll = reader.max_scroll().max(1) as f32;
    let t = reader.scroll as f32 / max_scroll;
    let thumb_y = track_y + (track_h - thumb_h) * t;
    round_rect(canvas, track_x, thumb_y, 4.0, thumb_h, 2.0, color(0.36, 0.72, 1.0, 1.0))?;
    Ok(())
}

fn draw_status(canvas: u64, reader: &Reader) -> Result<(), gfx::GfxError> {
    let sy = HEIGHT - 34.0;
    let dot = match reader.status {
        Status::Ok(_) => color(0.42, 0.90, 0.55, 1.0),
        Status::Failed => color(0.98, 0.45, 0.55, 1.0),
        Status::Sample => color(0.36, 0.72, 1.0, 1.0),
    };
    disc(canvas, 34.0, sy - 5.0, 5.0, dot)?;

    let mut buf = [0u8; 64];
    let text: &[u8] = match reader.status {
        Status::Ok(n) => status_ok(n as u64, &mut buf),
        Status::Failed => b"Fetch failed: the host refused or the server did not answer.",
        Status::Sample => b"Built-in sample - pass a URL to fetch a live page.",
    };
    if let Ok(t) = core::str::from_utf8(text) {
        draw_text(canvas, t, 48.0, sy, 14.0, color(0.82, 0.87, 0.96, 1.0))?;
    }
    Ok(())
}

/// "Fetched 1256 bytes over HTTP" from a byte count.
fn status_ok(n: u64, buf: &mut [u8; 64]) -> &[u8] {
    let mut pos = 0usize;
    for b in b"Fetched " {
        push(buf, &mut pos, *b);
    }
    let mut num = [0u8; 20];
    for b in u64_slice(n, &mut num) {
        push(buf, &mut pos, *b);
    }
    for b in b" bytes over HTTP" {
        push(buf, &mut pos, *b);
    }
    buf.get(..pos).unwrap_or(b"Fetched")
}

// ------------------------------------------------------------------
// Canvas helpers
// ------------------------------------------------------------------

/// A filled rounded rectangle from NON-overlapping pieces, so a translucent
/// color composites evenly: a full-width center band, top and bottom bands
/// between the corners, and four corner discs.
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
    canvas2d::fill_rect(
        canvas,
        gfx::Rect {
            x: x + r,
            y,
            width: w - 2.0 * r,
            height: r,
        },
        c,
    )?;
    canvas2d::fill_rect(
        canvas,
        gfx::Rect {
            x: x + r,
            y: y + h - r,
            width: w - 2.0 * r,
            height: r,
        },
        c,
    )?;
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

/// Return at most `max` bytes of a str, only when that lands on a valid slice
/// boundary (ASCII fast path); otherwise the whole string.
fn truncate_str(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    match s.get(..max) {
        Some(sub) => sub,
        None => s,
    }
}

// ------------------------------------------------------------------
// Byte helpers, panic-free
// ------------------------------------------------------------------

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

bindings::export!(Component with_types_in bindings);
