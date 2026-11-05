//! Krate tidy -- the folder app that used to be impossible.
//!
//! Pick a folder; Tidy sorts what is inside into Images, Documents, Audio,
//! Archives and Other, by extension. That is the whole app -- and it is the
//! worked example of the pattern that makes folder apps buildable at all:
//! **the pick is the grant**. The manifest declares no fs capability. The
//! person chooses a folder in the native dialog, the app receives a token,
//! and every ordinary fs call works under `picked/<token>/...` for this run,
//! inside that folder and nowhere else.
//!
//! An outside reviewer once asked for exactly this app and found it could
//! not exist: no path could be named, no dialog existed, and the generator
//! reached for `fs.*:**` because nothing else even looked like it might
//! work. This file is the after picture.
//!
//! `quick` proves the sorting brain without any dialog (headless runs
//! auto-cancel pickers): it classifies a fixed set of names, prints the
//! plan, draws a frame, and exits.

#![no_std]

extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use krate::bindings::krate::fs::files as fsapi;
use krate::bindings::krate::io::{args, stdio};
use krate::bindings::krate::time::clock;
use krate::gfx::{canvas2d, types as gfx};
use krate::layout::{Look, Rect, Said};
use krate::motion::Spring;
use krate::ui::{dialog, events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

/// The size the window is asked for on a desktop. What the host grants is
/// what Tidy lays out for: a phone hands back its whole screen.
const REQUEST_W: u32 = 420;
const REQUEST_H: u32 = 620;

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

/// Where one file belongs, by its extension. The categories a person would
/// actually make by hand.
fn category(name: &str) -> Option<&'static str> {
    let ext = name.rsplit_once('.').map(|(_, e)| e)?;
    let ext = ext.to_ascii_lowercase();
    Some(match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "bmp" | "svg" => "Images",
        "pdf" | "doc" | "docx" | "txt" | "md" | "rtf" | "odt" | "csv" | "xlsx" => "Documents",
        "mp3" | "wav" | "flac" | "m4a" | "ogg" | "aac" => "Audio",
        "zip" | "tar" | "gz" | "7z" | "rar" | "dmg" => "Archives",
        _ => "Other",
    })
}

/// One planned or completed move.
struct Move {
    name: String,
    dest: &'static str,
}

/// Sort the folder behind `token`. Lists it, makes the category folders,
/// moves each file. Everything runs under `picked/<token>/...`, which the
/// open-folder grant authorizes -- note there is not one fs capability in
/// the manifest.
fn tidy_picked(token: &str) -> Result<Vec<Move>, String> {
    let root = format!("picked/{token}");
    let names = fsapi::list(&root).map_err(|e| format!("could not list the folder: {e:?}"))?;

    let mut moves = Vec::new();
    for name in names {
        // Never descend into or move the folders we make ourselves, and skip
        // anything without an extension or that is a directory.
        let Some(dest) = category(&name) else {
            continue;
        };
        let stat = fsapi::stat(&format!("{root}/{name}"));
        if matches!(stat, Ok(s) if s.is_dir) {
            continue;
        }
        let _ = fsapi::mkdir(&format!("{root}/{dest}"));
        fsapi::rename(&format!("{root}/{name}"), &format!("{root}/{dest}/{name}"))
            .map_err(|e| format!("could not move {name}: {e:?}"))?;
        moves.push(Move { name, dest });
    }
    Ok(moves)
}

/// What the app is showing right now.
enum State {
    Idle,
    Done { folder: String, moves: Vec<Move> },
    Failed(String),
}

/// The categories as a person reads them, with a few of the extensions
/// that land in each.
const SORTS: [(&str, &str); 5] = [
    ("Images", "jpg, png, gif, webp, heic, svg"),
    ("Documents", "pdf, docx, txt, md, csv, xlsx"),
    ("Audio", "mp3, wav, flac, m4a, ogg"),
    ("Archives", "zip, tar, gz, 7z, rar, dmg"),
    ("Other", "anything else with an extension"),
];

const NOTE: &str = "Krate lets Tidy touch only the folder you pick, and only for this run.";
const PILL_LABEL: &str = "Pick a folder to tidy";
const SUBTITLE: &str = "Sort a folder into Images, Documents, Audio and more";

/// Tidy's colours for the person's look: the deep navy it was designed in
/// when the phone is dark, a pale version of it when it is light.
struct Theme {
    sky: (gfx::Color, gfx::Color),
    card: gfx::Color,
    card_line: gfx::Color,
    shadow: gfx::Color,
    ink: gfx::Color,
    muted: gfx::Color,
    accent: gfx::Color,
    glow: gfx::Color,
    bad: gfx::Color,
}

impl Theme {
    fn of(look: &Look) -> Self {
        if look.dark {
            Theme {
                sky: (color(0.05, 0.06, 0.11, 1.0), color(0.10, 0.12, 0.22, 1.0)),
                card: color(1.0, 1.0, 1.0, 0.05),
                card_line: color(1.0, 1.0, 1.0, 0.12),
                shadow: color(0.0, 0.0, 0.0, 0.4),
                ink: color(1.0, 1.0, 1.0, 0.95),
                muted: color(1.0, 1.0, 1.0, if look.high_contrast { 0.8 } else { 0.6 }),
                accent: color(0.42, 0.55, 1.0, 1.0),
                glow: color(0.30, 0.42, 1.0, 0.45),
                bad: color(1.0, 0.6, 0.6, 0.95),
            }
        } else {
            Theme {
                sky: (color(0.95, 0.96, 0.99, 1.0), color(0.88, 0.90, 0.98, 1.0)),
                card: color(1.0, 1.0, 1.0, 1.0),
                card_line: color(0.10, 0.12, 0.22, 0.10),
                shadow: color(0.10, 0.12, 0.30, 0.12),
                ink: color(0.07, 0.08, 0.15, 1.0),
                muted: if look.high_contrast {
                    color(0.20, 0.22, 0.30, 1.0)
                } else {
                    color(0.38, 0.40, 0.50, 1.0)
                },
                accent: color(0.28, 0.40, 0.95, 1.0),
                glow: color(0.28, 0.40, 0.95, 0.30),
                bad: color(0.75, 0.15, 0.18, 1.0),
            }
        }
    }
}

fn grect(r: Rect) -> gfx::Rect {
    rect(r.x, r.y, r.w, r.h)
}

/// Text measured and broken into lines once per string and size: the pill
/// breathes every frame, and on a phone the app's own code runs on an
/// interpreter, so a frame must not measure the same words again.
struct Words {
    canvas: u64,
    widths: BTreeMap<u64, f32>,
    wraps: BTreeMap<u64, Vec<String>>,
    fits: BTreeMap<u64, String>,
}

/// A key for a string at a size: FNV-1a over the bytes and the numbers, so
/// a lookup allocates nothing.
fn key(text: &str, size: f32, weight: u16, max: f32) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |b: u8| {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    };
    text.bytes().for_each(&mut eat);
    size.to_bits().to_le_bytes().into_iter().for_each(&mut eat);
    weight.to_le_bytes().into_iter().for_each(&mut eat);
    max.to_bits().to_le_bytes().into_iter().for_each(&mut eat);
    h
}

impl Words {
    fn new(canvas: u64) -> Self {
        Words {
            canvas,
            widths: BTreeMap::new(),
            wraps: BTreeMap::new(),
            fits: BTreeMap::new(),
        }
    }

    fn width(&mut self, text: &str, size: f32, weight: u16) -> f32 {
        let k = key(text, size, weight, 0.0);
        if let Some(w) = self.widths.get(&k) {
            return *w;
        }
        let w = canvas2d::measure_text_styled(self.canvas, text, size, style(weight, 0.0))
            .map(|m| m.width)
            .unwrap_or(text.len() as f32 * size * 0.55);
        if self.widths.len() > 2000 {
            self.widths.clear();
        }
        self.widths.insert(k, w);
        w
    }

    /// `text` broken into lines no wider than `max`.
    fn wrap(&mut self, text: &str, size: f32, weight: u16, max: f32) -> Vec<String> {
        let k = key(text, size, weight, max);
        if let Some(lines) = self.wraps.get(&k) {
            return lines.clone();
        }
        let mut lines = Vec::new();
        let mut line = String::new();
        for word in text.split_whitespace() {
            let tried = if line.is_empty() {
                String::from(word)
            } else {
                format!("{line} {word}")
            };
            if line.is_empty() || self.width(&tried, size, weight) <= max {
                line = tried;
            } else {
                lines.push(core::mem::replace(&mut line, String::from(word)));
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
        if self.wraps.len() > 500 {
            self.wraps.clear();
        }
        self.wraps.insert(k, lines.clone());
        lines
    }

    /// `text` cut to `max` wide, with an ellipsis when it had to be cut.
    fn fit(&mut self, text: &str, size: f32, weight: u16, max: f32) -> String {
        let k = key(text, size, weight, max);
        if let Some(fitted) = self.fits.get(&k) {
            return fitted.clone();
        }
        let fitted = if self.width(text, size, weight) <= max {
            String::from(text)
        } else {
            let chars: Vec<char> = text.chars().collect();
            let (mut lo, mut hi) = (0usize, chars.len());
            while lo < hi {
                let mid = (lo + hi).div_ceil(2);
                let cut: String = chars[..mid].iter().collect();
                if self.width(&format!("{cut}\u{2026}"), size, weight) <= max {
                    lo = mid;
                } else {
                    hi = mid - 1;
                }
            }
            let cut: String = chars[..lo].iter().collect();
            format!("{cut}\u{2026}")
        };
        if self.fits.len() > 500 {
            self.fits.clear();
        }
        self.fits.insert(k, fitted.clone());
        fitted
    }
}

/// One frame's drawing context: the canvas, the person's look, and the
/// screen reader's list being filled as things are drawn.
struct Frame<'a> {
    canvas: u64,
    look: Look,
    theme: Theme,
    words: &'a mut Words,
    said: Said,
    /// Measuring only: lay out without drawing, to size the card.
    dry: bool,
}

impl Frame<'_> {
    /// Draw lines from `top`, `size * leading` apart, and return the
    /// bottom of the last one.
    fn lines(
        &mut self,
        lines: &[String],
        x: f32,
        top: f32,
        size: f32,
        weight: u16,
        ink: gfx::Color,
    ) -> Result<f32, gfx::GfxError> {
        let step = size * 1.4;
        let mut y = top;
        for line in lines {
            if self.dry {
                y += step;
                continue;
            }
            canvas2d::draw_text_styled(
                self.canvas,
                line,
                gfx::Point {
                    x,
                    y: y + size * 1.05,
                },
                size,
                ink,
                style(weight, 0.0),
            )?;
            y += step;
        }
        Ok(y)
    }

    /// A paragraph wrapped to `w`, said as one piece of text.
    fn paragraph(
        &mut self,
        text: &str,
        at: Rect,
        size: f32,
        weight: u16,
        ink: gfx::Color,
    ) -> Result<f32, gfx::GfxError> {
        let lines = self.words.wrap(text, size, weight, at.w);
        let bottom = self.lines(&lines, at.x, at.y, size, weight, ink)?;
        self.said
            .text(Rect::new(at.x, at.y, at.w, bottom - at.y), text);
        Ok(bottom)
    }
}

/// Lay Tidy out for the screen it is on and draw it. Returns the pill's
/// rectangle, so the click handling hits exactly what was drawn.
///
/// A phone gets one column: the title, the card, and the pill at the
/// bottom where a thumb reaches it. A wider screen puts the title, the
/// note and the pill in a column on the left and gives the card the rest
/// of the width.
fn draw(
    win: u64,
    canvas: u64,
    words: &mut Words,
    state: &State,
    pill_scale: f32,
) -> Result<Rect, gfx::GfxError> {
    let size = canvas2d::canvas_size(canvas)?;
    let (w, h) = (size.width.max(1.0), size.height.max(1.0));
    let screen = krate::layout::Screen::of(win);
    let look = screen.look;
    let mut f = Frame {
        canvas,
        look,
        theme: Theme::of(&look),
        words,
        said: Said::new(),
        dry: false,
    };
    let (sky0, sky1) = f.theme.sky;
    canvas2d::linear_gradient_stops(
        canvas,
        rect(0.0, 0.0, w, h),
        115.0,
        &[
            gfx::GradientStop {
                offset: 0.0,
                color: sky0,
            },
            gfx::GradientStop {
                offset: 1.0,
                color: sky1,
            },
        ],
    )?;

    let (top, right, bottom, left) = screen.safe;
    let (free_w, free_h) = (w - left - right, h - top - bottom);
    // Three shapes. A phone held upright: one column, the pill along the
    // bottom where a thumb reaches it. A screen wider than it is tall (a
    // desktop window, a phone turned sideways): the title, note and pill
    // in a column beside the card. Anything else roomy (a tablet held
    // upright): one column of a readable width. The roomy shapes sit in
    // the middle of the screen rather than in its top corner.
    let two = free_w >= 640.0 && free_w > free_h;
    // A narrow desktop window is roomy too: the pill goes under the card
    // for a mouse, and along the bottom only for a thumb.
    let roomy = free_w >= 600.0 || !screen.touch;
    let pad = if roomy && free_h >= 560.0 { 40.0 } else { 20.0 };
    let mut area = Rect::new(0.0, 0.0, w, h)
        .inset(top, right, bottom, left)
        .pad(pad);
    let most = if two {
        1040.0
    } else if roomy {
        560.0
    } else {
        area.w
    };
    if area.w > most {
        area.x += (area.w - most) / 2.0;
        area.w = most;
    }

    let title = look.text(34.0);
    let body = look.text(15.0);
    let small = look.text(13.0);
    let pill_text = look.text(17.0);
    let pill_h = (pill_text * 3.0).max(screen.tap() + 8.0);

    // The column holding the title, the note and the pill: the whole
    // width in one column, a column beside the card otherwise.
    let mut room = area;
    let side = if two {
        let col = room.take_left((room.w * 0.4).clamp(280.0, 400.0));
        room.take_left(40.0);
        col
    } else {
        area
    };
    let sub_lines = f.words.wrap(SUBTITLE, body, 400, side.w);
    let note_lines = f.words.wrap(NOTE, small, 400, side.w);
    let head_h = title * 1.25 + sub_lines.len() as f32 * body * 1.4 + 20.0;
    let note_h = note_lines.len() as f32 * small * 1.4;
    let pill_w = side.w.min(if two { 360.0 } else { 420.0 });
    if !two {
        room.y += head_h;
        room.h = (room.h - head_h - 18.0 - note_h - 14.0 - pill_h).max(0.0);
    }

    // The card is as tall as what it holds, up to the room there is: lay
    // the content out once without drawing to learn its height. Both
    // passes lay out in the same room, so they make the same choices.
    let inset = if pad > 20.0 { 28.0 } else { 20.0 };
    let saved = core::mem::take(&mut f.said);
    f.dry = true;
    let end = draw_card(&mut f, state, room.pad(inset), body, small)?;
    f.dry = false;
    f.said = saved;
    let card_h = room.h.min(end - room.y + inset);

    let block = if two {
        (head_h + note_h + 20.0 + pill_h).max(card_h)
    } else {
        head_h + card_h + 18.0 + note_h + 14.0 + pill_h
    };
    let y0 = if roomy {
        area.y + ((area.h - block) / 2.0).max(0.0)
    } else {
        area.y
    };
    let card = if two {
        Rect::new(room.x, y0, room.w, card_h)
    } else {
        Rect::new(room.x, y0 + head_h, room.w, card_h)
    };
    let (note_at, pill_at) = if two {
        let note = Rect::new(side.x, y0 + head_h, side.w, note_h);
        let pill = Rect::new(side.x, note.bottom() + 20.0, pill_w, pill_h);
        (note, pill)
    } else if roomy {
        let note = Rect::new(side.x, card.bottom() + 18.0, side.w, note_h);
        let pill_x = side.x + (side.w - pill_w) / 2.0;
        (
            note,
            Rect::new(pill_x, note.bottom() + 14.0, pill_w, pill_h),
        )
    } else {
        let pill_x = side.x + (side.w - pill_w) / 2.0;
        let pill = Rect::new(pill_x, area.bottom() - pill_h, pill_w, pill_h);
        (
            Rect::new(side.x, pill.y - 14.0 - note_h, side.w, note_h),
            pill,
        )
    };

    // Title and the line under it.
    canvas2d::draw_text_styled(
        canvas,
        "Tidy",
        gfx::Point {
            x: side.x,
            y: y0 + title * 0.9,
        },
        title,
        f.theme.ink,
        style(700, -0.8),
    )?;
    f.said
        .heading(Rect::new(side.x, y0, side.w, title * 1.2), "Tidy");
    let muted = f.theme.muted;
    let sub_top = y0 + title * 1.25;
    let sub_end = f.lines(&sub_lines, side.x, sub_top, body, 400, muted)?;
    f.said.text(
        Rect::new(side.x, sub_top, side.w, sub_end - sub_top),
        SUBTITLE,
    );

    // The card: what will happen, or what did.
    canvas2d::drop_shadow_round_rect(
        canvas,
        rect(card.x, card.y + 6.0, card.w, card.h),
        radii(18.0),
        16.0,
        f.theme.shadow,
    )?;
    canvas2d::fill_round_rect(canvas, grect(card), radii(18.0), f.theme.card)?;
    canvas2d::stroke_round_rect(canvas, grect(card), radii(18.0), 1.0, f.theme.card_line)?;
    let inner = Rect::new(card.x, card.y, room.w, room.h).pad(inset);
    canvas2d::set_clip(canvas, card.x, card.y, card.w, card.h)?;
    draw_card(&mut f, state, inner, body, small)?;
    canvas2d::clear_clip(canvas)?;

    f.lines(&note_lines, note_at.x, note_at.y, small, 400, muted)?;
    f.said.text(note_at, NOTE);

    // The pill, breathing on its spring while idle.
    let grow_x = pill_at.w * (pill_scale - 1.0) / 2.0;
    let grow_y = pill_at.h * (pill_scale - 1.0) / 2.0;
    let pill = rect(
        pill_at.x - grow_x,
        pill_at.y - grow_y,
        pill_at.w + grow_x * 2.0,
        pill_at.h + grow_y * 2.0,
    );
    let round = radii(pill.height / 2.0);
    canvas2d::drop_shadow_round_rect(
        canvas,
        rect(pill.x, pill.y + 5.0, pill.width, pill.height),
        round,
        14.0,
        f.theme.glow,
    )?;
    canvas2d::fill_round_rect(canvas, pill, round, f.theme.accent)?;
    let label = f.words.fit(PILL_LABEL, pill_text, 600, pill.width - 24.0);
    let label_w = f.words.width(&label, pill_text, 600);
    canvas2d::draw_text_styled(
        canvas,
        &label,
        gfx::Point {
            x: pill.x + (pill.width - label_w) / 2.0,
            y: pill.y + pill.height / 2.0 + pill_text * 0.35,
        },
        pill_text,
        color(1.0, 1.0, 1.0, 1.0),
        style(600, 0.2),
    )?;
    f.said.button(pill_at, PILL_LABEL);

    canvas2d::present(canvas)?;
    f.said.send(win);
    Ok(pill_at)
}

/// What is inside the card for each state, laid out top-down in `inner`.
/// Returns where the content ends.
fn draw_card(
    f: &mut Frame<'_>,
    state: &State,
    inner: Rect,
    body: f32,
    small: f32,
) -> Result<f32, gfx::GfxError> {
    let (ink, muted) = (f.theme.ink, f.theme.muted);
    let end = match state {
        State::Idle => {
            let heading = f.look.text(20.0);
            let y = f.lines(
                &[String::from("Where things go")],
                inner.x,
                inner.y,
                heading,
                650,
                ink,
            )?;
            f.said.heading(
                Rect::new(inner.x, inner.y, inner.w, y - inner.y),
                "Where things go",
            );
            let mut y = y + 8.0;
            // A name column as wide as the widest name, when the
            // extensions still fit beside it; otherwise each under its
            // name.
            let name_w = SORTS
                .iter()
                .map(|(name, _)| f.words.width(name, body, 600))
                .fold(0.0_f32, f32::max)
                + 16.0;
            // Each set of extensions under its name when they do not fit
            // beside it -- unless the screen is too short for that too (a
            // phone turned sideways, at large text), when they stay beside
            // their names, shortened.
            let fits_beside = SORTS
                .iter()
                .all(|(_, ext)| f.words.width(ext, small, 400) <= inner.w - name_w);
            let stacked_h = SORTS.len() as f32 * (body * 1.4 + small * 1.4 + 12.0);
            let beside = fits_beside || stacked_h > inner.bottom() - y;
            let gap = if beside { 8.0 } else { 12.0 };
            for (name, ext) in SORTS {
                if y + body * 1.4 > inner.bottom() {
                    break;
                }
                let top = y;
                f.lines(&[String::from(name)], inner.x, y, body, 600, ink)?;
                let ext_lines = if beside {
                    alloc::vec![f.words.fit(ext, small, 400, inner.w - name_w)]
                } else {
                    f.words.wrap(ext, small, 400, inner.w)
                };
                let (ex, ey) = if beside {
                    (inner.x + name_w, y + (body - small) * 0.8)
                } else {
                    (inner.x, y + body * 1.4)
                };
                let end = f.lines(&ext_lines, ex, ey, small, 400, muted)?;
                y = end.max(y + body * 1.4) + gap;
                f.said.text(
                    Rect::new(inner.x, top, inner.w, y - top),
                    &format!("{name}: {ext}"),
                );
            }
            y - gap
        }
        State::Done { folder, moves } => {
            let big = f.look.text(26.0);
            let count = format!("{} moved", moves.len());
            let mut y = f.lines(&[count.clone()], inner.x, inner.y, big, 650, ink)?;
            let place = f.words.fit(&format!("in {folder}"), small, 400, inner.w);
            y = f.lines(&[place.clone()], inner.x, y, small, 400, muted)?;
            f.said.heading(
                Rect::new(inner.x, inner.y, inner.w, y - inner.y),
                &format!("{count} {place}"),
            );
            y += 10.0;
            let row = body * 1.5;
            // As many moves as fit, then how many more there are.
            let room = (((inner.bottom() - y) / row) as usize).max(1);
            let shown = if moves.len() > room {
                room.saturating_sub(1)
            } else {
                moves.len()
            };
            for one in moves.iter().take(shown) {
                let line = f.words.fit(
                    &format!("{}  ->  {}/", one.name, one.dest),
                    body,
                    400,
                    inner.w,
                );
                f.lines(&[line.clone()], inner.x, y, body, 400, ink)?;
                f.said.text(
                    Rect::new(inner.x, y, inner.w, row),
                    &format!("{} moved to {}", one.name, one.dest),
                );
                y += row;
            }
            if moves.len() > shown {
                let more = format!("and {} more", moves.len() - shown);
                f.lines(&[more.clone()], inner.x, y, small, 400, muted)?;
                f.said.text(Rect::new(inner.x, y, inner.w, row), &more);
                y += row;
            }
            y
        }
        State::Failed(why) => {
            let bad = f.theme.bad;
            let y = f.lines(
                &[String::from("That did not work:")],
                inner.x,
                inner.y,
                body,
                600,
                bad,
            )?;
            f.said.text(
                Rect::new(inner.x, inner.y, inner.w, y - inner.y),
                "That did not work:",
            );
            f.paragraph(
                why,
                Rect::new(inner.x, y + 4.0, inner.w, inner.h),
                small,
                400,
                muted,
            )?
        }
    };
    Ok(end)
}

fn out(line: &str) {
    let handle = stdio::stdout();
    let _ = handle.write(line.as_bytes());
    let _ = handle.write(b"\n");
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

/// The quick run proves the sorting brain with no dialog and no fs: it
/// classifies a fixed set of names and prints the plan the interactive run
/// would execute.
fn quick_proof() {
    let names = [
        "holiday.jpg",
        "invoice.pdf",
        "song.mp3",
        "backup.zip",
        "notes.txt",
        "mystery.bin",
        "no-extension",
    ];
    let mut planned = 0;
    for name in names {
        if let Some(dest) = category(name) {
            out(&format!("plan:{name}->{dest}"));
            planned += 1;
        }
    }
    out(&format!("planned:{planned}"));
}

struct Component;

impl krate::Guest for Component {
    fn run() -> i32 {
        let raw = args::raw();
        let quick = raw.split_whitespace().any(|word| word == "quick");

        let win = match window::create(
            "Tidy",
            types::WindowSize {
                width: REQUEST_W,
                height: REQUEST_H,
            },
        ) {
            Ok(win) => win,
            Err(_) => {
                out("window:no");
                return 1;
            }
        };
        if tree::set_root(win, &node(ROOT_ID, None, types::WidgetKind::Stack)).is_err() {
            out("tree:no");
            return 1;
        }
        let _ = tree::upsert_node(
            win,
            &node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas),
        );
        let canvas = match canvas2d::bind(win, CANVAS_ID) {
            Ok(canvas) => canvas,
            Err(_) => {
                out("bind:no");
                return 1;
            }
        };
        // No design size: every frame lays out from the canvas's real size
        // and the screen's covered edges, so a phone, a tablet and a wide
        // desktop window each get a layout made for them.
        let mut words = Words::new(canvas);
        let mut state = State::Idle;
        let mut pill_at = Rect::new(0.0, 0.0, 0.0, 0.0);
        let mut pill = Spring::rest_at(1.0, 14.0);
        let mut pill_target = 1.0f32;
        let started = clock::monotonic_nanos();
        let mut last = started;
        let mut frames: u32 = 0;
        // Draw only when something changed: the pill moving on its spring,
        // a resize, a new result, or the person's look. A frame drawn for
        // nothing costs a phone, whose interpreter runs this code, the
        // time it needs to answer a scroll elsewhere.
        let mut dirty = true;
        let mut look = Look::now();

        loop {
            let now = clock::monotonic_nanos();
            let dt = if quick {
                1.0 / 60.0
            } else {
                let dt = ((now.saturating_sub(last)) as f32 / 1_000_000_000.0).min(0.05);
                last = now;
                dt
            };
            let moving = !pill.settled(pill_target);
            pill.tick(pill_target, dt);

            if dirty || moving || quick {
                match draw(win, canvas, &mut words, &state, pill.value) {
                    Ok(at) => pill_at = at,
                    Err(_) => {
                        out("draw:no");
                        return 1;
                    }
                }
                dirty = false;
            }
            frames += 1;

            if quick {
                if frames >= 30 {
                    break;
                }
                continue;
            }

            if moving {
                let _ = window::request_redraw(win);
            }
            match events::wait(Some(if moving { 16 } else { 500 })) {
                Some(types::Event::CloseRequested(_)) => break,
                Some(types::Event::Resized(_))
                | Some(types::Event::RedrawRequested(_))
                | Some(types::Event::ThemeChanged(_)) => dirty = true,
                Some(types::Event::Pointer(p)) => {
                    let inside = pill_at.contains(p.x, p.y);
                    pill_target = if inside { 1.04 } else { 1.0 };
                    let pressed = p.pressed && p.button.is_some();
                    if inside && pressed {
                        match dialog::open_folder(win, "Pick a folder to tidy") {
                            Ok(Some(folder)) => {
                                state = match tidy_picked(&folder.token) {
                                    Ok(moves) => State::Done {
                                        folder: folder.name,
                                        moves,
                                    },
                                    Err(why) => State::Failed(why),
                                };
                            }
                            // Cancelled: a normal answer, nothing changes.
                            Ok(None) => {}
                            Err(e) => {
                                state = State::Failed(format!("{e:?}"));
                            }
                        }
                        dirty = true;
                    }
                }
                Some(_) | None => {}
            }
            // Dark mode or the text size changed while the app waited.
            let now_look = Look::now();
            if now_look != look {
                look = now_look;
                dirty = true;
            }
        }

        if quick {
            quick_proof();
        }
        out("tidy:ready");
        0
    }
}

krate::export!(Component);
