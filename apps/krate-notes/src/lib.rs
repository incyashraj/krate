//! Krate Notes -- a notes app, drawn on a canvas.
//!
//! The Apple-Notes shape: a list of note cards (title, date, snippet) and the
//! selected note as real type -- first line as the title, the rest as body
//! text with a caret. Tap a note to open it, type to write, "New note" to add
//! one. Everything persists as plain text files in the one directory the user
//! granted (`./notes/`), same paths and format as every earlier version of
//! this app.
//!
//! It is laid out from the canvas it really has, every frame. A desktop
//! window, a tablet or a phone turned sideways shows the list beside the
//! note. A phone held upright shows one at a time, the way a phone's own
//! notes app does: the list, then the note full screen with a way back, and
//! the on-screen keyboard comes up when a note opens. Colours follow light
//! and dark mode, every font size follows the person's text size, the body
//! wraps by the width the host really draws it at, and each frame tells the
//! screen reader what it shows.
//!
//! `#![no_std]` keeps it `krate:*`-only: the SDK owns the allocator and a
//! trapping panic handler, so no path drags in the `wasi:*` import set. All
//! state is fixed-size; no `format!`, `unwrap`, or panicking index.

#![no_std]

extern crate alloc;

use krate::bindings::krate::fs::files::{self, OpenMode};
use krate::bindings::krate::io::{args, stdio};
use krate::gfx::{canvas2d, types as gfx};
use krate::layout::{Look, Rect, Said, Screen};
use krate::ui::{clipboard, events, text_entry, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

/// The window asked for on a desktop. Only a first size: every frame is laid
/// out from the canvas the app really has.
const W: f32 = 900.0;
const H: f32 = 640.0;
/// Below this width the list and the note cannot both be read, so they are
/// shown one at a time.
const SPLIT_MIN: f32 = 600.0;

/// Note slots. Fixed so no allocation is needed.
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

const QUICK_WAIT_ROUNDS: u32 = 30;
const WAIT_ROUND_MILLIS: u32 = 50;

// ------------------------------------------------------------------
// Colours, for light and dark
// ------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Theme {
    dark: bool,
    /// The note being written on.
    paper_top: gfx::Color,
    paper_bot: gfx::Color,
    /// Behind the list.
    side: gfx::Color,
    row: gfx::Color,
    row_picked: gfx::Color,
    edge: gfx::Color,
    divider: gfx::Color,
    ink: gfx::Color,
    body: gfx::Color,
    sec: gfx::Color,
    quiet: gfx::Color,
    accent: gfx::Color,
    on_accent: gfx::Color,
}

const fn hex(v: u32) -> gfx::Color {
    gfx::Color {
        r: ((v >> 16) & 0xff) as f32 / 255.0,
        g: ((v >> 8) & 0xff) as f32 / 255.0,
        b: (v & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

fn theme(look: &Look) -> Theme {
    let contrast = look.high_contrast;
    if look.dark {
        Theme {
            dark: true,
            paper_top: hex(0x0B0E15),
            paper_bot: hex(0x0B0E15),
            side: hex(0x080A0F),
            row: hex(0x121723),
            // Accent at ~18% over the list ground, pre-blended.
            row_picked: hex(0x172742),
            edge: hex(0x1A202C),
            divider: if contrast {
                hex(0x4A5568)
            } else {
                hex(0x232A38)
            },
            ink: hex(0xF2F5FA),
            body: if contrast {
                hex(0xF0F2F6)
            } else {
                hex(0xD6DDE8)
            },
            sec: if contrast {
                hex(0xDCE1E8)
            } else {
                hex(0x9AA5B5)
            },
            quiet: if contrast {
                hex(0xBCC4CF)
            } else {
                hex(0x6B7686)
            },
            accent: hex(0x4C8DFF),
            on_accent: hex(0xFFFFFF),
        }
    } else {
        Theme {
            dark: false,
            paper_top: hex(0xFFFFFF),
            paper_bot: hex(0xFFFFFF),
            side: hex(0xF3F4F7),
            row: hex(0xFFFFFF),
            row_picked: hex(0xE4ECFC),
            edge: hex(0xE4E7EC),
            divider: if contrast {
                hex(0x9AA3B2)
            } else {
                hex(0xE2E5EA)
            },
            ink: hex(0x12151C),
            body: if contrast {
                hex(0x12151C)
            } else {
                hex(0x2C313A)
            },
            sec: if contrast {
                hex(0x2E3540)
            } else {
                hex(0x5B6472)
            },
            quiet: if contrast {
                hex(0x454D59)
            } else {
                hex(0x8A93A1)
            },
            accent: hex(0x2F6FEB),
            on_accent: hex(0xFFFFFF),
        }
    }
}

// ------------------------------------------------------------------
// Note buffers -- fixed-capacity, panic-free
// ------------------------------------------------------------------

#[derive(Clone, Copy)]
struct NoteBuf {
    bytes: [u8; CAP],
    len: usize,
}

impl NoteBuf {
    const fn new() -> Self {
        Self {
            bytes: [0; CAP],
            len: 0,
        }
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
// Persistence -- same plain-text-per-file scheme as always
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

fn gfx_rect(r: Rect) -> gfx::Rect {
    gfx::Rect {
        x: r.x,
        y: r.y,
        width: r.w,
        height: r.h,
    }
}

fn radii(r: f32) -> gfx::CornerRadii {
    gfx::CornerRadii {
        top_left: r,
        top_right: r,
        bottom_right: r,
        bottom_left: r,
    }
}

fn fill(canvas: u64, r: Rect, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_rect(canvas, gfx_rect(r), c)
}

fn rounded(canvas: u64, r: Rect, radius: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_round_rect(canvas, gfx_rect(r), radii(radius), c)
}

fn style(size: f32, weight: u16) -> gfx::TextStyle {
    gfx::TextStyle {
        weight,
        italic: false,
        letter_spacing: if size >= 24.0 { -0.4 } else { 0.0 },
        family: gfx::FontFamily::Sans,
    }
}

fn draw_text(
    canvas: u64,
    text: &str,
    x: f32,
    y: f32,
    size: f32,
    weight: u16,
    c: gfx::Color,
) -> Result<(), gfx::GfxError> {
    canvas2d::draw_text_styled(
        canvas,
        text,
        gfx::Point { x, y },
        size,
        c,
        style(size, weight),
    )
}

/// Rendered width of a string, measured by the host with the same font
/// layout `draw_text` draws with.
fn text_width(canvas: u64, s: &str, size: f32, weight: u16) -> f32 {
    canvas2d::measure_text_styled(canvas, s, size, style(size, weight)).map_or(0.0, |m| m.width)
}

/// Draw `s` at most `room` wide: the longest leading part that fits, then an
/// ellipsis. Returns the width drawn.
#[allow(clippy::too_many_arguments)]
fn fit(
    canvas: u64,
    s: &str,
    x: f32,
    y: f32,
    size: f32,
    weight: u16,
    c: gfx::Color,
    room: f32,
) -> Result<f32, gfx::GfxError> {
    if room <= 0.0 {
        return Ok(0.0);
    }
    let full = text_width(canvas, s, size, weight);
    if full <= room {
        draw_text(canvas, s, x, y, size, weight, c)?;
        return Ok(full);
    }
    let ell = text_width(canvas, "…", size, weight);
    let (mut lo, mut hi) = (0usize, s.len());
    while lo < hi {
        let mut mid = (lo + hi).div_ceil(2);
        while mid > lo && !s.is_char_boundary(mid) {
            mid -= 1;
        }
        if mid == lo {
            break;
        }
        if text_width(canvas, s.get(..mid).unwrap_or(""), size, weight) + ell <= room {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let part = s.get(..lo).unwrap_or("").trim_end();
    if part.is_empty() {
        return Ok(0.0);
    }
    draw_text(canvas, part, x, y, size, weight, c)?;
    let pw = text_width(canvas, part, size, weight);
    draw_text(canvas, "…", x + pw, y, size, weight, c)?;
    Ok(pw + ell)
}

fn push_byte(buf: &mut [u8], pos: &mut usize, byte: u8) {
    if let Some(slot) = buf.get_mut(*pos) {
        *slot = byte;
        *pos += 1;
    }
}

// ------------------------------------------------------------------
// Sidebar snippet -- body collapsed onto one line
// ------------------------------------------------------------------

/// Copy the body into `buf` with newlines and runs of spaces collapsed to a
/// single space. `fit` cuts it to the room there is when it is drawn.
fn snippet<'a>(body: &str, buf: &'a mut [u8; 96]) -> &'a str {
    let mut pos = 0usize;
    let mut last_space = true; // swallow leading whitespace
    for byte in body.as_bytes() {
        if pos >= buf.len() {
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
    let slice = buf.get(..pos).unwrap_or(&[]);
    core::str::from_utf8(slice).unwrap_or("").trim_end()
}

// ------------------------------------------------------------------
// Body wrap -- greedy word wrap, measured
// ------------------------------------------------------------------

/// One wrapped line of the body: a byte range of it.
#[derive(Clone, Copy)]
struct Span {
    start: usize,
    end: usize,
}

/// Up to this many wrapped lines are laid out; a full note is about thirty.
const MAX_LINES: usize = 96;

/// Wrap the body into lines at most `room` wide, measured with the face it
/// is drawn in. Returns how many lines were filled in `out`. A trailing
/// newline leaves an empty last line, which is where the caret goes.
fn wrap_body(canvas: u64, body: &str, size: f32, room: f32, out: &mut [Span; MAX_LINES]) -> usize {
    let bytes = body.as_bytes();
    let mut n = 0usize;
    let mut start = 0usize;
    let mut push = |s: usize, e: usize, n: &mut usize| {
        if let Some(slot) = out.get_mut(*n) {
            *slot = Span { start: s, end: e };
            *n += 1;
        }
    };
    loop {
        // The paragraph: up to the next newline.
        let para_end = bytes
            .get(start..)
            .and_then(|rest| rest.iter().position(|b| *b == b'\n'))
            .map_or(bytes.len(), |p| start + p);
        // Break it into lines that fit, at spaces where it can.
        let mut line_start = start;
        loop {
            let rest = body.get(line_start..para_end).unwrap_or("");
            if text_width(canvas, rest, size, 400) <= room {
                push(line_start, para_end, &mut n);
                break;
            }
            // The longest run of whole words that fits.
            let mut cut = None;
            let mut i = line_start;
            while i < para_end {
                if bytes.get(i) == Some(&b' ') {
                    if text_width(canvas, body.get(line_start..i).unwrap_or(""), size, 400) <= room
                    {
                        cut = Some(i);
                    } else {
                        break;
                    }
                }
                i += 1;
            }
            match cut {
                Some(c) if c > line_start => {
                    push(line_start, c, &mut n);
                    line_start = c + 1;
                }
                _ => {
                    // One word longer than the line: cut it by letters.
                    let mut e = line_start + 1;
                    while e < para_end
                        && text_width(canvas, body.get(line_start..e + 1).unwrap_or(""), size, 400)
                            <= room
                    {
                        e += 1;
                    }
                    push(line_start, e, &mut n);
                    line_start = e;
                }
            }
            if n >= MAX_LINES || line_start >= para_end {
                break;
            }
        }
        if para_end >= bytes.len() || n >= MAX_LINES {
            break;
        }
        start = para_end + 1;
        if start == bytes.len() {
            // A trailing newline: an empty line for the caret.
            push(start, start, &mut n);
            break;
        }
    }
    n
}

// ------------------------------------------------------------------
// The view: what is shown, and where the last frame drew what can be tapped
// ------------------------------------------------------------------

struct View {
    selected: usize,
    /// On a phone: the note is open full screen, not the list.
    open: bool,
    list_scroll: f32,
    list_max: f32,
    rows: [Option<Rect>; SLOTS],
    new_btn: Option<Rect>,
    back: Option<Rect>,
    editor: Option<Rect>,
    /// The last frame was drawn one at a time (a phone held upright).
    single: bool,
}

/// What a tap did.
enum Tap {
    Nothing,
    Pick(usize),
    New,
    Back,
    Editor,
}

impl View {
    fn tap(&self, x: f32, y: f32) -> Tap {
        if self.back.is_some_and(|r| r.contains(x, y)) {
            return Tap::Back;
        }
        if self.new_btn.is_some_and(|r| r.contains(x, y)) {
            return Tap::New;
        }
        if let Some(i) = self
            .rows
            .iter()
            .position(|r| r.is_some_and(|r| r.contains(x, y)))
        {
            return Tap::Pick(i);
        }
        if self.editor.is_some_and(|r| r.contains(x, y)) {
            return Tap::Editor;
        }
        Tap::Nothing
    }
}

// ------------------------------------------------------------------
// The frame
// ------------------------------------------------------------------

struct Sizes {
    title: f32,
    count: f32,
    row_title: f32,
    row_sub: f32,
    note_title: f32,
    body: f32,
    small: f32,
    tap: f32,
}

fn draw_frame(
    canvas: u64,
    win: u64,
    notes: &[NoteBuf; SLOTS],
    dates: &[&str; SLOTS],
    live: usize,
    view: &mut View,
) -> Result<(), gfx::GfxError> {
    let size = canvas2d::canvas_size(canvas).unwrap_or(gfx::Size {
        width: W,
        height: H,
    });
    let (w, h) = (size.width.max(1.0), size.height.max(1.0));
    let screen = Screen::of(win);
    let look = screen.look;
    let t = theme(&look);
    let mut said = Said::new();
    view.rows = [None; SLOTS];
    view.new_btn = None;
    view.back = None;
    view.editor = None;

    let single = w < SPLIT_MIN;
    view.single = single;
    let short = h < 500.0;
    let z = Sizes {
        title: look.text(if short { 22.0 } else { 28.0 }),
        count: look.text(13.0),
        row_title: look.text(if single { 17.0 } else { 16.0 }),
        row_sub: look.text(if single { 14.0 } else { 13.0 }),
        note_title: look.text(if single { 26.0 } else { 28.0 }),
        body: look.text(if single { 17.0 } else { 16.0 }),
        small: look.text(13.0),
        tap: screen.tap(),
    };
    let (top, right, bottom, left) = screen.safe;
    let full = Rect::new(0.0, 0.0, w, h);
    let safe = full.inset(top, right, bottom, left);

    if single {
        if view.open {
            canvas2d::linear_gradient(canvas, gfx_rect(full), t.paper_top, t.paper_bot)?;
            let mut area = safe.pad(20.0);
            area.y = safe.y + 8.0;
            area.h = safe.bottom() - area.y - 12.0;
            // The bar: the way back to the list, and when it was edited.
            let label = "‹ Notes";
            let bar = area.take_top((z.body * 2.4).max(z.tap));
            let lw = text_width(canvas, label, z.body, 500);
            let back = Rect::new(bar.x - 8.0, bar.y, lw + 24.0, bar.h);
            draw_text(
                canvas,
                label,
                bar.x,
                bar.y + bar.h * 0.5 + z.body * 0.36,
                z.body,
                500,
                t.accent,
            )?;
            said.button(back, "Back to notes");
            view.back = Some(back);
            let date = dates.get(view.selected).copied().unwrap_or("Today");
            let mut dbuf = [0u8; 32];
            let edited = edited_label(date, &mut dbuf);
            let ew = text_width(canvas, edited, z.small, 400);
            if bar.right() - ew > back.right() + 8.0 {
                draw_text(
                    canvas,
                    edited,
                    bar.right() - ew,
                    bar.y + bar.h * 0.5 + z.small * 0.36,
                    z.small,
                    400,
                    t.quiet,
                )?;
                said.text(Rect::new(bar.right() - ew, bar.y, ew, bar.h), edited);
            }
            area.take_top(10.0);
            draw_note(canvas, area, notes, view, &z, &t, &mut said)?;
        } else {
            canvas2d::linear_gradient(canvas, gfx_rect(full), t.side, t.side)?;
            let area = safe.inset(8.0, 16.0, 12.0, 16.0);
            draw_list(
                canvas, area, notes, dates, live, view, &z, &t, false, &mut said,
            )?;
        }
    } else {
        // Side by side: the list keeps a readable width that grows with the
        // text, and the note takes the rest.
        let side_w = (w * 0.32)
            .clamp(240.0, 340.0 * look.text_scale.clamp(1.0, 1.3))
            .min(w * 0.45);
        let side = Rect::new(0.0, 0.0, left + side_w, h);
        canvas2d::linear_gradient(canvas, gfx_rect(full), t.paper_top, t.paper_bot)?;
        fill(canvas, side, t.side)?;
        fill(canvas, Rect::new(side.right(), 0.0, 1.0, h), t.divider)?;
        let pad = if short { 12.0 } else { 16.0 };
        let list_area = Rect::new(
            left + pad,
            top + if short { 6.0 } else { 18.0 },
            side_w - pad * 2.0,
            0.0,
        );
        let list_area = Rect {
            h: h - bottom - pad - list_area.y,
            ..list_area
        };
        draw_list(
            canvas, list_area, notes, dates, live, view, &z, &t, true, &mut said,
        )?;

        // The note, with when it was edited above it.
        let ed = Rect::new(
            side.right() + 1.0,
            top,
            w - right - side.right() - 1.0,
            h - top - bottom,
        );
        let date = dates.get(view.selected).copied().unwrap_or("Today");
        let mut dbuf = [0u8; 32];
        let edited = edited_label(date, &mut dbuf);
        let bar_h = (z.small * 3.2).max(z.tap + 8.0);
        let ew = text_width(canvas, edited, z.small, 400);
        let by = ed.y + bar_h * 0.5 + z.small * 0.36;
        draw_text(
            canvas,
            edited,
            ed.x + (ed.w - ew) * 0.5,
            by,
            z.small,
            400,
            t.quiet,
        )?;
        said.text(Rect::new(ed.x + (ed.w - ew) * 0.5, ed.y, ew, bar_h), edited);
        fill(
            canvas,
            Rect::new(ed.x, ed.y + bar_h, w - ed.x, 1.0),
            t.divider,
        )?;
        let margin: f32 = if ed.w > 700.0 {
            56.0
        } else if short {
            24.0
        } else {
            36.0
        };
        let mut area = Rect::new(ed.x, ed.y + bar_h + 1.0, ed.w, ed.h - bar_h - 1.0)
            .pad(margin.min(ed.w * 0.08).max(20.0));
        // Keep a comfortable measure on a wide window.
        let measure = 680.0 * look.text_scale.clamp(1.0, 1.4);
        if area.w > measure {
            area.x += (area.w - measure) * 0.5;
            area.w = measure;
        }
        area.y = ed.y + bar_h + if short { 14.0 } else { 28.0 };
        area.h = ed.bottom() - area.y - 16.0;
        draw_note(canvas, area, notes, view, &z, &t, &mut said)?;
    }

    canvas2d::present(canvas)?;
    said.send(win);
    Ok(())
}

/// The list: a header with the count, one card per note, and the button
/// that makes a new one. Rows scroll when they do not all fit.
#[allow(clippy::too_many_arguments)]
fn draw_list(
    canvas: u64,
    area: Rect,
    notes: &[NoteBuf; SLOTS],
    dates: &[&str; SLOTS],
    live: usize,
    view: &mut View,
    z: &Sizes,
    t: &Theme,
    highlight: bool,
    said: &mut Said,
) -> Result<(), gfx::GfxError> {
    let mut area = area;
    let head = area.take_top(z.title * 1.25 + z.count * 1.9 + 10.0);
    let ty = head.y + z.title * 0.95;
    draw_text(canvas, "Notes", head.x + 4.0, ty, z.title, 700, t.ink)?;
    said.heading(Rect::new(head.x, head.y, head.w, z.title * 1.2), "Notes");
    let mut cbuf = [0u8; 16];
    let count_txt = count_label(live, &mut cbuf);
    let cy = ty + z.count * 1.6;
    draw_text(canvas, count_txt, head.x + 5.0, cy, z.count, 400, t.quiet)?;
    said.text(
        Rect::new(head.x, cy - z.count, head.w, z.count * 1.4),
        count_txt,
    );

    // The new-note button sits at the foot, always in reach of a thumb.
    let btn_h = (z.row_title * 2.6).max(z.tap + 4.0);
    let btn = area.take_bottom(btn_h);
    area.take_bottom(10.0);
    let full = live >= SLOTS;
    let label = if full {
        "Seven notes is the most"
    } else {
        "+  New note"
    };
    if full {
        canvas2d::stroke_round_rect(canvas, gfx_rect(btn), radii(12.0), 1.5, t.divider)?;
    } else {
        rounded(canvas, btn, 12.0, t.accent)?;
    }
    let lw = text_width(canvas, label, z.row_title, 600).min(btn.w - 16.0);
    let ink = if full { t.quiet } else { t.on_accent };
    fit(
        canvas,
        label,
        btn.x + (btn.w - lw) * 0.5,
        btn.y + btn.h * 0.5 + z.row_title * 0.36,
        z.row_title,
        600,
        ink,
        btn.w - 16.0,
    )?;
    if full {
        said.button(btn, "New note").disabled();
    } else {
        said.button(btn, "New note");
        view.new_btn = Some(btn);
    }

    // The rows.
    let row_h = (z.row_title * 1.35 + z.row_sub * 1.5 + 22.0).max(z.tap + 12.0);
    let gap = 8.0;
    let content = live as f32 * (row_h + gap) - gap;
    view.list_max = (content - area.h).max(0.0);
    view.list_scroll = view.list_scroll.clamp(0.0, view.list_max);
    let clip = view.list_max > 0.0;
    if clip {
        canvas2d::set_clip(canvas, area.x - 4.0, area.y, area.w + 8.0, area.h)?;
    }
    for i in 0..live {
        let y = area.y + i as f32 * (row_h + gap) - view.list_scroll;
        if y + row_h < area.y || y > area.bottom() {
            continue;
        }
        let row = Rect::new(area.x, y, area.w, row_h);
        let picked = highlight && i == view.selected;
        rounded(canvas, row, 12.0, if picked { t.row_picked } else { t.row })?;
        if !t.dark && !picked {
            canvas2d::stroke_round_rect(canvas, gfx_rect(row), radii(12.0), 1.0, t.edge)?;
        }
        if picked {
            // The accent bar on the row's left edge.
            rounded(
                canvas,
                Rect::new(row.x + 5.0, row.y + 12.0, 3.0, row.h - 24.0),
                1.5,
                t.accent,
            )?;
        }
        let note = notes.get(i).copied().unwrap_or(NoteBuf::new());
        let text = note.as_str();
        let title = first_line(text);
        let tx = row.x + 18.0;
        let room = row.right() - 14.0 - tx;
        let block = z.row_title + z.row_sub * 1.5;
        let ty = row.y + (row.h - block) * 0.5 + z.row_title * 0.9;
        let shown = if title.is_empty() { "New Note" } else { title };
        fit(
            canvas,
            shown,
            tx,
            ty,
            z.row_title,
            600,
            if title.is_empty() { t.quiet } else { t.ink },
            room,
        )?;
        // Second line: the date, then the start of the body.
        let date = dates.get(i).copied().unwrap_or("Today");
        let sy = ty + z.row_sub * 1.5;
        let dw = fit(canvas, date, tx, sy, z.row_sub, 500, t.sec, room)?;
        let mut sbuf = [0u8; 96];
        let snip = snippet(body_text(text), &mut sbuf);
        let snip = if snip.is_empty() {
            "No additional text"
        } else {
            snip
        };
        fit(
            canvas,
            snip,
            tx + dw + 10.0,
            sy,
            z.row_sub,
            400,
            t.quiet,
            room - dw - 10.0,
        )?;

        let top = y.max(area.y);
        let seen = Rect::new(row.x, top, row.w, (y + row_h).min(area.bottom()) - top);
        if let Some(slot) = view.rows.get_mut(i) {
            *slot = Some(seen);
        }
        said.button(seen, shown);
    }
    if clip {
        canvas2d::clear_clip(canvas)?;
    }
    Ok(())
}

/// The selected note: its first line as the title, the rest wrapped as
/// body text, and the caret at the end of what was typed.
fn draw_note(
    canvas: u64,
    area: Rect,
    notes: &[NoteBuf; SLOTS],
    view: &mut View,
    z: &Sizes,
    t: &Theme,
    said: &mut Said,
) -> Result<(), gfx::GfxError> {
    view.editor = Some(area);
    let note = notes.get(view.selected).copied().unwrap_or(NoteBuf::new());
    let text = note.as_str();
    let title = first_line(text);
    let title_base = area.y + z.note_title * 0.95;
    let body_lh = z.body * 1.5;
    let body_top = title_base + z.note_title * 0.45 + z.body * 0.6;
    let (caret_x, caret_base, caret_h);
    if text.is_empty() {
        draw_text(
            canvas,
            "New Note",
            area.x,
            title_base,
            z.note_title,
            700,
            t.quiet,
        )?;
        caret_x = area.x;
        caret_base = title_base;
        caret_h = z.note_title;
    } else {
        // A title longer than the line is cut rather than run off the page.
        let tw = fit(
            canvas,
            title,
            area.x,
            title_base,
            z.note_title,
            700,
            t.ink,
            area.w,
        )?;
        let has_break = text.len() > title.len();
        if !has_break {
            // Still typing the first line: the caret rides the title.
            caret_x = area.x + tw;
            caret_base = title_base;
            caret_h = z.note_title;
        } else {
            let body = body_text(text);
            let mut lines = [Span { start: 0, end: 0 }; MAX_LINES];
            let n = if body.is_empty() {
                0
            } else {
                wrap_body(canvas, body, z.body, area.w, &mut lines)
            };
            // The caret is always at the end, so when the body is longer
            // than the page, the end is what is shown.
            let fits = (((area.bottom() - body_top) / body_lh).max(1.0)) as usize;
            let first = n.saturating_sub(fits);
            canvas2d::set_clip(
                canvas,
                area.x - 2.0,
                body_top - 2.0,
                area.w + 6.0,
                area.bottom() - body_top + 4.0,
            )?;
            let mut last_w = 0.0;
            let mut last_base = body_top + z.body;
            for (k, span) in lines.iter().take(n).enumerate().skip(first) {
                let base = body_top + z.body + (k - first) as f32 * body_lh;
                let line = body.get(span.start..span.end).unwrap_or("");
                if !line.is_empty() {
                    draw_text(canvas, line, area.x, base, z.body, 400, t.body)?;
                }
                last_w = text_width(canvas, line, z.body, 400);
                last_base = base;
            }
            canvas2d::clear_clip(canvas)?;
            caret_x = area.x + last_w;
            caret_base = last_base;
            caret_h = z.body;
        }
    }
    // The caret: a slim accent bar on the last baseline.
    fill(
        canvas,
        Rect::new(
            caret_x + 1.0,
            caret_base - caret_h * 0.85,
            2.0,
            caret_h * 1.05,
        ),
        t.accent,
    )?;
    said.field(area, "Note", text);
    Ok(())
}

/// "3 notes" (or "1 note").
fn count_label(live: usize, buf: &mut [u8; 16]) -> &str {
    let mut pos = 0usize;
    let n = live.min(99);
    if n >= 10 {
        push_byte(buf, &mut pos, b'0' + (n / 10) as u8);
    }
    push_byte(buf, &mut pos, b'0' + (n % 10) as u8);
    for byte in if n == 1 {
        b" note".as_slice()
    } else {
        b" notes".as_slice()
    } {
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
// Widget tree -- one canvas filling the window
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

// ------------------------------------------------------------------
// Entry point
// ------------------------------------------------------------------

struct Component;

impl krate::Guest for Component {
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
        let mut saved_any = false;
        let mut dirty = false;

        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");

        if quick {
            // The store screenshot: three realistic notes, first selected,
            // saved to disk to prove the write path.
            for i in 0..INITIAL_NOTE_COUNT {
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
            }
        } else {
            // Load what exists; reveal any notes beyond the initial three that
            // an earlier session created.
            let mut found_any = false;
            for i in 0..INITIAL_NOTE_COUNT {
                if let Some(buf) = notes.get_mut(i) {
                    if load_note(i, buf) && !buf.is_empty() {
                        found_any = true;
                    }
                }
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
                for j in 0..INITIAL_NOTE_COUNT {
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
                }
            }
        }

        let mut view = View {
            selected: 0,
            open: false,
            list_scroll: 0.0,
            list_max: 0.0,
            rows: [None; SLOTS],
            new_btn: None,
            back: None,
            editor: None,
            single: false,
        };
        if draw_frame(canvas, win, &notes, &dates, live, &mut view).is_err() {
            let _ = window::close(win);
            return 34;
        }

        // Saving the note being edited before anything else is shown, so a
        // tap never loses work.
        let save_current =
            |notes: &[NoteBuf; SLOTS], selected: usize, dirty: &mut bool, saved_any: &mut bool| {
                if *dirty {
                    if save_note(selected, notes.get(selected).unwrap_or(&NoteBuf::new())) {
                        *saved_any = true;
                    }
                    *dirty = false;
                }
            };

        // A real session ends when the person closes the window (K-092).
        let rounds = if quick { QUICK_WAIT_ROUNDS } else { u32::MAX };
        let mut round = 0u32;
        'session: while round < rounds {
            round += 1;
            // Take everything already queued, then draw once.
            let mut redraw = false;
            let mut next = events::wait(Some(WAIT_ROUND_MILLIS));
            while let Some(event) = next {
                match event {
                    types::Event::CloseRequested(_) => break 'session,
                    // A new size is a new layout; a new theme, new text size.
                    types::Event::Resized(_)
                    | types::Event::ThemeChanged(_)
                    | types::Event::RedrawRequested(_) => redraw = true,
                    types::Event::Wheel(wh) => {
                        let next = (view.list_scroll + wh.dy).clamp(0.0, view.list_max);
                        redraw |= next != view.list_scroll;
                        view.list_scroll = next;
                    }
                    // On the release, so a finger that drags the list does
                    // not open the note it started on.
                    types::Event::Pointer(p) if p.button.is_some() && !p.pressed => {
                        match view.tap(p.x, p.y) {
                            Tap::Pick(index) => {
                                if index != view.selected {
                                    save_current(&notes, view.selected, &mut dirty, &mut saved_any);
                                    view.selected = index;
                                }
                                if view.single {
                                    view.open = true;
                                    let _ = text_entry::start(win);
                                }
                                redraw = true;
                            }
                            Tap::New if live < SLOTS => {
                                save_current(&notes, view.selected, &mut dirty, &mut saved_any);
                                view.selected = live;
                                live += 1;
                                if let Some(buf) = notes.get_mut(view.selected) {
                                    buf.clear();
                                }
                                if let Some(slot) = dates.get_mut(view.selected) {
                                    *slot = "Today";
                                }
                                view.open = true;
                                let _ = text_entry::start(win);
                                redraw = true;
                            }
                            Tap::Back => {
                                save_current(&notes, view.selected, &mut dirty, &mut saved_any);
                                view.open = false;
                                text_entry::stop(win);
                                redraw = true;
                            }
                            // A tap on the note brings the keyboard back.
                            Tap::Editor => {
                                let _ = text_entry::start(win);
                            }
                            _ => {}
                        }
                    }
                    types::Event::TextInput(text) => {
                        if view.single && !view.open {
                            // Typing on the list opens the selected note.
                            view.open = true;
                        }
                        if let Some(buf) = notes.get_mut(view.selected) {
                            buf.push_str(&text);
                            dirty = true;
                            redraw = true;
                        }
                    }
                    types::Event::Key(key) if key.pressed => {
                        let chord = key.modifiers.control || key.modifiers.meta;
                        let k = key.key.as_bytes();
                        if chord {
                            match k {
                                b"s" | b"S" => {
                                    if save_note(
                                        view.selected,
                                        notes.get(view.selected).unwrap_or(&NoteBuf::new()),
                                    ) {
                                        saved_any = true;
                                        dirty = false;
                                    }
                                }
                                b"v" | b"V" => {
                                    if let Ok(text) = clipboard::read_text() {
                                        if !text.is_empty() {
                                            if let Some(buf) = notes.get_mut(view.selected) {
                                                buf.push_str(&text);
                                                dirty = true;
                                                redraw = true;
                                            }
                                        }
                                    }
                                }
                                b"c" | b"C" => {
                                    if let Some(buf) = notes.get(view.selected) {
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
                                    if let Some(buf) = notes.get_mut(view.selected) {
                                        if !buf.is_empty() {
                                            buf.pop();
                                            dirty = true;
                                            redraw = true;
                                        }
                                    }
                                }
                                b"Enter" | b"Return" => {
                                    if let Some(buf) = notes.get_mut(view.selected) {
                                        buf.push(b'\n');
                                        dirty = true;
                                        redraw = true;
                                    }
                                }
                                // On a phone, Escape is the way back too.
                                b"Escape" if view.single && view.open => {
                                    save_current(&notes, view.selected, &mut dirty, &mut saved_any);
                                    view.open = false;
                                    text_entry::stop(win);
                                    redraw = true;
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
                next = events::poll();
            }
            if redraw {
                let _ = draw_frame(canvas, win, &notes, &dates, live, &mut view);
            }
        }

        // Never lose the note being edited on the way out -- but also never
        // erase a file by writing an empty buffer over it.
        if dirty {
            if let Some(buf) = notes.get(view.selected) {
                if !buf.is_empty() && save_note(view.selected, buf) {
                    saved_any = true;
                }
            }
        }

        let _ = window::close(win);

        let out = stdio::stdout();
        let _ = out.write(b"note:");
        if let Some(buf) = notes.get(view.selected) {
            let title = first_line(buf.as_str());
            let cut = title.get(..title.len().min(40)).unwrap_or("");
            let _ = out.write(cut.as_bytes());
        }
        let _ = out.write(b"\n");
        if saved_any {
            let _ = out.write(b"saved:yes\n");
        }

        // Closing the window is how a person quits an app, not a failure.
        // This returned 2 for it, so the demo the website hands a stranger
        // reported failure every time it was used correctly: headless, where
        // the host closes the window, and equally for anyone who clicked the
        // X. Anything wrapping the app -- a script, a shell `&&`, the
        // cold-install gate in CI -- saw a program that had failed (K-181).
        0
    }
}

krate::export!(Component);
