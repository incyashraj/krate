//! The permission wall as a trusted guest: the sheet a phone shows before
//! any app runs.
//!
//! The player passes one argument per field: the app's name, then three
//! arguments for every request -- `cap`, `rationale`, `required` ("1" or
//! "0"). The person taps rows to allow or deny (required rows stay on -- the
//! app says it cannot run without them), then Open or Cancel. The decision
//! goes to stdout as one line: `wall:open:<cap,cap,...>` or `wall:cancel`.
//! The player owns both sides of that pipe, so this sheet cannot be lied to
//! and cannot lie.
//!
//! It used to be one argument with the fields joined by `\x1e` and `\x1c`,
//! and nothing stopped a manifest string from containing those characters.
//! An app declaring only `ui.window` could name itself
//! `Notes\x1enet.connect\x1cSync your notes\x1c1` and this sheet would draw a
//! network request it had never declared, under a name still reading "Notes".
//! A crafted rationale was worse: it could flip its own row from required to
//! optional, and required rows are the ones a person cannot switch off.
//!
//! Arguments cannot be forged that way. The runtime refuses any argument
//! containing a newline or a NUL, so the boundaries between them are real,
//! which is why the structure is carried by the argument list rather than by
//! characters inside one string.
//!
//! `quick` renders one frame of a sample wall and answers `wall:open` with
//! everything granted, so check-app can verify the drawing path headless.
//!
//! `wall:notice`, a title and a message is the player saying why something
//! did not open (a missing file, a failed download, an app that stopped):
//! one card and a Close button, answered with `wall:closed`. It never
//! answers `wall:open`, so an app named "wall:notice" gains nothing.
//!
//! `wall:output`, then the app's name, how it ended, and every line it
//! printed, is what a command-line app showed: a phone has no terminal, so
//! one that printed and ended used to flash and vanish. A scrolling card in
//! a fixed-width font and a Close button, answered with `wall:closed`.
//!
//! `wall:home`, then a name and a line about it for every app the person
//! has, is the phone player's home screen: a tap answers `wall:pick:<n>`,
//! leaving answers `wall:closed`. It too never answers `wall:open` -- the
//! permission question is asked after the pick, every time.

#![no_std]

extern crate alloc;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use krate::bindings::krate::fs::files;
use krate::bindings::krate::io::{args, stdio};
use krate::bindings::krate::time::clock;
use krate::gfx::{canvas2d, canvas_images, types as gfx};
use krate::motion::Spring;
use krate::ui::{events, tree, types, window};

/// The screen the person came from, as the player left it: a small picture
/// (an 8-byte header of width and height, little-endian, then RGBA),
/// stretched across the screen behind the sheet so it reads as that screen
/// blurred, the way the phone's own sheets sit over the app they came from.
fn behind(canvas: u64) -> Option<u64> {
    let size = files::stat("behind.rgba").ok()?.size as usize;
    if !(8..=8 + 512 * 512 * 4).contains(&size) {
        return None;
    }
    let file = files::open("behind.rgba", files::OpenMode::Read).ok()?;
    let mut bytes = Vec::with_capacity(size);
    while bytes.len() < size {
        let chunk = file.read((size - bytes.len()) as u32).ok()?;
        if chunk.is_empty() {
            break;
        }
        bytes.extend_from_slice(&chunk);
    }
    let w = u32::from_le_bytes(bytes.get(0..4)?.try_into().ok()?);
    let h = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?);
    let px = bytes.get(8..)?;
    if w == 0 || h == 0 || px.len() != (w * h * 4) as usize {
        return None;
    }
    canvas_images::keep(canvas, w, h, px).ok()
}

/// Krate's file icon, 180 pixels square, straight RGBA: what an app without
/// an icon of its own wears on the sheet (F-314).
const FILE_ICON: &[u8] = include_bytes!("krate-file-180.rgba");
const FILE_ICON_SIDE: u32 = 180;

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;
/// Arguments arrive newline-joined, and the runtime refuses an argument that
/// contains one -- so this split cannot be steered by any manifest string.
const ARG_SEP: char = '\n';

/// The first argument when the player has something to tell, not ask.
const NOTICE: &str = "wall:notice";
/// The first argument for what a command-line app printed.
const OUTPUT: &str = "wall:output";
/// The first argument for the phone player's home screen.
const HOME: &str = "wall:home";
/// First argument of a permission question that carries a line about the
/// app after its name.
const ASK: &str = "wall:ask";

/// Name, then three fields per request.
const FIELDS_PER_ASK: usize = 3;

fn color(r: f32, g: f32, b: f32, a: f32) -> gfx::Color {
    gfx::Color { r, g, b, a }
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> gfx::Rect {
    gfx::Rect {
        x,
        y,
        width,
        height,
    }
}

fn radii(all: f32) -> gfx::CornerRadii {
    gfx::CornerRadii {
        top_left: all,
        top_right: all,
        bottom_right: all,
        bottom_left: all,
    }
}

fn style(weight: u16, spacing: f32) -> gfx::TextStyle {
    gfx::TextStyle {
        weight,
        italic: false,
        letter_spacing: spacing,
        family: gfx::FontFamily::Sans,
    }
}

fn out(line: &str) {
    let handle = stdio::stdout();
    let _ = handle.write(line.as_bytes());
    let _ = handle.write(b"\n");
}

struct Ask {
    cap: String,
    rationale: String,
    required: bool,
    granted: bool,
}

/// Read the argument list: name, then `cap`, `rationale`, `required` per ask.
///
/// A trailing group that is not whole is dropped rather than guessed at. The
/// wall is the one screen where a person decides what to trust, so a request
/// it cannot read completely is a request it does not draw.
fn parse_input(raw: &str) -> (String, String, Vec<Ask>) {
    let mut args: Vec<&str> = raw.split(ARG_SEP).collect();
    // `wall:ask`, the name, a line about the app (its version and size),
    // then the requests. The line is its own argument, never part of the
    // name, so a name cannot forge it.
    let detail = if args.first() == Some(&ASK) && args.len() >= 3 {
        let detail = args[2].trim().to_string();
        // What follows is the old shape: the name, then the requests.
        args.remove(2);
        args.remove(0);
        detail
    } else {
        String::new()
    };
    let name = args
        .first()
        .copied()
        .unwrap_or("This app")
        .trim()
        .to_string();

    let mut asks = Vec::new();
    let rest = args.get(1..).unwrap_or(&[]);
    for group in rest.chunks(FIELDS_PER_ASK) {
        if group.len() < FIELDS_PER_ASK {
            break;
        }
        let cap = group[0].trim();
        if cap.is_empty() {
            continue;
        }
        asks.push(Ask {
            cap: cap.to_string(),
            rationale: group[1].trim().to_string(),
            required: group[2].trim() == "1",
            granted: true,
        });
    }
    (name, detail, asks)
}

/// Word-wrap for the rationale lines: measure-based, honest at any width.
fn wrap_lines(canvas: u64, text: &str, size: f32, max_w: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        let fits = canvas2d::measure_text(canvas, &candidate, size)
            .map(|m| m.width <= max_w)
            .unwrap_or(true);
        if fits {
            current = candidate;
        } else {
            if !current.is_empty() {
                lines.push(current);
            }
            current = word.to_string();
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

struct Layout {
    w: f32,
    h: f32,
    col_x: f32,
    col_w: f32,
    /// Points at each edge something else covers (top, right, bottom,
    /// left): a phone's notch, home bar and rounded corners.
    safe: (f32, f32, f32, f32),
}

impl Layout {
    fn from_canvas(w: f32, h: f32) -> Self {
        let col_w = w.min(480.0);
        Layout {
            w,
            h,
            col_x: (w - col_w) / 2.0,
            col_w,
            safe: (0.0, 0.0, 0.0, 0.0),
        }
    }
}

/// Per-frame row geometry, computed while drawing and reused for taps so
/// the two can never disagree.
struct RowHit {
    index: usize,
    y0: f32,
    y1: f32,
}

/// What a finger is on.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Target {
    Open,
    Cancel,
    Close,
    Row(usize),
    /// The card's "See details" link.
    Details,
    /// The dimmed screen around the sheet: a tap there does nothing.
    Outside,
    /// The sheet itself, away from anything that acts.
    Sheet,
}

/// One finger down on the sheet: where, on what, and whether it has moved
/// far enough to be a scroll rather than a tap.
struct Press {
    target: Target,
    x: f32,
    y: f32,
    last_y: f32,
    moved: bool,
}

/// How far a finger may move and still tap, in points.
const TAP_SLOP: f32 = 10.0;

struct Sheet {
    name: String,
    detail: String,
    asks: Vec<Ask>,
    /// Each switch's knob, 0 off to 1 on, sprung toward its state.
    knobs: Vec<Spring>,
    scroll: f32,
    max_scroll: f32,
    entrance: Spring,
    press: Option<Press>,
    rows: Vec<RowHit>,
    /// Where the rows are shown (they scroll inside it).
    rows_area: gfx::Rect,
    sheet_area: gfx::Rect,
    open_button: gfx::Rect,
    cancel_button: gfx::Rect,
    close_button: gfx::Rect,
    /// The card's "See details" link, and whether the details are shown.
    details_button: gfx::Rect,
    details: bool,
    /// The sheet's window, to describe it to a screen reader.
    window: u64,
    /// Krate's file icon, kept on the canvas once.
    file_icon: Option<u64>,
    /// The screen the person came from, small, to blur behind the sheet.
    behind: Option<u64>,
}

/// What a request is, said the way a person says it, with the colour of its
/// tile.
fn describe(cap: &str) -> (String, gfx::Color) {
    let mut parts = cap.splitn(2, ':');
    let kind = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");
    let host = target.split(':').next().unwrap_or("");
    let blue = color(0.0, 0.48, 1.0, 1.0);
    let orange = color(1.0, 0.58, 0.0, 1.0);
    let pink = color(1.0, 0.18, 0.33, 1.0);
    let green = color(0.20, 0.78, 0.35, 1.0);
    let indigo = color(0.35, 0.34, 0.84, 1.0);
    let red = color(1.0, 0.23, 0.19, 1.0);
    let purple = color(0.69, 0.32, 0.87, 1.0);
    let gray = color(0.56, 0.56, 0.58, 1.0);
    let teal = color(0.19, 0.69, 0.78, 1.0);
    let own = target.is_empty() || target.starts_with("./") || target.starts_with('.');
    let (title, tint) = match kind {
        "net.fetch" | "net.connect" if !host.is_empty() && host != "*" => {
            (format!("Internet \u{00b7} {host}"), blue)
        }
        "net.fetch" | "net.connect" => ("Internet".to_string(), blue),
        "ui.window" => ("Its own window".to_string(), gray),
        "net.listen" => ("Local network".to_string(), blue),
        "net.ws" => ("Live connection".to_string(), blue),
        "hub" | "hub.rooms" => ("Krate online".to_string(), blue),
        "fs.read" | "fs.list" if own => ("Read its own files".to_string(), orange),
        "fs.write" | "fs.remove" if own => ("Save its own files".to_string(), orange),
        "fs.read" | "fs.list" => ("Read files".to_string(), orange),
        "fs.write" | "fs.remove" => ("Change files".to_string(), orange),
        "ui.dialog" if target.contains("save") => ("Save files you pick".to_string(), orange),
        "ui.dialog" => ("Files you pick".to_string(), orange),
        "audio.playback" => ("Sound".to_string(), pink),
        "audio.capture" => ("Microphone".to_string(), red),
        "speech" | "speech.say" | "speech.listen" => ("Speech".to_string(), pink),
        "camera.capture" | "camera" => ("Camera".to_string(), gray),
        "location.read" => ("Where you are".to_string(), blue),
        "bluetooth.connect" => ("Bluetooth devices you pick".to_string(), blue),
        "photos.add" => ("Add to your Photos".to_string(), orange),
        "store.kv" | "store.sql" => ("Keep its data".to_string(), green),
        "store.shared" => ("Share data with people you invite".to_string(), green),
        "store.secret" => ("Keep secrets safe".to_string(), green),
        "ui.clipboard" => ("Clipboard".to_string(), indigo),
        "ui.notify" => ("Notifications".to_string(), red),
        "ui.open-url" => ("Open links".to_string(), indigo),
        "gfx.gpu" => ("Graphics chip".to_string(), purple),
        "memory.budget" if !target.is_empty() => (format!("Memory \u{00b7} {target} MB"), teal),
        _ => (String::from(cap), gray),
    };
    (title, tint)
}

/// A small white drawing for a kind of request, centred at (x, y), for its
/// coloured tile.
fn ask_icon(canvas: u64, cap: &str, x: f32, y: f32) -> Result<(), gfx::GfxError> {
    let ink = color(1.0, 1.0, 1.0, 1.0);
    let w = 1.8;
    let bar = |x0: f32, y0: f32, bw: f32, bh: f32| {
        canvas2d::fill_round_rect(canvas, rect(x0, y0, bw, bh), radii(bw.min(bh) / 2.0), ink)
    };
    let kind = cap.split(':').next().unwrap_or("");
    let p = |x: f32, y: f32| gfx::Point { x, y };
    match kind {
        k if k.starts_with("ui.dialog") || k.starts_with("fs") => {
            // A folder.
            canvas2d::fill_round_rect(canvas, rect(x - 8.5, y - 7.0, 7.5, 4.0), radii(1.5), ink)?;
            canvas2d::fill_round_rect(canvas, rect(x - 8.5, y - 4.5, 17.0, 12.0), radii(2.5), ink)?;
        }
        "audio.playback" | "speech" | "speech.say" | "speech.listen" => {
            // A speaker.
            bar(x - 8.0, y - 3.0, 4.5, 6.0)?;
            canvas2d::fill_round_rect(canvas, rect(x - 5.0, y - 6.0, 4.0, 12.0), radii(1.0), ink)?;
            canvas2d::stroke_arc(canvas, p(x - 0.5, y), 4.5, -55.0, 110.0, w, ink)?;
            canvas2d::stroke_arc(canvas, p(x - 0.5, y), 8.5, -55.0, 110.0, w, ink)?;
        }
        "audio.capture" => {
            // A microphone.
            canvas2d::fill_round_rect(canvas, rect(x - 3.5, y - 8.5, 7.0, 11.5), radii(3.5), ink)?;
            canvas2d::stroke_arc(canvas, p(x, y - 1.0), 6.5, 0.0, 180.0, w, ink)?;
            bar(x - 0.9, y + 5.0, 1.8, 3.5)?;
            bar(x - 4.0, y + 7.6, 8.0, 1.8)?;
        }
        k if k.starts_with("store") => {
            // A stack of saved things.
            for i in 0..3 {
                let yy = y - 7.0 + i as f32 * 5.5;
                canvas2d::fill_round_rect(canvas, rect(x - 8.0, yy, 16.0, 4.0), radii(2.0), ink)?;
            }
        }
        k if k.starts_with("net") || k.starts_with("hub") => {
            // A globe.
            canvas2d::stroke_circle(canvas, p(x, y), 8.0, w, ink)?;
            bar(x - 8.0, y - 0.9, 16.0, w)?;
            canvas2d::stroke_round_rect(
                canvas,
                rect(x - 3.6, y - 8.0, 7.2, 16.0),
                radii(3.6),
                w,
                ink,
            )?;
        }
        "photos.add" => {
            // A picture: a frame, a sun and a hill.
            canvas2d::stroke_round_rect(
                canvas,
                rect(x - 9.0, y - 7.0, 18.0, 14.0),
                radii(3.0),
                w,
                ink,
            )?;
            canvas2d::fill_circle(canvas, p(x - 3.5, y - 2.5), 1.8, ink)?;
            for k in 0..9 {
                let t = k as f32 / 8.0;
                let hx = x - 7.0 + t * 14.0;
                let hy = y + 5.0 - (1.0 - (2.0 * t - 1.2).abs()).max(0.0) * 6.0;
                canvas2d::fill_circle(canvas, p(hx, hy), 1.3, ink)?;
            }
        }
        k if k.starts_with("camera") => {
            canvas2d::fill_round_rect(canvas, rect(x - 9.0, y - 5.5, 18.0, 13.0), radii(3.0), ink)?;
            canvas2d::fill_round_rect(canvas, rect(x - 3.5, y - 8.0, 7.0, 3.5), radii(1.2), ink)?;
            canvas2d::fill_circle(canvas, p(x, y + 1.0), 4.2, color(0.56, 0.56, 0.58, 1.0))?;
            canvas2d::fill_circle(canvas, p(x, y + 1.0), 2.6, ink)?;
        }
        "bluetooth.connect" => {
            // The Bluetooth rune: a stem, and two strokes crossing it to
            // the arrowheads on the right.
            let seg = |x0: f32, y0: f32, x1: f32, y1: f32| -> Result<(), gfx::GfxError> {
                for i in 0..=10 {
                    let t = i as f32 / 10.0;
                    canvas2d::fill_circle(
                        canvas,
                        p(x0 + (x1 - x0) * t, y0 + (y1 - y0) * t),
                        0.95,
                        ink,
                    )?;
                }
                Ok(())
            };
            seg(x, y - 8.0, x, y + 8.0)?;
            seg(x, y - 8.0, x + 5.0, y - 4.0)?;
            seg(x + 5.0, y - 4.0, x - 5.0, y + 4.0)?;
            seg(x, y + 8.0, x + 5.0, y + 4.0)?;
            seg(x + 5.0, y + 4.0, x - 5.0, y - 4.0)?;
        }
        "location.read" => {
            // A map pin: a round head over a point.
            canvas2d::fill_circle(canvas, p(x, y - 2.5), 6.5, ink)?;
            for i in 0..6 {
                let t = i as f32;
                canvas2d::fill_circle(canvas, p(x, y + 2.0 + t * 1.2), 3.8 - t * 0.6, ink)?;
            }
            canvas2d::fill_circle(canvas, p(x, y - 2.5), 2.4, color(0.0, 0.48, 1.0, 1.0))?;
        }
        "ui.notify" => {
            // A bell.
            canvas2d::fill_round_rect(canvas, rect(x - 6.0, y - 7.0, 12.0, 12.0), radii(6.0), ink)?;
            bar(x - 8.0, y + 2.5, 16.0, 2.5)?;
            canvas2d::fill_circle(canvas, p(x, y + 7.0), 2.2, ink)?;
        }
        "ui.clipboard" => {
            canvas2d::stroke_round_rect(
                canvas,
                rect(x - 6.5, y - 6.5, 13.0, 15.0),
                radii(2.5),
                w,
                ink,
            )?;
            bar(x - 3.5, y - 9.0, 7.0, 4.0)?;
            bar(x - 3.5, y - 1.0, 7.0, 1.6)?;
            bar(x - 3.5, y + 2.8, 7.0, 1.6)?;
        }
        "gfx.gpu" | "memory.budget" => {
            // A chip.
            canvas2d::stroke_round_rect(
                canvas,
                rect(x - 6.0, y - 6.0, 12.0, 12.0),
                radii(2.0),
                w,
                ink,
            )?;
            canvas2d::fill_round_rect(canvas, rect(x - 2.5, y - 2.5, 5.0, 5.0), radii(1.0), ink)?;
            for i in 0..3 {
                let o = -3.5 + i as f32 * 3.5;
                bar(x + o - 0.8, y - 9.5, 1.6, 3.0)?;
                bar(x + o - 0.8, y + 6.5, 1.6, 3.0)?;
                bar(x - 9.5, y + o - 0.8, 3.0, 1.6)?;
                bar(x + 6.5, y + o - 0.8, 3.0, 1.6)?;
            }
        }
        _ => {
            // A window: everything else an app asks for on its own screen.
            canvas2d::stroke_round_rect(
                canvas,
                rect(x - 8.5, y - 7.0, 17.0, 14.0),
                radii(3.0),
                w,
                ink,
            )?;
            bar(x - 8.5, y - 3.5, 17.0, w)?;
        }
    }
    Ok(())
}

/// `text` cut to `max_w` with an ellipsis, so a long name never runs under
/// the close button.
fn fit_line(canvas: u64, text: &str, size: f32, st: gfx::TextStyle, max_w: f32) -> String {
    let width = |t: &str| {
        canvas2d::measure_text_styled(canvas, t, size, st)
            .map(|m| m.width)
            .unwrap_or(0.0)
    };
    if width(text) <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut n = chars.len();
    while n > 0 {
        let candidate: String =
            chars[..n].iter().collect::<String>().trim_end().to_string() + "\u{2026}";
        if width(&candidate) <= max_w {
            return candidate;
        }
        n -= 1;
    }
    "\u{2026}".to_string()
}

/// At most `max` lines of wrapped text, the last ending in an ellipsis when
/// there was more.
fn wrap_at_most(canvas: u64, text: &str, size: f32, max_w: f32, max: usize) -> Vec<String> {
    let mut lines = wrap_lines(canvas, text, size, max_w);
    if lines.len() > max {
        let rest = lines[max - 1..].join(" ");
        lines.truncate(max - 1);
        lines.push(fit_line(canvas, &rest, size, style(450, 0.0), max_w));
    }
    lines
}

fn inside(r: gfx::Rect, x: f32, y: f32) -> bool {
    x >= r.x && x <= r.x + r.width && y >= r.y && y <= r.y + r.height
}

fn shade(c: gfx::Color, k: f32) -> gfx::Color {
    color(c.r * k, c.g * k, c.b * k, c.a)
}

/// The app's own colour, picked from its name: the same name always gets
/// the same colour, the way the home screen picks it.
fn name_hue(name: &str) -> gfx::Color {
    let seed = name
        .bytes()
        .fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    let hues = [
        color(0.30, 0.38, 0.96, 1.0),
        color(0.93, 0.36, 0.40, 1.0),
        color(0.10, 0.62, 0.58, 1.0),
        color(0.52, 0.34, 0.92, 1.0),
        color(0.96, 0.56, 0.22, 1.0),
    ];
    hues[(seed as usize) % hues.len()]
}

/// Measured rows: each request's title, its reason's lines, its height.
struct RowText {
    title: String,
    tint: gfx::Color,
    lines: Vec<String>,
    h: f32,
}

impl Sheet {
    fn new(name: String, detail: String, asks: Vec<Ask>) -> Self {
        let knobs = asks
            .iter()
            .map(|ask| Spring::rest_at(if ask.granted { 1.0 } else { 0.0 }, 30.0))
            .collect();
        let zero = rect(0.0, 0.0, 0.0, 0.0);
        Sheet {
            name,
            detail,
            asks,
            knobs,
            scroll: 0.0,
            max_scroll: 0.0,
            entrance: Spring::rest_at(0.0, 26.0),
            press: None,
            rows: Vec::new(),
            rows_area: zero,
            sheet_area: zero,
            open_button: zero,
            cancel_button: zero,
            close_button: zero,
            details_button: zero,
            details: false,
            window: 0,
            file_icon: None,
            behind: None,
        }
    }

    /// Whether anything is still moving (the sheet sliding in, a switch).
    fn animating(&self) -> bool {
        !self.entrance.settled(1.0)
            || self
                .asks
                .iter()
                .zip(&self.knobs)
                .any(|(ask, knob)| !knob.settled(if ask.granted { 1.0 } else { 0.0 }))
    }

    fn tick(&mut self, dt: f32) {
        self.entrance.tick(1.0, dt);
        for (ask, knob) in self.asks.iter().zip(self.knobs.iter_mut()) {
            knob.tick(if ask.granted { 1.0 } else { 0.0 }, dt);
        }
    }

    fn pressed(&self, target: Target) -> bool {
        self.press
            .as_ref()
            .is_some_and(|p| p.target == target && !p.moved)
    }

    /// The sheet: the app (its mark, name, version and size), what Krate
    /// promises about it, then one grouped card of what it asks to use --
    /// a coloured tile, what it is, the app's own reason, and a switch for
    /// what it can do without -- then Open and Not Now. A bottom sheet on a
    /// phone held upright; a centred card of two columns on a phone turned
    /// sideways; a centred card on anything wider.
    fn draw(&mut self, canvas: u64, l: &Layout) -> Result<(), gfx::GfxError> {
        if self.details {
            self.draw_details(canvas, l)
        } else {
            self.draw_compact(canvas, l)
        }
    }

    /// What every app has and the sheet does not ask about: its window, its
    /// output, the clock and randomness.
    fn worth_asking(cap: &str) -> bool {
        let kind = cap.split(':').next().unwrap_or("");
        !matches!(
            kind,
            "ui.window"
                | "io.stdout"
                | "io.stderr"
                | "io.stdin"
                | "io.args"
                | "io.env"
                | "time.clock"
                | "time.monotonic"
                | "random.bytes"
                | "memory.budget"
        )
    }

    /// The first thing a person sees (F-314): a small card, not a page.
    /// The app's mark and name, one row of tiles and one line for what it
    /// uses, a link to the details, and two buttons. The details -- every
    /// request with its reason and the switches for what is optional -- are
    /// one tap away.
    fn draw_compact(&mut self, canvas: u64, l: &Layout) -> Result<(), gfx::GfxError> {
        let ink = color(0.07, 0.07, 0.09, 1.0);
        let muted = color(0.45, 0.46, 0.50, 1.0);
        let accent = color(0.23, 0.30, 0.95, 1.0);
        let hue = name_hue(&self.name);
        let e = self.entrance.value.clamp(0.0, 1.2);

        // Behind the card: the screen the person came from, blurred, dimmed
        // a little as the card arrives; without one, light lit faintly in
        // the app's colour.
        match self.behind {
            Some(behind) => {
                canvas2d::clear(canvas, color(0.0, 0.0, 0.0, 1.0))?;
                canvas_images::draw(canvas, behind, rect(0.0, 0.0, l.w, l.h))?;
                canvas2d::fill_rect(
                    canvas,
                    rect(0.0, 0.0, l.w, l.h),
                    color(0.0, 0.0, 0.0, 0.22 * e.min(1.0)),
                )?;
            }
            None => {
                // A wash of the app's colour from the top, fading by the
                // middle. It was a radial glow across the whole screen: on a
                // phone's CPU painter that one call was most of every frame
                // (164 ms of 236 on a Galaxy S22 Ultra), so the sheet rose in
                // three or four steps. A vertical wash paints by rows and
                // costs a few milliseconds (K-1212).
                canvas2d::clear(canvas, color(0.90, 0.905, 0.92, 1.0))?;
                canvas2d::linear_gradient(
                    canvas,
                    rect(0.0, 0.0, l.w, l.h * 0.62),
                    color(hue.r, hue.g, hue.b, 0.26 * e.min(1.0)),
                    color(hue.r, hue.g, hue.b, 0.0),
                )?;
            }
        }

        let (top, right, bottom, left) = l.safe;
        let pad = 24.0;
        let cw = (l.w - left - right - 24.0).min(420.0);
        let cx = left + (l.w - left - right - cw) / 2.0;
        let inner = cw - pad * 2.0;

        // What it uses, said plainly.
        let shown: Vec<usize> = (0..self.asks.len())
            .filter(|&i| Self::worth_asking(&self.asks[i].cap))
            .collect();
        let mut names: Vec<String> = Vec::new();
        for &i in &shown {
            let (title, _) = describe(&self.asks[i].cap);
            let title = title.split(" \u{00b7} ").next().unwrap_or("").to_string();
            if !names.contains(&title) {
                names.push(title);
            }
        }
        // As many of them by name as fit on one line, then how many more.
        let said = |named: usize| -> String {
            let n = names.len();
            match (n, named) {
                (0, _) => String::from("Nothing beyond its own window."),
                (1, _) => format!("Uses {}.", names[0]),
                (_, k) if k >= n => {
                    format!("Uses {} and {}.", names[..n - 1].join(", "), names[n - 1])
                }
                (_, k) => format!("Uses {} and {} more.", names[..k].join(", "), n - k),
            }
        };
        let mut summary = said(names.len());
        for named in (1..=names.len().min(3)).rev() {
            summary = said(named);
            let fits = canvas2d::measure_text_styled(canvas, &summary, 15.0, style(500, 0.0))
                .map(|m| m.width <= inner)
                .unwrap_or(true);
            if fits {
                break;
            }
        }
        if names.len() <= 3 {
            let all = said(names.len());
            if canvas2d::measure_text_styled(canvas, &all, 15.0, style(500, 0.0))
                .map(|m| m.width <= inner)
                .unwrap_or(true)
            {
                summary = all;
            }
        }
        let summary_lines = wrap_at_most(canvas, &summary, 15.0, inner, 2);
        let title = format!("Open {}?", self.name);
        let title_lines = wrap_at_most(canvas, &title, 22.0, inner - 40.0, 2);
        let tiles = shown.len().min(6) + usize::from(shown.len() > 6);

        // Measure, then place the card at the bottom (centred on a wide or
        // turned screen).
        let tile = 36.0;
        let mut h = pad + 60.0 + 16.0 + title_lines.len() as f32 * 28.0 + 22.0;
        if tiles > 0 {
            h += 18.0 + tile;
        }
        h += 14.0 + summary_lines.len() as f32 * 21.0 + 10.0 + 20.0 + 22.0 + 52.0 + pad;
        let upright = l.h >= l.w || l.h >= 520.0;
        let foot = bottom.max(10.0) + 8.0;
        let slide = (1.0 - e) * (h + foot + 40.0);
        let cy = if upright && l.w < 600.0 {
            l.h - foot - h + slide
        } else {
            (top + (l.h - top - bottom - h) / 2.0).max(top + 8.0) + slide * 0.35
        };
        let card = rect(cx, cy, cw, h);
        self.sheet_area = card;
        canvas2d::drop_shadow_round_rect(
            canvas,
            card,
            radii(30.0),
            36.0,
            color(0.0, 0.0, 0.0, 0.18),
        )?;
        canvas2d::fill_round_rect(canvas, card, radii(30.0), color(1.0, 1.0, 1.0, 1.0))?;

        // Close, top right: a cross in a soft circle; it means Not now.
        let (xx, xy) = (cx + cw - 20.0 - 14.0, cy + 20.0 + 14.0);
        let close_fill = if self.pressed(Target::Close) {
            color(0.86, 0.86, 0.89, 1.0)
        } else {
            color(0.94, 0.94, 0.96, 1.0)
        };
        canvas2d::fill_circle(canvas, gfx::Point { x: xx, y: xy }, 14.0, close_fill)?;
        for k in 0..9 {
            let d = (k as f32 - 4.0) * 0.9;
            canvas2d::fill_circle(
                canvas,
                gfx::Point {
                    x: xx + d,
                    y: xy + d,
                },
                1.0,
                muted,
            )?;
            canvas2d::fill_circle(
                canvas,
                gfx::Point {
                    x: xx + d,
                    y: xy - d,
                },
                1.0,
                muted,
            )?;
        }
        self.close_button = rect(xx - 22.0, xy - 22.0, 44.0, 44.0);

        // The app's mark, centred.
        let mid = cx + cw / 2.0;
        let mut y = cy + pad;
        let mark = rect(mid - 30.0, y, 60.0, 60.0);
        if let Some(icon) = self.file_icon {
            canvas_images::draw(canvas, icon, mark)?;
        } else {
            canvas2d::drop_shadow_round_rect(
                canvas,
                mark,
                radii(15.0),
                10.0,
                color(hue.r, hue.g, hue.b, 0.35),
            )?;
            canvas2d::fill_round_rect(canvas, mark, radii(15.0), hue)?;
            let initial: String = self.name.chars().take(1).collect::<String>().to_uppercase();
            let im = canvas2d::measure_text_styled(canvas, &initial, 28.0, style(800, 0.0))?;
            canvas2d::draw_text_styled(
                canvas,
                &initial,
                gfx::Point {
                    x: mid - im.width / 2.0,
                    y: y + 40.0,
                },
                28.0,
                color(1.0, 1.0, 1.0, 1.0),
                style(800, 0.0),
            )?;
        }
        y += 60.0 + 16.0;

        // "Open Phone Check?", and its version and size.
        for line in &title_lines {
            let m = canvas2d::measure_text_styled(canvas, line, 22.0, style(750, -0.4))?;
            canvas2d::draw_text_styled(
                canvas,
                line,
                gfx::Point {
                    x: mid - m.width / 2.0,
                    y: y + 22.0,
                },
                22.0,
                ink,
                style(750, -0.4),
            )?;
            y += 28.0;
        }
        let detail = if self.detail.is_empty() {
            String::from("Krate app")
        } else {
            self.detail.clone()
        };
        let detail = fit_line(canvas, &detail, 14.0, style(500, 0.0), inner);
        let dm = canvas2d::measure_text_styled(canvas, &detail, 14.0, style(500, 0.0))?;
        canvas2d::draw_text_styled(
            canvas,
            &detail,
            gfx::Point {
                x: mid - dm.width / 2.0,
                y: y + 14.0,
            },
            14.0,
            muted,
            style(500, 0.0),
        )?;
        y += 22.0;

        // One row of tiles for what it uses.
        if tiles > 0 {
            y += 18.0;
            let gap = 8.0;
            let row_w = tiles as f32 * tile + (tiles - 1) as f32 * gap;
            let mut tx = mid - row_w / 2.0;
            for (n, &i) in shown.iter().take(6).enumerate() {
                let cap = &self.asks[i].cap;
                let (_, tint) = describe(cap);
                canvas2d::fill_round_rect(canvas, rect(tx, y, tile, tile), radii(10.0), tint)?;
                ask_icon(canvas, cap, tx + tile / 2.0, y + tile / 2.0)?;
                tx += tile + gap;
                let _ = n;
            }
            if shown.len() > 6 {
                canvas2d::fill_round_rect(
                    canvas,
                    rect(tx, y, tile, tile),
                    radii(10.0),
                    color(0.93, 0.93, 0.95, 1.0),
                )?;
                let more = format!("+{}", shown.len() - 6);
                let mm = canvas2d::measure_text_styled(canvas, &more, 13.0, style(700, 0.0))?;
                canvas2d::draw_text_styled(
                    canvas,
                    &more,
                    gfx::Point {
                        x: tx + (tile - mm.width) / 2.0,
                        y: y + 23.0,
                    },
                    13.0,
                    muted,
                    style(700, 0.0),
                )?;
            }
            y += tile;
        }

        // The same, in words.
        y += 14.0;
        for line in &summary_lines {
            let m = canvas2d::measure_text_styled(canvas, line, 15.0, style(500, 0.0))?;
            canvas2d::draw_text_styled(
                canvas,
                line,
                gfx::Point {
                    x: mid - m.width / 2.0,
                    y: y + 15.0,
                },
                15.0,
                muted,
                style(500, 0.0),
            )?;
            y += 21.0;
        }

        // See details.
        y += 10.0;
        let link = "See details";
        let lm = canvas2d::measure_text_styled(canvas, link, 14.0, style(600, 0.0))?;
        let link_ink = if self.pressed(Target::Details) {
            color(0.15, 0.20, 0.70, 1.0)
        } else {
            accent
        };
        canvas2d::draw_text_styled(
            canvas,
            link,
            gfx::Point {
                x: mid - lm.width / 2.0,
                y: y + 14.0,
            },
            14.0,
            link_ink,
            style(600, 0.0),
        )?;
        self.details_button = rect(mid - lm.width / 2.0 - 16.0, y - 10.0, lm.width + 32.0, 40.0);
        y += 20.0 + 22.0;

        // Not now and Open, side by side.
        let gap = 12.0;
        let bw = (inner - gap) / 2.0;
        let not_now = rect(cx + pad, y, bw, 52.0);
        let open = rect(cx + pad + bw + gap, y, bw, 52.0);
        let nn_fill = if self.pressed(Target::Cancel) {
            color(0.94, 0.94, 0.96, 1.0)
        } else {
            color(1.0, 1.0, 1.0, 1.0)
        };
        canvas2d::fill_round_rect(canvas, not_now, radii(26.0), nn_fill)?;
        canvas2d::stroke_round_rect(
            canvas,
            not_now,
            radii(26.0),
            1.2,
            color(0.85, 0.86, 0.89, 1.0),
        )?;
        let open_fill = if self.pressed(Target::Open) {
            color(0.22, 0.22, 0.26, 1.0)
        } else {
            ink
        };
        canvas2d::fill_round_rect(canvas, open, radii(26.0), open_fill)?;
        for (r, text, fill) in [
            (not_now, "Not now", ink),
            (open, "Open", color(1.0, 1.0, 1.0, 1.0)),
        ] {
            let m = canvas2d::measure_text_styled(canvas, text, 17.0, style(600, 0.0))?;
            canvas2d::draw_text_styled(
                canvas,
                text,
                gfx::Point {
                    x: r.x + (r.width - m.width) / 2.0,
                    y: r.y + 32.0,
                },
                17.0,
                fill,
                style(600, 0.0),
            )?;
        }
        self.cancel_button = not_now;
        self.open_button = open;
        self.rows_area = rect(0.0, 0.0, 0.0, 0.0);
        self.rows.clear();

        // What a screen reader says (IC-926).
        let mut said = krate::layout::Said::new();
        said.heading(krate::layout::Rect::new(cx, cy, cw, 120.0), &title);
        said.text(krate::layout::Rect::new(cx, cy + 120.0, cw, 40.0), &summary);
        said.button(
            krate::layout::Rect::new(
                self.details_button.x,
                self.details_button.y,
                self.details_button.width,
                self.details_button.height,
            ),
            link,
        );
        said.button(
            krate::layout::Rect::new(not_now.x, not_now.y, not_now.width, not_now.height),
            "Not now",
        );
        said.button(
            krate::layout::Rect::new(open.x, open.y, open.width, open.height),
            "Open",
        );
        said.send(self.window);

        canvas2d::present(canvas)
    }

    fn draw_details(&mut self, canvas: u64, l: &Layout) -> Result<(), gfx::GfxError> {
        let ink = color(0.07, 0.07, 0.09, 1.0);
        let muted = color(0.43, 0.44, 0.48, 1.0);
        let faint = color(0.60, 0.61, 0.65, 1.0);
        let accent = color(0.23, 0.30, 0.95, 1.0);
        let group = color(0.955, 0.957, 0.965, 1.0);
        let hue = name_hue(&self.name);
        let e = self.entrance.value.clamp(0.0, 1.2);

        // The screen behind: dark, lit faintly in the app's colour.
        canvas2d::clear(canvas, color(0.05, 0.055, 0.075, 1.0))?;
        // A wash, not a full-screen radial glow: see the sheet's backdrop.
        canvas2d::linear_gradient(
            canvas,
            rect(0.0, 0.0, l.w, l.h * 0.55),
            color(hue.r, hue.g, hue.b, 0.36 * e.min(1.0)),
            color(hue.r, hue.g, hue.b, 0.0),
        )?;

        let (top, right, bottom, left) = l.safe;
        let sideways = l.w > l.h && l.h < 520.0;
        let phone_upright = !sideways && l.w < 600.0;
        let pad = 24.0;

        // The sheet's frame.
        let (sx, sw) = if phone_upright {
            (0.0, l.w)
        } else if sideways {
            let sw = (l.w - left - right - 32.0).min(760.0);
            (left + (l.w - left - right - sw) / 2.0, sw)
        } else {
            let sw = (l.w - 32.0).min(460.0);
            ((l.w - sw) / 2.0, sw)
        };

        // Columns: one upright, two sideways (the app and the buttons on the
        // left, what it asks for on the right).
        let (head_x, head_w, list_x, list_w) = if sideways {
            let left_w = (sw * 0.42).max(260.0);
            (sx + pad, left_w - pad * 1.5, sx + left_w, sw - left_w - pad)
        } else {
            (sx + pad, sw - pad * 2.0, sx + pad, sw - pad * 2.0)
        };

        // Measure first, so the sheet is exactly as tall as what it says.
        let promise =
            format!(
            "{} runs sealed off from the rest of this device. It can use only what you allow here.",
            if self.name.chars().count() <= 24 { self.name.as_str() } else { "This app" }
        );
        let promise_lines = wrap_lines(canvas, &promise, 15.0, head_w);
        let tile = 32.0;
        let row_pad = 14.0;
        let text_x_off = row_pad + tile + 12.0;
        let mut texts = Vec::new();
        for ask in &self.asks {
            let (title, tint) = describe(&ask.cap);
            let right_w = if ask.required {
                canvas2d::measure_text_styled(canvas, "Required", 13.0, style(600, 0.0))?.width
                    + 8.0
            } else {
                51.0 + 8.0
            };
            let text_w = list_w - text_x_off - row_pad - right_w;
            let title = fit_line(canvas, &title, 16.0, style(600, -0.2), text_w);
            let lines = if ask.rationale.is_empty() {
                Vec::new()
            } else {
                wrap_at_most(canvas, &ask.rationale, 13.5, text_w, 3)
            };
            let h = (row_pad + 20.0 + lines.len() as f32 * 18.0 + row_pad - 2.0).max(60.0);
            texts.push(RowText {
                title,
                tint,
                lines,
                h,
            });
        }
        let rows_h: f32 = texts.iter().map(|t| t.h).sum();

        let head_h = 64.0 + 18.0 + promise_lines.len() as f32 * 21.0;
        let label_h = 30.0;
        let buttons_h = 54.0 + 6.0 + 44.0;
        let grabber_h = if phone_upright { 20.0 } else { pad };
        let foot = if phone_upright {
            bottom.max(12.0) + 4.0
        } else {
            pad - 8.0
        };

        // Room for the rows: all of them if they fit, else they scroll.
        let room = if phone_upright {
            l.h - top - 12.0
        } else {
            l.h - top.max(12.0) - bottom.max(12.0)
        };
        let (sheet_h, rows_shown) = if sideways {
            let left_h = grabber_h + head_h + 18.0 + buttons_h + foot;
            let right_needed = grabber_h + label_h + rows_h + foot;
            let sheet_h = left_h.max(right_needed).min(room);
            (
                sheet_h,
                (sheet_h - grabber_h - label_h - foot).max(60.0).min(rows_h),
            )
        } else {
            let fixed = grabber_h + head_h + 20.0 + label_h + 18.0 + buttons_h + foot;
            let shown = rows_h.min((room - fixed).max(60.0));
            ((fixed + shown).min(room.max(fixed + 60.0)), shown)
        };
        self.max_scroll = (rows_h - rows_shown).max(0.0);
        self.scroll = self.scroll.clamp(0.0, self.max_scroll);

        let slide = (1.0 - e) * (sheet_h + 40.0);
        let sy = if phone_upright {
            l.h - sheet_h + slide
        } else {
            (top + (l.h - top - bottom - sheet_h) / 2.0).max(top) + slide * 0.35
        };
        let sheet_radii = if phone_upright {
            gfx::CornerRadii {
                top_left: 28.0,
                top_right: 28.0,
                bottom_right: 0.0,
                bottom_left: 0.0,
            }
        } else {
            radii(28.0)
        };
        let frame = rect(
            sx,
            sy,
            sw,
            if phone_upright {
                sheet_h + 40.0
            } else {
                sheet_h
            },
        );
        self.sheet_area = frame;
        canvas2d::drop_shadow_round_rect(
            canvas,
            frame,
            sheet_radii,
            40.0,
            color(0.0, 0.0, 0.0, 0.35),
        )?;
        canvas2d::fill_round_rect(canvas, frame, sheet_radii, color(1.0, 1.0, 1.0, 1.0))?;

        if phone_upright {
            // The grabber: this is a sheet over the screen, not the screen.
            canvas2d::fill_round_rect(
                canvas,
                rect(sx + sw / 2.0 - 18.0, sy + 8.0, 36.0, 5.0),
                radii(2.5),
                color(0.84, 0.84, 0.87, 1.0),
            )?;
        }

        // Close, top right: a cross in a soft circle. Not sideways, where
        // the list fills that corner and Not Now sits right by the thumb.
        let (cx, cy) = (sx + sw - pad - 15.0 + 4.0, sy + grabber_h + 15.0);
        self.close_button = rect(0.0, 0.0, 0.0, 0.0);
        if !sideways {
            let close_fill = if self.pressed(Target::Close) {
                color(0.86, 0.86, 0.89, 1.0)
            } else {
                color(0.94, 0.94, 0.96, 1.0)
            };
            canvas2d::fill_circle(canvas, gfx::Point { x: cx, y: cy }, 15.0, close_fill)?;
            for k in 0..11 {
                let d = (k as f32 - 5.0) * 0.9;
                canvas2d::fill_circle(
                    canvas,
                    gfx::Point {
                        x: cx + d,
                        y: cy + d,
                    },
                    1.05,
                    muted,
                )?;
                canvas2d::fill_circle(
                    canvas,
                    gfx::Point {
                        x: cx + d,
                        y: cy - d,
                    },
                    1.05,
                    muted,
                )?;
            }
            self.close_button = rect(cx - 24.0, cy - 24.0, 48.0, 48.0);
        }

        // The app: its mark, its name, what it is.
        let hy = sy + grabber_h;
        let mark = rect(head_x, hy, 64.0, 64.0);
        if let Some(icon) = self.file_icon {
            canvas_images::draw(canvas, icon, mark)?;
        } else {
            canvas2d::drop_shadow_round_rect(
                canvas,
                mark,
                radii(16.0),
                10.0,
                color(hue.r, hue.g, hue.b, 0.35),
            )?;
            canvas2d::fill_round_rect(canvas, mark, radii(16.0), hue)?;
            canvas2d::fill_round_rect(
                canvas,
                rect(mark.x, mark.y, 64.0, 32.0),
                gfx::CornerRadii {
                    top_left: 16.0,
                    top_right: 16.0,
                    bottom_right: 0.0,
                    bottom_left: 0.0,
                },
                color(1.0, 1.0, 1.0, 0.10),
            )?;
            let initial: String = self.name.chars().take(1).collect::<String>().to_uppercase();
            let im = canvas2d::measure_text_styled(canvas, &initial, 30.0, style(800, 0.0))?;
            canvas2d::draw_text_styled(
                canvas,
                &initial,
                gfx::Point {
                    x: mark.x + 32.0 - im.width / 2.0,
                    y: mark.y + 43.0,
                },
                30.0,
                color(1.0, 1.0, 1.0, 1.0),
                style(800, 0.0),
            )?;
        }

        let name_x = head_x + 64.0 + 14.0;
        let name_w = if sideways {
            head_w - 78.0
        } else {
            (cx - 22.0) - name_x
        };
        let name = fit_line(canvas, &self.name, 21.0, style(750, -0.4), name_w);
        canvas2d::draw_text_styled(
            canvas,
            &name,
            gfx::Point {
                x: name_x,
                y: hy + 28.0,
            },
            21.0,
            ink,
            style(750, -0.4),
        )?;
        let detail = if self.detail.is_empty() {
            "Krate app".to_string()
        } else {
            self.detail.clone()
        };
        let detail = fit_line(canvas, &detail, 14.0, style(500, 0.0), name_w);
        canvas2d::draw_text_styled(
            canvas,
            &detail,
            gfx::Point {
                x: name_x,
                y: hy + 50.0,
            },
            14.0,
            muted,
            style(500, 0.0),
        )?;

        let mut y = hy + 64.0 + 26.0;
        for line in &promise_lines {
            canvas2d::draw_text_styled(
                canvas,
                line,
                gfx::Point { x: head_x, y },
                15.0,
                muted,
                style(450, 0.0),
            )?;
            y += 21.0;
        }

        // What it asks to use: one grouped card, scrolling inside itself
        // when there is more than fits.
        let (label_y, rows_x) = if sideways {
            (hy + 12.0, list_x)
        } else {
            (y + 14.0, list_x)
        };
        let count = self.asks.len();
        let label = if count == 1 {
            "IT ASKS FOR 1 THING".to_string()
        } else {
            format!("IT ASKS FOR {count} THINGS")
        };
        canvas2d::draw_text_styled(
            canvas,
            &label,
            gfx::Point {
                x: rows_x + 4.0,
                y: label_y,
            },
            12.0,
            faint,
            style(650, 0.8),
        )?;
        let rows_top = label_y + 12.0;
        let area = rect(rows_x, rows_top, list_w, rows_shown);
        self.rows_area = area;
        canvas2d::fill_round_rect(canvas, area, radii(16.0), group)?;
        canvas2d::set_clip(canvas, area.x, area.y, area.width, area.height)?;
        self.rows.clear();
        let mut ry = rows_top - self.scroll;
        for (index, t) in texts.iter().enumerate() {
            let ask = &self.asks[index];
            let y1 = ry + t.h;
            self.rows.push(RowHit { index, y0: ry, y1 });
            if ry < area.y + area.height && y1 > area.y {
                if self.pressed(Target::Row(index)) && !ask.required {
                    canvas2d::fill_rect(
                        canvas,
                        rect(area.x, ry, area.width, t.h),
                        color(0.90, 0.90, 0.92, 1.0),
                    )?;
                }
                let tile_y = ry + row_pad;
                let tile_r = rect(rows_x + row_pad, tile_y, tile, tile);
                let tint = if ask.granted {
                    t.tint
                } else {
                    color(0.74, 0.75, 0.78, 1.0)
                };
                canvas2d::fill_round_rect(canvas, tile_r, radii(8.0), tint)?;
                ask_icon(
                    canvas,
                    &ask.cap,
                    tile_r.x + tile / 2.0,
                    tile_r.y + tile / 2.0,
                )?;
                let tx = rows_x + text_x_off;
                canvas2d::draw_text_styled(
                    canvas,
                    &t.title,
                    gfx::Point {
                        x: tx,
                        y: ry + row_pad + 15.0,
                    },
                    16.0,
                    if ask.granted { ink } else { muted },
                    style(600, -0.2),
                )?;
                let mut ly = ry + row_pad + 15.0 + 19.0;
                for line in &t.lines {
                    canvas2d::draw_text_styled(
                        canvas,
                        line,
                        gfx::Point { x: tx, y: ly },
                        13.5,
                        muted,
                        style(450, 0.0),
                    )?;
                    ly += 18.0;
                }
                let mid = ry + row_pad + tile / 2.0;
                if ask.required {
                    // Required: the app cannot run without it, so no switch.
                    let tm =
                        canvas2d::measure_text_styled(canvas, "Required", 13.0, style(600, 0.0))?;
                    canvas2d::draw_text_styled(
                        canvas,
                        "Required",
                        gfx::Point {
                            x: rows_x + list_w - row_pad - tm.width,
                            y: mid + 4.5,
                        },
                        13.0,
                        faint,
                        style(600, 0.0),
                    )?;
                } else {
                    // An iOS-shaped switch, its knob sprung between ends.
                    let k = self.knobs[index].value.clamp(0.0, 1.0);
                    let pill = rect(rows_x + list_w - row_pad - 51.0, mid - 15.5, 51.0, 31.0);
                    let off = color(0.86, 0.86, 0.88, 1.0);
                    let on = color(0.20, 0.78, 0.35, 1.0);
                    let fill = color(
                        off.r + (on.r - off.r) * k,
                        off.g + (on.g - off.g) * k,
                        off.b + (on.b - off.b) * k,
                        1.0,
                    );
                    canvas2d::fill_round_rect(canvas, pill, radii(15.5), fill)?;
                    let knob_x = pill.x + 15.5 + k * 20.0;
                    canvas2d::fill_circle(
                        canvas,
                        gfx::Point {
                            x: knob_x,
                            y: mid + 1.0,
                        },
                        13.5,
                        color(0.0, 0.0, 0.0, 0.10),
                    )?;
                    canvas2d::fill_circle(
                        canvas,
                        gfx::Point { x: knob_x, y: mid },
                        13.5,
                        color(1.0, 1.0, 1.0, 1.0),
                    )?;
                }
                if index + 1 < count {
                    canvas2d::fill_rect(
                        canvas,
                        rect(tx, y1 - 0.5, rows_x + list_w - tx, 0.5),
                        color(0.0, 0.0, 0.0, 0.10),
                    )?;
                }
            }
            ry = y1;
        }
        canvas2d::clear_clip(canvas)?;
        if self.max_scroll > 0.0 {
            // More below (or above): a short fade at the cut edge.
            if self.scroll < self.max_scroll - 1.0 {
                canvas2d::fill_round_rect(
                    canvas,
                    rect(area.x, area.y + area.height - 22.0, area.width, 22.0),
                    gfx::CornerRadii {
                        top_left: 0.0,
                        top_right: 0.0,
                        bottom_right: 16.0,
                        bottom_left: 16.0,
                    },
                    color(group.r, group.g, group.b, 0.85),
                )?;
                let more = "More below";
                let mm = canvas2d::measure_text_styled(canvas, more, 12.0, style(600, 0.0))?;
                canvas2d::draw_text_styled(
                    canvas,
                    more,
                    gfx::Point {
                        x: area.x + (area.width - mm.width) / 2.0,
                        y: area.y + area.height - 7.0,
                    },
                    12.0,
                    faint,
                    style(600, 0.0),
                )?;
            }
        }

        // The choice.
        let buttons_y = if sideways {
            sy + sheet_h - foot - buttons_h
        } else {
            area.y + area.height + 18.0
        };
        let open = rect(head_x, buttons_y, head_w, 54.0);
        let open_fill = if self.pressed(Target::Open) {
            shade(accent, 0.82)
        } else {
            accent
        };
        canvas2d::fill_round_rect(canvas, open, radii(16.0), open_fill)?;
        let om = canvas2d::measure_text_styled(canvas, "Open", 17.0, style(650, 0.0))?;
        canvas2d::draw_text_styled(
            canvas,
            "Open",
            gfx::Point {
                x: open.x + (open.width - om.width) / 2.0,
                y: open.y + 33.0,
            },
            17.0,
            color(1.0, 1.0, 1.0, 1.0),
            style(650, 0.0),
        )?;
        self.open_button = open;
        let cancel = rect(head_x, open.y + 54.0 + 6.0, head_w, 44.0);
        let not_now = "Not Now";
        let nm = canvas2d::measure_text_styled(canvas, not_now, 16.0, style(600, 0.0))?;
        let nn_ink = if self.pressed(Target::Cancel) {
            color(accent.r, accent.g, accent.b, 0.45)
        } else {
            accent
        };
        canvas2d::draw_text_styled(
            canvas,
            not_now,
            gfx::Point {
                x: cancel.x + (cancel.width - nm.width) / 2.0,
                y: cancel.y + 27.0,
            },
            16.0,
            nn_ink,
            style(600, 0.0),
        )?;
        self.cancel_button = cancel;

        canvas2d::present(canvas)
    }

    /// What a point is on.
    fn target_at(&self, x: f32, y: f32) -> Target {
        if inside(self.open_button, x, y) {
            return Target::Open;
        }
        if inside(self.cancel_button, x, y) {
            return Target::Cancel;
        }
        if inside(self.close_button, x, y) {
            return Target::Close;
        }
        if !self.details && inside(self.details_button, x, y) {
            return Target::Details;
        }
        if inside(self.rows_area, x, y) {
            if let Some(row) = self.rows.iter().find(|r| y >= r.y0 && y < r.y1) {
                return Target::Row(row.index);
            }
        }
        if inside(self.sheet_area, x, y) {
            Target::Sheet
        } else {
            Target::Outside
        }
    }

    fn scroll_by(&mut self, dy: f32) {
        self.scroll = (self.scroll + dy).clamp(0.0, self.max_scroll);
    }

    /// Follow one event. A control acts when the finger lifts on the thing
    /// it went down on, and not if it moved far enough to be a scroll in
    /// between -- so a drag that starts on Open scrolls and never opens.
    /// Returns the decision once there is one.
    fn handle(&mut self, event: &types::Event) -> Option<bool> {
        match event {
            // Dismissed without answering is not consent: fail closed.
            types::Event::CloseRequested(_) => Some(false),
            types::Event::Wheel(wheel) => {
                if let Some(press) = self.press.as_mut() {
                    press.moved = true;
                }
                self.scroll_by(wheel.dy);
                None
            }
            types::Event::Pointer(p) if p.button.is_some() && p.pressed => {
                // Nothing on a sheet still sliding in is where it will be: a
                // quick tap where Open is about to arrive landed on the
                // screen behind it, which read as Not Now, and the app
                // seemed to bounce straight back to the Library.
                if self.entrance.value < 0.97 {
                    self.press = None;
                    return None;
                }
                self.press = Some(Press {
                    target: self.target_at(p.x, p.y),
                    x: p.x,
                    y: p.y,
                    last_y: p.y,
                    moved: false,
                });
                None
            }
            types::Event::Pointer(p) if p.button.is_some() => {
                let press = self.press.take()?;
                // A phone lets a press go far outside the window when the
                // touch turned into a scroll: never a tap, even on Outside.
                let far = p.x < -1000.0 || p.y < -1000.0;
                if far || press.moved || self.target_at(p.x, p.y) != press.target {
                    return None;
                }
                match press.target {
                    Target::Open => Some(true),
                    // In the details, the cross goes back to the card.
                    Target::Close if self.details => {
                        self.details = false;
                        None
                    }
                    Target::Cancel | Target::Close => Some(false),
                    Target::Details => {
                        self.details = true;
                        self.scroll = 0.0;
                        None
                    }
                    // The screen around the sheet does nothing: whether an
                    // app opens is a choice made on the sheet's own buttons.
                    Target::Outside => None,
                    Target::Row(index) => {
                        let ask = &mut self.asks[index];
                        if !ask.required {
                            ask.granted = !ask.granted;
                        }
                        None
                    }
                    Target::Sheet => None,
                }
            }
            types::Event::Pointer(p) => {
                // A finger moving: past the slop it is a scroll, and the rows
                // follow it.
                let in_rows = inside(self.rows_area, p.x, p.y);
                if let Some(press) = self.press.as_mut() {
                    let (dx, dy) = (p.x - press.x, p.y - press.y);
                    if !press.moved && dx * dx + dy * dy > TAP_SLOP * TAP_SLOP {
                        press.moved = true;
                    }
                    let step = press.last_y - p.y;
                    press.last_y = p.y;
                    if press.moved && (in_rows || matches!(press.target, Target::Row(_))) {
                        self.scroll_by(step);
                    }
                }
                None
            }
            _ => None,
        }
    }
}

/// The home screen: every app the person has, in the order given. Returns
/// the index tapped, or None when the window is dismissed.
fn home(apps: &[(&str, &str)]) -> Option<usize> {
    let win = window::create(
        "Krate",
        types::WindowSize {
            width: 390,
            height: 720,
        },
    )
    .ok()?;
    tree::set_root(win, &node(ROOT_ID, None, types::WidgetKind::Stack)).ok()?;
    let _ = tree::upsert_node(
        win,
        &node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas),
    );
    let canvas = canvas2d::bind(win, CANVAS_ID).ok()?;
    let mut scroll = 0.0f32;
    loop {
        let l = match canvas2d::canvas_size(canvas) {
            Ok(size) => Layout::from_canvas(size.width.max(1.0), size.height.max(1.0)),
            Err(_) => Layout::from_canvas(390.0, 720.0),
        };
        let rows = draw_home(canvas, &l, apps, scroll).ok()?;
        let _ = window::request_redraw(win);
        match events::wait(None) {
            Some(types::Event::CloseRequested(_)) => return None,
            Some(types::Event::Wheel(wheel)) => {
                let content = rows.last().map(|r| r.y1 + scroll).unwrap_or(0.0);
                let max = (content - l.h * 0.6).max(0.0);
                scroll = (scroll + wheel.dy).clamp(0.0, max);
            }
            // A phone tap arrives as a press and a release at one point.
            Some(types::Event::Pointer(p)) if p.pressed && p.button.is_some() => {
                if let Some(row) = rows.iter().find(|r| p.y >= r.y0 && p.y <= r.y1) {
                    return Some(row.index);
                }
            }
            _ => {}
        }
    }
}

/// Draw the home screen; returns where each app's row is.
fn draw_home(
    canvas: u64,
    l: &Layout,
    apps: &[(&str, &str)],
    scroll: f32,
) -> Result<Vec<RowHit>, gfx::GfxError> {
    canvas2d::clear(canvas, color(0.02, 0.024, 0.038, 1.0))?;
    let x = l.col_x + 20.0;
    let w = l.col_w - 40.0;
    let top = 96.0 - scroll;
    canvas2d::draw_text_styled(
        canvas,
        "Krate",
        gfx::Point { x, y: top },
        34.0,
        color(1.0, 1.0, 1.0, 1.0),
        style(750, -0.6),
    )?;
    canvas2d::draw_text(
        canvas,
        "Your apps",
        gfx::Point { x, y: top + 28.0 },
        15.0,
        color(1.0, 1.0, 1.0, 0.5),
    )?;
    let mut rows = Vec::new();
    let mut y = top + 52.0;
    for (index, (name, line)) in apps.iter().enumerate() {
        let h = 72.0;
        canvas2d::fill_round_rect(
            canvas,
            rect(x, y, w, h),
            radii(18.0),
            color(0.086, 0.096, 0.13, 1.0),
        )?;
        // The app's initial on a tile, until apps carry icons.
        let tile = rect(x + 14.0, y + 14.0, 44.0, 44.0);
        canvas2d::fill_round_rect(canvas, tile, radii(11.0), color(0.24, 0.45, 0.98, 1.0))?;
        let initial: String = name
            .chars()
            .next()
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_default();
        let m = canvas2d::measure_text_styled(canvas, &initial, 22.0, style(700, 0.0))?;
        canvas2d::draw_text_styled(
            canvas,
            &initial,
            gfx::Point {
                x: tile.x + 22.0 - m.width / 2.0,
                y: tile.y + 30.0,
            },
            22.0,
            color(1.0, 1.0, 1.0, 1.0),
            style(700, 0.0),
        )?;
        canvas2d::draw_text_styled(
            canvas,
            name,
            gfx::Point {
                x: x + 72.0,
                y: y + 32.0,
            },
            17.0,
            color(1.0, 1.0, 1.0, 0.95),
            style(600, 0.0),
        )?;
        canvas2d::draw_text(
            canvas,
            line,
            gfx::Point {
                x: x + 72.0,
                y: y + 52.0,
            },
            13.0,
            color(1.0, 1.0, 1.0, 0.5),
        )?;
        rows.push(RowHit {
            index,
            y0: y,
            y1: y + h,
        });
        y += h + 10.0;
    }
    // How apps arrive, always at the foot of the list.
    let hint = "Open a .krate from Messages, Mail or Files and it appears here.";
    let mut hy = y + 26.0;
    for line in wrap_lines(canvas, hint, 14.0, w) {
        canvas2d::draw_text(
            canvas,
            &line,
            gfx::Point { x, y: hy },
            14.0,
            color(1.0, 1.0, 1.0, 0.45),
        )?;
        hy += 19.0;
    }
    canvas2d::present(canvas)?;
    Ok(rows)
}

/// Show why something did not open, until Close is tapped or the window is
/// dismissed.
fn notice(title: &str, message: &str) {
    let Ok(win) = window::create(
        "Krate",
        types::WindowSize {
            width: 390,
            height: 720,
        },
    ) else {
        return;
    };
    if tree::set_root(win, &node(ROOT_ID, None, types::WidgetKind::Stack)).is_err() {
        return;
    }
    let _ = tree::upsert_node(
        win,
        &node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas),
    );
    let Ok(canvas) = canvas2d::bind(win, CANVAS_ID) else {
        return;
    };
    loop {
        let l = match canvas2d::canvas_size(canvas) {
            Ok(size) => Layout::from_canvas(size.width.max(1.0), size.height.max(1.0)),
            Err(_) => Layout::from_canvas(390.0, 720.0),
        };
        let close = match draw_notice(canvas, &l, title, message) {
            Ok(close) => close,
            Err(_) => return,
        };
        let _ = window::request_redraw(win);
        match events::wait(None) {
            Some(types::Event::CloseRequested(_)) => return,
            Some(types::Event::Pointer(p)) if p.pressed && p.button.is_some() => {
                let (bx, by, bw, bh) = close;
                if p.x >= bx && p.x <= bx + bw && p.y >= by && p.y <= by + bh {
                    return;
                }
            }
            _ => {}
        }
    }
}

/// Draw the notice card; returns where Close is.
fn draw_notice(
    canvas: u64,
    l: &Layout,
    title: &str,
    message: &str,
) -> Result<(f32, f32, f32, f32), gfx::GfxError> {
    canvas2d::clear(canvas, color(0.02, 0.024, 0.038, 1.0))?;
    let x = l.col_x + 24.0;
    let text_w = l.col_w - 48.0;
    let lines = wrap_lines(canvas, message, 15.5, text_w);
    let card_h = 72.0 + lines.len() as f32 * 21.0 + 92.0;
    let top = ((l.h - card_h) / 2.0).max(16.0);
    canvas2d::fill_round_rect(
        canvas,
        rect(l.col_x + 8.0, top, l.col_w - 16.0, card_h),
        radii(24.0),
        color(0.075, 0.086, 0.125, 1.0),
    )?;
    canvas2d::draw_text_styled(
        canvas,
        title,
        gfx::Point { x, y: top + 48.0 },
        22.0,
        color(1.0, 1.0, 1.0, 1.0),
        style(700, -0.3),
    )?;
    let mut y = top + 80.0;
    for line in &lines {
        canvas2d::draw_text(
            canvas,
            line,
            gfx::Point { x, y },
            15.5,
            color(1.0, 1.0, 1.0, 0.75),
        )?;
        y += 21.0;
    }
    let close = rect(x, y + 14.0, text_w, 50.0);
    canvas2d::fill_round_rect(canvas, close, radii(25.0), color(0.42, 0.55, 1.0, 1.0))?;
    let m = canvas2d::measure_text_styled(canvas, "Close", 17.0, style(650, 0.2))?;
    canvas2d::draw_text_styled(
        canvas,
        "Close",
        gfx::Point {
            x: close.x + close.width / 2.0 - m.width / 2.0,
            y: close.y + 32.0,
        },
        17.0,
        color(1.0, 1.0, 1.0, 1.0),
        style(650, 0.2),
    )?;
    canvas2d::present(canvas)?;
    Ok((close.x, close.y, close.width, close.height))
}

/// Show what a command-line app printed, until Close is tapped or the window
/// is dismissed. A finger or a wheel scrolls it.
fn output(name: &str, ended: &str, lines: &[&str]) {
    let Ok(win) = window::create(
        "Krate",
        types::WindowSize {
            width: 390,
            height: 720,
        },
    ) else {
        return;
    };
    if tree::set_root(win, &node(ROOT_ID, None, types::WidgetKind::Stack)).is_err() {
        return;
    }
    let _ = tree::upsert_node(
        win,
        &node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas),
    );
    let Ok(canvas) = canvas2d::bind(win, CANVAS_ID) else {
        return;
    };
    let mut scroll = 0.0f32;
    // Wrapped once per width, not on every frame.
    let mut wrapped: (f32, Vec<String>) = (0.0, Vec::new());
    loop {
        let l = match canvas2d::canvas_size(canvas) {
            Ok(size) => Layout::from_canvas(size.width.max(1.0), size.height.max(1.0)),
            Err(_) => Layout::from_canvas(390.0, 720.0),
        };
        let mut l = l;
        l.safe = krate::layout::Screen::of(win).safe;
        if wrapped.0 != l.col_w {
            wrapped = (l.col_w, wrap_output(canvas, lines, l.col_w - 48.0));
        }
        let drawn = match draw_output(canvas, &l, name, ended, &wrapped.1, scroll) {
            Ok(drawn) => drawn,
            Err(_) => return,
        };
        let (close, room) = drawn;
        let _ = window::request_redraw(win);
        match events::wait(None) {
            Some(types::Event::CloseRequested(_)) => return,
            Some(types::Event::Wheel(wheel)) => {
                scroll = (scroll + wheel.dy).clamp(0.0, room);
            }
            Some(types::Event::Pointer(p)) if p.pressed && p.button.is_some() => {
                if inside(close, p.x, p.y) {
                    return;
                }
            }
            _ => {}
        }
    }
}

/// The size of the fixed-width text, and the room between its lines.
const OUTPUT_SIZE: f32 = 13.0;
const OUTPUT_LINE: f32 = 18.0;

/// Break every printed line to the card's width, by characters: output
/// is laid out in columns (a hex dump, a table), so words are not the unit.
/// Each piece is measured as drawn -- box-drawing and other symbols come
/// from fallback faces of other widths, so no one character's width holds
/// for a whole line.
fn wrap_output(canvas: u64, lines: &[&str], max_w: f32) -> Vec<String> {
    let mono = gfx::TextStyle {
        family: gfx::FontFamily::Mono,
        ..style(400, 0.0)
    };
    let fits = |chars: &[char]| {
        let text: String = chars.iter().collect();
        canvas2d::measure_text_styled(canvas, &text, OUTPUT_SIZE, mono)
            .map(|m| m.width <= max_w)
            .unwrap_or(true)
    };
    let mut out = Vec::new();
    for line in lines {
        let mut rest: Vec<char> = line.replace('\t', "    ").chars().collect();
        if rest.is_empty() {
            out.push(String::new());
        }
        while !rest.is_empty() {
            // The longest start of the rest that fits; at least one
            // character, so a glyph wider than the card still moves on.
            let (mut lo, mut hi) = (1, rest.len());
            if fits(&rest) {
                lo = hi;
            }
            while lo < hi {
                let mid = (lo + hi).div_ceil(2);
                if fits(&rest[..mid]) {
                    lo = mid;
                } else {
                    hi = mid - 1;
                }
            }
            out.push(rest[..lo].iter().collect());
            rest.drain(..lo);
        }
    }
    out
}

/// Draw the output card; returns where Close is and how far it can scroll.
fn draw_output(
    canvas: u64,
    l: &Layout,
    name: &str,
    ended: &str,
    lines: &[String],
    scroll: f32,
) -> Result<(gfx::Rect, f32), gfx::GfxError> {
    canvas2d::clear(canvas, color(0.02, 0.024, 0.038, 1.0))?;
    let x = l.col_x + 24.0;
    let text_w = l.col_w - 48.0;
    let top = l.safe.0 + 24.0;
    canvas2d::draw_text_styled(
        canvas,
        &fit_line(canvas, name, 22.0, style(700, -0.3), text_w),
        gfx::Point { x, y: top + 26.0 },
        22.0,
        color(1.0, 1.0, 1.0, 1.0),
        style(700, -0.3),
    )?;
    canvas2d::draw_text(
        canvas,
        ended,
        gfx::Point { x, y: top + 52.0 },
        14.5,
        color(1.0, 1.0, 1.0, 0.6),
    )?;
    let close = rect(x, l.h - l.safe.2 - 24.0 - 50.0, text_w, 50.0);
    let area = rect(
        l.col_x + 8.0,
        top + 72.0,
        l.col_w - 16.0,
        (close.y - 16.0 - (top + 72.0)).max(40.0),
    );
    canvas2d::fill_round_rect(canvas, area, radii(18.0), color(0.075, 0.086, 0.125, 1.0))?;
    let mono = gfx::TextStyle {
        family: gfx::FontFamily::Mono,
        ..style(400, 0.0)
    };
    let content = 16.0 + lines.len() as f32 * OUTPUT_LINE + 8.0;
    let room = (content - area.height).max(0.0);
    let scroll = scroll.min(room);
    canvas2d::set_clip(canvas, area.x, area.y, area.width, area.height)?;
    if lines.is_empty() {
        canvas2d::draw_text(
            canvas,
            "It printed nothing.",
            gfx::Point {
                x,
                y: area.y + 30.0,
            },
            14.5,
            color(1.0, 1.0, 1.0, 0.5),
        )?;
    }
    // Only the lines in view are drawn: an app may print thousands.
    let first = ((scroll - 16.0) / OUTPUT_LINE).max(0.0) as usize;
    let shown = (area.height / OUTPUT_LINE) as usize + 2;
    for (i, line) in lines.iter().enumerate().skip(first).take(shown) {
        let y = area.y + 16.0 + (i as f32 + 1.0) * OUTPUT_LINE - 4.0 - scroll;
        canvas2d::draw_text_styled(
            canvas,
            line,
            gfx::Point { x, y },
            OUTPUT_SIZE,
            color(0.86, 0.9, 1.0, 0.92),
            mono,
        )?;
    }
    canvas2d::clear_clip(canvas)?;
    canvas2d::fill_round_rect(canvas, close, radii(25.0), color(0.42, 0.55, 1.0, 1.0))?;
    let m = canvas2d::measure_text_styled(canvas, "Close", 17.0, style(650, 0.2))?;
    canvas2d::draw_text_styled(
        canvas,
        "Close",
        gfx::Point {
            x: close.x + close.width / 2.0 - m.width / 2.0,
            y: close.y + 32.0,
        },
        17.0,
        color(1.0, 1.0, 1.0, 1.0),
        style(650, 0.2),
    )?;
    canvas2d::present(canvas)?;
    Ok((close, room))
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

struct Component;

impl krate::Guest for Component {
    fn run() -> i32 {
        let raw = args::raw();
        if raw.split(ARG_SEP).next() == Some(HOME) {
            let rest: Vec<&str> = raw.split(ARG_SEP).skip(1).collect();
            let apps: Vec<(&str, &str)> = rest
                .chunks(2)
                .filter(|pair| pair.len() == 2 && !pair[0].trim().is_empty())
                .map(|pair| (pair[0].trim(), pair[1].trim()))
                .collect();
            match home(&apps) {
                Some(index) => out(&format!("wall:pick:{index}")),
                None => out("wall:closed"),
            }
            return 0;
        }
        if raw.split(ARG_SEP).next() == Some(OUTPUT) {
            let mut parts = raw.split(ARG_SEP).skip(1);
            let name = parts.next().unwrap_or("").trim();
            let ended = parts.next().unwrap_or("").trim();
            let lines: Vec<&str> = parts.collect();
            output(name, ended, &lines);
            out("wall:closed");
            return 0;
        }
        let mut parts = raw.splitn(3, ARG_SEP);
        if parts.next() == Some(NOTICE) {
            let title = parts.next().unwrap_or("").trim();
            let message = parts.next().unwrap_or("").trim();
            notice(title, message);
            out("wall:closed");
            return 0;
        }
        // A real invocation carries a name and at least one whole request, so
        // four arguments at the least. Anything shorter (no args, a harness
        // probe) renders the sample sheet headlessly.
        let quick = raw.split(ARG_SEP).filter(|a| !a.is_empty()).count() < 1 + FIELDS_PER_ASK;

        let (name, detail, asks) = if quick {
            parse_input(&format!(
                "{ASK}{ARG_SEP}Sample app{ARG_SEP}Version 1.0 \u{00b7} 112 KB{ARG_SEP}net.fetch:example.com:443{ARG_SEP}Fetch the daily quote from example.com{ARG_SEP}0{ARG_SEP}fs.write:./notes/**{ARG_SEP}Save your notes in its own folder, never your other files{ARG_SEP}1{ARG_SEP}audio.playback{ARG_SEP}Play the sound of a page turning{ARG_SEP}0"
            ))
        } else {
            parse_input(&raw)
        };
        if asks.is_empty() {
            // Nothing privileged to ask about: the honest wall is no wall.
            out("wall:open:");
            return 0;
        }

        let win = match window::create(
            "Before it opens",
            types::WindowSize {
                width: 390,
                height: 720,
            },
        ) {
            Ok(win) => win,
            Err(_) => {
                // No window, no consent: fail closed.
                out("wall:cancel");
                return 1;
            }
        };
        if tree::set_root(win, &node(ROOT_ID, None, types::WidgetKind::Stack)).is_err() {
            out("wall:cancel");
            return 1;
        }
        let _ = tree::upsert_node(
            win,
            &node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas),
        );
        let canvas = match canvas2d::bind(win, CANVAS_ID) {
            Ok(canvas) => canvas,
            Err(_) => {
                out("wall:cancel");
                return 1;
            }
        };

        let mut sheet = Sheet::new(name, detail, asks);
        sheet.window = win;
        sheet.file_icon =
            canvas_images::keep(canvas, FILE_ICON_SIDE, FILE_ICON_SIDE, FILE_ICON).ok();
        sheet.behind = behind(canvas);
        if quick {
            // The sample is drawn at rest, as a person sees it once it has
            // slid in.
            sheet.entrance = Spring::rest_at(1.0, 18.0);
        }
        let mut last = clock::monotonic_nanos();
        let mut frames: u32 = 0;
        let mut decision: Option<bool> = None;

        loop {
            let mut l = match canvas2d::canvas_size(canvas) {
                Ok(size) => Layout::from_canvas(size.width.max(1.0), size.height.max(1.0)),
                Err(_) => Layout::from_canvas(390.0, 720.0),
            };
            l.safe = krate::layout::Screen::of(win).safe;
            let now = clock::monotonic_nanos();
            let dt = if quick {
                1.0 / 60.0
            } else {
                let dt = ((now.saturating_sub(last)) as f32 / 1_000_000_000.0).min(0.05);
                last = now;
                dt
            };
            sheet.tick(dt);

            if sheet.draw(canvas, &l).is_err() {
                out("wall:cancel");
                return 1;
            }
            frames += 1;

            if quick {
                if frames >= 40 {
                    decision = Some(true);
                    break;
                }
                continue;
            }

            let _ = window::request_redraw(win);
            // Every event already queued is followed before the next frame
            // -- a tap's press and release often arrive together, and a
            // 60 Hz drag outruns a loop that takes one per frame.
            let wait = if sheet.animating() { Some(16) } else { None };
            let mut next = events::wait(wait);
            while let Some(event) = next {
                if let Some(choice) = sheet.handle(&event) {
                    decision = Some(choice);
                    break;
                }
                next = events::poll();
            }
            if decision.is_some() {
                break;
            }
        }

        match decision {
            Some(true) => {
                let granted: Vec<&str> = sheet
                    .asks
                    .iter()
                    .filter(|ask| ask.granted)
                    .map(|ask| ask.cap.as_str())
                    .collect();
                out(&format!("wall:open:{}", granted.join(",")));
            }
            _ => out("wall:cancel"),
        }
        0
    }
}

krate::export!(Component);
