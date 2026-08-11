//! The permission wall as a trusted guest: the sheet a phone shows before
//! any app runs.
//!
//! The player passes one argument built of ASCII separators -- records
//! split by `\x1e`, fields by `\x1c` -- so names and rationales keep their
//! spaces. First record is the app's name; each further record is
//! `cap\x1crationale\x1crequired`. The person taps rows to allow or deny
//! (required rows stay on -- the app says it cannot run without them),
//! then Open or Cancel. The decision goes to stdout as one line:
//! `wall:open:<cap,cap,...>` or `wall:cancel`. The player owns both sides
//! of that pipe, so this sheet cannot be lied to and cannot lie.
//!
//! `quick` renders one frame of a sample wall and answers `wall:open` with
//! everything granted, so check-app can verify the drawing path headless.

#![no_std]

#[allow(warnings)]
mod bindings;

extern crate alloc;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::time::clock;
use bindings::krate::ui::{events, tree, types, window};
use krate::motion::Spring;

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;
const RECORD_SEP: char = '\u{1e}';
const FIELD_SEP: char = '\u{1c}';

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

fn parse_input(raw: &str) -> (String, Vec<Ask>) {
    let mut records = raw.split(RECORD_SEP);
    let name = records.next().unwrap_or("This app").trim().to_string();
    let asks = records
        .filter_map(|record| {
            let mut fields = record.split(FIELD_SEP);
            let cap = fields.next()?.trim();
            if cap.is_empty() {
                return None;
            }
            let rationale = fields.next().unwrap_or("").trim();
            let required = fields.next().unwrap_or("0").trim() == "1";
            Some(Ask {
                cap: cap.to_string(),
                rationale: rationale.to_string(),
                required,
                granted: true,
            })
        })
        .collect();
    (name, asks)
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
}

impl Layout {
    fn from_canvas(w: f32, h: f32) -> Self {
        let col_w = w.min(480.0);
        Layout {
            w,
            h,
            col_x: (w - col_w) / 2.0,
            col_w,
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

struct Sheet {
    name: String,
    asks: Vec<Ask>,
    scroll: f32,
    entrance: Spring,
    rows: Vec<RowHit>,
    open_button: (f32, f32, f32, f32),
    cancel_button: (f32, f32, f32, f32),
}

impl Sheet {
    fn new(name: String, asks: Vec<Ask>) -> Self {
        Sheet {
            name,
            asks,
            scroll: 0.0,
            entrance: Spring::rest_at(0.0, 16.0),
            rows: Vec::new(),
            open_button: (0.0, 0.0, 0.0, 0.0),
            cancel_button: (0.0, 0.0, 0.0, 0.0),
        }
    }

    fn draw(&mut self, canvas: u64, l: &Layout) -> Result<(), gfx::GfxError> {
        canvas2d::clear(canvas, color(0.02, 0.024, 0.038, 1.0))?;
        self.rows.clear();

        // The sheet slides up on its spring: motion as reassurance that
        // this is deliberate chrome, not a glitch.
        let slide = (1.0 - self.entrance.value) * 40.0;
        let sheet_top = (l.h * 0.10 + slide).min(l.h);
        canvas2d::fill_round_rect(
            canvas,
            rect(l.col_x, sheet_top, l.col_w, l.h - sheet_top + 24.0),
            radii(24.0),
            color(0.075, 0.086, 0.125, 1.0),
        )?;
        // Grab handle.
        canvas2d::fill_round_rect(
            canvas,
            rect(l.col_x + l.col_w / 2.0 - 22.0, sheet_top + 10.0, 44.0, 4.0),
            radii(2.0),
            color(1.0, 1.0, 1.0, 0.25),
        )?;

        let x = l.col_x + 24.0;
        let text_w = l.col_w - 48.0;
        canvas2d::draw_text_styled(
            canvas,
            &self.name,
            gfx::Point {
                x,
                y: sheet_top + 52.0,
            },
            24.0,
            color(1.0, 1.0, 1.0, 1.0),
            style(700, -0.4),
        )?;
        canvas2d::draw_text(
            canvas,
            "wants to:",
            gfx::Point {
                x,
                y: sheet_top + 76.0,
            },
            14.0,
            color(1.0, 1.0, 1.0, 0.5),
        )?;

        // Buttons pin to the bottom; rows scroll between header and them.
        let buttons_top = l.h - 132.0;
        let rows_top = sheet_top + 96.0;
        canvas2d::set_clip(canvas, l.col_x, rows_top, l.col_w, buttons_top - rows_top - 8.0)?;

        let mut y = rows_top + 8.0 - self.scroll;
        for index in 0..self.asks.len() {
            let lines = wrap_lines(canvas, &self.asks[index].rationale, 14.5, text_w - 64.0);
            let ask = &self.asks[index];
            let row_h = 34.0 + lines.len() as f32 * 19.0;
            let y1 = y + row_h;
            self.rows.push(RowHit { index, y0: y, y1 });

            if y1 > rows_top && y < buttons_top {
                let mut line_y = y + 24.0;
                for line in &lines {
                    canvas2d::draw_text_styled(
                        canvas,
                        line,
                        gfx::Point { x, y: line_y },
                        14.5,
                        color(1.0, 1.0, 1.0, if ask.granted { 0.92 } else { 0.45 }),
                        style(550, 0.0),
                    )?;
                    line_y += 19.0;
                }
                canvas2d::draw_text(
                    canvas,
                    &ask.cap,
                    gfx::Point { x, y: line_y },
                    11.5,
                    color(1.0, 1.0, 1.0, 0.35),
                )?;

                // The toggle: a pill that reads at a glance. Required rows
                // keep a filled, dimmed pill -- the app says it cannot run
                // without them, and the honest control is one that shows
                // it cannot be turned off here.
                let pill = rect(l.col_x + l.col_w - 68.0, y + 10.0, 44.0, 26.0);
                let (fill, knob_x, alpha) = if ask.granted {
                    (color(0.42, 0.55, 1.0, 1.0), pill.x + pill.width - 22.0, 1.0)
                } else {
                    (color(1.0, 1.0, 1.0, 0.14), pill.x + 4.0, 0.9)
                };
                let fill = if ask.required {
                    color(0.42, 0.55, 1.0, 0.45)
                } else {
                    fill
                };
                canvas2d::fill_round_rect(canvas, pill, radii(13.0), fill)?;
                canvas2d::fill_circle(
                    canvas,
                    gfx::Point {
                        x: knob_x + 9.0,
                        y: pill.y + 13.0,
                    },
                    9.0,
                    color(1.0, 1.0, 1.0, alpha),
                )?;

                // Row divider.
                canvas2d::fill_rect(
                    canvas,
                    rect(x, y1 - 1.0, text_w, 1.0),
                    color(1.0, 1.0, 1.0, 0.06),
                )?;
            }
            y = y1 + 6.0;
        }
        canvas2d::clear_clip(canvas)?;

        // The choice.
        let open = rect(l.col_x + 24.0, buttons_top, l.col_w - 48.0, 52.0);
        canvas2d::drop_shadow_round_rect(
            canvas,
            rect(open.x, open.y + 5.0, open.width, open.height),
            radii(26.0),
            14.0,
            color(0.30, 0.42, 1.0, 0.4),
        )?;
        canvas2d::fill_round_rect(canvas, open, radii(26.0), color(0.42, 0.55, 1.0, 1.0))?;
        let open_label = "Open";
        let m = canvas2d::measure_text_styled(canvas, open_label, 17.0, style(650, 0.2))?;
        canvas2d::draw_text_styled(
            canvas,
            open_label,
            gfx::Point {
                x: open.x + open.width / 2.0 - m.width / 2.0,
                y: open.y + 33.0,
            },
            17.0,
            color(1.0, 1.0, 1.0, 1.0),
            style(650, 0.2),
        )?;
        self.open_button = (open.x, open.y, open.width, open.height);

        let cancel_y = buttons_top + 66.0;
        let cm = canvas2d::measure_text(canvas, "Cancel", 15.0)?;
        let cx = l.w / 2.0 - cm.width / 2.0;
        canvas2d::draw_text(
            canvas,
            "Cancel",
            gfx::Point { x: cx, y: cancel_y + 20.0 },
            15.0,
            color(1.0, 1.0, 1.0, 0.55),
        )?;
        self.cancel_button = (cx - 24.0, cancel_y, cm.width + 48.0, 32.0);

        canvas2d::present(canvas)
    }

    fn content_height(&self) -> f32 {
        self.rows.last().map(|r| r.y1 + self.scroll).unwrap_or(0.0)
    }

    /// What a tap at (x, y) means. Rows toggle; buttons decide.
    fn tap(&mut self, x: f32, y: f32) -> Option<bool> {
        let (bx, by, bw, bh) = self.open_button;
        if x >= bx && x <= bx + bw && y >= by && y <= by + bh {
            return Some(true);
        }
        let (cx, cy, cw, ch) = self.cancel_button;
        if x >= cx && x <= cx + cw && y >= cy && y <= cy + ch {
            return Some(false);
        }
        for row in &self.rows {
            if y >= row.y0 && y <= row.y1 {
                let ask = &mut self.asks[row.index];
                if !ask.required {
                    ask.granted = !ask.granted;
                }
                break;
            }
        }
        None
    }
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
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

struct Component;

impl bindings::Guest for Component {
    fn run() -> i32 {
        let raw = args::raw();
        // Real invocations always carry the record separator; anything else
        // (no args, a harness probe) renders the sample sheet headlessly.
        let quick = !raw.contains(RECORD_SEP);

        let (name, asks) = if quick {
            parse_input(&format!(
                "Sample app{RECORD_SEP}net.fetch:example.com:443{FIELD_SEP}Fetch the daily quote from example.com{FIELD_SEP}0{RECORD_SEP}fs.write:./notes/**{FIELD_SEP}Save your notes in its own folder -- never your files{FIELD_SEP}1"
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
        let _ = tree::upsert_node(win, &node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas));
        let canvas = match canvas2d::bind(win, CANVAS_ID) {
            Ok(canvas) => canvas,
            Err(_) => {
                out("wall:cancel");
                return 1;
            }
        };

        let mut sheet = Sheet::new(name, asks);
        let mut last = clock::monotonic_nanos();
        let mut frames: u32 = 0;
        let mut decision: Option<bool> = None;

        loop {
            let l = match canvas2d::canvas_size(canvas) {
                Ok(size) => Layout::from_canvas(size.width.max(1.0), size.height.max(1.0)),
                Err(_) => Layout::from_canvas(390.0, 720.0),
            };
            let now = clock::monotonic_nanos();
            let dt = if quick {
                1.0 / 60.0
            } else {
                let dt = ((now.saturating_sub(last)) as f32 / 1_000_000_000.0).min(0.05);
                last = now;
                dt
            };
            sheet.entrance.tick(1.0, dt);

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
            let wait = if sheet.entrance.settled(1.0) {
                None
            } else {
                Some(16)
            };
            // Drain what is already queued before drawing again. Taking one
            // event per frame means a 60 Hz drag outruns the loop and the
            // backlog grows for as long as the finger moves -- the sheet
            // would scroll seconds behind the thumb (the same bug found in
            // krate-gram on a real iPhone). Polling costs nothing when the
            // queue is empty.
            loop {
                match events::poll() {
                    Some(types::Event::Wheel(wheel)) => {
                        let max = (sheet.content_height() - l.h * 0.5).max(0.0);
                        sheet.scroll = (sheet.scroll + wheel.dy).clamp(0.0, max);
                    }
                    Some(types::Event::CloseRequested(_)) => {
                        decision = Some(false);
                        break;
                    }
                    Some(_) => {}
                    None => break,
                }
            }
            if decision.is_some() {
                break;
            }
            match events::wait(wait) {
                // Dismissed without answering is not consent: fail closed.
                Some(types::Event::CloseRequested(_)) => {
                    decision = Some(false);
                    break;
                }
                Some(types::Event::Wheel(wheel)) => {
                    let max = (sheet.content_height() - l.h * 0.5).max(0.0);
                    sheet.scroll = (sheet.scroll + wheel.dy).clamp(0.0, max);
                }
                Some(types::Event::Pointer(p)) => {
                    if p.pressed && p.button.is_some() {
                        if let Some(choice) = sheet.tap(p.x, p.y) {
                            decision = Some(choice);
                            break;
                        }
                    }
                }
                Some(_) | None => {}
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

bindings::export!(Component with_types_in bindings);
