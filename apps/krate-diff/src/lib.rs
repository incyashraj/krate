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
//! It lays itself out from the canvas it really has: a desktop window gets
//! the rail and two columns, a phone one column, the way a phone shows a
//! diff.
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



use alloc::string::String;
use krate::gfx::{canvas2d, types as gfx};
use krate::bindings::krate::io::{args, stdio};
use krate::layout::{Look, Rect, Said};
use krate::ui::{events, tree, types, window};

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
/// Below this width the file rail and a second column cannot both be read,
/// so the change is shown as one column, the way a phone shows a diff.
const SPLIT_MIN: f32 = 900.0;

/// One frame being drawn: the canvas, the person's text size (every font
/// size and every row height goes through `t`), and what the screen reader
/// will be told it shows.
struct Frame {
    canvas: u64,
    t: f32,
    said: Said,
}

impl Frame {
    fn pt(&self, points: f32) -> f32 {
        points * self.t
    }

    /// Draw `s` at most `room` wide: the longest leading part that fits,
    /// then an ellipsis. A line cut short reads as "there is more"; a line
    /// running over the next column reads as a broken app. Every string
    /// here is ASCII.
    fn fit(&self, s: &str, x: f32, y: f32, size: f32, c: gfx::Color, room: f32) {
        if room <= 0.0 {
            return;
        }
        if width_of(self.canvas, s, size) <= room {
            text(self.canvas, s, x, y, size, c);
            return;
        }
        let ell = width_of(self.canvas, "…", size);
        let (mut lo, mut hi) = (0usize, s.len());
        while lo < hi {
            let mid = (lo + hi + 1) / 2;
            if width_of(self.canvas, s.get(..mid).unwrap_or(""), size) + ell <= room {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        let part = s.get(..lo).unwrap_or("").trim_end();
        text(self.canvas, part, x, y, size, c);
        text(self.canvas, "…", x + width_of(self.canvas, part, size), y, size, c);
    }

    fn card(&self, x: f32, y: f32, w: f32, h: f32, fill_c: gfx::Color) -> Result<(), gfx::GfxError> {
        rounded(self.canvas, x, y, w, h, 14.0, fill_c)?;
        canvas2d::stroke_round_rect(
            self.canvas,
            gfx::Rect { x, y, width: w, height: h },
            gfx::CornerRadii { top_left: 14.0, top_right: 14.0, bottom_right: 14.0, bottom_left: 14.0 },
            1.0,
            CARD_EDGE,
        )
    }

    /// Tell the screen reader one row: what kind of line it is, then the
    /// line itself, as one phrase.
    fn say_row(&mut self, at: Rect, kind: Kind, line: &str) {
        let lead = match kind {
            Add => "added: ",
            Del => "removed: ",
            Hunk => "section: ",
            Same => "",
        };
        let mut buf = [0u8; 160];
        let mut n = 0usize;
        for part in [lead, line] {
            for byte in part.bytes() {
                if n < buf.len() {
                    buf[n] = byte;
                    n += 1;
                }
            }
        }
        let phrase = core::str::from_utf8(buf.get(..n).unwrap_or(&[])).unwrap_or(line);
        self.said.text(at, phrase);
    }
}

/// Laid out from the canvas the app really has, every frame: a desktop
/// window gets the file rail and two columns, a phone one column. Text
/// follows the person's text size.
fn draw(canvas: u64, win: u64) -> Result<(), gfx::GfxError> {
    let size = canvas2d::canvas_size(canvas).unwrap_or(gfx::Size { width: WIDTH, height: HEIGHT });
    let w = size.width.max(1.0);
    let h = size.height.max(1.0);
    let mut f = Frame { canvas, t: Look::now().text_scale.clamp(0.8, 2.4), said: Said::new() };
    let narrow = w < SPLIT_MIN;
    let margin = if narrow { 14.0 } else { MARGIN };
    canvas2d::linear_gradient(
        canvas,
        gfx::Rect { x: 0.0, y: 0.0, width: w, height: h },
        BG_TOP,
        BG_BOT,
    )?;

    // ---- the head -----------------------------------------------------------
    let title_size = f.pt(if narrow { 16.0 } else { 18.0 });
    let title_y = 20.0 + title_size * 1.3;
    let title = "Narrow a wildcard grant to its scope";
    f.fit(title, margin, title_y, title_size, INK, w - 2.0 * margin);
    f.said.heading(Rect::new(margin, title_y - title_size, w - 2.0 * margin, title_size * 1.3), title);
    let small = f.pt(12.0);
    let meta_y = title_y + small * 2.0;
    let mut b = [0u8; 12];
    text(canvas, "6 files", margin, meta_y, small, INK_QUIET);
    let mut x = margin + width_of(canvas, "6 files", small) + 14.0;
    text(canvas, signed(&mut b, b'+', 79), x, meta_y, small, ADD_INK);
    x += width_of(canvas, "+79", small) + 10.0;
    let mut b2 = [0u8; 12];
    text(canvas, signed(&mut b2, b'-', 9), x, meta_y, small, DEL_INK);
    x += width_of(canvas, "-9", small) + 18.0;
    f.said.text(Rect::new(margin, meta_y - small, x - margin, small * 1.4), "6 files, 79 lines added, 9 removed");
    let note = "reviewing locally -- nothing left this machine";
    if w - margin - x >= width_of(canvas, note, small) {
        text_right(canvas, note, w - margin, meta_y, small, INK_QUIET);
        f.said.text(Rect::new(w - margin - width_of(canvas, note, small), meta_y - small, width_of(canvas, note, small), small * 1.4), note);
    }

    let body_y = meta_y + small + 8.0;
    let body_h = h - body_y - margin;
    if body_h > 60.0 {
        if narrow {
            draw_unified(&mut f, margin, body_y, w - 2.0 * margin, body_h)?;
        } else {
            draw_split(&mut f, w, body_y, body_h)?;
        }
    }
    canvas2d::present(canvas)?;
    f.said.send(win);
    Ok(())
}

/// The desktop layout: the file rail, then the change as two columns.
fn draw_split(f: &mut Frame, w: f32, body_y: f32, body_h: f32) -> Result<(), gfx::GfxError> {
    let canvas = f.canvas;
    // ---- the file rail ------------------------------------------------------
    let rail_w = RAIL_W * f.t.max(1.0);
    f.card(MARGIN, body_y, rail_w, body_h, RAIL_BG)?;
    let label = f.pt(10.5);
    text(canvas, "FILES", MARGIN + 16.0, body_y + 26.0, label, INK_QUIET);
    let name_size = f.pt(13.0);
    let row_h = 46.0 * f.t;

    for (i, file) in FILES.iter().enumerate() {
        let ry = body_y + 42.0 + i as f32 * row_h;
        if ry + row_h > body_y + body_h {
            break;
        }
        if file.3 {
            rounded(canvas, MARGIN + 8.0, ry - 2.0, rail_w - 16.0, row_h - 6.0, 8.0, SEL)?;
        }
        // The path, tail-first: a reviewer scans file names, not directories,
        // and the directory is only there to disambiguate.
        let (dir, name) = split_path(file.0);
        let mut ab = [0u8; 12];
        let mut db = [0u8; 12];
        let a = signed(&mut ab, b'+', file.1);
        let aw = width_of(canvas, a, label);
        let d = signed(&mut db, b'-', file.2);
        let dw = width_of(canvas, d, label);
        let right = MARGIN + rail_w - 16.0;
        let name_y = ry + name_size * 1.1;
        f.fit(name, MARGIN + 18.0, name_y, name_size, if file.3 { INK } else { INK_DIM }, rail_w - 52.0 - aw - dw);
        f.fit(dir, MARGIN + 18.0, name_y + label * 1.5, label, INK_QUIET, rail_w - 36.0);
        text(canvas, d, right - dw, name_y, label, DEL_INK);
        text(canvas, a, right - dw - 8.0 - aw, name_y, label, ADD_INK);
        f.said.button(Rect::new(MARGIN + 8.0, ry - 2.0, rail_w - 16.0, row_h - 6.0), file.0);
    }

    // ---- the diff -----------------------------------------------------------
    let dx = MARGIN + rail_w + 14.0;
    let dw = w - MARGIN - dx;
    f.card(dx, body_y, dw, body_h, CARD)?;

    // Two columns, split down the middle, each with its own gutter. The split
    // is a real line: without it the eye cannot tell which side it is on.
    let col_w = (dw - 2.0) / 2.0;
    let right_x = dx + col_w + 2.0;
    let gutter = GUTTER * f.t.max(1.0);
    let code_room = col_w - gutter - 24.0;
    fill(canvas, dx + col_w, body_y + 40.0, 1.0, body_h - 40.0, HAIRLINE)?;

    text(canvas, "BEFORE", dx + gutter + 14.0, body_y + 26.0, label, INK_QUIET);
    text(canvas, "AFTER", right_x + gutter + 14.0, body_y + 26.0, label, INK_QUIET);
    fill(canvas, dx + 1.0, body_y + 39.0, dw - 2.0, 1.0, HAIRLINE)?;

    let lead = LEAD * f.t;
    let code = f.pt(CODE);
    let num = f.pt(11.0);
    let first = body_y + 40.0 + lead;
    // Only whole rows: a clipped half-line at the bottom is the clearest tell
    // that this is a picture rather than a viewport.
    let room = body_h - (first - body_y) - 14.0;
    let max_rows = if room > 0.0 { (room / lead) as usize + 1 } else { 0 };

    for (i, r) in ROWS.iter().enumerate() {
        if i >= max_rows {
            break;
        }
        let y = first + i as f32 * lead;
        let top = y - lead * 0.68;
        f.say_row(Rect::new(dx, top, dw, lead), r.2, r.3);

        if r.2 == Hunk {
            // The hunk header spans both columns: it belongs to neither side.
            fill(canvas, dx + 1.0, top, dw - 2.0, lead, HUNK_BED)?;
            f.fit(r.3, dx + gutter + 14.0, y, code, HUNK_INK, dw - gutter - 28.0);
            continue;
        }

        // Left side: everything except an addition.
        if r.2 != Add {
            if r.2 == Del {
                fill(canvas, dx + 1.0, top, col_w - 1.0, lead, DEL_BED)?;
                fill(canvas, dx + 1.0, top, 2.5, lead, DEL_INK)?;
            }
            let mut nb = [0u8; 12];
            text_right(canvas, uint(&mut nb, r.0), dx + gutter, y, num, INK_QUIET);
            let ink = if r.2 == Del { DEL_INK } else { INK_DIM };
            f.fit(r.3, dx + gutter + 14.0, y, code, ink, code_room);
        }

        // Right side: everything except a deletion.
        if r.2 != Del {
            if r.2 == Add {
                fill(canvas, right_x, top, col_w - 1.0, lead, ADD_BED)?;
                fill(canvas, right_x, top, 2.5, lead, ADD_INK)?;
            }
            let mut nb = [0u8; 12];
            text_right(canvas, uint(&mut nb, r.1), right_x + gutter, y, num, INK_QUIET);
            let ink = if r.2 == Add { ADD_INK } else { INK_DIM };
            f.fit(r.3, right_x + gutter + 14.0, y, code, ink, code_room);
        }
    }
    Ok(())
}

/// The phone layout: the file being read named at the top of one card, and
/// the change as one column, old and new line numbers side by side, the way
/// `git diff` prints it.
fn draw_unified(f: &mut Frame, x: f32, y: f32, w: f32, h: f32) -> Result<(), gfx::GfxError> {
    let canvas = f.canvas;
    f.card(x, y, w, h, CARD)?;
    let Some(selected) = FILES.iter().find(|file| file.3).or(FILES.first()) else {
        return Ok(());
    };
    let small = f.pt(11.0);
    let head_y = y + 14.0 + small * 1.2;
    let mut ab = [0u8; 12];
    let mut db = [0u8; 12];
    let a = signed(&mut ab, b'+', selected.1);
    let d = signed(&mut db, b'-', selected.2);
    let dw = width_of(canvas, d, small);
    let aw = width_of(canvas, a, small);
    let right = x + w - 14.0;
    text(canvas, d, right - dw, head_y, small, DEL_INK);
    text(canvas, a, right - dw - 8.0 - aw, head_y, small, ADD_INK);
    let (_, name) = split_path(selected.0);
    let name_size = f.pt(12.5);
    f.fit(name, x + 14.0, head_y, name_size, INK, w - 28.0 - dw - aw - 16.0);
    f.said.heading(Rect::new(x, y, w, head_y - y + 8.0), selected.0);
    let line_y = head_y + 12.0;
    fill(canvas, x + 1.0, line_y, w - 2.0, 1.0, HAIRLINE)?;

    let lead = 20.0 * f.t;
    let code = f.pt(11.5);
    let num = f.pt(10.0);
    let num_w = width_of(canvas, "99", num) + 8.0;
    let old_x = x + 8.0 + num_w;
    let new_x = old_x + 6.0 + num_w;
    let code_x = new_x + 12.0;
    let code_room = x + w - 10.0 - code_x;
    let first = line_y + lead;
    let room = y + h - 10.0 - first;
    let max_rows = if room > 0.0 { (room / lead) as usize + 1 } else { 0 };

    for (i, r) in ROWS.iter().enumerate() {
        if i >= max_rows {
            break;
        }
        let ry = first + i as f32 * lead;
        let top = ry - lead * 0.68;
        f.say_row(Rect::new(x, top, w, lead), r.2, r.3);
        let (bed, stripe, ink) = match r.2 {
            Hunk => (Some(HUNK_BED), None, HUNK_INK),
            Add => (Some(ADD_BED), Some(ADD_INK), ADD_INK),
            Del => (Some(DEL_BED), Some(DEL_INK), DEL_INK),
            Same => (None, None, INK_DIM),
        };
        if let Some(c) = bed {
            fill(canvas, x + 1.0, top, w - 2.0, lead, c)?;
        }
        if let Some(c) = stripe {
            fill(canvas, x + 1.0, top, 2.5, lead, c)?;
        }
        if r.2 == Hunk {
            f.fit(r.3, x + 12.0, ry, code, ink, w - 24.0);
            continue;
        }
        let mut ob = [0u8; 12];
        let mut nb = [0u8; 12];
        if r.0 > 0 {
            text_right(canvas, uint(&mut ob, r.0), old_x, ry, num, INK_QUIET);
        }
        if r.1 > 0 {
            text_right(canvas, uint(&mut nb, r.1), new_x, ry, num, INK_QUIET);
        }
        f.fit(r.3, code_x, ry, code, ink, code_room);
    }
    Ok(())
}

/// `crates/policy/src/lib.rs` as (`crates/policy/src`, `lib.rs`).
fn split_path(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(i) => (path.get(..i).unwrap_or(""), path.get(i + 1..).unwrap_or(path)),
        None => ("", path),
    }
}

// ---- widget scaffolding -----------------------------------------------------

fn stack_root() -> types::WidgetNode {
    types::WidgetNode {
        id: ROOT_ID,
        parent: None,
        kind: types::WidgetKind::Stack,
        label: None,
        role: None,
        style: types::Style { width: None, height: None, grow: 1.0, padding: 0.0, text: None, place: None, box_: None },
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
        style: types::Style { width: None, height: None, grow: 1.0, padding: 0.0, text: None, place: None, box_: None },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

struct Component;

impl krate::Guest for Component {
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

        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");

        let _ = draw(canvas, win);

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
                Some(types::Event::Resized(_)) | Some(types::Event::ThemeChanged(_)) => {
                    let _ = draw(canvas, win);
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

krate::export!(Component);
