//! Diff -- a side-by-side review of a change, drawn entirely on a canvas.
//!
//! The developer tool people reach for most and Krate had no example of: a
//! file list down the left with per-file add/remove counts, and the change
//! itself on the right as two columns with the old on one side and the new
//! on the other. Added lines carry a green stripe and a tinted ground,
//! removed lines a red one, and a hunk header separates the regions the way
//! `git diff` does.
//!
//! Why it is worth having: it exercises the parts of the canvas that a real
//! tool needs and a toy does not -- a monospaced gutter that stays aligned
//! when line numbers change width, two independently clipped columns, and
//! several hundred short strings a frame without the layout drifting.
//!
//! The change shown is a fixed sample so the first frame is the same on
//! every machine. A real one reads the two sides through `fs.read`, which is
//! why the capability is declared: the point is that a reviewer's app needs
//! nothing but the folder it was pointed at.
//!
//! `#![no_std]` keeps it `krate:*`-only: the SDK owns the allocator and a
//! trapping panic handler. State is fixed-size; numbers are formatted by
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

const WIDTH: f32 = 1240.0;
const HEIGHT: f32 = 780.0;

const MARGIN: f32 = 24.0;
/// The file rail. Fixed, because a diff is read on the right and the rail is
/// only there to say which file you are in.
const RAIL_W: f32 = 232.0;

const MAX_WAIT_ROUNDS: u32 = 600_000;
const WAIT_ROUND_MILLIS: u32 = 33;

// ---- palette ----------------------------------------------------------------

const BG_TOP: gfx::Color = rgb(0.043, 0.055, 0.082);
const BG_BOT: gfx::Color = rgb(0.063, 0.078, 0.114);
const CARD: gfx::Color = rgb(0.086, 0.106, 0.149);
const CARD_EDGE: gfx::Color = rgb(0.137, 0.165, 0.220);
const RAIL_BG: gfx::Color = rgb(0.071, 0.086, 0.125);
const INK: gfx::Color = rgb(0.949, 0.961, 0.980);
const INK_DIM: gfx::Color = rgb(0.604, 0.647, 0.710);
const INK_QUIET: gfx::Color = rgb(0.365, 0.408, 0.471);
const SEL: gfx::Color = rgb(0.129, 0.157, 0.220);

/// Add and remove carry the whole read: a reviewer finds the colour before
/// reading a word, so nothing else in the app may be green or red.
const ADD_INK: gfx::Color = rgb(0.494, 0.867, 0.588);
const ADD_BED: gfx::Color = rgb(0.075, 0.145, 0.106);
const DEL_INK: gfx::Color = rgb(0.945, 0.494, 0.514);
const DEL_BED: gfx::Color = rgb(0.161, 0.082, 0.098);
const HUNK_INK: gfx::Color = rgb(0.478, 0.573, 0.804);
const HUNK_BED: gfx::Color = rgb(0.086, 0.106, 0.161);

const HAIRLINE: gfx::Color = gfx::Color { r: 1.0, g: 1.0, b: 1.0, a: 0.045 };

const fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
}

// ---- the change -------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Same,
    Add,
    Del,
    Hunk,
}
use Kind::{Add, Del, Hunk, Same};

/// One row of the diff. `old` and `new` are the line numbers on each side;
/// 0 means the side has no line here, which is what makes an add or a
/// delete line up against a blank rather than shifting the other column.
struct Row(u32, u32, Kind, &'static str);

const ROWS: [Row; 26] = [
    Row(0, 0, Hunk, "@@ -18,7 +18,12 @@ fn resolve(cap: &Capability)"),
    Row(18, 18, Same, "    let mut granted = Vec::new();"),
    Row(19, 19, Same, "    for want in manifest.capabilities() {"),
    Row(20, 0, Del, "        if policy.allows(want) {"),
    Row(21, 0, Del, "            granted.push(want.clone());"),
    Row(0, 20, Add, "        // A wildcard grant is not a grant of everything:"),
    Row(0, 21, Add, "        // fs.read:notes/** must not imply fs.read:/ (K-086)."),
    Row(0, 22, Add, "        let scope = want.scope().narrowed();"),
    Row(0, 23, Add, "        if policy.allows_scoped(want, &scope) {"),
    Row(0, 24, Add, "            granted.push(want.with_scope(scope));"),
    Row(22, 25, Same, "        }"),
    Row(23, 26, Same, "    }"),
    Row(24, 27, Same, "    granted"),
    Row(25, 28, Same, "}"),
    Row(0, 0, Hunk, "@@ -63,9 +68,14 @@ impl Policy"),
    Row(63, 68, Same, "    /// Whether this capability may be granted at all."),
    Row(64, 69, Same, "    pub fn allows(&self, cap: &Capability) -> bool {"),
    Row(65, 0, Del, "        self.rules.iter().any(|r| r.matches(cap))"),
    Row(0, 70, Add, "        self.allows_scoped(cap, &cap.scope())"),
    Row(66, 71, Same, "    }"),
    Row(0, 72, Add, ""),
    Row(0, 73, Add, "    /// The same question, asked about a narrowed scope."),
    Row(0, 74, Add, "    pub fn allows_scoped(&self, cap: &Capability, s: &Scope) -> bool {"),
    Row(0, 75, Add, "        self.rules.iter().any(|r| r.matches_scoped(cap, s))"),
    Row(0, 76, Add, "    }"),
    Row(67, 77, Same, "}"),
];

/// One file in the change, with what it did.
struct FileRow(&'static str, u32, u32, bool);

const FILES: [FileRow; 6] = [
    FileRow("crates/policy/src/lib.rs", 9, 2, true),
    FileRow("crates/policy/src/scope.rs", 41, 0, false),
    FileRow("crates/manifest/src/cap.rs", 6, 6, false),
    FileRow("crates/runtime/src/host.rs", 3, 1, false),
    FileRow("crates/cli/src/main.rs", 2, 0, false),
    FileRow("BUGS.md", 18, 0, false),
];

// ---- drawing helpers --------------------------------------------------------

fn fill(canvas: u64, x: f32, y: f32, w: f32, h: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_rect(canvas, gfx::Rect { x, y, width: w, height: h }, c)
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

fn text(canvas: u64, s: &str, x: f32, y: f32, size: f32, c: gfx::Color) {
    let _ = canvas2d::draw_text(canvas, s, gfx::Point { x, y }, size, c);
}

fn width_of(canvas: u64, s: &str, size: f32) -> f32 {
    match canvas2d::measure_text(canvas, s, size) {
        Ok(m) => m.width,
        Err(_) => 0.0,
    }
}

fn text_right(canvas: u64, s: &str, right: f32, y: f32, size: f32, c: gfx::Color) {
    text(canvas, s, right - width_of(canvas, s, size), y, size, c);
}

/// Format an unsigned integer into `buf`, returning the used slice.
fn uint(buf: &mut [u8; 12], mut value: u32) -> &str {
    if value == 0 {
        buf[0] = b'0';
        return core::str::from_utf8(&buf[..1]).unwrap_or("0");
    }
    let mut tmp = [0u8; 12];
    let mut n = 0usize;
    while value > 0 && n < tmp.len() {
        tmp[n] = b'0' + (value % 10) as u8;
        value /= 10;
        n += 1;
    }
    for i in 0..n {
        buf[i] = tmp[n - 1 - i];
    }
    core::str::from_utf8(&buf[..n]).unwrap_or("0")
}

/// "+9" / "-2", built without `format!`.
fn signed(buf: &mut [u8; 12], sign: u8, value: u32) -> &str {
    let mut inner = [0u8; 12];
    let digits = uint(&mut inner, value).len();
    buf[0] = sign;
    buf[1..1 + digits].copy_from_slice(&inner[..digits]);
    core::str::from_utf8(&buf[..1 + digits]).unwrap_or("+0")
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

// ---- the frame --------------------------------------------------------------

const LEAD: f32 = 21.0;
const CODE: f32 = 12.5;
/// The line-number gutter. Wide enough for four digits so the code does not
/// shift sideways when a file passes line 999.
const GUTTER: f32 = 42.0;

fn draw(canvas: u64) -> Result<(), gfx::GfxError> {
    canvas2d::linear_gradient(
        canvas,
        gfx::Rect { x: 0.0, y: 0.0, width: WIDTH, height: HEIGHT },
        BG_TOP,
        BG_BOT,
    )?;

    // ---- the head -----------------------------------------------------------
    text(canvas, "Narrow a wildcard grant to its scope", MARGIN, 44.0, 18.0, INK);
    let mut b = [0u8; 12];
    text(canvas, "6 files", MARGIN, 68.0, 12.0, INK_QUIET);
    let mut x = MARGIN + width_of(canvas, "6 files", 12.0) + 14.0;
    text(canvas, signed(&mut b, b'+', 79), x, 68.0, 12.0, ADD_INK);
    x += width_of(canvas, "+79", 12.0) + 10.0;
    let mut b2 = [0u8; 12];
    text(canvas, signed(&mut b2, b'-', 9), x, 68.0, 12.0, DEL_INK);
    text_right(canvas, "reviewing locally -- nothing left this machine", WIDTH - MARGIN, 68.0, 12.0, INK_QUIET);

    let body_y = 88.0;
    let body_h = HEIGHT - body_y - MARGIN;

    // ---- the file rail ------------------------------------------------------
    rounded(canvas, MARGIN, body_y, RAIL_W, body_h, 14.0, RAIL_BG)?;
    canvas2d::stroke_round_rect(
        canvas,
        gfx::Rect { x: MARGIN, y: body_y, width: RAIL_W, height: body_h },
        gfx::CornerRadii { top_left: 14.0, top_right: 14.0, bottom_right: 14.0, bottom_left: 14.0 },
        1.0,
        CARD_EDGE,
    )?;
    text(canvas, "FILES", MARGIN + 16.0, body_y + 26.0, 10.5, INK_QUIET);

    for (i, f) in FILES.iter().enumerate() {
        let ry = body_y + 42.0 + i as f32 * 46.0;
        if f.3 {
            rounded(canvas, MARGIN + 8.0, ry - 2.0, RAIL_W - 16.0, 40.0, 8.0, SEL)?;
        }
        // The path, tail-first: a reviewer scans file names, not directories,
        // and the directory is only there to disambiguate.
        let name = match f.0.rfind('/') {
            Some(i) => &f.0[i + 1..],
            None => f.0,
        };
        let dir = match f.0.rfind('/') {
            Some(i) => &f.0[..i],
            None => "",
        };
        text(canvas, name, MARGIN + 18.0, ry + 14.0, 13.0, if f.3 { INK } else { INK_DIM });
        text(canvas, dir, MARGIN + 18.0, ry + 30.0, 10.5, INK_QUIET);

        let mut ab = [0u8; 12];
        let mut db = [0u8; 12];
        let a = signed(&mut ab, b'+', f.1);
        let aw = width_of(canvas, a, 10.5);
        let d = signed(&mut db, b'-', f.2);
        let dw = width_of(canvas, d, 10.5);
        let right = MARGIN + RAIL_W - 16.0;
        text(canvas, d, right - dw, ry + 14.0, 10.5, DEL_INK);
        text(canvas, a, right - dw - 8.0 - aw, ry + 14.0, 10.5, ADD_INK);
    }

    // ---- the diff -----------------------------------------------------------
    let dx = MARGIN + RAIL_W + 14.0;
    let dw = WIDTH - MARGIN - dx;
    rounded(canvas, dx, body_y, dw, body_h, 14.0, CARD)?;
    canvas2d::stroke_round_rect(
        canvas,
        gfx::Rect { x: dx, y: body_y, width: dw, height: body_h },
        gfx::CornerRadii { top_left: 14.0, top_right: 14.0, bottom_right: 14.0, bottom_left: 14.0 },
        1.0,
        CARD_EDGE,
    )?;

    // Two columns, split down the middle, each with its own gutter. The split
    // is a real line: without it the eye cannot tell which side it is on.
    let col_w = (dw - 2.0) / 2.0;
    let right_x = dx + col_w + 2.0;
    fill(canvas, dx + col_w, body_y + 40.0, 1.0, body_h - 40.0, HAIRLINE)?;

    text(canvas, "BEFORE", dx + GUTTER + 14.0, body_y + 26.0, 10.5, INK_QUIET);
    text(canvas, "AFTER", right_x + GUTTER + 14.0, body_y + 26.0, 10.5, INK_QUIET);
    fill(canvas, dx + 1.0, body_y + 39.0, dw - 2.0, 1.0, HAIRLINE)?;

    let first = body_y + 60.0;
    // Only whole rows: a clipped half-line at the bottom is the clearest tell
    // that this is a picture rather than a viewport.
    let room = body_h - (first - body_y) - 14.0;
    let max_rows = if room > 0.0 { (room / LEAD) as usize } else { 0 };

    for (i, r) in ROWS.iter().enumerate() {
        if i >= max_rows {
            break;
        }
        let y = first + i as f32 * LEAD;

        if r.2 == Hunk {
            // The hunk header spans both columns: it belongs to neither side.
            fill(canvas, dx + 1.0, y - 14.0, dw - 2.0, LEAD, HUNK_BED)?;
            text(canvas, r.3, dx + GUTTER + 14.0, y, CODE, HUNK_INK);
            continue;
        }

        // Left side: everything except an addition.
        if r.2 != Add {
            let bed = if r.2 == Del { Some(DEL_BED) } else { None };
            if let Some(c) = bed {
                fill(canvas, dx + 1.0, y - 14.0, col_w - 1.0, LEAD, c)?;
                fill(canvas, dx + 1.0, y - 14.0, 2.5, LEAD, DEL_INK)?;
            }
            let mut nb = [0u8; 12];
            text_right(canvas, uint(&mut nb, r.0), dx + GUTTER, y, 11.0, INK_QUIET);
            let ink = if r.2 == Del { DEL_INK } else { INK_DIM };
            text(canvas, r.3, dx + GUTTER + 14.0, y, CODE, ink);
        }

        // Right side: everything except a deletion.
        if r.2 != Del {
            let bed = if r.2 == Add { Some(ADD_BED) } else { None };
            if let Some(c) = bed {
                fill(canvas, right_x, y - 14.0, col_w - 1.0, LEAD, c)?;
                fill(canvas, right_x, y - 14.0, 2.5, LEAD, ADD_INK)?;
            }
            let mut nb = [0u8; 12];
            text_right(canvas, uint(&mut nb, r.1), right_x + GUTTER, y, 11.0, INK_QUIET);
            let ink = if r.2 == Add { ADD_INK } else { INK_DIM };
            text(canvas, r.3, right_x + GUTTER + 14.0, y, CODE, ink);
        }
    }

    canvas2d::present(canvas)
}

// ---- widget scaffolding -----------------------------------------------------

fn stack_root() -> types::WidgetNode {
    types::WidgetNode {
        id: ROOT_ID,
        parent: None,
        kind: types::WidgetKind::Stack,
        label: None,
        role: None,
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

struct Component;

impl bindings::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize { width: WIDTH as u32, height: HEIGHT as u32 };
        let Ok(win) = window::create("Diff", size) else {
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
        let _ = canvas2d::set_design_size(canvas, gfx::Size { width: WIDTH, height: HEIGHT });

        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");

        let _ = draw(canvas);

        if quick {
            let out = stdio::stdout();
            let _ = out.write(b"diff:ok\n");
            let _ = out.flush();
            let _ = window::close(win);
            return 0;
        }

        for _ in 0..MAX_WAIT_ROUNDS {
            match events::wait(Some(WAIT_ROUND_MILLIS)) {
                Some(types::Event::CloseRequested(id)) if id == win => break,
                Some(types::Event::Resized(_)) => {
                    let _ = draw(canvas);
                }
                _ => {}
            }
        }

        let _ = window::close(win);
        let out = stdio::stdout();
        let _ = out.write(b"diff:ok\n");
        let _ = out.flush();
        0
    }
}

bindings::export!(Component with_types_in bindings);
