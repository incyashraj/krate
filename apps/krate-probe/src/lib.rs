//! Probe -- an HTTP client, drawn entirely on a canvas.
//!
//! The layout: a request collection in a left rail, a method/URL/Send bar
//! across the top, and a split main area with the request headers above and
//! the response below. The response is the hero -- a status chip, timing and
//! size, and a syntax-highlighted JSON body.
//!
//! It makes no network calls and asks for no network capability. Every
//! request in the rail carries a saved response, and clicking one shows it.
//! That is the honest framing: this renders stored responses, the way a
//! client's history pane does, and it never pretends the wire was touched.
//!
//! `#![no_std]` keeps it `krate:*`-only: the SDK owns the allocator and a
//! trapping panic handler. All state is fixed-size; numbers are formatted by
//! hand; no `format!`, `unwrap`, or panicking index anywhere.

#![no_std]
#![allow(clippy::needless_range_loop)]

extern crate alloc;

extern crate krate as _krate_runtime;

#[allow(warnings)]
mod bindings;

use alloc::string::String;
use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const WIDTH: f32 = 1180.0;
const HEIGHT: f32 = 760.0;

const MAX_WAIT_ROUNDS: u32 = 600_000;
const WAIT_ROUND_MILLIS: u32 = 33;
const MAX_IDLE_ROUNDS: u32 = 300;

// ---- palette ----------------------------------------------------------------

const BG: gfx::Color = rgb(0.055, 0.063, 0.082);
const RAIL: gfx::Color = rgb(0.075, 0.086, 0.110);
const PANE: gfx::Color = rgb(0.086, 0.098, 0.125);
const EDGE: gfx::Color = rgb(0.133, 0.149, 0.184);
const FIELD: gfx::Color = rgb(0.110, 0.125, 0.157);

const INK: gfx::Color = rgb(0.925, 0.941, 0.965);
const INK_DIM: gfx::Color = rgb(0.576, 0.616, 0.678);
const INK_QUIET: gfx::Color = rgb(0.376, 0.412, 0.475);

const ACCENT: gfx::Color = rgb(0.510, 0.400, 0.945);
const SEL_WASH: gfx::Color = rgb(0.128, 0.126, 0.208);

const GREEN: gfx::Color = rgb(0.302, 0.827, 0.514);
const AMBER: gfx::Color = rgb(0.976, 0.706, 0.290);
const RED: gfx::Color = rgb(0.945, 0.400, 0.400);

// Method tag grounds: pre-flattened, because a translucent rounded rect doubles
// its alpha where the corner discs meet the body rect.
const TAG_GET: gfx::Color = rgb(0.075, 0.169, 0.129);
const TAG_POST: gfx::Color = rgb(0.196, 0.149, 0.063);
const TAG_DELETE: gfx::Color = rgb(0.196, 0.098, 0.106);

// JSON token colours. Keys read as the structure, so they take the accent that
// carries the app; strings and numbers separate from each other and from the
// punctuation, which drops back far enough to stop competing with either.
const J_KEY: gfx::Color = rgb(0.639, 0.545, 0.984);
const J_STR: gfx::Color = rgb(0.612, 0.859, 0.482);
const J_NUM: gfx::Color = rgb(0.976, 0.729, 0.412);
const J_LIT: gfx::Color = rgb(0.376, 0.706, 0.980);
const J_PUNCT: gfx::Color = rgb(0.404, 0.443, 0.510);

const fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
}

// ---- the saved collection ---------------------------------------------------

/// HTTP method, which fixes both the tag colour and the tag ground.
#[derive(Clone, Copy, PartialEq)]
enum Method {
    Get,
    Post,
    Delete,
}

impl Method {
    fn label(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Delete => "DEL",
        }
    }
    fn ink(self) -> gfx::Color {
        match self {
            Method::Get => GREEN,
            Method::Post => AMBER,
            Method::Delete => RED,
        }
    }
    fn ground(self) -> gfx::Color {
        match self {
            Method::Get => TAG_GET,
            Method::Post => TAG_POST,
            Method::Delete => TAG_DELETE,
        }
    }
}

/// One saved request: what the rail shows and what the panes below replay.
struct Request {
    method: Method,
    path: &'static str,
    url: &'static str,
    status: u16,
    status_text: &'static str,
    millis: u32,
    /// Body size in bytes; rendered as KB with one decimal.
    bytes: u32,
    body: &'static [Line],
}

/// A syntax-coloured JSON token. Splitting the body into tokens rather than
/// re-lexing a flat string keeps the highlighter trivially correct: there is
/// no string-vs-key ambiguity to get wrong at draw time.
struct Tok(&'static str, TokKind);

#[derive(Clone, Copy, PartialEq)]
enum TokKind {
    Key,
    Str,
    Num,
    Lit,
    Punct,
}

use TokKind::{Key, Lit, Num, Punct, Str};

/// One rendered line: an indent depth in levels, then its tokens.
struct Line(u8, &'static [Tok]);

const BODY_CUSTOMERS: &[Line] = &[
    Line(0, &[Tok("{", Punct)]),
    Line(1, &[Tok("\"object\"", Key), Tok(": ", Punct), Tok("\"list\"", Str), Tok(",", Punct)]),
    Line(1, &[Tok("\"url\"", Key), Tok(": ", Punct), Tok("\"/v1/customers\"", Str), Tok(",", Punct)]),
    Line(1, &[Tok("\"has_more\"", Key), Tok(": ", Punct), Tok("true", Lit), Tok(",", Punct)]),
    Line(1, &[Tok("\"data\"", Key), Tok(": [", Punct)]),
    Line(2, &[Tok("{", Punct)]),
    Line(3, &[Tok("\"id\"", Key), Tok(": ", Punct), Tok("\"cus_QbT7mZa1kP\"", Str), Tok(",", Punct)]),
    Line(3, &[Tok("\"email\"", Key), Tok(": ", Punct), Tok("\"ada@lovelace.dev\"", Str), Tok(",", Punct)]),
    Line(3, &[Tok("\"name\"", Key), Tok(": ", Punct), Tok("\"Ada Lovelace\"", Str), Tok(",", Punct)]),
    Line(3, &[Tok("\"balance\"", Key), Tok(": ", Punct), Tok("0", Num), Tok(",", Punct)]),
    Line(3, &[Tok("\"address\"", Key), Tok(": {", Punct)]),
    Line(4, &[Tok("\"city\"", Key), Tok(": ", Punct), Tok("\"London\"", Str), Tok(",", Punct)]),
    Line(4, &[Tok("\"postal_code\"", Key), Tok(": ", Punct), Tok("\"NW1 4RY\"", Str)]),
    Line(3, &[Tok("},", Punct)]),
    Line(3, &[Tok("\"metadata\"", Key), Tok(": {", Punct)]),
    Line(4, &[Tok("\"plan\"", Key), Tok(": ", Punct), Tok("\"pro_annual\"", Str)]),
    Line(3, &[Tok("}", Punct)]),
    Line(2, &[Tok("},", Punct)]),
    Line(2, &[Tok("{", Punct)]),
    Line(3, &[Tok("\"id\"", Key), Tok(": ", Punct), Tok("\"cus_Qb9xLr4vNc\"", Str), Tok(",", Punct)]),
    Line(3, &[Tok("\"email\"", Key), Tok(": ", Punct), Tok("\"grace@hopper.dev\"", Str)]),
    Line(2, &[Tok("}", Punct)]),
    Line(1, &[Tok("]", Punct)]),
    Line(0, &[Tok("}", Punct)]),
];

const BODY_CHARGES: &[Line] = &[
    Line(0, &[Tok("{", Punct)]),
    Line(1, &[Tok("\"id\"", Key), Tok(": ", Punct), Tok("\"ch_3PmT1kR8vQ2Lb0\"", Str), Tok(",", Punct)]),
    Line(1, &[Tok("\"object\"", Key), Tok(": ", Punct), Tok("\"charge\"", Str), Tok(",", Punct)]),
    Line(1, &[Tok("\"amount\"", Key), Tok(": ", Punct), Tok("4200", Num), Tok(",", Punct)]),
    Line(1, &[Tok("\"amount_captured\"", Key), Tok(": ", Punct), Tok("4200", Num), Tok(",", Punct)]),
    Line(1, &[Tok("\"currency\"", Key), Tok(": ", Punct), Tok("\"usd\"", Str), Tok(",", Punct)]),
    Line(1, &[Tok("\"customer\"", Key), Tok(": ", Punct), Tok("\"cus_QbT7mZa1kP\"", Str), Tok(",", Punct)]),
    Line(1, &[Tok("\"captured\"", Key), Tok(": ", Punct), Tok("true", Lit), Tok(",", Punct)]),
    Line(1, &[Tok("\"paid\"", Key), Tok(": ", Punct), Tok("true", Lit), Tok(",", Punct)]),
    Line(1, &[Tok("\"description\"", Key), Tok(": ", Punct), Tok("\"Pro annual, 12 seats\"", Str), Tok(",", Punct)]),
    Line(1, &[Tok("\"payment_method_details\"", Key), Tok(": {", Punct)]),
    Line(2, &[Tok("\"type\"", Key), Tok(": ", Punct), Tok("\"card\"", Str), Tok(",", Punct)]),
    Line(2, &[Tok("\"card\"", Key), Tok(": {", Punct)]),
    Line(3, &[Tok("\"brand\"", Key), Tok(": ", Punct), Tok("\"visa\"", Str), Tok(",", Punct)]),
    Line(3, &[Tok("\"last4\"", Key), Tok(": ", Punct), Tok("\"4242\"", Str), Tok(",", Punct)]),
    Line(3, &[Tok("\"exp_month\"", Key), Tok(": ", Punct), Tok("11", Num), Tok(",", Punct)]),
    Line(3, &[Tok("\"exp_year\"", Key), Tok(": ", Punct), Tok("2029", Num), Tok(",", Punct)]),
    Line(3, &[Tok("\"country\"", Key), Tok(": ", Punct), Tok("\"GB\"", Str)]),
    Line(2, &[Tok("}", Punct)]),
    Line(1, &[Tok("},", Punct)]),
    Line(1, &[Tok("\"refunded\"", Key), Tok(": ", Punct), Tok("false", Lit), Tok(",", Punct)]),
    Line(1, &[Tok("\"status\"", Key), Tok(": ", Punct), Tok("\"succeeded\"", Str)]),
    Line(0, &[Tok("}", Punct)]),
];

const BODY_INVOICES: &[Line] = &[
    Line(0, &[Tok("{", Punct)]),
    Line(1, &[Tok("\"object\"", Key), Tok(": ", Punct), Tok("\"list\"", Str), Tok(",", Punct)]),
    Line(1, &[Tok("\"url\"", Key), Tok(": ", Punct), Tok("\"/v1/invoices\"", Str), Tok(",", Punct)]),
    Line(1, &[Tok("\"has_more\"", Key), Tok(": ", Punct), Tok("true", Lit), Tok(",", Punct)]),
    Line(1, &[Tok("\"data\"", Key), Tok(": [", Punct)]),
    Line(2, &[Tok("{", Punct)]),
    Line(3, &[Tok("\"id\"", Key), Tok(": ", Punct), Tok("\"in_1PmQ8dR8vQ2Lb0\"", Str), Tok(",", Punct)]),
    Line(3, &[Tok("\"customer\"", Key), Tok(": ", Punct), Tok("\"cus_QbT7mZa1kP\"", Str), Tok(",", Punct)]),
    Line(3, &[Tok("\"amount_due\"", Key), Tok(": ", Punct), Tok("50400", Num), Tok(",", Punct)]),
    Line(3, &[Tok("\"amount_paid\"", Key), Tok(": ", Punct), Tok("50400", Num), Tok(",", Punct)]),
    Line(3, &[Tok("\"currency\"", Key), Tok(": ", Punct), Tok("\"usd\"", Str), Tok(",", Punct)]),
    Line(3, &[Tok("\"period_end\"", Key), Tok(": ", Punct), Tok("1788048094", Num), Tok(",", Punct)]),
    Line(3, &[Tok("\"lines\"", Key), Tok(": {", Punct)]),
    Line(4, &[Tok("\"object\"", Key), Tok(": ", Punct), Tok("\"list\"", Str), Tok(",", Punct)]),
    Line(4, &[Tok("\"total_count\"", Key), Tok(": ", Punct), Tok("1", Num)]),
    Line(3, &[Tok("},", Punct)]),
    Line(3, &[Tok("\"status\"", Key), Tok(": ", Punct), Tok("\"paid\"", Str)]),
    Line(2, &[Tok("},", Punct)]),
    Line(2, &[Tok("{", Punct)]),
    Line(3, &[Tok("\"id\"", Key), Tok(": ", Punct), Tok("\"in_1PkR2wR8vQ2Lb0\"", Str), Tok(",", Punct)]),
    Line(3, &[Tok("\"customer\"", Key), Tok(": ", Punct), Tok("\"cus_Qb9xLr4vNc\"", Str), Tok(",", Punct)]),
    Line(3, &[Tok("\"amount_due\"", Key), Tok(": ", Punct), Tok("12000", Num), Tok(",", Punct)]),
    Line(3, &[Tok("\"amount_paid\"", Key), Tok(": ", Punct), Tok("9500", Num), Tok(",", Punct)]),
    Line(3, &[Tok("\"status\"", Key), Tok(": ", Punct), Tok("\"open\"", Str)]),
    Line(2, &[Tok("}", Punct)]),
    Line(1, &[Tok("]", Punct)]),
    Line(0, &[Tok("}", Punct)]),
];

const BODY_SUBSCRIPTION: &[Line] = &[
    Line(0, &[Tok("{", Punct)]),
    Line(1, &[Tok("\"id\"", Key), Tok(": ", Punct), Tok("\"sub_1PmQ8dR8vQ2Lb0\"", Str), Tok(",", Punct)]),
    Line(1, &[Tok("\"object\"", Key), Tok(": ", Punct), Tok("\"subscription\"", Str), Tok(",", Punct)]),
    Line(1, &[Tok("\"customer\"", Key), Tok(": ", Punct), Tok("\"cus_Qb9xLr4vNc\"", Str), Tok(",", Punct)]),
    Line(1, &[Tok("\"status\"", Key), Tok(": ", Punct), Tok("\"canceled\"", Str), Tok(",", Punct)]),
    Line(1, &[Tok("\"canceled_at\"", Key), Tok(": ", Punct), Tok("1756598494", Num), Tok(",", Punct)]),
    Line(1, &[Tok("\"cancel_at_period_end\"", Key), Tok(": ", Punct), Tok("false", Lit), Tok(",", Punct)]),
    Line(1, &[Tok("\"items\"", Key), Tok(": {", Punct)]),
    Line(2, &[Tok("\"object\"", Key), Tok(": ", Punct), Tok("\"list\"", Str), Tok(",", Punct)]),
    Line(2, &[Tok("\"data\"", Key), Tok(": [", Punct)]),
    Line(3, &[Tok("{", Punct)]),
    Line(4, &[Tok("\"id\"", Key), Tok(": ", Punct), Tok("\"si_QbT7mZa1kP\"", Str), Tok(",", Punct)]),
    Line(4, &[Tok("\"quantity\"", Key), Tok(": ", Punct), Tok("12", Num)]),
    Line(3, &[Tok("}", Punct)]),
    Line(2, &[Tok("]", Punct)]),
    Line(1, &[Tok("},", Punct)]),
    Line(1, &[Tok("\"livemode\"", Key), Tok(": ", Punct), Tok("false", Lit)]),
    Line(0, &[Tok("}", Punct)]),
];

const REQ_COUNT: usize = 4;
const REQUESTS: [Request; REQ_COUNT] = [
    Request {
        method: Method::Get,
        path: "/v1/customers",
        url: "https://api.stripe.com/v1/customers?limit=2",
        status: 200,
        status_text: "OK",
        millis: 142,
        bytes: 1231,
        body: BODY_CUSTOMERS,
    },
    Request {
        method: Method::Post,
        path: "/v1/charges",
        url: "https://api.stripe.com/v1/charges",
        status: 201,
        status_text: "Created",
        millis: 268,
        bytes: 774,
        body: BODY_CHARGES,
    },
    Request {
        method: Method::Get,
        path: "/v1/invoices",
        url: "https://api.stripe.com/v1/invoices?status=open",
        status: 200,
        status_text: "OK",
        millis: 96,
        bytes: 1088,
        body: BODY_INVOICES,
    },
    Request {
        method: Method::Delete,
        path: "/v1/subscriptions/:id",
        url: "https://api.stripe.com/v1/subscriptions/sub_1PmQ8dR8vQ2Lb0",
        status: 200,
        status_text: "OK",
        millis: 187,
        bytes: 512,
        body: BODY_SUBSCRIPTION,
    },
];

/// Request headers, shown as key/value rows above the response. Three, not
/// every header a client would send: the response is the hero and the rows
/// above it are there to establish what a request is, not to be exhaustive.
const HEADERS: [(&str, &str); 3] = [
    ("Authorization", "Bearer sk_live_51QbT7mZa1kPx9Rd"),
    ("Content-Type", "application/json"),
    ("Stripe-Version", "2026-08-27"),
];

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

/// Rendered width of a string at a font size, measured by the host with the
/// same layout `draw_text` uses. The face is proportional, so counting
/// characters and multiplying does not line anything up.
fn est_width(canvas: u64, s: &str, size: f32) -> f32 {
    match canvas2d::measure_text(canvas, s, size) {
        Ok(m) => m.width,
        Err(_) => 0.0,
    }
}

fn text_right(canvas: u64, s: &str, right: f32, y: f32, size: f32, c: gfx::Color) {
    text(canvas, s, right - est_width(canvas, s, size), y, size, c);
}

fn rounded(canvas: u64, x: f32, y: f32, w: f32, h: f32, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
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

/// Write `n` in decimal into `buf`, returning the used slice.
fn uint<'b>(buf: &'b mut [u8; 16], mut n: u32) -> &'b str {
    let mut tmp = [0u8; 12];
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
        if let (Some(slot), Some(&d)) = (buf.get_mut(out), tmp.get(i)) {
            *slot = d;
            out += 1;
        }
    }
    core::str::from_utf8(buf.get(..out).unwrap_or(b"0")).unwrap_or("0")
}

/// Byte count as a human size: "812 B" under a kilobyte, else "1.2 KB".
fn size_str<'b>(buf: &'b mut [u8; 16], bytes: u32) -> &'b str {
    let mut out = 0usize;
    let push = |b: u8, buf: &mut [u8; 16], out: &mut usize| {
        if let Some(slot) = buf.get_mut(*out) {
            *slot = b;
            *out += 1;
        }
    };
    if bytes < 1024 {
        let mut nbuf = [0u8; 16];
        let n = uint(&mut nbuf, bytes);
        for &b in n.as_bytes() {
            push(b, buf, &mut out);
        }
        push(b' ', buf, &mut out);
        push(b'B', buf, &mut out);
    } else {
        // One decimal place, rounded, without floating point.
        let tenths = (bytes as u64 * 10 + 512) / 1024;
        let whole = (tenths / 10) as u32;
        let frac = (tenths % 10) as u8;
        let mut nbuf = [0u8; 16];
        let n = uint(&mut nbuf, whole);
        for &b in n.as_bytes() {
            push(b, buf, &mut out);
        }
        push(b'.', buf, &mut out);
        push(b'0' + frac, buf, &mut out);
        push(b' ', buf, &mut out);
        push(b'K', buf, &mut out);
        push(b'B', buf, &mut out);
    }
    core::str::from_utf8(buf.get(..out).unwrap_or(b"0 B")).unwrap_or("0 B")
}

// ---- layout -----------------------------------------------------------------

const RAIL_W: f32 = 210.0;
const TOPBAR_H: f32 = 64.0;

const PAD: f32 = 16.0;
/// Main column: everything right of the rail, inset by the gutter.
const MAIN_X: f32 = RAIL_W + PAD;
const MAIN_W: f32 = WIDTH - MAIN_X - PAD;

/// Request pane sits under the top bar; the response takes the rest. The
/// request pane is sized to its four header rows and no more, because every
/// pixel it does not use goes to the response, which is the thing worth
/// looking at.
const REQ_Y: f32 = TOPBAR_H + PAD;
const REQ_H: f32 = 126.0;
const RES_Y: f32 = REQ_Y + REQ_H + PAD;
const RES_H: f32 = HEIGHT - RES_Y - PAD;

/// JSON body metrics. The indent is a fixed pixel step rather than spaces,
/// because the face is proportional and space-padding would drift by depth.
const CODE_SIZE: f32 = 12.5;
const CODE_LEAD: f32 = 17.5;
const CODE_INDENT: f32 = 18.0;
/// Left edge of the body text, after the gutter that holds line numbers.
const GUTTER_W: f32 = 44.0;

const RAIL_ROW_H: f32 = 40.0;
const RAIL_ROWS_Y: f32 = 132.0;

// ---- drawing ----------------------------------------------------------------

fn draw(canvas: u64, selected: usize) -> Result<(), gfx::GfxError> {
    let req = match REQUESTS.get(selected) {
        Some(r) => r,
        None => return Ok(()),
    };

    fill(canvas, 0.0, 0.0, WIDTH, HEIGHT, BG)?;
    draw_rail(canvas, selected)?;
    draw_topbar(canvas, req)?;
    draw_request(canvas, req)?;
    draw_response(canvas, req)?;
    canvas2d::present(canvas)
}

/// A method tag: fixed width so every row's path text starts on one column.
const TAG_W: f32 = 42.0;

fn method_tag(canvas: u64, m: Method, x: f32, y: f32, h: f32, size: f32) -> Result<(), gfx::GfxError> {
    rounded(canvas, x, y, TAG_W, h, 5.0, m.ground())?;
    let label = m.label();
    let w = est_width(canvas, label, size);
    // Centre the label in the fixed tag: the three method words differ in
    // width, so a constant inset would leave DEL floating and POST tight.
    text(canvas, label, x + (TAG_W - w) * 0.5, y + h * 0.5 + size * 0.36, size, m.ink());
    Ok(())
}

fn draw_rail(canvas: u64, selected: usize) -> Result<(), gfx::GfxError> {
    fill(canvas, 0.0, 0.0, RAIL_W, HEIGHT, RAIL)?;
    fill(canvas, RAIL_W - 1.0, 0.0, 1.0, HEIGHT, EDGE)?;

    // Wordmark.
    disc(canvas, PAD + 8.0, 34.0, 8.0, ACCENT)?;
    disc(canvas, PAD + 8.0, 34.0, 3.0, RAIL)?;
    text(canvas, "Probe", PAD + 24.0, 40.0, 17.0, INK);

    // Search field: a shape, not a control, so it carries no caret or hint of
    // being typed into.
    rounded(canvas, PAD, 58.0, RAIL_W - PAD * 2.0, 28.0, 7.0, FIELD)?;
    disc(canvas, PAD + 15.0, 72.0, 4.5, INK_QUIET)?;
    disc(canvas, PAD + 15.0, 72.0, 2.8, FIELD)?;
    fill(canvas, PAD + 18.0, 75.0, 5.0, 1.6, INK_QUIET)?;
    text(canvas, "Filter requests", PAD + 28.0, 76.5, 12.0, INK_QUIET);

    // Folder header. The disclosure triangle points down (the folder is open),
    // drawn as vertical columns that shorten as they go right.
    let fy = 108.0;
    let tx = PAD + 2.0;
    let mut i = 0.0f32;
    while i < 5.0 {
        // Each row is narrower than the last and sits one pixel lower, so the
        // rows close to a point at the bottom: an open folder's marker.
        let w = 9.0 - i * 2.0;
        fill(canvas, tx + i, fy - 5.0 + i, w, 1.2, INK_QUIET)?;
        i += 1.0;
    }
    text(canvas, "Stripe API", PAD + 18.0, fy + 5.0, 12.5, INK_DIM);
    let mut cbuf = [0u8; 16];
    let count = uint(&mut cbuf, REQ_COUNT as u32);
    text_right(canvas, count, RAIL_W - PAD, fy + 5.0, 11.5, INK_QUIET);

    for i in 0..REQ_COUNT {
        let r = match REQUESTS.get(i) {
            Some(r) => r,
            None => break,
        };
        let ry = RAIL_ROWS_Y + i as f32 * RAIL_ROW_H;
        if i == selected {
            rounded(canvas, 8.0, ry, RAIL_W - 16.0, RAIL_ROW_H - 4.0, 7.0, SEL_WASH)?;
            // Accent spine on the active row, the way an editor marks a tab.
            rounded(canvas, 8.0, ry + 6.0, 2.5, RAIL_ROW_H - 16.0, 1.25, ACCENT)?;
        }
        method_tag(canvas, r.method, 18.0, ry + 8.0, 20.0, 9.5)?;
        let ink = if i == selected { INK } else { INK_DIM };
        text(canvas, r.path, 18.0 + TAG_W + 10.0, ry + 22.5, 12.5, ink);
    }

    // Environment footer, pinned to the bottom of the rail.
    let ey = HEIGHT - 34.0;
    fill(canvas, 0.0, ey - 20.0, RAIL_W - 1.0, 1.0, EDGE)?;
    disc(canvas, PAD + 5.0, ey - 4.0, 4.0, GREEN)?;
    text(canvas, "Test environment", PAD + 16.0, ey, 11.5, INK_QUIET);
    Ok(())
}

fn draw_topbar(canvas: u64, req: &Request) -> Result<(), gfx::GfxError> {
    fill(canvas, RAIL_W, 0.0, WIDTH - RAIL_W, TOPBAR_H, RAIL)?;
    fill(canvas, RAIL_W, TOPBAR_H - 1.0, WIDTH - RAIL_W, 1.0, EDGE)?;

    let bar_y = 16.0;
    let bar_h = 32.0;
    method_tag(canvas, req.method, MAIN_X, bar_y, bar_h, 11.5)?;

    // Send button, sized to its label, right-aligned; the URL strip fills what
    // is left between the tag and the button.
    let send_label = "Send";
    let send_w = est_width(canvas, send_label, 13.0) + 40.0;
    let send_x = WIDTH - PAD - send_w;

    let url_x = MAIN_X + TAG_W + 8.0;
    let url_w = send_x - 10.0 - url_x;
    rounded(canvas, url_x, bar_y, url_w, bar_h, 7.0, FIELD)?;
    // The scheme and host read quieter than the path, the way a client greys
    // the part you never edit.
    let host = "https://api.stripe.com";
    text(canvas, host, url_x + 12.0, bar_y + 21.0, 13.0, INK_QUIET);
    let rest = req.url.get(host.len()..).unwrap_or(req.url);
    text(canvas, rest, url_x + 12.0 + est_width(canvas, host, 13.0), bar_y + 21.0, 13.0, INK);

    rounded(canvas, send_x, bar_y, send_w, bar_h, 7.0, ACCENT)?;
    let sw = est_width(canvas, send_label, 13.0);
    text(canvas, send_label, send_x + (send_w - sw) * 0.5, bar_y + 21.0, 13.0, INK);
    Ok(())
}

/// A pane: rounded ground with a hairline top edge, plus a title row.
fn pane(canvas: u64, x: f32, y: f32, w: f32, h: f32) -> Result<(), gfx::GfxError> {
    rounded(canvas, x, y, w, h, 10.0, PANE)?;
    fill(canvas, x + 10.0, y, w - 20.0, 1.0, EDGE)?;
    Ok(())
}

fn draw_request(canvas: u64, _req: &Request) -> Result<(), gfx::GfxError> {
    pane(canvas, MAIN_X, REQ_Y, MAIN_W, REQ_H)?;

    // Tab strip: Headers is active, the rest are the other request panes a
    // client carries. The underline marks the active one.
    let tabs = ["Headers", "Body", "Auth", "Query"];
    let mut tx = MAIN_X + 18.0;
    let ty = REQ_Y + 28.0;
    for (i, label) in tabs.iter().enumerate() {
        let w = est_width(canvas, label, 12.5);
        let active = i == 0;
        text(canvas, label, tx, ty, 12.5, if active { INK } else { INK_QUIET });
        if active {
            rounded(canvas, tx, ty + 10.0, w, 2.0, 1.0, ACCENT)?;
        }
        tx += w + 22.0;
    }
    let mut nbuf = [0u8; 16];
    let n = uint(&mut nbuf, HEADERS.len() as u32);
    text_right(canvas, n, MAIN_X + MAIN_W - 18.0, ty, 11.5, INK_QUIET);
    fill(canvas, MAIN_X + 1.0, REQ_Y + 42.0, MAIN_W - 2.0, 1.0, EDGE)?;

    // Key/value rows on a shared column, so the values line up as a block.
    let key_x = MAIN_X + 18.0;
    let val_x = MAIN_X + 158.0;
    let row_h = 25.0;
    let mut ry = REQ_Y + 64.0;
    for (k, v) in HEADERS.iter() {
        text(canvas, k, key_x, ry, 12.5, INK_DIM);
        text(canvas, v, val_x, ry, 12.5, INK);
        ry += row_h;
    }
    // The column rule reads as a table, and is what makes it a key/value pane
    // rather than two lists that happen to share a row.
    fill(canvas, val_x - 16.0, REQ_Y + 52.0, 1.0, row_h * HEADERS.len() as f32 + 4.0, EDGE)?;
    Ok(())
}

fn draw_response(canvas: u64, req: &Request) -> Result<(), gfx::GfxError> {
    pane(canvas, MAIN_X, RES_Y, MAIN_W, RES_H)?;

    // Status row: the chip, then timing and size as quiet metadata.
    let hy = RES_Y + 30.0;
    let ok = req.status < 300;
    let chip_ink = if ok { GREEN } else { AMBER };
    let chip_ground = if ok { TAG_GET } else { TAG_POST };

    let mut sbuf = [0u8; 16];
    let code = uint(&mut sbuf, req.status as u32);
    let code_w = est_width(canvas, code, 12.5);
    let text_w = est_width(canvas, req.status_text, 12.5);
    let chip_w = code_w + text_w + 6.0 + 26.0;
    rounded(canvas, MAIN_X + 18.0, hy - 14.0, chip_w, 22.0, 6.0, chip_ground)?;
    text(canvas, code, MAIN_X + 31.0, hy + 1.0, 12.5, chip_ink);
    text(canvas, req.status_text, MAIN_X + 31.0 + code_w + 6.0, hy + 1.0, 12.5, chip_ink);

    // Timing and size, each with a quiet label, separated by a dot.
    let mut x = MAIN_X + 18.0 + chip_w + 18.0;
    let mut mbuf = [0u8; 16];
    let ms = uint(&mut mbuf, req.millis);
    text(canvas, ms, x, hy + 1.0, 12.5, INK);
    x += est_width(canvas, ms, 12.5) + 2.0;
    text(canvas, " ms", x, hy + 1.0, 12.5, INK_DIM);
    x += est_width(canvas, " ms", 12.5) + 10.0;
    disc(canvas, x, hy - 4.0, 1.6, INK_QUIET)?;
    x += 10.0;
    let mut zbuf = [0u8; 16];
    let sz = size_str(&mut zbuf, req.bytes);
    text(canvas, sz, x, hy + 1.0, 12.5, INK);

    text_right(canvas, "application/json", MAIN_X + MAIN_W - 18.0, hy + 1.0, 11.5, INK_QUIET);
    fill(canvas, MAIN_X + 1.0, RES_Y + 46.0, MAIN_W - 2.0, 1.0, EDGE)?;

    draw_json(canvas, req.body, MAIN_X, RES_Y + 46.0, MAIN_W, RES_H - 46.0)
}

/// The syntax-highlighted body, with a line-number gutter.
fn draw_json(canvas: u64, body: &[Line], x: f32, y: f32, w: f32, h: f32) -> Result<(), gfx::GfxError> {
    // Gutter ground, one shade off the pane so numbers sit in their own column.
    fill(canvas, x + 1.0, y, GUTTER_W, h - 10.0, rgb(0.075, 0.086, 0.110))?;
    fill(canvas, x + GUTTER_W, y, 1.0, h - 10.0, EDGE)?;

    let text_x = x + GUTTER_W + 16.0;
    let first_base = y + 26.0;
    // Only draw whole lines: a clipped half-line at the bottom edge is the
    // clearest possible tell that this is a picture and not a viewport. The
    // footer strip is reserved whether or not the notice lands in it, so the
    // last line always has the same air under it.
    const FOOTER: f32 = 26.0;
    let room = h - 26.0 - FOOTER;
    let max_lines = if room > 0.0 { (room / CODE_LEAD) as usize } else { 0 };

    for (i, line) in body.iter().enumerate() {
        if i >= max_lines {
            break;
        }
        let by = first_base + i as f32 * CODE_LEAD;

        let mut nbuf = [0u8; 16];
        let n = uint(&mut nbuf, i as u32 + 1);
        text_right(canvas, n, x + GUTTER_W - 12.0, by, 11.0, INK_QUIET);

        let mut tx = text_x + line.0 as f32 * CODE_INDENT;
        for tok in line.1.iter() {
            let c = match tok.1 {
                Key => J_KEY,
                Str => J_STR,
                Num => J_NUM,
                Lit => J_LIT,
                Punct => J_PUNCT,
            };
            text(canvas, tok.0, tx, by, CODE_SIZE, c);
            tx += est_width(canvas, tok.0, CODE_SIZE);
        }
    }

    // Truncation notice, only when there is more body than room for it.
    if body.len() > max_lines {
        let hidden = body.len() - max_lines;
        let mut nbuf = [0u8; 16];
        let n = uint(&mut nbuf, hidden as u32);
        let label = " more lines";
        let total = est_width(canvas, n, 11.0) + est_width(canvas, label, 11.0);
        let ly = y + h - 14.0;
        text(canvas, n, x + w - 18.0 - total, ly, 11.0, INK_DIM);
        text(canvas, label, x + w - 18.0 - total + est_width(canvas, n, 11.0), ly, 11.0, INK_QUIET);
    }
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
        // No fixed size, and grow, so the canvas fills whatever window the
        // host gives it and the design size handles the scaling (K-003).
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

/// Which rail row a click landed on, if any.
fn hit_rail(x: f32, y: f32) -> Option<usize> {
    if x < 8.0 || x > RAIL_W - 8.0 {
        return None;
    }
    for i in 0..REQ_COUNT {
        let ry = RAIL_ROWS_Y + i as f32 * RAIL_ROW_H;
        if y >= ry && y < ry + RAIL_ROW_H - 4.0 {
            return Some(i);
        }
    }
    None
}

// ---- app --------------------------------------------------------------------

struct Component;

impl bindings::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize { width: WIDTH as u32, height: HEIGHT as u32 };
        let Ok(win) = window::create("Probe", size) else {
            return 30;
        };
        if window::show(win).is_err() {
            return 31;
        }
        if tree::set_root(win, &stack_root()).is_err() || tree::upsert_node(win, &canvas_node()).is_err() {
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
        // The app's own coordinate system: draw in these numbers and the host
        // scales them to any window, centred, never stretched (K-096).
        let _ = canvas2d::set_design_size(canvas, gfx::Size { width: WIDTH, height: HEIGHT });

        let mut selected = 0usize;

        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");

        let _ = draw(canvas, selected);

        if quick {
            let out = stdio::stdout();
            let _ = out.write(b"probe:ok\n");
            let _ = out.flush();
            let _ = window::close(win);
            return 0;
        }

        let mut idle = 0u32;
        for _ in 0..MAX_WAIT_ROUNDS {
            match events::wait(Some(WAIT_ROUND_MILLIS)) {
                Some(types::Event::Pointer(p)) if p.pressed => {
                    idle = 0;
                    if let Some(i) = hit_rail(p.x, p.y) {
                        if i != selected {
                            selected = i;
                            let _ = draw(canvas, selected);
                        }
                    }
                }
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
        let _ = out.write(b"probe:ok\n");
        let _ = out.flush();
        0
    }
}

bindings::export!(Component with_types_in bindings);
