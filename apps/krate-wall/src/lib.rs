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
mod theme;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use krate::bindings::krate::fs::files;
use krate::bindings::krate::io::{args, stdio};
use krate::bindings::krate::time::clock;
use krate::gfx::{canvas2d, canvas_images, types as gfx};
use krate::layout::Look;
use krate::motion::Spring;
use krate::ui::{events, tree, types, window};

use theme::{look_now, theme, Theme};

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

/// Word-wrap measured in the style it is drawn in, honest at any width. A
/// word wider than the whole line (a long name at a large text size) is
/// broken by characters rather than left running off the card.
fn wrap_lines(canvas: u64, text: &str, size: f32, st: gfx::TextStyle, max_w: f32) -> Vec<String> {
    let fits = |t: &str| {
        canvas2d::measure_text_styled(canvas, t, size, st)
            .map(|m| m.width <= max_w)
            .unwrap_or(true)
    };
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if fits(&candidate) {
            current = candidate;
            continue;
        }
        if !current.is_empty() {
            lines.push(core::mem::take(&mut current));
        }
        if fits(word) {
            current = word.to_string();
            continue;
        }
        // Too wide on its own: as many characters a line as fit, at least
        // one, so the loop always moves on.
        let chars: Vec<char> = word.chars().collect();
        let mut start = 0;
        while start < chars.len() {
            let mut end = chars.len();
            while end > start + 1 && !fits(&chars[start..end].iter().collect::<String>()) {
                end -= 1;
            }
            let piece: String = chars[start..end].iter().collect();
            if end == chars.len() {
                current = piece;
            } else {
                lines.push(piece);
            }
            start = end;
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

#[derive(Clone, Copy)]
struct Layout {
    w: f32,
    h: f32,
    col_x: f32,
    col_w: f32,
    /// Points at each edge something else covers (top, right, bottom,
    /// left): a phone's notch, home bar and rounded corners.
    safe: (f32, f32, f32, f32),
    /// How the person asked things to look: dark mode, contrast, text size.
    look: Look,
    /// Krate's colours for that look.
    t: Theme,
    /// The window, to describe each frame to a screen reader.
    win: u64,
}

impl Layout {
    /// The canvas as it is this frame, with the phone's look read afresh,
    /// so a sheet left open follows the phone into dark mode.
    fn now(canvas: u64, win: u64) -> Self {
        let (w, h) = match canvas2d::canvas_size(canvas) {
            Ok(size) => (size.width.max(1.0), size.height.max(1.0)),
            Err(_) => (390.0, 720.0),
        };
        let col_w = w.min(480.0);
        let look = look_now();
        Layout {
            w,
            h,
            col_x: (w - col_w) / 2.0,
            col_w,
            safe: krate::layout::Screen::of(win).safe,
            look,
            t: theme(&look),
            win,
        }
    }

    /// A font size, or a distance between lines of text, at the person's
    /// text size.
    fn f(&self, points: f32) -> f32 {
        self.look.text(points)
    }

    /// Whether the person's text is large enough that text gets more lines
    /// before it is cut short.
    fn large(&self) -> bool {
        self.look.text_scale > 1.15
    }

    /// The same, with the text a step smaller, while it is still larger
    /// than the default: for a screen too small to hold a card at the size
    /// the person asked for. Smaller words beat an Open button off the
    /// screen or a message cut short.
    fn smaller(&self) -> Option<Layout> {
        (self.look.text_scale > 1.0).then(|| {
            let mut l = *self;
            l.look.text_scale = (l.look.text_scale * 0.9).max(1.0);
            l
        })
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
    /// Where the rows are shown: a tap here lands on one.
    rows_area: gfx::Rect,
    /// What a finger drags to scroll: the rows, or the whole of the
    /// details when they are taller than the screen.
    scroll_area: gfx::Rect,
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
        "fs.write" | "fs.remove" | "fs.mkdir" if own => ("Save its own files".to_string(), orange),
        "fs.read" | "fs.list" => ("Read files".to_string(), orange),
        "fs.write" | "fs.remove" | "fs.mkdir" => ("Change files".to_string(), orange),
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
        "store.org" => ("Your organisation's data".to_string(), green),
        "store.group" => ("Share data with its maker's other apps".to_string(), green),
        "ui.dropzone" => ("Files you drop on it".to_string(), orange),
        "ui.menu" => ("A menu".to_string(), gray),
        "ui.print" => ("Print".to_string(), gray),
        "ui.screen-capture" => ("See your screen".to_string(), red),
        "net.connection" => ("Whether you are online".to_string(), blue),
        "speech.transcribe" => ("Speech".to_string(), pink),
        "random.bytes" => ("Random numbers".to_string(), gray),
        "process.run" => ("Run programs".to_string(), red),
        "process.terminal" => ("A terminal".to_string(), red),
        "ui.clipboard" => ("Clipboard".to_string(), indigo),
        "ui.notify" => ("Notifications".to_string(), red),
        "ui.open-url" => ("Open links".to_string(), indigo),
        "gfx.gpu" => ("Graphics chip".to_string(), purple),
        "memory.budget" if !target.is_empty() => (format!("Memory \u{00b7} {target} MB"), teal),
        _ => (String::from(cap), gray),
    };
    (title, tint)
}

/// A request's title as it reads after "Uses" in the sheet's one sentence:
/// "Uses the internet, its own files and 2 more", never "Uses Internet, Keep
/// its data".
fn in_a_sentence(title: &str) -> String {
    let said = match title {
        "Internet" => "the internet",
        "Keep its data" => "its own storage",
        "Read its own files" | "Save its own files" => "its own files",
        "Read files" => "your files",
        "Change files" => "changes to your files",
        "Save files you pick" => "files you pick",
        "Share data with people you invite" => "data shared by invite",
        "Keep secrets safe" => "safe storage for secrets",
        "Share data with its maker's other apps" => "data shared with its maker's apps",
        "Add to your Photos" => "your Photos",
        "Open links" => "opening links",
        "Run programs" => "running programs",
        "Print" => "printing",
        "Its own window" => "its own window",
        other => {
            // Nouns already ("Sound", "Where you are"): lower-case the first
            // letter, except names (Krate, Bluetooth).
            let keep = other.starts_with("Krate") || other.starts_with("Bluetooth");
            let mut chars = other.chars();
            return match chars.next() {
                Some(first) if !keep => first.to_lowercase().chain(chars).collect(),
                _ => other.to_string(),
            };
        }
    };
    said.to_string()
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
fn wrap_at_most(
    canvas: u64,
    text: &str,
    size: f32,
    st: gfx::TextStyle,
    max_w: f32,
    max: usize,
) -> Vec<String> {
    let mut lines = wrap_lines(canvas, text, size, st, max_w);
    let max = max.max(1);
    if lines.len() > max {
        let rest = lines[max - 1..].join(" ");
        lines.truncate(max - 1);
        lines.push(fit_line(canvas, &rest, size, st, max_w));
    }
    lines
}

/// Where to put a line's baseline so the text sits in the middle of a box
/// `h` high starting at `y`.
fn centred_baseline(y: f32, h: f32, size: f32) -> f32 {
    y + h / 2.0 + size * 0.35
}

/// Behind the sheet: the screen the person came from, blurred and dimmed a
/// little as the sheet arrives; without one, Krate's backdrop with the
/// app's colour washing in from the top.
fn draw_backdrop(
    canvas: u64,
    l: &Layout,
    behind: Option<u64>,
    hue: gfx::Color,
    e: f32,
) -> Result<(), gfx::GfxError> {
    let t = &l.t;
    match behind {
        Some(behind) => {
            canvas2d::clear(canvas, color(0.0, 0.0, 0.0, 1.0))?;
            canvas_images::draw(canvas, behind, rect(0.0, 0.0, l.w, l.h))?;
            canvas2d::fill_rect(
                canvas,
                rect(0.0, 0.0, l.w, l.h),
                color(0.0, 0.0, 0.0, t.dim * e.min(1.0)),
            )?;
        }
        None => {
            // A wash of the app's colour from the top, fading by the
            // middle. It was a radial glow across the whole screen: on a
            // phone's CPU painter that one call was most of every frame
            // (164 ms of 236 on a Galaxy S22 Ultra), so the sheet rose in
            // three or four steps. A vertical wash paints by rows and
            // costs a few milliseconds (K-1212).
            canvas2d::clear(canvas, t.backdrop)?;
            canvas2d::linear_gradient(
                canvas,
                rect(0.0, 0.0, l.w, l.h * 0.62),
                color(hue.r, hue.g, hue.b, t.wash * e.min(1.0)),
                color(hue.r, hue.g, hue.b, 0.0),
            )?;
        }
    }
    Ok(())
}

/// A card: a modest shadow (a wide blur is slow on a phone's CPU painter),
/// its fill, and a hairline where the shadow alone would not show it.
fn draw_card(
    canvas: u64,
    t: &Theme,
    area: gfx::Rect,
    corners: gfx::CornerRadii,
) -> Result<(), gfx::GfxError> {
    canvas2d::drop_shadow_round_rect(canvas, area, corners, 20.0, t.shadow)?;
    canvas2d::fill_round_rect(canvas, area, corners, t.card)?;
    if t.edge.a > 0.0 {
        canvas2d::stroke_round_rect(canvas, area, corners, 1.0, t.edge)?;
    }
    Ok(())
}

/// The close cross in a soft circle of radius `r`, centred at (x, y).
fn draw_close(
    canvas: u64,
    t: &Theme,
    x: f32,
    y: f32,
    r: f32,
    pressed: bool,
) -> Result<(), gfx::GfxError> {
    let fill = if pressed { t.soft_press } else { t.soft };
    canvas2d::fill_circle(canvas, gfx::Point { x, y }, r, fill)?;
    let arm = (r * 0.62 / 0.9) as i32 / 2;
    for k in -arm..=arm {
        let d = k as f32 * 0.9;
        for (dx, dy) in [(d, d), (d, -d)] {
            canvas2d::fill_circle(
                canvas,
                gfx::Point {
                    x: x + dx,
                    y: y + dy,
                },
                1.05,
                t.ink2,
            )?;
        }
    }
    Ok(())
}

/// A filled, rounded button with its label centred.
fn draw_button(
    canvas: u64,
    l: &Layout,
    area: gfx::Rect,
    label: &str,
    fill: gfx::Color,
    ink: gfx::Color,
) -> Result<(), gfx::GfxError> {
    let size = l.f(17.0);
    canvas2d::fill_round_rect(canvas, area, radii(area.height.min(52.0) / 2.0), fill)?;
    let label = fit_line(canvas, label, size, style(600, 0.0), area.width - 16.0);
    let m = canvas2d::measure_text_styled(canvas, &label, size, style(600, 0.0))?;
    canvas2d::draw_text_styled(
        canvas,
        &label,
        gfx::Point {
            x: area.x + (area.width - m.width) / 2.0,
            y: centred_baseline(area.y, area.height, size),
        },
        size,
        ink,
        style(600, 0.0),
    )
}

/// How tall a button is for the person's text size.
fn button_h(l: &Layout) -> f32 {
    52.0f32.max(l.f(17.0) + 30.0)
}

fn inside(r: gfx::Rect, x: f32, y: f32) -> bool {
    x >= r.x && x <= r.x + r.width && y >= r.y && y <= r.y + r.height
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
    titles: Vec<String>,
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
            scroll_area: zero,
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
        let t = l.t;
        let hue = name_hue(&self.name);
        let e = self.entrance.value.clamp(0.0, 1.2);

        let (top, right, bottom, left) = l.safe;
        let pad = 24.0;
        let cw = (l.w - left - right - 24.0).min(420.0);
        let cx = left + (l.w - left - right - cw) / 2.0;
        let inner = cw - pad * 2.0;
        let (title_size, detail_size, summary_size, link_size) =
            (l.f(22.0), l.f(14.0), l.f(15.0), l.f(14.0));
        let more_lines = if l.large() { 3 } else { 2 };

        // What it uses, said plainly.
        let shown: Vec<usize> = (0..self.asks.len())
            .filter(|&i| Self::worth_asking(&self.asks[i].cap))
            .collect();
        let mut names: Vec<String> = Vec::new();
        for &i in &shown {
            let (title, _) = describe(&self.asks[i].cap);
            let title = in_a_sentence(title.split(" \u{00b7} ").next().unwrap_or(""));
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
        let one_line = |text: &str| {
            canvas2d::measure_text_styled(canvas, text, summary_size, style(500, 0.0))
                .map(|m| m.width <= inner)
                .unwrap_or(true)
        };
        let mut summary = said(names.len());
        for named in (1..=names.len().min(3)).rev() {
            summary = said(named);
            if one_line(&summary) {
                break;
            }
        }
        if names.len() <= 3 && one_line(&said(names.len())) {
            summary = said(names.len());
        }
        let summary_lines = wrap_at_most(
            canvas,
            &summary,
            summary_size,
            style(500, 0.0),
            inner,
            more_lines,
        );
        let title = format!("Open {}?", self.name);
        let title_lines = wrap_at_most(
            canvas,
            &title,
            title_size,
            style(750, -0.4),
            inner - 40.0,
            more_lines,
        );
        let detail = if self.detail.is_empty() {
            String::from("Krate app")
        } else {
            self.detail.clone()
        };
        let detail_lines = wrap_at_most(canvas, &detail, detail_size, style(500, 0.0), inner, 2);
        let tiles = shown.len().min(6) + usize::from(shown.len() > 6);

        // Not now and Open side by side, or one over the other when the
        // person's text is too large for both on one line.
        let gap = 12.0;
        let bh = button_h(l);
        let half = (inner - gap) / 2.0;
        let widest = ["Not now", "Open"]
            .iter()
            .filter_map(|text| {
                canvas2d::measure_text_styled(canvas, text, l.f(17.0), style(600, 0.0)).ok()
            })
            .fold(0.0f32, |w, m| w.max(m.width));
        let stacked = widest > half - 28.0;
        let buttons_h = if stacked { bh * 2.0 + 10.0 } else { bh };

        // Measure, then place the card at the bottom (centred on a wide or
        // turned screen). When it is taller than the screen -- a small
        // phone at a large text size -- the tiles go first, then the mark:
        // the words and the buttons always stay.
        let tile = 36.0;
        let upright = l.h >= l.w || l.h >= 520.0;
        let foot = bottom.max(10.0) + 8.0;
        let room = l.h - top.max(8.0) - foot;
        let height = |mark: bool, tiles: bool| {
            let mut h = pad;
            if mark {
                h += 60.0 + 16.0;
            }
            h += title_lines.len() as f32 * l.f(28.0) + detail_lines.len() as f32 * l.f(20.0);
            if tiles {
                h += 18.0 + tile;
            }
            h + 14.0
                + summary_lines.len() as f32 * l.f(21.0)
                + 10.0
                + l.f(20.0)
                + 22.0
                + buttons_h
                + pad
        };
        let (show_mark, show_tiles) = if height(true, tiles > 0) <= room {
            (true, tiles > 0)
        } else if height(true, false) <= room {
            (true, false)
        } else {
            (false, false)
        };
        let h = height(show_mark, show_tiles);
        if h > room {
            if let Some(smaller) = l.smaller() {
                return self.draw_compact(canvas, &smaller);
            }
        }
        draw_backdrop(canvas, l, self.behind, hue, e)?;
        let slide = (1.0 - e) * (h + foot + 40.0);
        let cy = if upright && l.w < 600.0 {
            (l.h - foot - h).max(top.max(8.0)) + slide
        } else {
            (top + (l.h - top - bottom - h) / 2.0).max(top + 8.0) + slide * 0.35
        };
        let card = rect(cx, cy, cw, h);
        self.sheet_area = card;
        draw_card(canvas, &t, card, radii(30.0))?;

        // Close, top right: a cross in a soft circle; it means Not now.
        let (xx, xy) = (cx + cw - 20.0 - 14.0, cy + 20.0 + 14.0);
        draw_close(canvas, &t, xx, xy, 14.0, self.pressed(Target::Close))?;
        self.close_button = rect(xx - 22.0, xy - 22.0, 44.0, 44.0);

        // The app's mark, centred.
        let mid = cx + cw / 2.0;
        let mut y = cy + pad;
        if show_mark {
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
                    theme::WHITE,
                    style(800, 0.0),
                )?;
            }
            y += 60.0 + 16.0;
        }

        // "Open Phone Check?", and its version and size.
        let centred = |text: &str, y: f32, size: f32, ink: gfx::Color, st: gfx::TextStyle| {
            let m = canvas2d::measure_text_styled(canvas, text, size, st)?;
            canvas2d::draw_text_styled(
                canvas,
                text,
                gfx::Point {
                    x: mid - m.width / 2.0,
                    y,
                },
                size,
                ink,
                st,
            )
        };
        for line in &title_lines {
            centred(line, y + title_size, title_size, t.ink, style(750, -0.4))?;
            y += l.f(28.0);
        }
        for line in &detail_lines {
            centred(line, y + detail_size, detail_size, t.ink2, style(500, 0.0))?;
            y += l.f(20.0);
        }

        // One row of tiles for what it uses.
        if show_tiles {
            y += 18.0;
            let gap = 8.0;
            let row_w = tiles as f32 * tile + (tiles - 1) as f32 * gap;
            let mut tx = mid - row_w / 2.0;
            for &i in shown.iter().take(6) {
                let cap = &self.asks[i].cap;
                let (_, tint) = describe(cap);
                canvas2d::fill_round_rect(canvas, rect(tx, y, tile, tile), radii(10.0), tint)?;
                ask_icon(canvas, cap, tx + tile / 2.0, y + tile / 2.0)?;
                tx += tile + gap;
            }
            if shown.len() > 6 {
                canvas2d::fill_round_rect(canvas, rect(tx, y, tile, tile), radii(10.0), t.soft)?;
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
                    t.ink2,
                    style(700, 0.0),
                )?;
            }
            y += tile;
        }

        // The same, in words.
        y += 14.0;
        for line in &summary_lines {
            centred(
                line,
                y + summary_size,
                summary_size,
                t.ink2,
                style(500, 0.0),
            )?;
            y += l.f(21.0);
        }

        // See details.
        y += 10.0;
        let link = "See details";
        let lm = canvas2d::measure_text_styled(canvas, link, link_size, style(600, 0.0))?;
        let link_ink = if self.pressed(Target::Details) {
            t.accent_press
        } else {
            t.accent
        };
        centred(link, y + link_size, link_size, link_ink, style(600, 0.0))?;
        self.details_button = rect(
            mid - lm.width / 2.0 - 16.0,
            y - 10.0,
            lm.width + 32.0,
            l.f(20.0) + 20.0,
        );
        y += l.f(20.0) + 22.0;

        // Not now and Open.
        let (open, not_now) = if stacked {
            (
                rect(cx + pad, y, inner, bh),
                rect(cx + pad, y + bh + 10.0, inner, bh),
            )
        } else {
            (
                rect(cx + pad + half + gap, y, half, bh),
                rect(cx + pad, y, half, bh),
            )
        };
        let nn_fill = if self.pressed(Target::Cancel) {
            t.soft
        } else {
            t.card
        };
        draw_button(canvas, l, not_now, "Not now", nn_fill, t.ink)?;
        canvas2d::stroke_round_rect(
            canvas,
            not_now,
            radii(not_now.height.min(52.0) / 2.0),
            1.2,
            t.stroke,
        )?;
        let open_fill = if self.pressed(Target::Open) {
            t.btn_press
        } else {
            t.btn
        };
        draw_button(canvas, l, open, "Open", open_fill, t.btn_ink)?;
        self.cancel_button = not_now;
        self.open_button = open;
        self.rows_area = rect(0.0, 0.0, 0.0, 0.0);
        self.scroll_area = rect(0.0, 0.0, 0.0, 0.0);
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
        let t = l.t;
        let hue = name_hue(&self.name);
        let e = self.entrance.value.clamp(0.0, 1.2);

        let (top, right, bottom, left) = l.safe;
        let sideways = l.w > l.h && l.h < 520.0;
        let phone_upright = !sideways && l.w < 600.0;
        let pad = 24.0;
        let (name_size, detail_size, promise_size, label_size) =
            (l.f(21.0), l.f(14.0), l.f(15.0), l.f(12.0));
        let (row_title_size, reason_size, required_size) = (l.f(16.0), l.f(13.5), l.f(13.0));

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
        let grabber_h = if phone_upright { 20.0 } else { pad };
        // The close cross, top right (not sideways: there the list fills
        // that corner and Not Now sits right by the thumb).
        let (close_x, close_y) = (sx + sw - pad - 15.0 + 4.0, 0.0);

        // Measure first, so the sheet is exactly as tall as what it says.
        let beside_w = if sideways {
            head_w - 78.0
        } else {
            (close_x - 22.0) - (head_x + 78.0)
        };
        // The name sits beside the mark while it fits there on one line;
        // at a large text size it goes under the mark, across the sheet.
        let beside = !l.large()
            || canvas2d::measure_text_styled(canvas, &self.name, name_size, style(750, -0.4))
                .map(|m| m.width <= beside_w)
                .unwrap_or(true);
        let (name_x, name_w) = if beside {
            (head_x + 78.0, beside_w)
        } else {
            (head_x, head_w)
        };
        let name_lines = wrap_at_most(canvas, &self.name, name_size, style(750, -0.4), name_w, 2);
        let detail = if self.detail.is_empty() {
            "Krate app".to_string()
        } else {
            self.detail.clone()
        };
        let detail_lines = wrap_at_most(canvas, &detail, detail_size, style(500, 0.0), name_w, 2);
        let name_block =
            name_lines.len() as f32 * l.f(26.0) + 4.0 + detail_lines.len() as f32 * l.f(18.0);
        let head_block = if beside {
            name_block.max(64.0)
        } else {
            64.0 + 10.0 + name_block
        };
        let promise =
            format!(
            "{} runs sealed off from the rest of this device. It can use only what you allow here.",
            if self.name.chars().count() <= 24 { self.name.as_str() } else { "This app" }
        );
        let promise_lines = wrap_lines(canvas, &promise, promise_size, style(450, 0.0), head_w);
        let tile = 32.0;
        let row_pad = 14.0;
        let text_x_off = row_pad + tile + 12.0;
        let required_w =
            canvas2d::measure_text_styled(canvas, "Required", required_size, style(600, 0.0))?
                .width;
        let reason_lines = if l.large() { 6 } else { 3 };
        let mut texts = Vec::new();
        for ask in &self.asks {
            let (title, tint) = describe(&ask.cap);
            let right_w = if ask.required {
                required_w + 8.0
            } else {
                51.0 + 8.0
            };
            let text_w = list_w - text_x_off - row_pad - right_w;
            let titles = wrap_at_most(canvas, &title, row_title_size, style(600, -0.2), text_w, 2);
            let lines = if ask.rationale.is_empty() {
                Vec::new()
            } else {
                wrap_at_most(
                    canvas,
                    &ask.rationale,
                    reason_size,
                    style(450, 0.0),
                    text_w,
                    reason_lines,
                )
            };
            let h = (row_pad
                + titles.len() as f32 * l.f(20.0)
                + lines.len() as f32 * l.f(18.0)
                + row_pad
                - 2.0)
                .max(60.0);
            texts.push(RowText {
                titles,
                tint,
                lines,
                h,
            });
        }
        let rows_h: f32 = texts.iter().map(|t| t.h).sum();

        // From the top of the app's name to the top of the rows.
        let promise_h = 12.0 + promise_lines.len() as f32 * l.f(21.0);
        let label_h = 10.0 + label_size + 10.0;
        let head_h = head_block + promise_h;
        let open_h = 54.0f32.max(l.f(17.0) + 32.0);
        let cancel_h = 44.0f32.max(l.f(16.0) + 26.0);
        let buttons_h = open_h + 6.0 + cancel_h;
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
        // Sideways the rows scroll in their own column. Otherwise the whole
        // of it -- the app, the promise and the rows -- scrolls above
        // buttons that stay put, so a small phone at a large text size
        // still shows a whole request at a time.
        let content = head_h + label_h + rows_h;
        // Sideways, the app and its buttons share a short column: a step
        // smaller when they do not fit it.
        if sideways && grabber_h + head_h + 18.0 + buttons_h + foot > room {
            if let Some(smaller) = l.smaller() {
                return self.draw_details(canvas, &smaller);
            }
        }
        let (sheet_h, rows_shown) = if sideways {
            let left_h = grabber_h + head_h + 18.0 + buttons_h + foot;
            let right_needed = grabber_h + label_h + rows_h + foot;
            let sheet_h = left_h.max(right_needed).min(room);
            let shown = (sheet_h - grabber_h - label_h - foot).max(60.0).min(rows_h);
            self.max_scroll = (rows_h - shown).max(0.0);
            (sheet_h, shown)
        } else {
            let fixed = grabber_h + 18.0 + buttons_h + foot;
            let sheet_h = (fixed + content).min(room.max(fixed + 120.0));
            self.max_scroll = (content - (sheet_h - fixed)).max(0.0);
            (sheet_h, rows_h)
        };
        self.scroll = self.scroll.clamp(0.0, self.max_scroll);
        // What scrolls the rows only, and what scrolls everything.
        let (rows_scroll, all_scroll) = if sideways {
            (self.scroll, 0.0)
        } else {
            (0.0, self.scroll)
        };

        // Too little of it in view at once on a small screen at a large
        // text size: a step smaller, while still larger than the default.
        let view_h = sheet_h - grabber_h - 18.0 - buttons_h - foot;
        if !sideways && view_h < content.min(l.h * 0.55) {
            if let Some(smaller) = l.smaller() {
                return self.draw_details(canvas, &smaller);
            }
        }
        draw_backdrop(canvas, l, self.behind, hue, e)?;
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
        draw_card(canvas, &t, frame, sheet_radii)?;

        if phone_upright {
            // The grabber: this is a sheet over the screen, not the screen.
            canvas2d::fill_round_rect(
                canvas,
                rect(sx + sw / 2.0 - 18.0, sy + 8.0, 36.0, 5.0),
                radii(2.5),
                t.grabber,
            )?;
        }

        // Where the scrolling part is shown: under the grabber, above the
        // buttons.
        let buttons_y = sy + sheet_h - foot - buttons_h;
        let view = if sideways {
            // The left column stops above the buttons; the rows set
            // their own clip.
            rect(sx, sy, sw, (buttons_y - 10.0 - sy).max(0.0))
        } else {
            let view_top = sy + if phone_upright { 18.0 } else { 8.0 };
            rect(sx, view_top, sw, buttons_y - 10.0 - view_top)
        };
        canvas2d::set_clip(canvas, view.x, view.y, view.width, view.height)?;

        // The app: its mark, its name, what it is.
        let hy = sy + grabber_h - all_scroll;
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
                theme::WHITE,
                style(800, 0.0),
            )?;
        }

        // The name and the line about it, in the middle of the mark's
        // height beside it, or under it.
        let mut ny = if beside {
            hy + ((64.0 - name_block) / 2.0).max(0.0)
        } else {
            hy + 64.0 + 10.0
        };
        for line in &name_lines {
            ny += l.f(26.0);
            canvas2d::draw_text_styled(
                canvas,
                line,
                gfx::Point {
                    x: name_x,
                    y: ny - l.f(5.0),
                },
                name_size,
                t.ink,
                style(750, -0.4),
            )?;
        }
        ny += 4.0;
        for line in &detail_lines {
            ny += l.f(18.0);
            canvas2d::draw_text_styled(
                canvas,
                line,
                gfx::Point {
                    x: name_x,
                    y: ny - l.f(4.0),
                },
                detail_size,
                t.ink2,
                style(500, 0.0),
            )?;
        }

        let mut y = hy + head_block + 12.0;
        for line in &promise_lines {
            y += l.f(21.0);
            canvas2d::draw_text_styled(
                canvas,
                line,
                gfx::Point {
                    x: head_x,
                    y: y - l.f(6.0),
                },
                promise_size,
                t.ink2,
                style(450, 0.0),
            )?;
        }

        // What it asks to use: one grouped card, scrolling inside itself
        // when there is more than fits.
        let label_top = if sideways { hy } else { y };
        let rows_x = list_x;
        let count = self.asks.len();
        let label = if count == 1 {
            "IT ASKS FOR 1 THING".to_string()
        } else {
            format!("IT ASKS FOR {count} THINGS")
        };
        let label = fit_line(canvas, &label, label_size, style(650, 0.8), list_w - 8.0);
        canvas2d::draw_text_styled(
            canvas,
            &label,
            gfx::Point {
                x: rows_x + 4.0,
                y: label_top + 10.0 + label_size,
            },
            label_size,
            t.ink3,
            style(650, 0.8),
        )?;
        let rows_top = label_top + label_h;
        let area = rect(rows_x, rows_top, list_w, rows_shown);
        canvas2d::fill_round_rect(canvas, area, radii(16.0), t.group)?;
        // The part of the rows in view: where a tap lands on a row.
        let rows_view = if sideways { area } else { view };
        let shown_top = area.y.max(rows_view.y);
        let shown_bottom = (area.y + area.height).min(rows_view.y + rows_view.height);
        self.rows_area = rect(
            area.x,
            shown_top,
            area.width,
            (shown_bottom - shown_top).max(0.0),
        );
        self.scroll_area = if sideways { area } else { view };
        if sideways {
            canvas2d::set_clip(canvas, area.x, area.y, area.width, area.height)?;
        }
        let visible = self.rows_area;
        self.rows.clear();
        let mut ry = rows_top - rows_scroll;
        for (index, row) in texts.iter().enumerate() {
            let ask = &self.asks[index];
            let y1 = ry + row.h;
            self.rows.push(RowHit { index, y0: ry, y1 });
            if ry < visible.y + visible.height && y1 > visible.y {
                if self.pressed(Target::Row(index)) && !ask.required {
                    canvas2d::fill_rect(canvas, rect(area.x, ry, area.width, row.h), t.row_press)?;
                }
                let tile_y = ry + row_pad;
                let tile_r = rect(rows_x + row_pad, tile_y, tile, tile);
                let tint = if ask.granted { row.tint } else { t.denied };
                canvas2d::fill_round_rect(canvas, tile_r, radii(8.0), tint)?;
                ask_icon(
                    canvas,
                    &ask.cap,
                    tile_r.x + tile / 2.0,
                    tile_r.y + tile / 2.0,
                )?;
                let tx = rows_x + text_x_off;
                let mut ly = ry + row_pad - l.f(5.0);
                for title in &row.titles {
                    ly += l.f(20.0);
                    canvas2d::draw_text_styled(
                        canvas,
                        title,
                        gfx::Point { x: tx, y: ly },
                        row_title_size,
                        if ask.granted { t.ink } else { t.ink2 },
                        style(600, -0.2),
                    )?;
                }
                for line in &row.lines {
                    ly += l.f(18.0);
                    canvas2d::draw_text_styled(
                        canvas,
                        line,
                        gfx::Point { x: tx, y: ly },
                        reason_size,
                        t.ink2,
                        style(450, 0.0),
                    )?;
                }
                let mid = ry + row_pad + tile / 2.0;
                if ask.required {
                    // Required: the app cannot run without it, so no switch.
                    canvas2d::draw_text_styled(
                        canvas,
                        "Required",
                        gfx::Point {
                            x: rows_x + list_w - row_pad - required_w,
                            y: mid + required_size * 0.35,
                        },
                        required_size,
                        t.ink3,
                        style(600, 0.0),
                    )?;
                } else {
                    // An iOS-shaped switch, its knob sprung between ends.
                    let k = self.knobs[index].value.clamp(0.0, 1.0);
                    let pill = rect(rows_x + list_w - row_pad - 51.0, mid - 15.5, 51.0, 31.0);
                    let (off, on) = (t.switch_off, t.switch_on);
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
                        t.knob_shadow,
                    )?;
                    canvas2d::fill_circle(
                        canvas,
                        gfx::Point { x: knob_x, y: mid },
                        13.5,
                        theme::WHITE,
                    )?;
                }
                if index + 1 < count {
                    canvas2d::fill_rect(
                        canvas,
                        rect(tx, y1 - 0.5, rows_x + list_w - tx, 0.5),
                        t.line,
                    )?;
                }
            }
            ry = y1;
        }
        canvas2d::clear_clip(canvas)?;
        // Close stays where it is while the rest scrolls under it.
        let close_y = close_y + sy + grabber_h + 15.0;
        self.close_button = rect(0.0, 0.0, 0.0, 0.0);
        if !sideways {
            draw_close(
                canvas,
                &t,
                close_x,
                close_y,
                15.0,
                self.pressed(Target::Close),
            )?;
            self.close_button = rect(close_x - 24.0, close_y - 24.0, 48.0, 48.0);
        }

        if self.max_scroll > 0.0 && self.scroll < self.max_scroll - 1.0 {
            // More below: a short band at the cut edge, in the colour of
            // what it covers.
            let more_size = l.f(12.0);
            let fade_h = 22.0f32.max(more_size + 12.0);
            let (band, under, corners) = if sideways {
                (
                    rect(area.x, area.y + area.height - fade_h, area.width, fade_h),
                    t.group,
                    gfx::CornerRadii {
                        top_left: 0.0,
                        top_right: 0.0,
                        bottom_right: 16.0,
                        bottom_left: 16.0,
                    },
                )
            } else {
                (
                    rect(view.x, view.y + view.height - fade_h, view.width, fade_h),
                    t.card,
                    radii(0.0),
                )
            };
            // A short fade into a solid band, so the words under it go
            // out of sight rather than through "More below". Small, so
            // cheap on a phone's CPU painter.
            canvas2d::linear_gradient(
                canvas,
                rect(band.x, band.y - fade_h, band.width, fade_h),
                color(under.r, under.g, under.b, 0.0),
                under,
            )?;
            canvas2d::fill_round_rect(canvas, band, corners, under)?;
            let more = "More below";
            let mm = canvas2d::measure_text_styled(canvas, more, more_size, style(600, 0.0))?;
            canvas2d::draw_text_styled(
                canvas,
                more,
                gfx::Point {
                    x: band.x + (band.width - mm.width) / 2.0,
                    y: centred_baseline(band.y, band.height, more_size),
                },
                more_size,
                t.ink3,
                style(600, 0.0),
            )?;
        }

        // The choice.
        let open = rect(head_x, buttons_y, head_w, open_h);
        let open_fill = if self.pressed(Target::Open) {
            t.btn_press
        } else {
            t.btn
        };
        draw_button(canvas, l, open, "Open", open_fill, t.btn_ink)?;
        self.open_button = open;
        let cancel = rect(head_x, open.y + open_h + 6.0, head_w, cancel_h);
        let not_now = "Not Now";
        let nn_size = l.f(16.0);
        let nm = canvas2d::measure_text_styled(canvas, not_now, nn_size, style(600, 0.0))?;
        let nn_ink = if self.pressed(Target::Cancel) {
            t.accent_press
        } else {
            t.accent
        };
        canvas2d::draw_text_styled(
            canvas,
            not_now,
            gfx::Point {
                x: cancel.x + (cancel.width - nm.width) / 2.0,
                y: centred_baseline(cancel.y, cancel.height, nn_size),
            },
            nn_size,
            nn_ink,
            style(600, 0.0),
        )?;
        self.cancel_button = cancel;

        // What a screen reader says (IC-926): the app, each request and
        // whether it is on, and the two buttons.
        let mut said = krate::layout::Said::new();
        said.heading(
            krate::layout::Rect::new(head_x, hy, head_w, head_block),
            &self.name,
        );
        said.text(
            krate::layout::Rect::new(head_x, hy + head_block, head_w, promise_h),
            &promise,
        );
        for (hit, row) in self.rows.iter().zip(&texts) {
            // Only the rows in view: the rest are scrolled out of the card.
            if hit.y1 <= area.y || hit.y0 >= area.y + area.height {
                continue;
            }
            let ask = &self.asks[hit.index];
            let state = if ask.required {
                "required"
            } else if ask.granted {
                "on"
            } else {
                "off"
            };
            let words = format!("{}, {}. {}", row.titles.join(" "), state, ask.rationale);
            let r = krate::layout::Rect::new(area.x, hit.y0, area.width, hit.y1 - hit.y0);
            if ask.required {
                said.text(r, &words);
            } else {
                said.button(r, &words);
            }
        }
        said.button(
            krate::layout::Rect::new(open.x, open.y, open.width, open.height),
            "Open",
        );
        said.button(
            krate::layout::Rect::new(cancel.x, cancel.y, cancel.width, cancel.height),
            not_now,
        );
        said.send(self.window);

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
                let in_rows = inside(self.scroll_area, p.x, p.y);
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

/// The window and canvas every mode draws on, or None when the player gave
/// no window.
fn open_canvas(title: &str) -> Option<(u64, u64)> {
    let win = window::create(
        title,
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
    Some((win, canvas))
}

/// The home screen: every app the person has, in the order given. Returns
/// the index tapped, or None when the window is dismissed.
fn home(apps: &[(&str, &str)]) -> Option<usize> {
    let (win, canvas) = open_canvas("Krate")?;
    let mut scroll = 0.0f32;
    loop {
        let l = Layout::now(canvas, win);
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
    let t = l.t;
    canvas2d::clear(canvas, t.page)?;
    let x = l.col_x + 20.0;
    let w = l.col_w - 40.0;
    let top = l.safe.0 + 24.0 + l.f(40.0) - scroll;
    canvas2d::draw_text_styled(
        canvas,
        "Krate",
        gfx::Point { x, y: top },
        l.f(34.0),
        t.ink,
        style(750, -0.6),
    )?;
    canvas2d::draw_text_styled(
        canvas,
        "Your apps",
        gfx::Point {
            x,
            y: top + l.f(28.0),
        },
        l.f(15.0),
        t.ink2,
        style(450, 0.0),
    )?;
    let mut rows = Vec::new();
    let mut y = top + l.f(52.0);
    let (name_size, line_size) = (l.f(17.0), l.f(13.0));
    let text_x = x + 72.0;
    let text_w = w - 72.0 - 14.0;
    for (index, (name, line)) in apps.iter().enumerate() {
        let h = 72.0f32.max(name_size + line_size + 34.0);
        draw_card(canvas, &t, rect(x, y, w, h), radii(18.0))?;
        // The app's initial on a tile, until apps carry icons.
        let tile = rect(x + 14.0, y + (h - 44.0) / 2.0, 44.0, 44.0);
        canvas2d::fill_round_rect(canvas, tile, radii(11.0), t.accent)?;
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
            theme::WHITE,
            style(700, 0.0),
        )?;
        let gap = 4.0;
        let block = name_size + gap + line_size;
        let name_y = y + (h - block) / 2.0 + name_size * 0.85;
        canvas2d::draw_text_styled(
            canvas,
            &fit_line(canvas, name, name_size, style(600, 0.0), text_w),
            gfx::Point {
                x: text_x,
                y: name_y,
            },
            name_size,
            t.ink,
            style(600, 0.0),
        )?;
        canvas2d::draw_text_styled(
            canvas,
            &fit_line(canvas, line, line_size, style(450, 0.0), text_w),
            gfx::Point {
                x: text_x,
                y: name_y + gap + line_size * 1.15,
            },
            line_size,
            t.ink2,
            style(450, 0.0),
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
    let hint_size = l.f(14.0);
    let mut hy = y + 8.0;
    for line in wrap_lines(canvas, hint, hint_size, style(450, 0.0), w) {
        hy += l.f(19.0);
        canvas2d::draw_text_styled(
            canvas,
            &line,
            gfx::Point { x, y: hy },
            hint_size,
            t.ink3,
            style(450, 0.0),
        )?;
    }
    canvas2d::present(canvas)?;
    Ok(rows)
}

/// Show why something did not open, until Close is tapped or the window is
/// dismissed.
fn notice(title: &str, message: &str) {
    let Some((win, canvas)) = open_canvas("Krate") else {
        return;
    };
    loop {
        let l = Layout::now(canvas, win);
        let close = match draw_notice(canvas, &l, title, message) {
            Ok(close) => close,
            Err(_) => return,
        };
        let _ = window::request_redraw(win);
        match events::wait(None) {
            Some(types::Event::CloseRequested(_)) => return,
            Some(types::Event::Pointer(p)) if p.pressed && p.button.is_some() => {
                if inside(close, p.x, p.y) {
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
) -> Result<gfx::Rect, gfx::GfxError> {
    let t = l.t;
    let (top, _, bottom, _) = l.safe;
    let x = l.col_x + 40.0;
    let text_w = l.col_w - 80.0;
    let (title_size, message_size) = (l.f(22.0), l.f(15.5));
    let title_lines = wrap_at_most(canvas, title, title_size, style(700, -0.3), text_w, 3);
    let bh = button_h(l);
    // Everything but the message, then as much of the message as the
    // screen has room for: the Close button never leaves it.
    let fixed = 28.0 + title_lines.len() as f32 * l.f(28.0) + 8.0 + 20.0 + bh + 24.0;
    let room = l.h - top - bottom - 32.0;
    let most = (((room - fixed) / l.f(21.0)).max(0.0) as usize).max(2);
    if wrap_lines(canvas, message, message_size, style(450, 0.0), text_w).len() > most {
        if let Some(smaller) = l.smaller() {
            return draw_notice(canvas, &smaller, title, message);
        }
    }
    canvas2d::clear(canvas, t.page)?;
    let lines = wrap_at_most(canvas, message, message_size, style(450, 0.0), text_w, most);
    let card_h = fixed + lines.len() as f32 * l.f(21.0);
    let card_top = (top + (l.h - top - bottom - card_h) / 2.0).max(top + 16.0);
    draw_card(
        canvas,
        &t,
        rect(l.col_x + 16.0, card_top, l.col_w - 32.0, card_h),
        radii(24.0),
    )?;
    let mut y = card_top + 28.0;
    for line in &title_lines {
        y += l.f(28.0);
        canvas2d::draw_text_styled(
            canvas,
            line,
            gfx::Point { x, y: y - l.f(6.0) },
            title_size,
            t.ink,
            style(700, -0.3),
        )?;
    }
    y += 8.0;
    for line in &lines {
        y += l.f(21.0);
        canvas2d::draw_text_styled(
            canvas,
            line,
            gfx::Point { x, y: y - l.f(5.0) },
            message_size,
            t.ink2,
            style(450, 0.0),
        )?;
    }
    let close = rect(x, y + 20.0, text_w, bh);
    draw_button(canvas, l, close, "Close", t.btn, t.btn_ink)?;

    let mut said = krate::layout::Said::new();
    said.heading(
        krate::layout::Rect::new(x, card_top, text_w, 28.0 + l.f(28.0)),
        title,
    );
    said.text(
        krate::layout::Rect::new(x, card_top + 40.0, text_w, close.y - card_top - 40.0),
        message,
    );
    said.button(
        krate::layout::Rect::new(close.x, close.y, close.width, close.height),
        "Close",
    );
    said.send(l.win);
    canvas2d::present(canvas)?;
    Ok(close)
}

/// Show what a command-line app printed, until Close is tapped or the window
/// is dismissed. A finger or a wheel scrolls it.
fn output(name: &str, ended: &str, lines: &[&str]) {
    let Some((win, canvas)) = open_canvas("Krate") else {
        return;
    };
    let mut scroll = 0.0f32;
    // Sized and wrapped once per width and text size, not on every frame.
    let mut wrapped: ((f32, f32), f32, Vec<String>) = ((0.0, 0.0), 0.0, Vec::new());
    loop {
        let l = Layout::now(canvas, win);
        let key = (l.col_w, l.look.text_scale);
        if wrapped.0 != key {
            let max_w = l.col_w - 48.0;
            let size = output_size(canvas, lines, l.f(OUTPUT_SIZE), max_w);
            wrapped = (key, size, wrap_output(canvas, lines, size, max_w));
        }
        let drawn = match draw_output(canvas, &l, name, ended, &wrapped.2, wrapped.1, scroll) {
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

/// The size of the fixed-width text, and the room between its lines, at
/// the default text size.
const OUTPUT_SIZE: f32 = 13.0;
const OUTPUT_LINE: f32 = 18.0;

/// The fixed-width face output is drawn in.
fn mono() -> gfx::TextStyle {
    gfx::TextStyle {
        family: gfx::FontFamily::Mono,
        ..style(400, 0.0)
    }
}

/// Lines measured when choosing the output's size: enough for any screen
/// of output, bounded so thousands of printed lines do not each cost a
/// measurement on a phone.
const OUTPUT_MEASURED: usize = 2000;

/// The size to draw what an app printed at, the way a terminal fits its
/// columns: `base` (the person's text size) when the widest line fits,
/// else small enough that it does -- but never under 0.7 of `base`, nor
/// under 9 points, so it stays readable. Lines still too wide then wrap.
fn output_size(canvas: u64, lines: &[&str], base: f32, max_w: f32) -> f32 {
    let widest = lines
        .iter()
        .take(OUTPUT_MEASURED)
        .filter_map(|line| {
            canvas2d::measure_text_styled(canvas, &line.replace('\t', "    "), base, mono()).ok()
        })
        .fold(0.0f32, |w, m| w.max(m.width));
    if widest <= max_w || widest <= 0.0 {
        return base;
    }
    let floor = (base * 0.7).max(9.0).min(base);
    // A hair under the exact ratio: widths do not scale perfectly with size.
    (base * max_w / widest * 0.995).max(floor)
}

/// Break every line still too wide for the card. A break goes at the last
/// space that fits, the boundary between two columns of a hex dump or a
/// table, and mid-token only when a token alone is wider than the card.
/// Each piece is measured as drawn -- box-drawing and other symbols come
/// from fallback faces of other widths, so no one character's width holds
/// for a whole line.
fn wrap_output(canvas: u64, lines: &[&str], size: f32, max_w: f32) -> Vec<String> {
    let mono = mono();
    let fits = |chars: &[char]| {
        let text: String = chars.iter().collect();
        canvas2d::measure_text_styled(canvas, &text, size, mono)
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
            if lo < rest.len() {
                // Back to the last space that fits, if there is one past
                // the first character.
                if let Some(space) = (1..=lo).rev().find(|&i| rest[i] == ' ') {
                    let piece: String = rest[..space].iter().collect();
                    if !piece.trim().is_empty() {
                        lo = space;
                    }
                }
            }
            let piece: String = rest[..lo].iter().collect();
            out.push(piece.trim_end().to_string());
            rest.drain(..lo);
            // The spaces at a break are the break.
            while rest.first() == Some(&' ') {
                rest.remove(0);
            }
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
    mono_size: f32,
    scroll: f32,
) -> Result<(gfx::Rect, f32), gfx::GfxError> {
    let t = l.t;
    canvas2d::clear(canvas, t.page)?;
    let x = l.col_x + 24.0;
    let text_w = l.col_w - 48.0;
    let (name_size, ended_size) = (l.f(22.0), l.f(14.5));
    let mono_line = mono_size * OUTPUT_LINE / OUTPUT_SIZE;
    let mut y = l.safe.0 + 24.0;
    let name_lines = wrap_at_most(canvas, name, name_size, style(700, -0.3), text_w, 2);
    for line in &name_lines {
        y += l.f(28.0);
        canvas2d::draw_text_styled(
            canvas,
            line,
            gfx::Point { x, y: y - l.f(4.0) },
            name_size,
            t.ink,
            style(700, -0.3),
        )?;
    }
    let ended_lines = wrap_at_most(canvas, ended, ended_size, style(450, 0.0), text_w, 2);
    for line in &ended_lines {
        y += l.f(20.0);
        canvas2d::draw_text_styled(
            canvas,
            line,
            gfx::Point { x, y: y - l.f(2.0) },
            ended_size,
            t.ink2,
            style(450, 0.0),
        )?;
    }
    let heading_h = y - l.safe.0;
    let bh = button_h(l);
    let close = rect(x, l.h - l.safe.2 - 24.0 - bh, text_w, bh);
    let area_top = y + 16.0;
    let area = rect(
        l.col_x + 8.0,
        area_top,
        l.col_w - 16.0,
        (close.y - 16.0 - area_top).max(40.0),
    );
    draw_card(canvas, &t, area, radii(18.0))?;
    let mono = mono();
    let content = 16.0 + lines.len() as f32 * mono_line + 8.0;
    let room = (content - area.height).max(0.0);
    let scroll = scroll.min(room);
    canvas2d::set_clip(canvas, area.x, area.y, area.width, area.height)?;
    if lines.is_empty() {
        canvas2d::draw_text_styled(
            canvas,
            "It printed nothing.",
            gfx::Point {
                x,
                y: area.y + 16.0 + l.f(14.5),
            },
            l.f(14.5),
            t.ink3,
            style(450, 0.0),
        )?;
    }
    // Only the lines in view are drawn: an app may print thousands.
    let first = ((scroll - 16.0) / mono_line).max(0.0) as usize;
    let shown = (area.height / mono_line) as usize + 2;
    for (i, line) in lines.iter().enumerate().skip(first).take(shown) {
        let y = area.y + 16.0 + (i as f32 + 1.0) * mono_line - mono_line * 0.25 - scroll;
        canvas2d::draw_text_styled(canvas, line, gfx::Point { x, y }, mono_size, t.ink, mono)?;
    }
    canvas2d::clear_clip(canvas)?;
    draw_button(canvas, l, close, "Close", t.btn, t.btn_ink)?;

    let mut said = krate::layout::Said::new();
    said.heading(
        krate::layout::Rect::new(x, l.safe.0, text_w, heading_h),
        name,
    );
    if !lines.is_empty() {
        let in_view: Vec<&str> = lines
            .iter()
            .skip(first)
            .take(shown)
            .map(String::as_str)
            .collect();
        said.text(
            krate::layout::Rect::new(area.x, area.y, area.width, area.height),
            &in_view.join("\n"),
        );
    }
    said.button(
        krate::layout::Rect::new(close.x, close.y, close.width, close.height),
        "Close",
    );
    said.send(l.win);
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
            // slid in. `quick details` draws it with the details open, so
            // that view can be looked at headless too.
            sheet.entrance = Spring::rest_at(1.0, 18.0);
            sheet.details = raw.split(ARG_SEP).any(|a| a.trim() == "details");
        }
        let mut last = clock::monotonic_nanos();
        let mut frames: u32 = 0;
        let mut decision: Option<bool> = None;

        loop {
            let l = Layout::now(canvas, win);
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
