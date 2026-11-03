//! Query -- a SQL client for the app's own database, drawn on one canvas.
//!
//! The layout is the one a developer already knows: a table list down the left
//! with row counts, a syntax-coloured statement across the top, and the answer
//! underneath as a real results grid -- column headers on a rule, alternating
//! row tint, text left and numbers right. A status strip reports the shape of
//! the answer the way a client does.
//!
//! It lays itself out from the canvas it really has, every frame. A desktop
//! window or a tablet gets the rail beside the editor and grid; a phone gets
//! one column -- the tables as a strip of chips, the statement soft-wrapped
//! full width, the results under it -- and the page scrolls as one. When the
//! person's text is too large for three columns on a phone, each result row
//! becomes two lines instead of a shrunk grid. Every font size and row height
//! follows their text size, and each frame tells the screen reader what it
//! shows.
//!
//! The grid is not a picture of a query. The app creates the table, seeds it,
//! and runs the statement shown in the editor through `store.sql`, so the rows
//! on screen are what the database returned. A host that withholds `store.sql`
//! still gets a window: the same rows come from a const array and the status
//! strip says the connection is unavailable, because a preview that renders
//! nothing teaches nothing.
//!
//! `#![no_std]` is load-bearing here, not a preference. `query` returns a
//! nested `list<row<list<value>>>` and the generated glue that lifts it calls
//! `Vec::with_capacity`; in a std-linked guest that reaches std's
//! allocation-error handler, which drags the whole `wasi:*` import set into the
//! component and the Krate linker rejects it. Owning the allocator and a
//! trapping panic handler makes the same path trap instead of leak.

#![no_std]
#![allow(clippy::needless_range_loop)]

extern crate alloc;

use alloc::string::String;
use krate::gfx::{canvas2d, types as gfx};
use krate::bindings::krate::io::{args, stdio};
use krate::bindings::krate::store::sql;
use krate::layout::{Rect, Said, Screen};
use krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

/// The window asked for on a desktop. Only a request: every frame lays out
/// from the canvas the host really gave.
const WIDTH: f32 = 1180.0;
const HEIGHT: f32 = 760.0;

const WAIT_ROUND_MILLIS: u32 = 33;

// ---- palette ---------------------------------------------------------------
// Cooler and flatter than Pulse's: a database client is a tool you stare at all
// day, so the ground is near-black and the only saturated colour is the one
// carrying meaning (keywords, the selected table, the connected dot).

const BG: gfx::Color = rgb(0.055, 0.063, 0.078);
const RAIL: gfx::Color = rgb(0.075, 0.086, 0.106);
const PANE: gfx::Color = rgb(0.086, 0.098, 0.122);
const EDITOR: gfx::Color = rgb(0.067, 0.078, 0.098);
const LINE: gfx::Color = rgb(0.141, 0.161, 0.196);
const LINE_SOFT: gfx::Color = rgb(0.110, 0.126, 0.157);
/// Every second grid row, one step off the pane so the eye tracks across.
const ROW_TINT: gfx::Color = rgb(0.098, 0.110, 0.137);

const INK: gfx::Color = rgb(0.902, 0.925, 0.957);
const INK_DIM: gfx::Color = rgb(0.573, 0.616, 0.686);
const INK_QUIET: gfx::Color = rgb(0.373, 0.412, 0.478);

const ACCENT: gfx::Color = rgb(0.353, 0.596, 1.0);
/// Pre-blended: a translucent rounded shape doubles alpha where the rect and
/// its corner discs overlap, so selections use flattened opaque tints.
const SEL_WASH: gfx::Color = rgb(0.129, 0.180, 0.286);
const GREEN: gfx::Color = rgb(0.310, 0.827, 0.549);

// Syntax colours. Three roles only -- keyword, string, number -- because a
// client that colours nine things is harder to read than one that colours the
// three that change the meaning of a statement.
const KEYWORD: gfx::Color = rgb(0.780, 0.545, 0.980);
const STRING: gfx::Color = rgb(0.980, 0.702, 0.404);
const NUMBER: gfx::Color = rgb(0.404, 0.824, 0.729);
const IDENT: gfx::Color = rgb(0.847, 0.882, 0.925);
/// Commas, dots, brackets and operators: quieter than the words they join.
const PUNCT: gfx::Color = INK_DIM;

const fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
}

// ---- the schema this client is pointed at ----------------------------------

/// One table in the left rail: name, row count, and whether it is the one the
/// statement reads from. Counts are the sizes a database of this shape has --
/// they are the schema's own numbers, not query results, so they stay const.
struct Table {
    name: &'static str,
    rows: u64,
}

const TABLES: [Table; 5] = [
    Table { name: "users", rows: 1_284 },
    Table { name: "orders", rows: 8_912 },
    Table { name: "products", rows: 342 },
    Table { name: "sessions", rows: 24_061 },
    Table { name: "events", rows: 91_203 },
];
/// `users` is what the shown statement drives from, so it is the selected row.
const SELECTED_TABLE: usize = 0;

/// The statement in the editor, and the one actually sent to `store.sql`. One
/// constant serves both so the grid can never drift from the text above it.
const STATEMENT: &str = concat!(
    "SELECT u.email, count(o.id) AS orders, sum(o.total) AS revenue ",
    "FROM users u JOIN orders o ON o.user_id = u.id ",
    "GROUP BY u.email ORDER BY revenue DESC LIMIT 8;",
);

/// The statement wrapped for the editor strip. Broken at clause boundaries, the
/// way a person would break it, rather than at whatever column ran out.
const STATEMENT_LINES: [&str; 3] = [
    "SELECT u.email, count(o.id) AS orders, sum(o.total) AS revenue",
    "FROM users u JOIN orders o ON o.user_id = u.id",
    "GROUP BY u.email ORDER BY revenue DESC LIMIT 8;",
];

/// Words the highlighter treats as keywords. Matched whole and
/// case-insensitively, so `count` in `count(o.id)` colours but `orders` does
/// not accidentally match `ORDER`.
const KEYWORDS: [&str; 14] = [
    "select", "from", "join", "on", "group", "by", "order", "desc", "asc", "limit", "where", "as",
    "count", "sum",
];

// ---- the seed rows ---------------------------------------------------------

/// The customers the database is seeded with, and the fallback the grid draws
/// when `store.sql` is not granted. Revenue is in cents so the sum stays an
/// integer all the way through SQL and formatting.
const SEED: [(&str, i64, i64); 8] = [
    ("dana.whitfield@northlane.io", 47, 1_284_050),
    ("m.okonkwo@bridgeport.co", 39, 1_106_720),
    ("sarah.lindqvist@velum.se", 34, 982_400),
    ("t.nakamura@hokkaido-eng.jp", 31, 874_915),
    ("priya.raghavan@sundial.in", 28, 806_240),
    ("j.almeida@portomar.pt", 24, 691_380),
    ("elena.vasquez@cordoba.mx", 21, 604_775),
    ("chris@fieldnotes.dev", 18, 512_090),
];

/// Rows held for drawing. The statement says LIMIT 8; the pool is larger so a
/// host that returns more cannot walk off the end.
const MAX_ROWS: usize = 16;
const EMAIL_CAP: usize = 48;

#[derive(Clone, Copy)]
struct Record {
    email: [u8; EMAIL_CAP],
    email_len: usize,
    orders: i64,
    revenue: i64,
}

const ZERO_RECORD: Record = Record {
    email: [0u8; EMAIL_CAP],
    email_len: 0,
    orders: 0,
    revenue: 0,
};
// ---- text helpers ----------------------------------------------------------

/// Rendered width, measured by the host with the same layout `draw_text` uses.
/// The face is proportional, so a right-aligned number only lines up if the
/// width comes from the host rather than from a character count.
fn est_width(canvas: u64, s: &str, size: f32) -> f32 {
    canvas2d::measure_text(canvas, s, size).map_or(0.0, |m| m.width)
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

/// Format an integer into `buf` with thousands separators: "24,061". Digits
/// go right to left with a comma before every fourth slot, then reversed.
fn grouped(buf: &mut [u8; 24], value: i64) -> &str {
    let mut tmp = [0u8; 28];
    let mut n = 0usize;
    let mut v = value.unsigned_abs();
    loop {
        if n % 4 == 3 {
            if let Some(slot) = tmp.get_mut(n) {
                *slot = b',';
            }
            n += 1;
        }
        if let Some(slot) = tmp.get_mut(n) {
            *slot = b'0' + (v % 10) as u8;
        }
        n += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    if value < 0 {
        if let Some(slot) = tmp.get_mut(n) {
            *slot = b'-';
        }
        n += 1;
    }
    let n = n.min(buf.len());
    for i in 0..n {
        if let (Some(dst), Some(src)) = (buf.get_mut(i), tmp.get(n - 1 - i)) {
            *dst = *src;
        }
    }
    as_str(buf, n)
}

/// Format cents as money into `buf`: "$12,840.50". Revenue is kept in cents
/// so the sum stays an integer all the way through SQL and formatting.
fn money(buf: &mut [u8; 24], cents: i64) -> &str {
    let mut whole = [0u8; 24];
    let digits = grouped(&mut whole, (cents / 100).saturating_abs());
    let frac = (cents % 100).unsigned_abs() as u8;
    let sign: &[u8] = if cents < 0 { b"-$" } else { b"$" };
    let mut n = 0usize;
    for part in [sign, digits.as_bytes(), &[b'.', b'0' + frac / 10, b'0' + frac % 10]] {
        if let Some(dst) = buf.get_mut(n..) {
            n += copy_into(dst, part);
        }
    }
    as_str(buf, n)
}

/// Copy `src` into `dst`, up to `dst.len()`, returning bytes written.
fn copy_into(dst: &mut [u8], src: &[u8]) -> usize {
    let n = src.len().min(dst.len());
    if let (Some(d), Some(s)) = (dst.get_mut(..n), src.get(..n)) {
        d.copy_from_slice(s);
    }
    n
}

fn as_str(bytes: &[u8], len: usize) -> &str {
    core::str::from_utf8(bytes.get(..len).unwrap_or(&[])).unwrap_or("")
}

/// A short phrase built in a fixed buffer, for what the screen reader is
/// told: there is no `format!` here. A part that would not fit whole is left
/// off rather than cut mid-character.
struct Phrase {
    buf: [u8; 160],
    len: usize,
}

impl Phrase {
    fn new() -> Self {
        Phrase { buf: [0u8; 160], len: 0 }
    }

    fn push(&mut self, part: &str) -> &mut Self {
        if let Some(dst) = self.buf.get_mut(self.len..self.len + part.len()) {
            dst.copy_from_slice(part.as_bytes());
            self.len += part.len();
        }
        self
    }

    fn as_str(&self) -> &str {
        as_str(&self.buf, self.len)
    }
}

/// The byte offset of the `k`th character, so a cut always lands between
/// characters even if an address is not ASCII.
fn byte_at(s: &str, k: usize) -> usize {
    s.char_indices().nth(k).map_or(s.len(), |(i, _)| i)
}

// ---- the display list ------------------------------------------------------
//
// Laying out a frame measures a few hundred strings and works out where
// everything goes. On a phone the app's own code runs on an interpreter, and
// doing all of that on every scroll step cost three phone frames per step. So
// a layout is recorded once -- when the size, the text size or the answer
// changes -- as a list of drawing steps, and a scroll step only replays the
// list with the scrolling part moved.

#[derive(Clone, Copy, PartialEq)]
enum Kind { Clear, Fill, Round, Disc, Text, Clip, Unclip, Heading, Button, Said }

/// One recorded step. `x, y, w, h` is its box -- for text the origin, the
/// measured width and the size; for a disc the centre and, in `w`, the
/// radius -- `r` and `rb` the top and bottom corner radii and `text` a range
/// of the arena.
#[derive(Clone, Copy)]
struct Op {
    kind: Kind,
    scrolls: bool,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
    rb: f32,
    c: gfx::Color,
    text: (usize, usize),
}

const NO_OP: Op = Op { kind: Kind::Fill, scrolls: false, x: 0.0, y: 0.0, w: 0.0, h: 0.0, r: 0.0, rb: 0.0, c: BG, text: (0, 0) };
const MAX_OPS: usize = 640;
const ARENA: usize = 8192;

struct List {
    ops: [Op; MAX_OPS],
    n: usize,
    arena: [u8; ARENA],
    used: usize,
    /// What the list was laid out for: size, text size, safe edges, answer.
    key: [f32; 8],
    /// Whether the next steps move with the scroll.
    scrolls: bool,
}

impl List {
    fn clear(&mut self) { (self.n, self.used, self.scrolls) = (0, 0, false) }

    /// Record one step. A list that is full drops the rest of the frame
    /// rather than writing past its end.
    fn push(&mut self, kind: Kind, b: Rect, r: f32, c: gfx::Color, s: &str) { self.push_radii(kind, b, (r, r), c, s) }

    fn push_radii(&mut self, kind: Kind, b: Rect, (r, rb): (f32, f32), c: gfx::Color, s: &str) {
        let start = self.used;
        if let Some(dst) = self.arena.get_mut(start..start + s.len()) {
            dst.copy_from_slice(s.as_bytes());
            self.used += s.len();
        }
        if let Some(slot) = self.ops.get_mut(self.n) {
            let text = (start, self.used);
            *slot = Op { kind, scrolls: self.scrolls, x: b.x, y: b.y, w: b.w, h: b.h, r, rb, c, text };
            self.n += 1;
        }
    }

    /// Draw the list with the scrolling part moved up by `scroll`, then tell
    /// the screen reader what is on screen. Nothing is skipped for being out
    /// of sight: the clip already costs the host nothing there, and a string
    /// first drawn mid-scroll cost a whole frame to lay out (measured).
    fn replay(&self, canvas: u64, win: u64, scroll: f32) -> Result<(), gfx::GfxError> {
        let mut said = Said::new();
        for op in self.ops.get(..self.n).unwrap_or(&[]) {
            let y = if op.scrolls { op.y - scroll } else { op.y };
            let bytes = self.arena.get(op.text.0..op.text.1).unwrap_or(&[]);
            // SAFETY: the arena only ever receives whole `&str`s, and each op
            // spans exactly one of them, so its bytes are valid UTF-8.
            // Checking again on every replay was a tenth of a phone frame.
            let s = unsafe { core::str::from_utf8_unchecked(bytes) };
            let at = Rect::new(op.x, y, op.w, op.h);
            let rect = gfx::Rect { x: op.x, y, width: op.w, height: op.h };
            match op.kind {
                Kind::Clear => canvas2d::clear(canvas, op.c)?,
                Kind::Fill => canvas2d::fill_rect(canvas, rect, op.c)?,
                Kind::Round => {
                    let most = (op.w * 0.5).min(op.h * 0.5);
                    let (r, rb) = (op.r.min(most).max(0.0), op.rb.min(most).max(0.0));
                    let radii = gfx::CornerRadii { top_left: r, top_right: r, bottom_right: rb, bottom_left: rb };
                    canvas2d::fill_round_rect(canvas, rect, radii, op.c)?
                }
                Kind::Disc => canvas2d::fill_circle(canvas, gfx::Point { x: op.x, y }, op.w, op.c)?,
                Kind::Text => {
                    let _ = canvas2d::draw_text(canvas, s, gfx::Point { x: op.x, y }, op.h, op.c);
                }
                Kind::Clip => canvas2d::set_clip(canvas, op.x, y, op.w, op.h)?,
                Kind::Unclip => canvas2d::clear_clip(canvas)?,
                Kind::Heading => drop(said.heading(at, s)),
                Kind::Button => drop(said.button(at, s)),
                Kind::Said => drop(said.text(at, s)),
            }
        }
        canvas2d::present(canvas)?;
        said.send(win);
        Ok(())
    }
}

// ---- layout ----------------------------------------------------------------

/// Below this width the table rail and a grid beside it cannot both be read,
/// so the client stacks into one column and the page scrolls as one.
const SPLIT_MIN: f32 = 720.0;
/// The rail's width at the default text size. It grows with larger text, up
/// to a third of the window, so table names are not cut to a letter.
const RAIL_W: f32 = 200.0;
const HEADER_H: f32 = 38.0;
const ROW_H: f32 = 44.0;
/// Two-line rows, on a phone with large text: the address, then its numbers.
const STACKED_ROW_H: f32 = 62.0;
/// The narrowest the email column may get, at the default text size, before
/// a row stops being a grid row. Below it the address would show as a few
/// letters and an ellipsis, which answers nothing.
const EMAIL_MIN: f32 = 130.0;

/// What the person is looking at, and where the last layout put what can be
/// tapped. Taps are tested against these rects, so the drawing and the hit
/// test can never disagree.
struct View {
    /// On a phone the whole column scrolls; on a wide window only the rows of
    /// the grid do. Either way it is one offset, clamped to what was drawn.
    scroll: f32,
    max_scroll: f32,
    run: Option<Rect>,
    /// How many times the statement has run; a new answer is a new layout.
    runs: u32,
}

impl View {
    /// A tap (or a click) at `x`, `y`: whether it landed on Run.
    fn tap(&self, x: f32, y: f32) -> bool { self.run.is_some_and(|r| r.contains(x, y)) }

    /// The wheel, or a finger dragging. Returns whether anything moved.
    fn wheel(&mut self, dy: f32) -> bool {
        let next = (self.scroll + dy).clamp(0.0, self.max_scroll);
        let moved = next != self.scroll;
        self.scroll = next;
        moved
    }
}

/// The rows the database gave back, and whether it really was the database.
struct Answer {
    rows: [Record; MAX_ROWS],
    count: usize,
    live: bool,
}

impl Answer {
    fn records(&self) -> &[Record] { self.rows.get(..self.count).unwrap_or(&[]) }
}

/// One layout being recorded: the canvas (for measuring) and its width, the
/// person's text size -- every font size and row height goes through `t` --
/// whether this is the one-column layout, whether the screen is short (a
/// phone turned sideways), and the list it records into.
struct Frame<'a> {
    canvas: u64,
    w: f32,
    t: f32,
    narrow: bool,
    short: bool,
    list: &'a mut List,
}

impl Frame<'_> {
    /// Above the statement: its QUERY header, or on a short screen, where
    /// every line of height is wanted for the answer, a little padding.
    fn head(&self) -> f32 { self.pt(if self.short { 12.0 } else { 42.0 }) }
    /// One line of the statement.
    fn lead(&self) -> f32 { self.pt(if self.short { 22.0 } else { 26.0 }) }

    fn pt(&self, points: f32) -> f32 { points * self.t }
    fn width(&self, s: &str, size: f32) -> f32 { est_width(self.canvas, s, size) }
    fn fill(&mut self, x: f32, y: f32, w: f32, h: f32, c: gfx::Color) { self.list.push(Kind::Fill, Rect::new(x, y, w, h), 0.0, c, "") }
    fn round(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, c: gfx::Color) { self.list.push(Kind::Round, Rect::new(x, y, w, h), r, c, "") }
    /// A box with its top corners rounded `top` and its bottom ones `bottom`:
    /// the first and last bands of a card.
    fn round_ends(&mut self, b: Rect, top: f32, bottom: f32, c: gfx::Color) { self.list.push_radii(Kind::Round, b, (top, bottom), c, "") }
    /// A card: a hairline edge round a filled body, the way a phone sets a
    /// section off from the ground.
    fn card(&mut self, b: Rect, r: f32, c: gfx::Color) {
        self.rounded_box(b, (r, r), LINE_SOFT, true);
        self.rounded_box(Rect::new(b.x + 1.0, b.y + 1.0, b.w - 2.0, b.h - 2.0), (r - 1.0, r - 1.0), c, false);
    }
    /// A rounded box drawn as its two rounded ends and a plain middle (or,
    /// for an `outline` that a fill will cover, just the middle's two sides).
    /// The pixels are a single rounded rect's; the host's rounded fill costs
    /// by area, and a whole card of it was several times the rest of a
    /// phone's scroll frame (measured). Each end is a band twice its radius
    /// tall (the host fits a corner to half a shape's height), overlapping
    /// the middle; the colours are opaque, so the overlap never shows.
    fn rounded_box(&mut self, b: Rect, (top, bottom): (f32, f32), c: gfx::Color, outline: bool) {
        let most = (b.w * 0.5).min(b.h * 0.5);
        let (top, bottom) = (top.min(most).max(0.0), bottom.min(most).max(0.0));
        if b.h <= 2.0 * (top + bottom) {
            self.round_ends(b, top, bottom, c);
            return;
        }
        if top > 0.0 {
            self.round_ends(Rect::new(b.x, b.y, b.w, 2.0 * top), top, 0.0, c);
        }
        let mid = b.h - top - bottom;
        if outline {
            self.fill(b.x, b.y + top, 1.0, mid, c);
            self.fill(b.right() - 1.0, b.y + top, 1.0, mid, c);
        } else {
            self.fill(b.x, b.y + top, b.w, mid, c);
        }
        if bottom > 0.0 {
            self.round_ends(Rect::new(b.x, b.bottom() - 2.0 * bottom, b.w, 2.0 * bottom), 0.0, bottom, c);
        }
    }
    fn disc(&mut self, x: f32, y: f32, r: f32, c: gfx::Color) { self.list.push(Kind::Disc, Rect::new(x, y, r, 0.0), 0.0, c, "") }
    fn clip(&mut self, b: Rect) { self.list.push(Kind::Clip, b, 0.0, BG, "") }
    fn unclip(&mut self) { self.list.push(Kind::Unclip, Rect::new(0.0, 0.0, 0.0, 0.0), 0.0, BG, "") }
    fn say(&mut self, kind: Kind, at: Rect, s: &str) { self.list.push(kind, at, 0.0, BG, s) }

    /// Draw `s` with its left edge at `x`; returns its width.
    fn text(&mut self, s: &str, x: f32, y: f32, size: f32, c: gfx::Color) -> f32 {
        let w = self.width(s, size);
        self.list.push(Kind::Text, Rect::new(x, y, w, size), 0.0, c, s);
        w
    }

    fn text_right(&mut self, s: &str, right: f32, y: f32, size: f32, c: gfx::Color) -> f32 {
        let w = self.width(s, size);
        self.list.push(Kind::Text, Rect::new(right - w, y, w, size), 0.0, c, s);
        w
    }

    /// Draw `s` at most `room` wide: the longest leading part that fits,
    /// then an ellipsis. A cell cut short reads as "there is more"; one
    /// running into the next column reads as a broken app.
    fn fit(&mut self, s: &str, x: f32, y: f32, size: f32, c: gfx::Color, room: f32) {
        if room <= 0.0 {
            return;
        }
        // A hair of slack: a box sized to the text's own measured width must
        // not lose its last letter to rounding.
        if self.width(s, size) <= room + 0.5 {
            self.text(s, x, y, size, c);
            return;
        }
        let ell = "\u{2026}";
        let ell_w = self.width(ell, size);
        let (mut lo, mut hi) = (0usize, s.chars().count());
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            let part = s.get(..byte_at(s, mid)).unwrap_or("");
            if self.width(part, size) + ell_w <= room {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        // One string, so a cut cell is one call to the host, not two.
        let mut p = Phrase::new();
        p.push(s.get(..byte_at(s, lo)).unwrap_or("").trim_end()).push(ell);
        self.text(p.as_str(), x, y, size, c);
    }
}

/// Where the grid's columns go in a box of a given width, worked out once
/// so the header, the rows and the scroll limit all use the same numbers.
struct Grid {
    pad: f32,
    email_x: f32,
    email_w: f32,
    orders_right: f32,
    revenue_right: f32,
    /// Rows are two lines: the address alone, then orders and revenue.
    stacked: bool,
    header_h: f32,
    row_h: f32,
}

impl Grid {
    fn new(f: &Frame, b: Rect, answer: &Answer) -> Grid {
        let pad = if f.narrow { 14.0 } else { 28.0 };
        let cell = f.pt(13.5);
        let head = f.pt(11.5);
        // Each numeric column is as wide as its widest value really is,
        // measured, so a right-aligned figure never reaches the rule.
        let mut orders_min = f.width("orders", head);
        let mut revenue_min = f.width("revenue", head);
        let mut widest = 0.0f32;
        for record in answer.records() {
            widest = widest.max(f.width(as_str(&record.email, record.email_len), cell));
            let mut buf = [0u8; 24];
            orders_min = orders_min.max(f.width(grouped(&mut buf, record.orders), cell));
            revenue_min = revenue_min.max(f.width(money(&mut buf, record.revenue), cell));
        }
        let avail = (b.w - 4.0 * pad).max(0.0);
        // The desktop proportions the grid was drawn with: about a half for
        // the address, a sixth and a quarter for the figures.
        let orders_w = orders_min.max(avail * 0.16);
        let revenue_w = revenue_min.max(avail * 0.25);
        let email_w = avail - orders_w - revenue_w;
        // A phone would rather give a row two lines than cut an address:
        // there the whole address is the answer, and the column is narrow.
        let stacked = email_w < EMAIL_MIN * f.t || (f.narrow && widest > email_w + 0.5);
        let orders_right = b.x + pad + email_w + pad + orders_w;
        Grid {
            pad,
            email_x: b.x + pad,
            email_w: if stacked { b.w - 2.0 * pad } else { email_w },
            orders_right,
            revenue_right: if stacked { b.right() - pad } else { orders_right + pad + revenue_w },
            stacked,
            header_h: HEADER_H * f.t,
            row_h: f.t * if stacked { STACKED_ROW_H } else { ROW_H },
        }
    }

    /// The rules between columns, at the middle of each gap.
    fn rules(&self) -> [f32; 2] {
        [self.email_x + self.email_w + self.pad * 0.5, self.orders_right + self.pad * 0.5]
    }
}

// ---- drawing ---------------------------------------------------------------

/// Draw a frame: lay it out again only when what it is laid out for changed
/// (the window, the text size, the answer), otherwise replay the last layout
/// at the current scroll.
fn draw(canvas: u64, win: u64, answer: &Answer, view: &mut View, list: &mut List) -> Result<(), gfx::GfxError> {
    let size = canvas2d::canvas_size(canvas).unwrap_or(gfx::Size { width: WIDTH, height: HEIGHT });
    let (w, h) = (size.width.max(1.0), size.height.max(1.0));
    let screen = Screen::of(win);
    let t = screen.look.text_scale.clamp(0.8, 2.4);
    let (st, sr, sb, sl) = screen.safe;
    let key = [w, h, t, st, sr, sb, sl, view.runs as f32];
    if list.key != key {
        list.key = key;
        list.clear();
        let mut f = Frame { canvas, w, t, narrow: w < SPLIT_MIN, short: screen.short(), list };
        lay_out(&mut f, h, screen, answer, view);
    }
    view.scroll = view.scroll.min(view.max_scroll);
    list.replay(canvas, win, view.scroll)
}

/// Record one layout from the canvas the app really has: the rail beside
/// the editor and grid when wide, one scrolling column when narrow.
fn lay_out(f: &mut Frame, h: f32, screen: Screen, answer: &Answer, view: &mut View) {
    let (safe_top, safe_right, safe_bottom, safe_left) = screen.safe;
    let w = f.w;
    // The ground as one clear, not a full-screen rect: on a phone that rect
    // was a tenth of every scroll frame.
    f.list.push(Kind::Clear, Rect::new(0.0, 0.0, w, h), 0.0, BG, "");

    // A finger needs a 44-point Run button, so the bar grows to hold one.
    let bar_h = (f.pt(15.0) * 2.9).max(44.0).max(screen.tap() + 12.0);
    let bar_bottom = safe_top + bar_h;
    let status_h = (f.pt(11.5) * 3.0).max(34.0);
    let status_top = h - safe_bottom - status_h;
    let body = Rect::new(safe_left, bar_bottom, w - safe_left - safe_right, status_top - bar_bottom);
    view.max_scroll = 0.0;

    if body.h > 0.0 {
        if f.narrow {
            draw_column(f, body, answer, view);
        } else {
            draw_wide(f, body, answer, view);
        }
    }
    // The bars go last so a scrolled column passes under them.
    f.list.scrolls = false;
    let bar = Rect::new(safe_left, safe_top, body.w, bar_h);
    view.run = Some(draw_titlebar(f, bar, screen.tap(), answer.live));
    draw_status(f, Rect::new(safe_left, status_top, body.w, status_h), h, answer);
}

/// The desktop and tablet layout: the rail of tables, then the statement
/// over the results. The panes run edge to edge, the way a client fills its
/// window.
fn draw_wide(f: &mut Frame, body: Rect, answer: &Answer, view: &mut View) {
    let rail_right = body.x + (RAIL_W * f.t.max(1.0)).min(body.w / 3.0);
    draw_rail(f, Rect::new(body.x, body.y, rail_right - body.x, body.h));

    let pane_w = f.w - rail_right;
    let pad = 28.0;
    let lines = sql_block(f, 0.0, 0.0, pane_w - 2.0 * pad, false);
    // The statement never takes more than half: the answer is the point.
    let editor = Rect::new(rail_right, body.y, pane_w, editor_height(f, lines).min(body.h * 0.5));
    f.clip(editor);
    draw_editor(f, editor, pad, false);
    f.unclip();

    let pane = Rect::new(rail_right, editor.bottom(), pane_w, body.bottom() - editor.bottom());
    let grid = Grid::new(f, pane, answer);
    let rows = pane.h - grid.header_h;
    view.max_scroll = (answer.count.max(1) as f32 * grid.row_h - rows).max(0.0);
    draw_grid(f, pane, &grid, answer, true, pane.h + view.max_scroll);
}

/// The phone layout: one column -- tables, statement, results -- scrolled
/// as one page between the title bar and the status strip.
fn draw_column(f: &mut Frame, body: Rect, answer: &Answer, view: &mut View) {
    let margin = 14.0;
    let x = body.x + margin;
    let w = body.w - 2.0 * margin;
    f.clip(Rect::new(0.0, body.y, f.w, body.h));
    f.list.scrolls = true;

    let mut y = body.y + margin;
    y += draw_chips(f, x, y, w) + margin;

    // The statement and the results are each a rounded card, inset to the
    // chips' edge, with the ground showing between them.
    let pad = 14.0;
    let lines = sql_block(f, 0.0, 0.0, w - 2.0 * pad, false);
    let editor = Rect::new(x, y, w, editor_height(f, lines));
    draw_editor(f, editor, pad, true);
    y += editor.h + margin;

    let grid = Grid::new(f, Rect::new(x, y, w, 0.0), answer);
    let grid_h = grid.header_h + answer.count.max(1) as f32 * grid.row_h;
    draw_grid(f, Rect::new(x, y, w, grid_h), &grid, answer, false, grid_h);
    y += grid_h + margin;

    f.list.scrolls = false;
    f.unclip();
    view.max_scroll = (y - body.bottom()).max(0.0);
}

/// The window chrome: app mark, the connection this client is pointed at
/// (when there is room for it), and Run, whose rect it returns. Flat,
/// because a title bar competing with the grid is a title bar in the way.
fn draw_titlebar(f: &mut Frame, bar: Rect, tap: f32, live: bool) -> Rect {
    let t = f.t;
    f.fill(0.0, 0.0, f.w, bar.bottom(), RAIL);
    f.fill(0.0, bar.bottom() - 1.0, f.w, 1.0, LINE);
    let margin = if f.narrow { 14.0 } else { 20.0 };
    let cy = bar.y + bar.h * 0.5;

    let mark = 18.0 * t;
    let mx = bar.x + margin;
    f.round(mx, cy - mark * 0.5, mark, mark, 5.0 * t, ACCENT);
    // Two hairlines inside the mark read as stacked records at this size.
    f.fill(mx + 4.0 * t, cy - 3.0 * t, 10.0 * t, 1.6 * t, BG);
    f.fill(mx + 4.0 * t, cy + 2.0 * t, 10.0 * t, 1.6 * t, BG);
    let title = f.pt(15.0);
    let title_x = mx + mark + 10.0 * t;
    let title_end = title_x + f.text("Query", title_x, cy + title * 0.35, title, INK);
    f.say(Kind::Heading, Rect::new(mx, cy - title * 0.7, title_end - mx, title * 1.4), "Query");

    // Run: label plus a small triangle, drawn from half-point slices so its
    // edge stays smooth at a large text size without a path API. It runs the
    // statement against the database again.
    let label = f.pt(12.5);
    let label_w = f.width("Run", label);
    let run_w = (label_w + 44.0 * t).max(tap);
    let run_h = (label * 1.9).max(tap);
    let run = Rect::new(bar.right() - margin - run_w, cy - run_h * 0.5, run_w, run_h);
    f.round(run.x, run.y, run.w, run.h, 6.0 * t, SEL_WASH);
    let tri_x = run.x + (run.w - label_w - 30.0 * t) * 0.5;
    for k in 0..10 {
        let k = k as f32 * 0.5;
        f.fill(tri_x + k * t, cy - (5.0 - k) * t, 0.7 * t, (10.0 - 2.0 * k) * t, ACCENT);
    }
    f.text("Run", tri_x + 16.0 * t, cy + label * 0.35, label, ACCENT);
    f.say(Kind::Button, run, "Run query");

    // The connection chip, centred, the way a client puts the thing you must
    // never be wrong about in the middle of the frame -- but only where it
    // clears the title and Run. A phone says it in the status strip.
    let host = "app.db";
    let chip_w = f.width(host, label) + 40.0 * t;
    let chip_h = 24.0 * t;
    let chip_x = (f.w - chip_w) * 0.5;
    if chip_x > title_end + 16.0 && chip_x + chip_w < run.x - 16.0 {
        f.round(chip_x, cy - chip_h * 0.5, chip_w, chip_h, 6.0 * t, PANE);
        f.disc(chip_x + 15.0 * t, cy, 3.5 * t, if live { GREEN } else { INK_QUIET });
        f.text(host, chip_x + 26.0 * t, cy + label * 0.35, label, INK_DIM);
        let says = if live { "Connected to app.db" } else { "app.db unavailable" };
        f.say(Kind::Said, Rect::new(chip_x, cy - chip_h * 0.5, chip_w, chip_h), says);
    }
    run
}

/// "users, 1,284 rows", plus "read by the query" for the selected one.
fn say_table(f: &mut Frame, r: Rect, table: &Table, selected: bool) {
    let mut buf = [0u8; 24];
    let mut p = Phrase::new();
    p.push(table.name).push(", ").push(grouped(&mut buf, table.rows as i64)).push(" rows");
    if selected {
        p.push(", read by the query");
    }
    f.say(Kind::Said, r, p.as_str());
}

fn draw_rail(f: &mut Frame, r: Rect) {
    let t = f.t;
    f.fill(0.0, r.y, r.right(), r.h, RAIL);
    f.fill(r.right() - 1.0, r.y, 1.0, r.h, LINE);

    let left = r.x + 20.0;
    let right = r.right() - 20.0;
    let label = f.pt(10.5);
    // A short screen tightens the rail so all five tables still show.
    let (head, top, row_h) = if f.short { (24.0, 34.0, 32.0) } else { (34.0, 50.0, 36.0) };
    let head = r.y + head * t;
    f.text("TABLES", left, head, label, INK_QUIET);
    f.text_right("5", right, head, label, INK_QUIET);
    f.say(Kind::Heading, Rect::new(left, head - label, right - left, label * 1.4), "Tables");

    let top = r.y + top * t;
    let row_h = row_h * t;
    let name_size = f.pt(13.5);
    let count_size = f.pt(11.5);
    for (i, table) in TABLES.iter().enumerate() {
        let y = top + i as f32 * row_h;
        if y + row_h > r.bottom() {
            break;
        }
        let selected = i == SELECTED_TABLE;
        if selected {
            f.round(r.x + 8.0, y, r.w - 20.0, row_h - 4.0, 6.0, SEL_WASH);
            // The active-marker bar: what tells you at a glance which table the
            // statement above is actually reading.
            // At the rail's own edge: on a phone turned sideways the
            // screen's edge is under the notch, a hand away from the row.
            f.round(r.x, y + 6.0 * t, 3.0, row_h - 16.0 * t, 1.5, ACCENT);
        }

        // Table glyph: a filled cap over two rules, the shape every client uses.
        let ink = if selected { ACCENT } else { INK_QUIET };
        let gy = y + (row_h - 4.0) * 0.5 - 5.0 * t;
        f.fill(left + 2.0, gy, 11.0 * t, 3.0 * t, ink);
        f.fill(left + 2.0, gy + 5.0 * t, 11.0 * t, 1.4 * t, ink);
        f.fill(left + 2.0, gy + 8.5 * t, 11.0 * t, 1.4 * t, ink);

        let base = y + (row_h - 4.0) * 0.5 + name_size * 0.35;
        let mut buf = [0u8; 24];
        let count = grouped(&mut buf, table.rows as i64);
        let count_w = f.text_right(count, right, base, count_size, if selected { INK_DIM } else { INK_QUIET });
        let name_x = left + 2.0 + 20.0 * t;
        let ink = if selected { INK } else { INK_DIM };
        f.fit(table.name, name_x, base, name_size, ink, right - count_w - 10.0 - name_x);
        say_table(f, Rect::new(r.x + 8.0, y, r.w - 20.0, row_h - 4.0), table, selected);
    }
}

/// The phone's table list: one chip per table, wrapping onto as many lines
/// as the width and text size need. Returns the height it took.
fn draw_chips(f: &mut Frame, x: f32, y: f32, w: f32) -> f32 {
    let t = f.t;
    let label = f.pt(10.5);
    f.text("TABLES", x + 2.0, y + label, label, INK_QUIET);
    f.say(Kind::Heading, Rect::new(x, y, w, label * 1.4), "Tables");

    let name_size = f.pt(12.5);
    let count_size = f.pt(11.0);
    let chip_h = (30.0 * t).max(name_size * 2.0);
    let (pad, gap) = (12.0 * t, 8.0);
    let (mut pen, mut line_y) = (x, y + label * 1.6);
    for (i, table) in TABLES.iter().enumerate() {
        let mut buf = [0u8; 24];
        let count = grouped(&mut buf, table.rows as i64);
        let name_w = f.width(table.name, name_size);
        let count_w = f.width(count, count_size);
        let chip_w = (pad + name_w + 6.0 * t + count_w + pad).min(w);
        if pen > x && pen + chip_w > x + w {
            pen = x;
            line_y += chip_h + gap;
        }
        let selected = i == SELECTED_TABLE;
        // Every table is a pill, so the strip reads as a set of choices; the
        // one the statement reads wears the selection tint.
        f.round(pen, line_y, chip_w, chip_h, 8.0 * t, if selected { SEL_WASH } else { PANE });
        let base = line_y + chip_h * 0.5 + name_size * 0.35;
        f.text_right(count, pen + chip_w - pad, base, count_size, if selected { INK_DIM } else { INK_QUIET });
        let ink = if selected { ACCENT } else { INK_DIM };
        f.fit(table.name, pen + pad, base, name_size, ink, chip_w - 2.0 * pad - 6.0 * t - count_w);
        say_table(f, Rect::new(pen, line_y, chip_w, chip_h), table, selected);
        pen += chip_w + gap;
    }
    line_y + chip_h - y
}

/// How tall the editor is for a statement of `lines` visual lines.
fn editor_height(f: &Frame, lines: usize) -> f32 {
    f.head() + lines as f32 * f.lead() + f.pt(if f.short { 10.0 } else { 20.0 })
}

/// The statement pane: a QUERY label, the file name when it fits, and the
/// statement itself with a line-number gutter. A `card` (the phone) is a
/// rounded card; otherwise the pane runs edge to edge over a rule.
fn draw_editor(f: &mut Frame, b: Rect, pad: f32, card: bool) {
    if card {
        f.card(b, 12.0 * f.t, EDITOR);
    } else {
        f.fill(b.x, b.y, b.w, b.h, EDITOR);
        f.fill(b.x, b.bottom() - 1.0, b.w, 1.0, LINE);
    }
    if !f.short {
        let label = f.pt(10.5);
        let base = b.y + f.pt(28.0);
        let query_w = f.text("QUERY", b.x + pad, base, label, INK_QUIET);
        let file = "untitled.sql";
        let file_size = f.pt(11.5);
        if query_w + f.width(file, file_size) + 24.0 < b.w - 2.0 * pad {
            f.text_right(file, b.right() - pad, base, file_size, INK_QUIET);
        }
    }
    let lines = sql_block(f, b.x + pad, b.y + f.head(), b.w - 2.0 * pad, true);
    let block = Rect::new(b.x, b.y, b.w, editor_height(f, lines));
    f.say(Kind::Said, block, STATEMENT);
}

/// Lay out (and, with `paint`, record) the statement `room` points wide,
/// soft-wrapping between words the way an editor does. Each of the
/// statement's own lines gets one line number, on its first visual line.
/// Returns how many visual lines it takes, so the editor can be sized first.
///
/// Each word is coloured by role and the punctuation between words a step
/// dimmer, and neighbouring runs of one colour -- with the spaces between
/// them -- go to the host as one string, so a line costs one call per change
/// of colour rather than one per token.
fn sql_block(f: &mut Frame, x: f32, top: f32, room: f32, paint: bool) -> usize {
    let size = f.pt(if f.narrow { 13.5 } else { 14.5 });
    let num = f.pt(11.5);
    let line_h = f.lead();
    // Gutter rule with line numbers: the cue that this strip is an editor and
    // not a caption.
    let rule_x = x + f.width("3", num) + 12.0 * f.t;
    let code_x = rule_x + 16.0 * f.t;
    let limit = x + room;
    let baseline = |visual: usize| top + visual as f32 * line_h + line_h * 0.7;

    let mut visual = 0usize;
    for (i, line) in STATEMENT_LINES.iter().enumerate() {
        if paint {
            let mut nbuf = [0u8; 24];
            f.text_right(grouped(&mut nbuf, i as i64 + 1), rule_x - 10.0 * f.t, baseline(visual), num, INK_QUIET);
        }
        let bytes = line.as_bytes();
        let mut pen = code_x;
        // The run being gathered: its bytes in `line`, where it starts, its
        // colour. Drawn when the colour changes, the line wraps or ends.
        let mut run: Option<(usize, usize, f32, gfx::Color)> = None;
        let mut start = 0usize;
        while start < bytes.len() {
            let space = bytes.get(start) == Some(&b' ');
            let mut end = start;
            while end < bytes.len() && (bytes.get(end) == Some(&b' ')) == space {
                end += 1;
            }
            let piece = line.get(start..end).unwrap_or("");
            let piece_w = f.width(piece, size);
            if space {
                // A space at the start of a wrapped line is not drawn.
                if pen > code_x {
                    pen += piece_w;
                }
                start = end;
                continue;
            }
            if pen > code_x && pen + piece_w > limit {
                flush(f, line, run.take(), baseline(visual), size, paint);
                visual += 1;
                pen = code_x;
            }
            if piece_w > limit - pen {
                // One word wider than the whole line: cut, never overflowing.
                flush(f, line, run.take(), baseline(visual), size, paint);
                if paint {
                    f.fit(piece, pen, baseline(visual), size, IDENT, limit - pen);
                }
            } else {
                // Words, and the punctuation that rides with them.
                let mut s = start;
                let mut px = pen;
                while s < end {
                    let word = is_word_byte(bytes.get(s).copied().unwrap_or(b' '));
                    let mut e = s;
                    while e < end && is_word_byte(bytes.get(e).copied().unwrap_or(b' ')) == word {
                        e += 1;
                    }
                    let part = line.get(s..e).unwrap_or("");
                    // Punctuation is a step dimmer than an identifier, but
                    // only a step: at INK_QUIET the dot in `u.email` went and
                    // the qualifier read as two unrelated words.
                    let colour = if word { word_colour(part) } else { PUNCT };
                    run = match run {
                        Some((rs, _, rx, rc)) if rc.r == colour.r && rc.g == colour.g && rc.b == colour.b => Some((rs, e, rx, rc)),
                        other => {
                            flush(f, line, other, baseline(visual), size, paint);
                            Some((s, e, px, colour))
                        }
                    };
                    px += f.width(part, size);
                    s = e.max(s + 1);
                }
            }
            pen += piece_w;
            start = end;
        }
        flush(f, line, run, baseline(visual), size, paint);
        visual += 1;
    }
    if paint {
        f.fill(rule_x, top + 6.0 * f.t, 1.0, line_h * visual as f32, LINE_SOFT);
    }
    visual
}

/// Record one gathered run of the statement.
fn flush(f: &mut Frame, line: &str, run: Option<(usize, usize, f32, gfx::Color)>, y: f32, size: f32, paint: bool) {
    if let (true, Some((s, e, x, c))) = (paint, run) {
        f.text(line.get(s..e).unwrap_or(""), x, y, size, c);
    }
}

/// Bytes that belong to a word. Digits and `_` count so `user_id` and `8` stay
/// whole; `.` does not, so `u.email` is two identifiers and a keyword cannot
/// swallow the reference beside it.
fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'\''
}

/// The colour for one word run. Keywords match whole and case-insensitively,
/// so `count` in `count(o.id)` colours but `orders` does not match `ORDER`.
fn word_colour(word: &str) -> gfx::Color {
    if KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(word)) {
        return KEYWORD;
    }
    match word.as_bytes().first() {
        Some(&b'\'') => STRING,
        Some(&b) if b.is_ascii_digit() => NUMBER,
        _ => IDENT,
    }
}

/// The results: a header band, banded rows, figures right-aligned. With
/// `inside` the rows scroll inside the pane (the desktop), its banding
/// carried `reach` down so a scrolled pane is still banded; otherwise the
/// pane is as tall as its rows, a rounded card, and the page scrolls past it
/// (the phone).
fn draw_grid(f: &mut Frame, b: Rect, g: &Grid, answer: &Answer, inside: bool, reach: f32) {
    let t = f.t;
    // Header band, then the rule that separates it from data. The rule is the
    // single strongest signal that this is a result set. A card's bands stay
    // inside its hairline edge, and its first and last follow its corners.
    let (edge, r) = if inside { (0.0, 0.0) } else { (1.0, 12.0 * t - 1.0) };
    let (band_x, band_w) = (b.x + edge, b.w - 2.0 * edge);
    if inside {
        f.fill(b.x, b.y, b.w, b.h, PANE);
    } else {
        f.card(b, r + 1.0, PANE);
    }
    f.rounded_box(Rect::new(band_x, b.y + edge, band_w, g.header_h - edge), (r, 0.0), RAIL, false);
    f.fill(band_x, b.y + g.header_h - 1.0, band_w, 1.0, LINE);

    let head = f.pt(11.5);
    let hb = b.y + g.header_h * 0.5 + head * 0.35;
    let rules = g.rules();
    f.text("email", g.email_x, hb, head, INK_QUIET);
    f.text_right("revenue", g.revenue_right, hb, head, INK_QUIET);
    if !g.stacked {
        f.text_right("orders", g.orders_right, hb, head, INK_QUIET);
        // Column separators sit on the boundary between two columns, not at a
        // fixed offset from the label. A right-aligned header grows leftward,
        // so an offset rule eventually runs through its own text -- which is
        // exactly what it did to `orders`.
        for x in rules {
            f.fill(x, b.y + 10.0 * t, 1.0, g.header_h - 20.0 * t, LINE);
        }
    }
    f.say(Kind::Said, Rect::new(b.x, b.y, b.w, g.header_h), "Results: email, orders, revenue");

    let rows_top = b.y + g.header_h;
    let rows_bottom = b.bottom();
    let rows_end = b.y + reach;
    if inside {
        f.clip(Rect::new(b.x, rows_top, b.w, rows_bottom - rows_top));
        f.list.scrolls = true;
    }

    // Banding and column rules first, so every stroke sits under the text
    // rather than through it. On the desktop the banding runs the full height
    // of the pane: a result set that stops mid-pane leaves a slab of empty
    // colour that reads as a rendering fault, and every real client carries
    // it down instead. The phone's pane ends with its rows.
    let bands = answer.count.max(1);
    let mut k = 0usize;
    loop {
        let y = rows_top + k as f32 * g.row_h;
        if y >= rows_end - 0.5 || (!inside && k >= bands) {
            break;
        }
        let last = !inside && k + 1 == bands;
        if k % 2 == 1 {
            let band = Rect::new(band_x, y, band_w, g.row_h.min(rows_end - y) - if last { edge } else { 0.0 });
            f.rounded_box(band, (0.0, if last { r } else { 0.0 }), ROW_TINT, false);
        }
        if !last {
            f.fill(band_x, y + g.row_h - 1.0, band_w, 1.0, LINE_SOFT);
        }
        k += 1;
    }
    if !g.stacked {
        for x in rules {
            f.fill(x, rows_top, 1.0, rows_end.max(rows_bottom) - rows_top, LINE_SOFT);
        }
    }

    let cell = f.pt(13.5);
    let small = f.pt(12.5);
    for (i, record) in answer.records().iter().enumerate() {
        let y = rows_top + i as f32 * g.row_h;
        let email = as_str(&record.email, record.email_len);
        let mut ob = [0u8; 24];
        let orders = grouped(&mut ob, record.orders);
        let mut mb = [0u8; 24];
        let amount = money(&mut mb, record.revenue);
        if g.stacked {
            f.fit(email, g.email_x, y + g.row_h * 0.42, cell, INK, g.email_w);
            let second = y + g.row_h * 0.8;
            let n_w = f.text(orders, g.email_x, second, small, INK_DIM);
            f.text(" orders", g.email_x + n_w, second, small, INK_QUIET);
            f.text_right(amount, g.revenue_right, second, cell, INK);
        } else {
            // Baseline sits slightly above centre because lowercase-heavy email
            // text has more mass below the midline than the digits beside it.
            let base = y + g.row_h * 0.5 + 5.0 * t;
            f.fit(email, g.email_x, base, cell, INK, g.email_w);
            f.text_right(orders, g.orders_right, base, cell, INK_DIM);
            f.text_right(amount, g.revenue_right, base, cell, INK);
        }
        let mut p = Phrase::new();
        p.push(email).push(", ").push(orders).push(" orders, ").push(amount);
        f.say(Kind::Said, Rect::new(b.x, y, b.w, g.row_h), p.as_str());
    }

    if answer.count == 0 {
        let line = "No rows returned.";
        f.text(line, g.email_x, rows_top + g.row_h * 0.5 + cell * 0.35, cell, INK_QUIET);
        f.say(Kind::Said, Rect::new(b.x, rows_top, b.w, g.row_h), line);
    }
    if inside {
        f.list.scrolls = false;
        f.unclip();
    }
}

/// The status strip: the shape of the answer and where it came from, then
/// the engine at the right when there is room. The column count is left off
/// whole when it does not fit; where the rows came from is cut last.
fn draw_status(f: &mut Frame, r: Rect, h: f32, answer: &Answer) {
    f.fill(0.0, r.y, f.w, h - r.y, RAIL);
    f.fill(0.0, r.y, f.w, 1.0, LINE);

    let size = f.pt(11.5);
    let base = r.y + r.h * 0.5 + size * 0.35;
    let margin = if f.narrow { 14.0 } else { 20.0 };
    let (left, right) = (r.x + margin, r.right() - margin);

    let mut buf = [0u8; 24];
    let n = grouped(&mut buf, answer.count as i64);
    let unit = if answer.count == 1 { " row" } else { " rows" };
    // Honest about the degraded path: a preview host that withheld store.sql
    // must not be able to pass this off as a live connection.
    let where_from = if answer.live { "connected to app.db" } else { "sample data \u{b7} app.db unavailable" };
    let (sep, columns, engine) = ("  \u{b7}  ", "3 columns", "SQLite \u{b7} UTF-8");

    // The count is what the eye wants, so it alone is brighter; the words
    // after it are one quiet run, only there to name the units.
    let mut tail = Phrase::new();
    tail.push(unit).push(sep).push(columns).push(sep).push(where_from);
    let tail_w = f.width(tail.as_str(), size);
    let n_w = f.width(n, size);
    let engine_w = f.width(engine, size);
    let mut limit = right;
    if n_w + tail_w + 24.0 + engine_w <= right - left {
        f.text_right(engine, right, base, size, INK_QUIET);
        limit = right - engine_w - 24.0;
    } else if n_w + tail_w > right - left {
        tail = Phrase::new();
        tail.push(unit).push(sep).push(where_from);
    }
    f.text(n, left, base, size, INK_DIM);
    f.fit(tail.as_str(), left + n_w, base, size, INK_QUIET, limit - left - n_w);

    let mut p = Phrase::new();
    p.push(n).push(unit).push(", ").push(where_from);
    f.say(Kind::Said, r, p.as_str());
}

// ---- widget scaffolding -----------------------------------------------------

/// A node with no fixed size that grows, so the canvas fills whatever window
/// the host gives it rather than pinning itself to one size (K-003).
fn node(id: u64, parent: Option<u64>, kind: types::WidgetKind, role: Option<String>) -> types::WidgetNode {
    let style = types::Style { width: None, height: None, grow: 1.0, padding: 0.0, text: None, place: None, box_: None };
    types::WidgetNode { id, parent, kind, label: None, role, style, checked: None, value: None, selected: None, text_cursor: None }
}

// ---- the database -----------------------------------------------------------

/// Create the table, seed it once, and run the shown statement. Returns the
/// rows the database gave back and whether SQL was actually available: a host
/// that withheld the capability gets the seed drawn straight, so the window is
/// never blank, but the status strip has to be told the truth.
fn load(rows: &mut [Record; MAX_ROWS]) -> (usize, bool) {
    for schema in [
        "CREATE TABLE IF NOT EXISTS users (id INTEGER PRIMARY KEY, email TEXT NOT NULL UNIQUE)",
        "CREATE TABLE IF NOT EXISTS orders (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, total INTEGER NOT NULL)",
    ] {
        if sql::execute(schema, &[]).is_err() {
            return (fill_from_seed(rows), false);
        }
    }

    // Seed only when empty, so reopening does not multiply the ledger.
    let seeded = match sql::query("SELECT COUNT(*) FROM users", &[]) {
        Ok(result) => first_integer(&result) > 0,
        Err(_) => return (fill_from_seed(rows), false),
    };
    if !seeded && !seed_tables() {
        return (fill_from_seed(rows), false);
    }

    let result = match sql::query(STATEMENT, &[]) {
        Ok(result) => result,
        Err(_) => return (fill_from_seed(rows), false),
    };

    // Copy out of the SQL result once, into fixed buffers, so the render loop
    // never holds a borrow on host-allocated rows.
    let mut n = 0usize;
    for (row, slot) in result.rows.iter().zip(rows.iter_mut()) {
        let mut record = ZERO_RECORD;
        if let Some(sql::Value::Text(email)) = row.values.first() {
            record.email_len = copy_into(&mut record.email, email.as_bytes());
        }
        record.orders = integer_at(row, 1);
        record.revenue = integer_at(row, 2);
        *slot = record;
        n += 1;
    }
    if n == 0 {
        return (fill_from_seed(rows), false);
    }
    (n, true)
}

/// Insert one user and that user's orders as a single flat total. The statement
/// aggregates with `sum(o.total)`, so one order row per customer carrying the
/// whole revenue produces the same answer with far fewer round trips than
/// inventing forty-seven individual orders would.
fn seed_tables() -> bool {
    for (i, &(email, orders, revenue)) in SEED.iter().enumerate() {
        let user_id = i as i64 + 1;
        let user = [sql::Value::Integer(user_id), sql::Value::Text(pure_string(email))];
        if sql::execute("INSERT INTO users (id, email) VALUES (?, ?)", &user).is_err() {
            return false;
        }
        // One order row per unit keeps `count(o.id)` honest: the grid's orders
        // column has to come from the database counting, not from us telling it
        // a number. Revenue is split across them and the remainder rides on the
        // first, so `sum(o.total)` lands exactly on the seeded figure.
        let per = revenue / orders;
        let remainder = revenue - per * orders;
        for k in 0..orders {
            let total = if k == 0 { per + remainder } else { per };
            let order = [sql::Value::Integer(user_id), sql::Value::Integer(total)];
            if sql::execute("INSERT INTO orders (user_id, total) VALUES (?, ?)", &order).is_err() {
                return false;
            }
        }
    }
    true
}

/// Draw the seed straight when SQL is unavailable, so a preview host still
/// renders a believable grid instead of an empty pane.
fn fill_from_seed(rows: &mut [Record; MAX_ROWS]) -> usize {
    let mut n = 0usize;
    for (slot, &(email, orders, revenue)) in rows.iter_mut().zip(SEED.iter()) {
        *slot = Record { orders, revenue, ..ZERO_RECORD };
        slot.email_len = copy_into(&mut slot.email, email.as_bytes());
        n += 1;
    }
    n
}

/// The first column of the first row as an integer, for COUNT(*).
fn first_integer(result: &sql::QueryResult) -> i64 {
    match result.rows.first().and_then(|row| row.values.first()) {
        Some(sql::Value::Integer(n)) => *n,
        _ => 0,
    }
}

/// One column of a row as an integer. `sum()` can come back as a real
/// depending on how the host's engine widened the accumulator, so both shapes
/// are accepted rather than silently reading as zero.
fn integer_at(row: &sql::Row, index: usize) -> i64 {
    match row.values.get(index) {
        Some(sql::Value::Integer(n)) => *n,
        Some(sql::Value::Real(f)) => *f as i64,
        _ => 0,
    }
}
// ---- app --------------------------------------------------------------------

struct Component;

impl krate::Guest for Component {
    fn run() -> i32 {
        let mut answer = Answer { rows: [ZERO_RECORD; MAX_ROWS], count: 0, live: false };
        (answer.count, answer.live) = load(&mut answer.rows);

        let size = types::WindowSize { width: WIDTH as u32, height: HEIGHT as u32 };
        let Ok(win) = window::create("Query", size) else {
            return 30;
        };
        if window::show(win).is_err() {
            return 31;
        }
        if tree::set_root(win, &node(ROOT_ID, None, types::WidgetKind::Stack, None)).is_err()
            || tree::upsert_node(win, &node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas, Some(pure_string("canvas")))).is_err()
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

        let raw = args::raw();
        let quick = raw.as_bytes().split(|byte| *byte == b'\n').next().is_some_and(|first| first == b"quick");

        let mut view = View { scroll: 0.0, max_scroll: 0.0, run: None, runs: 0 };
        // On the stack, made once: the layout recorded for scroll steps to
        // replay (a few tens of kilobytes, nothing allocated per frame).
        let mut list = List { ops: [NO_OP; MAX_OPS], n: 0, arena: [0u8; ARENA], used: 0, key: [-1.0; 8], scrolls: false };
        let _ = draw(canvas, win, &answer, &mut view, &mut list);

        if quick {
            let out = stdio::stdout();
            let _ = out.write(b"query:ok\n");
            let _ = out.flush();
            let _ = window::close(win);
            return 0;
        }

        // A real session ends when the person closes the window, never on a
        // round count.
        'session: loop {
            // Take everything already queued, then draw once: a finger
            // dragging the page sends far more events than there are frames.
            let mut dirty = false;
            let mut next = events::wait(Some(WAIT_ROUND_MILLIS));
            while let Some(event) = next {
                match event {
                    types::Event::CloseRequested(id) if id == win => break 'session,
                    // A new size is a new layout; a new theme, a new text size.
                    types::Event::Resized(_)
                    | types::Event::ThemeChanged(_)
                    | types::Event::RedrawRequested(_) => dirty = true,
                    // On the release, so a finger that drags to scroll does
                    // not run the statement it started on.
                    types::Event::Pointer(p) if p.button.is_some() && !p.pressed && view.tap(p.x, p.y) => {
                        (answer.count, answer.live) = load(&mut answer.rows);
                        view.runs = view.runs.wrapping_add(1);
                        dirty = true;
                    }
                    types::Event::Wheel(w) => dirty |= view.wheel(w.dy),
                    _ => {}
                }
                next = events::poll();
            }
            if dirty {
                let _ = draw(canvas, win, &answer, &mut view, &mut list);
            }
        }

        let _ = window::close(win);
        let out = stdio::stdout();
        let _ = out.write(b"query:ok\n");
        let _ = out.flush();
        0
    }
}

krate::export!(Component);
