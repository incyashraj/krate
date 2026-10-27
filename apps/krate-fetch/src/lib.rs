//! Krate fetch -- a modern reader over a real HTTP fetch.
//!
//! The wall it tests: can an app reach the network and show what it got? A
//! weather app, a feed reader, an API client -- all of it needs a real HTTP
//! round trip, not a stub. This app performs a GET with `net::begin`, polls
//! it each round while the window keeps drawing, and renders the response
//! body in a scrollable reader, with a status line that reports the byte
//! count or the exact error. If networking is faked or blocked, the status
//! line says so rather than the app pretending.
//!
//! The whole UI is drawn on one `gfx.canvas2d`: a considered dark ground, a
//! bold title, a URL bar, a rounded Fetch button drawn and hit-tested by hand,
//! and the body laid out as readable lines inside a rounded content card with a
//! scrollbar. Native widgets cannot be styled by the app, so every pixel is
//! painted here instead, at the level of the Nova reference.
//!
//! It lays itself out from the canvas it really has, every frame: a phone
//! gets the whole width, a desktop a centred column at a comfortable reading
//! measure, and long lines wrap to whatever room there is. Every font size
//! follows the person's text size, and each frame tells the screen reader
//! what it shows.
//!
//! The URL comes from the first app argument, so a real run points it at a
//! server and the bytes come back live. When the app is launched with only the
//! automated `quick` flag and no URL, it shows a clearly-labelled built-in
//! sample document so the reader is populated -- the live fetch path is still
//! exactly the code a URL run takes. Everything is byte work on the response,
//! no parsing library, only `krate:*` imports and no reachable panic.

#![no_std]
extern crate alloc;

use alloc::vec::Vec;
use krate::bindings::krate::io::{args, stdio};
use krate::gfx::{canvas2d, types as gfx};
use krate::layout::{Look, Rect, Said, Screen};
use krate::net::{self, FetchStatus, HttpMethod, Request};
use krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

/// The window asked for on a desktop. Only a first guess: everything below
/// is laid out from the canvas the app is actually given.
const WIDTH: f32 = 640.0;
const HEIGHT: f32 = 700.0;
/// The widest the reading column grows. Past about 70 characters a line is
/// hard to follow back to its start, so a wide window gets margins instead.
const MAX_COLUMN: f32 = 720.0;

const QUICK_ROUNDS: u32 = 4;
const ROUND_MILLIS: u32 = 50;

/// How many lines of the response the reader keeps, and how long each is.
/// A line longer than the card wraps; one past the cap is cut.
const MAX_LINES: usize = 64;
const LINE_CAP: usize = 320;

/// A clearly-labelled built-in document shown when no URL is given, so the shot
/// is populated. A real URL run replaces this with the live response.
const SAMPLE: &str = "\
Krate Reader
============

This is a built-in sample page.

Pass a URL as the first argument and Krate performs a real HTTP GET, then lays the response out here line by line:

krate run fetch.krate -- https://example.com

The body arrives as raw bytes over net.http-client, with no parsing library in the guest. The reader splits on newlines and wraps each line to the card, whatever screen it is on.

The status line below reports the byte count on a live fetch, or the exact network error if the host refused or the connection failed.";

/// Outcome of the fetch, for the status line.
#[derive(Clone, Copy, PartialEq)]
enum Status {
    /// A live request is in flight; the window keeps drawing meanwhile.
    Loading,
    /// Live fetch succeeded; carries the byte count.
    Ok(usize),
    /// Live fetch failed.
    Failed,
    /// No URL was given; showing the built-in sample.
    Sample,
}

struct Component;

/// One row on screen: bytes `start..end` of stored line `line`. A long line
/// is several rows; a blank one is a row with nothing in it.
#[derive(Clone, Copy)]
struct Row {
    line: usize,
    start: usize,
    end: usize,
}

/// The reader's content: a fixed pool of fixed-width line buffers, plus the
/// URL, the status, and the current scroll position.
struct Reader {
    lines: [[u8; LINE_CAP]; MAX_LINES],
    line_lens: [usize; MAX_LINES],
    n_lines: usize,
    url: [u8; 160],
    url_len: usize,
    status: Status,
    /// The lines wrapped to the card, and what they were wrapped for (room,
    /// text size, body). Wrapping measures text, so it is done when one of
    /// those changes, not every frame: a phone runs this code on an
    /// interpreter, and a scroll must not re-measure the whole page.
    rows: Vec<Row>,
    wrapped_for: Option<(u32, u32, u32)>,
    body_gen: u32,
    /// First visible row, and what the last frame could show: scrolling is
    /// clamped to what the person is looking at.
    scroll: usize,
    visible: usize,
    step: f32,
    /// Wheel movement not yet worth a whole row (a trackpad sends small steps).
    wheel: f32,
    /// Fetch button state.
    hover: bool,
    pressed: bool,
    /// Rounds since start, so the in-flight status can show it is alive.
    tick: u32,
    /// The in-flight request, if any: `net::begin` handed it back at once,
    /// and the event loop asks `net::poll` about it every round.
    pending: Option<u64>,
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
            rows: Vec::new(),
            wrapped_for: None,
            body_gen: 0,
            scroll: 0,
            visible: 1,
            step: 24.0,
            wheel: 0.0,
            hover: false,
            pressed: false,
            tick: 0,
            pending: None,
        }
    }

    fn url_str(&self) -> &str {
        valid_prefix(self.url.get(..self.url_len).unwrap_or(&[]))
    }

    fn set_url(&mut self, url: &[u8]) {
        self.url_len = copy_into(&mut self.url, url);
    }

    /// Split `body` on newlines and store the first `MAX_LINES` lines into the
    /// fixed pool, collapsing runs of blank lines to a single spacer.
    fn load_body(&mut self, body: &[u8]) {
        self.n_lines = 0;
        self.scroll = 0;
        self.body_gen = self.body_gen.wrapping_add(1);
        let mut start = 0usize;
        let mut i = 0usize;
        while i <= body.len() && self.n_lines < MAX_LINES {
            let at_end = i == body.len();
            let is_nl = !at_end && body.get(i) == Some(&b'\n');
            if at_end || is_nl {
                let slice = body.get(start..i).unwrap_or(&[]);
                let blank = slice
                    .iter()
                    .all(|b| *b == b' ' || *b == b'\r' || *b == b'\t');
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

    /// A stored line as text. A live body is anyone's bytes, and the cap can
    /// cut a character in half, so this keeps the valid part rather than
    /// dropping the whole line.
    fn line(&self, i: usize) -> &str {
        match (self.lines.get(i), self.line_lens.get(i)) {
            (Some(buf), Some(len)) => valid_prefix(buf.get(..*len).unwrap_or(&[])),
            _ => "",
        }
    }

    fn row_text(&self, row: Row) -> &str {
        self.line(row.line).get(row.start..row.end).unwrap_or("")
    }

    fn max_scroll(&self) -> usize {
        self.rows.len().saturating_sub(self.visible)
    }

    fn scroll_by(&mut self, delta: i32) {
        let max = self.max_scroll();
        if delta < 0 {
            self.scroll = self.scroll.saturating_sub(delta.unsigned_abs() as usize);
        } else {
            self.scroll = (self.scroll + delta as usize).min(max);
        }
    }
}

impl krate::Guest for Component {
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

        // Perform the fetch up front: a live GET when a URL was given, else the
        // built-in sample so the reader is populated for the automated shot.
        do_fetch(&mut reader, has_url, &out);

        let _ = draw(canvas, win, &mut reader);

        // A real session ends when the person closes the window, never on a
        // round count (K-092). `quick` keeps its bound so a headless check
        // cannot hang.
        let rounds = if has_quick { QUICK_ROUNDS } else { u32::MAX };
        let mut r = 0u32;
        while r < rounds {
            let changed = match events::wait(Some(ROUND_MILLIS)) {
                Some(types::Event::CloseRequested(id)) if id == win => break,
                // A phone turned, a window narrowed, the text size or the
                // theme changed: lay everything out again.
                Some(types::Event::Resized(_)) | Some(types::Event::ThemeChanged(_)) => true,
                Some(types::Event::Pointer(p)) => {
                    // The same layout the frame was drawn from, so a click
                    // lands on the button the person sees.
                    let on_btn = layout(canvas, win).btn.contains(p.x, p.y);
                    if p.pressed {
                        reader.pressed = on_btn;
                    } else {
                        if on_btn && reader.pressed {
                            do_fetch(&mut reader, has_url, &out);
                        }
                        reader.pressed = false;
                    }
                    reader.hover = on_btn;
                    true
                }
                // dy is in logical pixels and positive scrolls down. Ctrl
                // means zoom on every desktop, so a ctrl-wheel is left alone.
                Some(types::Event::Wheel(w)) if !w.modifiers.control => {
                    reader.wheel += w.dy;
                    let rows = (reader.wheel / reader.step.max(1.0)) as i32;
                    reader.wheel -= rows as f32 * reader.step;
                    reader.scroll_by(rows);
                    rows != 0
                }
                Some(types::Event::Key(k)) if k.pressed => {
                    let page = reader.visible.saturating_sub(2).max(1) as i32;
                    match k.key.as_bytes() {
                        b"ArrowDown" => reader.scroll_by(1),
                        b"ArrowUp" => reader.scroll_by(-1),
                        b"PageDown" => reader.scroll_by(page),
                        b"PageUp" => reader.scroll_by(-page),
                        _ => {}
                    }
                    true
                }
                // Nothing happened: only an in-flight fetch has anything new
                // to show (its status moves so the person sees it is alive).
                _ => reader.status == Status::Loading,
            };
            // The request, if one is in flight, is asked about once per round
            // -- after the event, so a click during a slow fetch still lands.
            let settled = settle_fetch(&mut reader, &out);
            reader.tick = reader.tick.wrapping_add(1);
            if changed || settled {
                let _ = draw(canvas, win, &mut reader);
            }
            r += 1;
        }

        let _ = window::close(win);
        0
    }
}

/// One poll of the in-flight request. Returns true when something changed and
/// the reader should be drawn again.
fn settle_fetch(reader: &mut Reader, out: &stdio::OutputStream) -> bool {
    let Some(handle) = reader.pending else {
        return false;
    };
    match net::poll(handle) {
        // Still going: draw the frame, take the next event, ask again.
        FetchStatus::Pending => false,
        FetchStatus::Ready(res) => {
            reader.pending = None;
            let n = res.body.len();
            reader.load_body(&res.body);
            reader.status = Status::Ok(n);
            let _ = out.write(b"fetch:ok:");
            let _ = out.write(u64_slice(n as u64, &mut [0u8; 20]));
            let _ = out.write(b"\n");
            true
        }
        // A finished handle answers UnknownHandle if asked again; both mean
        // "this request is over and did not deliver".
        FetchStatus::Failed(_) | FetchStatus::UnknownHandle => {
            reader.pending = None;
            fetch_failed(reader, out);
            true
        }
    }
}

fn fetch_failed(reader: &mut Reader, out: &stdio::OutputStream) {
    reader.load_body(
        b"The fetch failed.\n\nThe host refused the connection or the server did not answer. Check the address, or try again with the Fetch button.",
    );
    reader.status = Status::Failed;
    let _ = out.write(b"fetch:error\n");
}

/// Start the fetch: a live GET when a URL is present, else load the sample.
/// Prints the machine-readable result line for automated runs (a live one
/// prints it when the request settles).
fn do_fetch(reader: &mut Reader, has_url: bool, out: &stdio::OutputStream) {
    if has_url {
        // `net::begin`, never the blocking `get`. This app is named by the
        // authoring pack as THE example of a request that does not freeze
        // the window -- and for a long time it called the blocking form
        // from its pointer handler, so every generated app that fetched
        // anything inherited a window that stopped answering until the
        // server did (K-770). `begin` returns a handle at once; the event
        // loop polls it each round and keeps drawing the last known
        // state, which is the whole point.
        let req = Request {
            method: HttpMethod::Get,
            url: alloc::string::String::from(reader.url_str()),
            headers: Vec::new(),
            body: Vec::new(),
            timeout_millis: Some(15_000),
        };
        match net::begin(req) {
            Ok(handle) => {
                reader.pending = Some(handle);
                reader.status = Status::Loading;
                // Say what is happening in the card, not only the status
                // line: an empty card reads as "nothing came back".
                reader.load_body(
                    b"Waiting for the server...\n\nThe window keeps answering while the request is out: scroll, resize, or press Fetch again.",
                );
            }
            Err(_) => fetch_failed(reader, out),
        }
        return;
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

/// Where everything goes, worked out from the canvas the app really has and
/// the person's text size. Drawing and hit-testing both ask this one
/// function, so a tap always lands on what was drawn.
struct Layout {
    canvas: Rect,
    /// The reading column: the whole width on a phone, centred on a desktop.
    col: Rect,
    title: f32,
    title_y: f32,
    /// The subtitle, left out when a phone is turned and height is short.
    sub: Option<(f32, f32)>,
    url: Rect,
    btn: Rect,
    btn_text: f32,
    card: Rect,
    /// Inside the card: where text starts, how wide it may run, how tall a
    /// row is, and how many rows fit.
    text_x: f32,
    text_top: f32,
    room: f32,
    body: f32,
    head: f32,
    step: f32,
    visible: usize,
    status: Rect,
    status_text: f32,
}

fn layout(canvas: u64, win: u64) -> Layout {
    let size = canvas2d::canvas_size(canvas).unwrap_or(gfx::Size {
        width: WIDTH,
        height: HEIGHT,
    });
    let (w, h) = (size.width.max(1.0), size.height.max(1.0));
    let look = Look::now();
    // The notch, the home bar and the close button Krate draws on a phone
    // cover the edges; text and controls stay inside them.
    let screen = Screen::of(win);
    let (st, sr, sb, sl) = screen.safe;
    let whole = Rect::new(0.0, 0.0, w, h);
    let margin = if w < 600.0 { 16.0 } else { 28.0 };
    let safe = whole.inset(st, sr, sb, sl).pad(margin);
    let col_w = safe.w.min(MAX_COLUMN * look.text_scale.max(1.0));
    let column = Rect::new(safe.x + (safe.w - col_w) * 0.5, safe.y, col_w, safe.h);
    let mut col = column;
    // A phone turned sideways has height for the reader or for a big
    // header, not both: the reader wins.
    let short = h < 500.0;

    let title = look.text(if short { 22.0 } else { 30.0 });
    let bar_h = (46.0 * look.text_scale).max(screen.tap());
    let top = col.y;
    let (title_y, sub) = if short {
        // The title sits at the left of the bar's row instead of above it;
        // the column gets its full width back below the bar.
        col.take_left(width_of(canvas, "Reader", title) + 24.0);
        (top + bar_h * 0.5 + title * 0.35, None)
    } else {
        let s = look.text(13.0);
        let head_h = title * 1.1 + s * 1.9;
        col.take_top(head_h + 16.0);
        (top + title, Some((s, top + head_h - s * 0.4)))
    };

    // The URL bar and the button share a row while the bar keeps room to
    // show an address; otherwise the button goes under it, full width.
    let btn_text = look.text(17.0);
    let btn_w = width_of(canvas, "Fetch", btn_text) + 56.0;
    let (url, btn) = if col.w - btn_w - 12.0 >= 180.0 * look.text_scale {
        let mut bar = col.take_top(bar_h);
        let btn = bar.take_right(btn_w);
        (bar.inset(0.0, 12.0, 0.0, 0.0), btn)
    } else {
        let url = col.take_top(bar_h);
        col.take_top(10.0);
        (url, col.take_top(bar_h))
    };
    col.take_top(16.0);
    // Below the bar the reader has the column's full width again.
    col = Rect::new(column.x, col.y, column.w, col.h);

    let status_text = look.text(13.5);
    let status = col.take_bottom(status_text * 2.4);
    let card = col.inset(0.0, 0.0, 8.0, 0.0);

    let body = look.text(if screen.compact() { 16.0 } else { 15.0 });
    let head = look.text(18.0);
    let step = body * 1.6;
    let pad_y = 18.0;
    let visible = (((card.h - 2.0 * pad_y) / step) as usize).max(1);
    Layout {
        canvas: whole,
        col: column,
        title,
        title_y,
        sub,
        url,
        btn,
        btn_text,
        card,
        text_x: card.x + 22.0,
        text_top: card.y + pad_y,
        // Leave the scrollbar its lane on the right.
        room: card.w - 44.0 - 10.0,
        body,
        head,
        step,
        visible,
        status,
        status_text,
    }
}

/// Wrap every stored line to `room`, breaking at a space where one fits.
/// The first line is the page's heading and is measured at its own size.
fn wrap_rows(canvas: u64, reader: &Reader, l: &Layout) -> Vec<Row> {
    let mut rows = Vec::new();
    for line in 0..reader.n_lines {
        let text = reader.line(line);
        let size = if line == 0 { l.head } else { l.body };
        if text.is_empty() {
            rows.push(Row { line, start: 0, end: 0 });
            continue;
        }
        let mut start = 0usize;
        while start < text.len() {
            let rest = text.get(start..).unwrap_or("");
            let take = fit_len(canvas, rest, size, l.room);
            rows.push(Row { line, start, end: start + take });
            start += take;
            // The space a line broke at does not start the next row.
            while text.as_bytes().get(start) == Some(&b' ') {
                start += 1;
            }
        }
    }
    rows
}

/// How many bytes of `s` fit in `room`: all of it, or the longest leading
/// part that ends at a space, or -- for one long word -- as much as fits.
/// Always at least one character, so wrapping always moves forward.
fn fit_len(canvas: u64, s: &str, size: f32, room: f32) -> usize {
    if width_of(canvas, s, size) <= room {
        return s.len();
    }
    let (mut lo, mut hi) = (0usize, s.len());
    while lo < hi {
        let mut mid = (lo + hi + 1) / 2;
        while mid > lo && !s.is_char_boundary(mid) {
            mid -= 1;
        }
        if mid == lo {
            break;
        }
        if width_of(canvas, s.get(..mid).unwrap_or(""), size) <= room {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let part = s.get(..lo).unwrap_or("");
    match part.rfind(' ') {
        Some(space) if space > 0 => space,
        _ if lo > 0 => lo,
        _ => s.chars().next().map(char::len_utf8).unwrap_or(s.len()),
    }
}

// ------------------------------------------------------------------
// Rendering
// ------------------------------------------------------------------

fn draw(canvas: u64, win: u64, reader: &mut Reader) -> Result<(), gfx::GfxError> {
    let l = layout(canvas, win);
    let key = (l.room.to_bits(), l.body.to_bits(), reader.body_gen);
    if reader.wrapped_for != Some(key) {
        reader.rows = wrap_rows(canvas, reader, &l);
        reader.wrapped_for = Some(key);
    }
    // What this frame can show decides how far scrolling may go; a window
    // made taller pulls the last rows back into view.
    reader.visible = l.visible;
    reader.step = l.step;
    reader.scroll = reader.scroll.min(reader.max_scroll());
    let mut said = Said::new();

    // Considered dark ground with a soft top glow.
    let c = l.canvas;
    canvas2d::linear_gradient(
        canvas,
        gfx::Rect { x: c.x, y: c.y, width: c.w, height: c.h },
        color(0.078, 0.090, 0.145, 1.0),
        color(0.039, 0.047, 0.078, 1.0),
    )?;
    canvas2d::radial_gradient(
        canvas,
        gfx::Point { x: l.col.x + 52.0, y: l.col.y },
        300.0,
        color(0.30, 0.50, 0.95, 0.14),
        color(0.30, 0.50, 0.95, 0.0),
    )?;

    // ---- header ----
    let ink = color(0.96, 0.97, 1.0, 1.0);
    draw_text(canvas, "Reader", l.col.x, l.title_y, l.title, ink)?;
    said.heading(Rect::new(l.col.x, l.title_y - l.title, l.col.w, l.title * 1.2), "Reader");
    if let Some((size, y)) = l.sub {
        let sub = "Fetch a page over HTTP and read it";
        fit(canvas, sub, l.col.x, y, size, color(0.55, 0.63, 0.82, 1.0), l.col.w);
        said.text(Rect::new(l.col.x, y - size, l.col.w, size * 1.4), sub);
    }

    // ---- URL bar ----
    let u = l.url;
    round_rect(canvas, u, 12.0, color(0.129, 0.145, 0.216, 1.0))?;
    // A small globe dot as an affordance.
    disc(canvas, u.x + 22.0, u.y + u.h * 0.5, 5.0, color(0.36, 0.72, 1.0, 1.0))?;
    let url_size = l.body;
    let url = reader.url_str();
    fit(
        canvas,
        url,
        u.x + 40.0,
        u.y + u.h * 0.5 + url_size * 0.35,
        url_size,
        color(0.86, 0.90, 0.98, 1.0),
        u.w - 54.0,
    );
    said.text(u, url);

    // ---- Fetch button ----
    draw_button(canvas, l.btn, l.btn_text, "Fetch", reader.hover, reader.pressed)?;
    said.button(l.btn, "Fetch");

    // ---- content card ----
    let k = l.card;
    round_rect(canvas, k, 16.0, color(0.105, 0.121, 0.184, 1.0))?;
    // Lit top edge (opaque, inset from corners).
    canvas2d::fill_rect(
        canvas,
        gfx::Rect { x: k.x + 16.0, y: k.y, width: (k.w - 32.0).max(0.0), height: 1.5 },
        color(0.20, 0.22, 0.32, 1.0),
    )?;

    // Only the rows on screen are drawn and said; the rest wait for a scroll.
    let mut vis = 0usize;
    while vis < l.visible {
        let Some(row) = reader.rows.get(reader.scroll + vis).copied() else {
            break;
        };
        let t = reader.row_text(row);
        if !t.is_empty() {
            // The page's first line reads as a heading; render it brighter.
            let (size, ink) = if row.line == 0 {
                (l.head, ink)
            } else {
                (l.body, color(0.80, 0.85, 0.94, 1.0))
            };
            let top = l.text_top + vis as f32 * l.step;
            draw_text(canvas, t, l.text_x, top + l.step * 0.72, size, ink)?;
            let at = Rect::new(l.text_x, top, l.room, l.step);
            if row.line == 0 {
                said.heading(at, t);
            } else {
                said.text(at, t);
            }
        }
        vis += 1;
    }

    // Scrollbar on the right of the card, when content overflows.
    draw_scrollbar(canvas, reader, &l)?;

    // ---- status line ----
    draw_status(canvas, reader, &l, &mut said)?;

    canvas2d::present(canvas)?;
    said.send(win);
    Ok(())
}

fn draw_button(
    canvas: u64,
    b: Rect,
    size: f32,
    label: &str,
    hover: bool,
    pressed: bool,
) -> Result<(), gfx::GfxError> {
    // Opaque shadow, then an opaque accent body that reacts to hover/press.
    round_rect(canvas, Rect::new(b.x, b.y + 3.0, b.w, b.h), 12.0, color(0.020, 0.025, 0.045, 1.0))?;
    let (r, g, bl) = (0.24f32, 0.55f32, 1.0f32);
    let k = if pressed {
        0.80
    } else if hover {
        1.12
    } else {
        1.0
    };
    round_rect(
        canvas,
        b,
        12.0,
        color((r * k).min(1.0), (g * k).min(1.0), (bl * k).min(1.0), 1.0),
    )?;
    let lw = width_of(canvas, label, size);
    draw_text(
        canvas,
        label,
        b.x + (b.w - lw) * 0.5,
        b.y + b.h * 0.5 + size * 0.35,
        size,
        color(1.0, 1.0, 1.0, 1.0),
    )
}

fn draw_scrollbar(canvas: u64, reader: &Reader, l: &Layout) -> Result<(), gfx::GfxError> {
    let total = reader.rows.len();
    if total <= l.visible {
        return Ok(());
    }
    let track = Rect::new(l.card.right() - 14.0, l.card.y + 16.0, 4.0, l.card.h - 32.0);
    round_rect(canvas, track, 2.0, color(0.20, 0.23, 0.32, 1.0))?;
    let thumb_h = (track.h * l.visible as f32 / total as f32).max(24.0);
    let t = reader.scroll as f32 / reader.max_scroll().max(1) as f32;
    let thumb = Rect::new(track.x, track.y + (track.h - thumb_h) * t, 4.0, thumb_h);
    round_rect(canvas, thumb, 2.0, color(0.36, 0.72, 1.0, 1.0))
}

fn draw_status(
    canvas: u64,
    reader: &Reader,
    l: &Layout,
    said: &mut Said,
) -> Result<(), gfx::GfxError> {
    let s = l.status;
    let size = l.status_text;
    let mid = s.y + s.h * 0.5;
    let dot = match reader.status {
        // The amber dot breathes while the request is out, so a slow server
        // reads as "working" rather than "stuck".
        Status::Loading => color(0.98, 0.80, 0.35, 0.55 + 0.45 * pulse(reader.tick)),
        Status::Ok(_) => color(0.42, 0.90, 0.55, 1.0),
        Status::Failed => color(0.98, 0.45, 0.55, 1.0),
        Status::Sample => color(0.36, 0.72, 1.0, 1.0),
    };
    disc(canvas, s.x + 6.0, mid, 5.0, dot)?;

    let mut buf = [0u8; 64];
    let text: &str = match reader.status {
        Status::Loading => "Fetching... the window stays live",
        Status::Ok(n) => valid_prefix(status_ok(n as u64, &mut buf)),
        Status::Failed => "Fetch failed: the host did not answer",
        Status::Sample => "Built-in sample -- pass a URL to fetch",
    };
    let x = s.x + 20.0;
    fit(canvas, text, x, mid + size * 0.35, size, color(0.82, 0.87, 0.96, 1.0), s.right() - x);
    said.text(s, text);
    Ok(())
}

/// 0..1 and back over about a second of rounds.
fn pulse(tick: u32) -> f32 {
    let phase = (tick % 20) as f32 / 10.0;
    if phase > 1.0 {
        2.0 - phase
    } else {
        phase
    }
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

fn round_rect(canvas: u64, at: Rect, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    // One host call, antialiased on the curve. The bands-plus-corner-discs
    // version this replaced showed seams where the pieces met.
    canvas2d::fill_round_rect(
        canvas,
        gfx::Rect { x: at.x, y: at.y, width: at.w.max(0.0), height: at.h.max(0.0) },
        gfx::CornerRadii { top_left: r, top_right: r, bottom_right: r, bottom_left: r },
        c,
    )
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

fn width_of(canvas: u64, s: &str, size: f32) -> f32 {
    canvas2d::measure_text(canvas, s, size).map(|m| m.width).unwrap_or(0.0)
}

/// Draw `s` at most `room` wide: whole when it fits, else cut with an
/// ellipsis. A cut line reads as "there is more"; a line running off the
/// card reads as a broken app. Used for one-line things (the address, the
/// status); the page body wraps instead.
fn fit(canvas: u64, s: &str, x: f32, y: f32, size: f32, c: gfx::Color, room: f32) {
    if room <= 0.0 {
        return;
    }
    if width_of(canvas, s, size) <= room {
        let _ = draw_text(canvas, s, x, y, size, c);
        return;
    }
    let ell = width_of(canvas, "…", size);
    let (mut lo, mut hi) = (0usize, s.len());
    while lo < hi {
        let mut mid = (lo + hi + 1) / 2;
        while mid > lo && !s.is_char_boundary(mid) {
            mid -= 1;
        }
        if mid == lo {
            break;
        }
        if width_of(canvas, s.get(..mid).unwrap_or(""), size) + ell <= room {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let part = s.get(..lo).unwrap_or("").trim_end();
    let _ = draw_text(canvas, part, x, y, size, c);
    let _ = draw_text(canvas, "…", x + width_of(canvas, part, size), y, size, c);
}

fn color(r: f32, g: f32, b: f32, a: f32) -> gfx::Color {
    gfx::Color { r, g, b, a }
}

// ------------------------------------------------------------------
// Byte helpers, panic-free
// ------------------------------------------------------------------

/// The longest leading part of `bytes` that is valid UTF-8.
fn valid_prefix(bytes: &[u8]) -> &str {
    match core::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => core::str::from_utf8(bytes.get(..e.valid_up_to()).unwrap_or(&[])).unwrap_or(""),
    }
}

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

krate::export!(Component);
