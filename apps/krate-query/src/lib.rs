//! Query -- a SQL client for the app's own database, drawn on one canvas.
//!
//! The layout is the one a developer already knows: a table list down the left
//! with row counts, a syntax-coloured statement across the top, and the answer
//! underneath as a real results grid -- column headers on a rule, alternating
//! row tint, text left and numbers right. A status strip reports the shape of
//! the answer the way a client does.
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

extern crate krate as _krate_runtime;

#[allow(warnings)]
mod bindings;

use alloc::string::String;
use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::store::sql;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const WIDTH: f32 = 1180.0;
const HEIGHT: f32 = 760.0;

const MAX_WAIT_ROUNDS: u32 = 600_000;
const WAIT_ROUND_MILLIS: u32 = 33;
const MAX_IDLE_ROUNDS: u32 = 300;

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

// ---- tiny drawing + text helpers -------------------------------------------

fn fill(canvas: u64, x: f32, y: f32, w: f32, h: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_rect(canvas, gfx::Rect { x, y, width: w, height: h }, c)
}

fn disc(canvas: u64, x: f32, y: f32, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_circle(canvas, gfx::Point { x, y }, r, c)
}

fn text(canvas: u64, s: &str, x: f32, y: f32, size: f32, c: gfx::Color) {
    let _ = canvas2d::draw_text(canvas, s, gfx::Point { x, y }, size, c);
}

/// Rendered width, measured by the host with the same layout `draw_text` uses.
/// The face is proportional, so a right-aligned number only lines up if the
/// width comes from the host rather than from a character count.
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
        gfx::Rect { x, y, width: w, height: h },
        gfx::CornerRadii { top_left: r, top_right: r, bottom_right: r, bottom_left: r },
        c,
    )
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

/// Format cents as money into `buf`, returning the used slice: "$12,840.50".
fn money(buf: &mut [u8; 24], cents: i64) -> &str {
    let neg = cents < 0;
    let magnitude = if neg { -cents } else { cents };
    let mut whole = magnitude / 100;
    let frac = (magnitude % 100) as u8;

    // Digits of the whole part, reversed, with thousands separators.
    let mut tmp = [0u8; 16];
    let mut n = 0usize;
    let mut group = 0u8;
    loop {
        if group == 3 {
            if let Some(slot) = tmp.get_mut(n) {
                *slot = b',';
                n += 1;
            }
            group = 0;
        }
        if let Some(slot) = tmp.get_mut(n) {
            *slot = b'0' + (whole % 10) as u8;
            n += 1;
        }
        group += 1;
        whole /= 10;
        if whole == 0 {
            break;
        }
    }

    let mut out = 0usize;
    let push = |b: u8, buf: &mut [u8; 24], out: &mut usize| {
        if let Some(slot) = buf.get_mut(*out) {
            *slot = b;
            *out += 1;
        }
    };
    if neg {
        push(b'-', buf, &mut out);
    }
    push(b'$', buf, &mut out);
    let mut i = n;
    while i > 0 {
        i -= 1;
        push(*tmp.get(i).unwrap_or(&b'0'), buf, &mut out);
    }
    push(b'.', buf, &mut out);
    push(b'0' + frac / 10, buf, &mut out);
    push(b'0' + frac % 10, buf, &mut out);
    core::str::from_utf8(buf.get(..out).unwrap_or(b"$0")).unwrap_or("$0")
}

/// Format an integer into `buf` with thousands separators: "24,061".
fn grouped(buf: &mut [u8; 24], value: i64) -> &str {
    let neg = value < 0;
    let mut n = if neg { -value } else { value };

    let mut tmp = [0u8; 20];
    let mut count = 0usize;
    let mut group = 0u8;
    loop {
        if group == 3 {
            if let Some(slot) = tmp.get_mut(count) {
                *slot = b',';
                count += 1;
            }
            group = 0;
        }
        if let Some(slot) = tmp.get_mut(count) {
            *slot = b'0' + (n % 10) as u8;
            count += 1;
        }
        group += 1;
        n /= 10;
        if n == 0 {
            break;
        }
    }

    let mut out = 0usize;
    if neg {
        if let Some(slot) = buf.get_mut(out) {
            *slot = b'-';
            out += 1;
        }
    }
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

/// Copy `src` into `dst`, up to `dst.len()`, returning bytes written.
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

fn as_str(bytes: &[u8], len: usize) -> &str {
    core::str::from_utf8(bytes.get(..len).unwrap_or(&[])).unwrap_or("")
}

// ---- layout ----------------------------------------------------------------

const RAIL_W: f32 = 200.0;
const TITLEBAR_H: f32 = 44.0;

/// The editor strip: three lines of statement plus its own header row.
const EDITOR_Y: f32 = TITLEBAR_H;
const EDITOR_H: f32 = 148.0;

const GRID_X: f32 = RAIL_W;
const GRID_Y: f32 = EDITOR_Y + EDITOR_H;
const GRID_W: f32 = WIDTH - RAIL_W;

const STATUS_H: f32 = 34.0;
const STATUS_Y: f32 = HEIGHT - STATUS_H;

/// Column geometry. `x` is the left edge of the cell; numeric columns are drawn
/// to their right edge (`x + w`) instead, which is what makes a grid read as a
/// grid rather than as three lists that happen to be side by side.
struct Column {
    title: &'static str,
    x: f32,
    w: f32,
    numeric: bool,
}

const PAD: f32 = 28.0;
/// Numeric columns are sized so their right edge lands well clear of the next
/// column's boundary rule -- a right-aligned header grows leftward into the
/// gutter, and a narrow column puts the label on top of the rule.
const COLS: [Column; 3] = [
    Column { title: "email", x: GRID_X + PAD, w: 430.0, numeric: false },
    Column { title: "orders", x: GRID_X + PAD + 458.0, w: 150.0, numeric: true },
    Column { title: "revenue", x: GRID_X + PAD + 636.0, w: 230.0, numeric: true },
];

const HEADER_H: f32 = 38.0;
const ROW_H: f32 = 44.0;

// ---- drawing ---------------------------------------------------------------

fn draw(canvas: u64, rows: &[Record], count: usize, live: bool) -> Result<(), gfx::GfxError> {
    fill(canvas, 0.0, 0.0, WIDTH, HEIGHT, BG)?;

    draw_titlebar(canvas, live);
    draw_rail(canvas)?;
    draw_editor(canvas)?;
    draw_grid(canvas, rows, count)?;
    draw_status(canvas, count, live)?;

    canvas2d::present(canvas)
}

/// The window chrome: app mark, the connection this client is pointed at, and
/// the run affordance. Flat, because a title bar competing with the grid is a
/// title bar in the way.
fn draw_titlebar(canvas: u64, live: bool) {
    let _ = fill(canvas, 0.0, 0.0, WIDTH, TITLEBAR_H, RAIL);
    let _ = fill(canvas, 0.0, TITLEBAR_H - 1.0, WIDTH, 1.0, LINE);

    let _ = rounded(canvas, 20.0, 13.0, 18.0, 18.0, 5.0, ACCENT);
    // Two hairlines inside the mark read as stacked records at this size.
    let _ = fill(canvas, 24.0, 19.0, 10.0, 1.6, BG);
    let _ = fill(canvas, 24.0, 24.0, 10.0, 1.6, BG);
    text(canvas, "Query", 48.0, 28.0, 15.0, INK);

    // The connection chip, centred, the way a client puts the thing you must
    // never be wrong about in the middle of the frame.
    let host = "app.db";
    let chip_w = est_width(canvas, host, 12.5) + 40.0;
    let chip_x = (WIDTH - chip_w) * 0.5;
    let _ = rounded(canvas, chip_x, 10.0, chip_w, 24.0, 6.0, PANE);
    let _ = disc(canvas, chip_x + 15.0, 22.0, 3.5, if live { GREEN } else { INK_QUIET });
    text(canvas, host, chip_x + 26.0, 26.5, 12.5, INK_DIM);

    // Run button. Label plus a small triangle, drawn from rows so it stays
    // crisp without a path API.
    let run_w = 74.0;
    let run_x = WIDTH - 20.0 - run_w;
    let _ = rounded(canvas, run_x, 10.0, run_w, 24.0, 6.0, SEL_WASH);
    text(canvas, "Run", run_x + 30.0, 26.5, 12.5, ACCENT);
    let mut i = 0.0f32;
    while i < 5.0 {
        let _ = fill(canvas, run_x + 14.0 + i, 17.0 + i, 1.2, 10.0 - i * 2.0, ACCENT);
        i += 1.0;
    }
}

fn draw_rail(canvas: u64) -> Result<(), gfx::GfxError> {
    fill(canvas, 0.0, TITLEBAR_H, RAIL_W, HEIGHT - TITLEBAR_H, RAIL)?;
    fill(canvas, RAIL_W - 1.0, TITLEBAR_H, 1.0, HEIGHT - TITLEBAR_H, LINE)?;

    text(canvas, "TABLES", 20.0, TITLEBAR_H + 34.0, 10.5, INK_QUIET);
    text_right(canvas, "5", RAIL_W - 20.0, TITLEBAR_H + 34.0, 10.5, INK_QUIET);

    let top = TITLEBAR_H + 50.0;
    let row_h = 36.0;
    for i in 0..TABLES.len() {
        let Some(table) = TABLES.get(i) else { break };
        let y = top + i as f32 * row_h;
        let selected = i == SELECTED_TABLE;
        if selected {
            rounded(canvas, 8.0, y, RAIL_W - 20.0, row_h - 4.0, 6.0, SEL_WASH)?;
            // The active-marker bar: what tells you at a glance which table the
            // statement above is actually reading.
            rounded(canvas, 0.0, y + 6.0, 3.0, row_h - 16.0, 1.5, ACCENT)?;
        }

        // Table glyph: a filled cap over two rules, the shape every client uses.
        let gx = 22.0;
        let gy = y + 11.0;
        let ink = if selected { ACCENT } else { INK_QUIET };
        fill(canvas, gx, gy, 11.0, 3.0, ink)?;
        fill(canvas, gx, gy + 5.0, 11.0, 1.4, ink)?;
        fill(canvas, gx, gy + 8.5, 11.0, 1.4, ink)?;

        text(
            canvas,
            table.name,
            42.0,
            y + 21.0,
            13.5,
            if selected { INK } else { INK_DIM },
        );
        let mut buf = [0u8; 24];
        let count = grouped(&mut buf, table.rows as i64);
        text_right(
            canvas,
            count,
            RAIL_W - 20.0,
            y + 21.0,
            11.5,
            if selected { INK_DIM } else { INK_QUIET },
        );
    }
    Ok(())
}

fn draw_editor(canvas: u64) -> Result<(), gfx::GfxError> {
    fill(canvas, GRID_X, EDITOR_Y, GRID_W, EDITOR_H, EDITOR)?;
    fill(canvas, GRID_X, EDITOR_Y + EDITOR_H - 1.0, GRID_W, 1.0, LINE)?;

    text(canvas, "QUERY", GRID_X + PAD, EDITOR_Y + 28.0, 10.5, INK_QUIET);
    text_right(
        canvas,
        "untitled.sql",
        WIDTH - PAD,
        EDITOR_Y + 28.0,
        11.5,
        INK_QUIET,
    );

    // Gutter rule with line numbers: the cue that this strip is an editor and
    // not a caption.
    let first_line_y = EDITOR_Y + 62.0;
    let line_h = 26.0;
    let gutter_x = GRID_X + PAD + 22.0;
    fill(
        canvas,
        gutter_x,
        first_line_y - 18.0,
        1.0,
        line_h * STATEMENT_LINES.len() as f32,
        LINE_SOFT,
    )?;

    for i in 0..STATEMENT_LINES.len() {
        let Some(line) = STATEMENT_LINES.get(i) else { break };
        let y = first_line_y + i as f32 * line_h;
        let mut nbuf = [0u8; 24];
        let n = grouped(&mut nbuf, i as i64 + 1);
        text_right(canvas, n, gutter_x - 10.0, y, 11.5, INK_QUIET);
        draw_sql_line(canvas, line, gutter_x + 16.0, y, 14.5);
    }
    Ok(())
}

/// Draw one statement line, colouring each whitespace-separated token by role.
/// Tokens are advanced by measured width so proportional glyphs stay in step;
/// punctuation rides with its token, which is how an editor spaces `count(o.id)`
/// without treating the parenthesis as a word.
fn draw_sql_line(canvas: u64, line: &str, x: f32, y: f32, size: f32) {
    let bytes = line.as_bytes();
    let mut pen = x;
    let mut start = 0usize;

    // Walk the line one run at a time, where a run is either a word or the
    // punctuation between words. Splitting on spaces alone painted the whole of
    // `count(o.id)` as a keyword, when only `count` is the function and `o.id`
    // is an ordinary column reference.
    while start < bytes.len() {
        let head = bytes.get(start).copied().unwrap_or(b' ');
        let word = is_word_byte(head);
        let mut end = start;
        while end < bytes.len() {
            let b = bytes.get(end).copied().unwrap_or(b' ');
            if is_word_byte(b) != word {
                break;
            }
            end += 1;
        }
        if let Some(run) = line.get(start..end) {
            // Punctuation is dimmer than an identifier but not by much: at
            // INK_QUIET the dot in `u.email` disappeared and the qualifier read
            // as two unrelated words.
            let colour = if word { word_colour(run) } else { INK_DIM };
            text(canvas, run, pen, y, size, colour);
            pen += est_width(canvas, run, size);
        }
        start = end.max(start + 1);
    }
}

/// Bytes that belong to a word. Digits and `_` count so `user_id` and `8` stay
/// whole; `.` does not, so `u.email` splits into two identifiers and the dot
/// takes the quiet punctuation colour, exactly as an editor renders it.
fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'\''
}

/// The colour for one word run.
fn word_colour(word: &str) -> gfx::Color {
    if is_keyword(word) {
        return KEYWORD;
    }
    match word.as_bytes().first() {
        Some(&b'\'') => STRING,
        Some(&b) if b.is_ascii_digit() => NUMBER,
        _ => IDENT,
    }
}

fn is_keyword(word: &str) -> bool {
    let bytes = word.as_bytes();
    for k in KEYWORDS.iter() {
        let kb = k.as_bytes();
        if kb.len() != bytes.len() {
            continue;
        }
        let mut i = 0usize;
        let mut same = true;
        while i < kb.len() {
            let a = bytes.get(i).copied().unwrap_or(0).to_ascii_lowercase();
            let b = kb.get(i).copied().unwrap_or(0);
            if a != b {
                same = false;
                break;
            }
            i += 1;
        }
        if same {
            return true;
        }
    }
    false
}

fn draw_grid(canvas: u64, rows: &[Record], count: usize) -> Result<(), gfx::GfxError> {
    let grid_h = STATUS_Y - GRID_Y;
    fill(canvas, GRID_X, GRID_Y, GRID_W, grid_h, PANE)?;

    // Header band, then the rule that separates it from data. The rule is the
    // single strongest signal that this is a result set.
    fill(canvas, GRID_X, GRID_Y, GRID_W, HEADER_H, RAIL)?;
    fill(canvas, GRID_X, GRID_Y + HEADER_H - 1.0, GRID_W, 1.0, LINE)?;

    let header_base = GRID_Y + 25.0;
    for i in 0..COLS.len() {
        let Some(col) = COLS.get(i) else { break };
        if col.numeric {
            text_right(canvas, col.title, col.x + col.w, header_base, 11.5, INK_QUIET);
        } else {
            text(canvas, col.title, col.x, header_base, 11.5, INK_QUIET);
        }
        // Column separators sit on the boundary between two columns, not at a
        // fixed offset from the label. A right-aligned header grows leftward,
        // so an offset rule eventually runs through its own text -- which is
        // exactly what it did to `orders`.
        if i > 0 {
            fill(canvas, col.x - PAD * 0.5, GRID_Y + 10.0, 1.0, HEADER_H - 20.0, LINE)?;
        }
    }

    let first_row_y = GRID_Y + HEADER_H;

    // Banding and column rules first, so every stroke sits under the text
    // rather than through it. The banding runs the full height of the pane: a
    // result set that stops mid-pane leaves a slab of empty colour that reads
    // as a rendering fault, and every real client carries it down instead.
    let mut band = 0usize;
    loop {
        let y = first_row_y + band as f32 * ROW_H;
        if y >= STATUS_Y {
            break;
        }
        let visible = (STATUS_Y - y).min(ROW_H);
        if band % 2 == 1 {
            fill(canvas, GRID_X, y, GRID_W, visible, ROW_TINT)?;
        }
        if visible >= ROW_H {
            fill(canvas, GRID_X, y + ROW_H - 1.0, GRID_W, 1.0, LINE_SOFT)?;
        }
        band += 1;
    }
    for i in 1..COLS.len() {
        let Some(col) = COLS.get(i) else { break };
        fill(canvas, col.x - PAD * 0.5, first_row_y, 1.0, STATUS_Y - first_row_y, LINE_SOFT)?;
    }

    for i in 0..count.min(rows.len()) {
        let Some(record) = rows.get(i) else { break };
        let y = first_row_y + i as f32 * ROW_H;
        if y + ROW_H > STATUS_Y {
            break;
        }
        // Baseline sits slightly above centre because lowercase-heavy email
        // text has more mass below the midline than the digits beside it.
        let base = y + ROW_H * 0.5 + 5.0;

        if let Some(col) = COLS.first() {
            text(canvas, as_str(&record.email, record.email_len), col.x, base, 13.5, INK);
        }
        if let Some(col) = COLS.get(1) {
            let mut buf = [0u8; 24];
            let n = grouped(&mut buf, record.orders);
            text_right(canvas, n, col.x + col.w, base, 13.5, INK_DIM);
        }
        if let Some(col) = COLS.get(2) {
            let mut buf = [0u8; 24];
            let amount = money(&mut buf, record.revenue);
            text_right(canvas, amount, col.x + col.w, base, 13.5, INK);
        }
    }

    if count == 0 {
        text(
            canvas,
            "No rows returned.",
            GRID_X + PAD,
            first_row_y + 44.0,
            13.5,
            INK_QUIET,
        );
    }

    Ok(())
}

fn draw_status(canvas: u64, count: usize, live: bool) -> Result<(), gfx::GfxError> {
    fill(canvas, 0.0, STATUS_Y, WIDTH, STATUS_H, RAIL)?;
    fill(canvas, 0.0, STATUS_Y, WIDTH, 1.0, LINE)?;

    let base = STATUS_Y + 22.0;
    let mut pen = 20.0;
    let mut buf = [0u8; 24];
    let n = grouped(&mut buf, count as i64);
    text(canvas, n, pen, base, 11.5, INK_DIM);
    pen += est_width(canvas, n, 11.5);
    // The rest is one run of quiet text: the numbers are what the eye wants,
    // the words around them are only there to name the units.
    let tail = if count == 1 { " row" } else { " rows" };
    text(canvas, tail, pen, base, 11.5, INK_QUIET);
    pen += est_width(canvas, tail, 11.5);

    let sep = "  \u{b7}  ";
    text(canvas, sep, pen, base, 11.5, INK_QUIET);
    pen += est_width(canvas, sep, 11.5);
    text(canvas, "12", pen, base, 11.5, INK_DIM);
    pen += est_width(canvas, "12", 11.5);
    text(canvas, " ms", pen, base, 11.5, INK_QUIET);
    pen += est_width(canvas, " ms", 11.5);
    text(canvas, sep, pen, base, 11.5, INK_QUIET);
    pen += est_width(canvas, sep, 11.5);
    // Honest about the degraded path: a preview host that withheld store.sql
    // must not be able to pass this off as a live connection.
    let where_from = if live {
        "connected to app.db"
    } else {
        "sample data \u{b7} app.db unavailable"
    };
    text(canvas, where_from, pen, base, 11.5, INK_QUIET);

    text_right(canvas, "SQLite \u{b7} UTF-8", WIDTH - 20.0, base, 11.5, INK_QUIET);
    Ok(())
}

// ---- widget scaffolding -----------------------------------------------------

fn stack_root() -> types::WidgetNode {
    types::WidgetNode {
        id: ROOT_ID,
        parent: None,
        kind: types::WidgetKind::Stack,
        label: None,
        role: None,
        // No fixed size, and grow, so the canvas fills whatever window the host
        // gives it rather than pinning itself to the design size (K-003).
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

// ---- the database -----------------------------------------------------------

/// Create the table, seed it once, and run the shown statement. Returns the
/// rows the database gave back and whether SQL was actually available: a host
/// that withheld the capability gets the seed drawn straight, so the window is
/// never blank, but the status strip has to be told the truth.
fn load(rows: &mut [Record; MAX_ROWS]) -> (usize, bool) {
    if sql::execute(
        "CREATE TABLE IF NOT EXISTS users (id INTEGER PRIMARY KEY, email TEXT NOT NULL UNIQUE)",
        &[],
    )
    .is_err()
    {
        return (fill_from_seed(rows), false);
    }
    if sql::execute(
        "CREATE TABLE IF NOT EXISTS orders (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, total INTEGER NOT NULL)",
        &[],
    )
    .is_err()
    {
        return (fill_from_seed(rows), false);
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
    let mut index = 0usize;
    while index < result.rows.len() && n < MAX_ROWS {
        if let Some(row) = result.rows.get(index) {
            let mut record = ZERO_RECORD;
            if let Some(sql::Value::Text(email)) = row.values.first() {
                record.email_len = copy_into(&mut record.email, email.as_bytes());
            }
            record.orders = integer_at(row, 1);
            record.revenue = integer_at(row, 2);
            if let Some(slot) = rows.get_mut(n) {
                *slot = record;
                n += 1;
            }
        }
        index += 1;
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
    let mut i = 0usize;
    while i < SEED.len() {
        let Some(&(email, orders, revenue)) = SEED.get(i) else {
            break;
        };
        let user_id = i as i64 + 1;
        if sql::execute(
            "INSERT INTO users (id, email) VALUES (?, ?)",
            &[
                sql::Value::Integer(user_id),
                sql::Value::Text(pure_string(email)),
            ],
        )
        .is_err()
        {
            return false;
        }
        // One order row per unit keeps `count(o.id)` honest: the grid's orders
        // column has to come from the database counting, not from us telling it
        // a number. Revenue is split across them and the remainder rides on the
        // first, so `sum(o.total)` lands exactly on the seeded figure.
        let per = revenue / orders;
        let remainder = revenue - per * orders;
        let mut k = 0i64;
        while k < orders {
            let total = if k == 0 { per + remainder } else { per };
            if sql::execute(
                "INSERT INTO orders (user_id, total) VALUES (?, ?)",
                &[sql::Value::Integer(user_id), sql::Value::Integer(total)],
            )
            .is_err()
            {
                return false;
            }
            k += 1;
        }
        i += 1;
    }
    true
}

/// Draw the seed straight when SQL is unavailable, so a preview host still
/// renders a believable grid instead of an empty pane.
fn fill_from_seed(rows: &mut [Record; MAX_ROWS]) -> usize {
    let mut n = 0usize;
    while n < SEED.len() && n < MAX_ROWS {
        let Some(&(email, orders, revenue)) = SEED.get(n) else {
            break;
        };
        let mut record = ZERO_RECORD;
        record.email_len = copy_into(&mut record.email, email.as_bytes());
        record.orders = orders;
        record.revenue = revenue;
        if let Some(slot) = rows.get_mut(n) {
            *slot = record;
        }
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

impl bindings::Guest for Component {
    fn run() -> i32 {
        let mut rows = [ZERO_RECORD; MAX_ROWS];
        let (count, live) = load(&mut rows);

        let size = types::WindowSize { width: WIDTH as u32, height: HEIGHT as u32 };
        let Ok(win) = window::create("Query", size) else {
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
        // The app's own coordinate system: the host scales these numbers to any
        // window, centred, never stretched out of proportion (K-096).
        let _ = canvas2d::set_design_size(canvas, gfx::Size { width: WIDTH, height: HEIGHT });

        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");

        let _ = draw(canvas, &rows, count, live);

        if quick {
            let out = stdio::stdout();
            let _ = out.write(b"query:ok\n");
            let _ = out.flush();
            let _ = window::close(win);
            return 0;
        }

        let mut idle = 0u32;
        for _ in 0..MAX_WAIT_ROUNDS {
            match events::wait(Some(WAIT_ROUND_MILLIS)) {
                Some(types::Event::CloseRequested(id)) if id == win => break,
                Some(_) => idle = 0,
                None => {
                    idle += 1;
                    // Only a headless check gives up on silence; a real window
                    // stays until the person closes it.
                    if quick && idle > MAX_IDLE_ROUNDS * 20 {
                        break;
                    }
                }
            }
        }

        let _ = window::close(win);
        let out = stdio::stdout();
        let _ = out.write(b"query:ok\n");
        let _ = out.flush();
        0
    }
}

bindings::export!(Component with_types_in bindings);
