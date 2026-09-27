//! Kitchen Timer -- a countdown that talks, drawn on a canvas2d.
//!
//! Pick the minutes with - and +, press Start. The remaining time shows big
//! inside a progress ring, and the computer's own voice says how many
//! minutes are left each time a whole minute passes, then "Time is up" at
//! the end. Every spoken line is also shown on screen, so a machine with no
//! voice still gets the whole message.
//!
//! `#![no_std]` keeps the component `krate:*`-only: the SDK owns the
//! allocator and panic handler, numbers are built in fixed byte buffers, and
//! nothing indexes a slice directly.

#![no_std]

extern crate alloc;

use alloc::string::String;
use krate::bindings::krate::io::{args, stdio};
use krate::bindings::krate::speech::synthesis;
use krate::bindings::krate::time::clock;
use krate::gfx::{canvas2d, types as gfx};
use krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const WIDTH: f32 = 520.0;
const HEIGHT: f32 = 700.0;

const MIN_MINUTES: u64 = 1;
const MAX_MINUTES: u64 = 99;
const DEFAULT_MINUTES: u64 = 5;
const MINUTE_MS: u64 = 60_000;

const WAIT_ROUND_MILLIS: u32 = 50;

// ---- geometry -----------------------------------------------------

const CX: f32 = WIDTH * 0.5;
const RING_CY: f32 = 262.0;
const RING_R: f32 = 160.0;
const RING_STROKE: f32 = 14.0;

const PICK_Y: f32 = 470.0;
const PICK_H: f32 = 56.0;
const PICK_BTN_W: f32 = 64.0;
const PICK_VALUE_W: f32 = 180.0;

const BTN_Y: f32 = 560.0;
const BTN_H: f32 = 52.0;
const BTN_PRIMARY_W: f32 = 180.0;
const BTN_GHOST_W: f32 = 120.0;
const BTN_GAP: f32 = 16.0;

const SAID_Y: f32 = 656.0;

// ---- palette ------------------------------------------------------

const BG_TOP: gfx::Color = rgb(0x14, 0x11, 0x0E);
const BG_BOT: gfx::Color = rgb(0x1C, 0x17, 0x12);
const TRACK: gfx::Color = rgb(0x33, 0x2B, 0x23);
const ACCENT: gfx::Color = rgb(0xFF, 0x8A, 0x3D);
const DONE: gfx::Color = rgb(0xFF, 0x5A, 0x4E);
const INK: gfx::Color = rgb(0xFA, 0xF4, 0xEC);
const INK_DIM: gfx::Color = rgb(0xB5, 0xA8, 0x98);
const INK_QUIET: gfx::Color = rgb(0x7A, 0x6D, 0x5E);
const WHITE: gfx::Color = rgb(0xFF, 0xFF, 0xFF);

const fn rgb(r: u8, g: u8, b: u8) -> gfx::Color {
    gfx::Color {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    }
}

const fn tint(c: gfx::Color, a: f32) -> gfx::Color {
    gfx::Color {
        r: c.r,
        g: c.g,
        b: c.b,
        a,
    }
}

// ------------------------------------------------------------------
// Timer state
// ------------------------------------------------------------------

struct Timer {
    /// Minutes picked with - and +.
    minutes: u64,
    /// Milliseconds left in the countdown.
    remaining_ms: u64,
    running: bool,
    /// Reached zero and said so; Start begins a fresh countdown.
    finished: bool,
    /// `now_millis` reading the last time the countdown was folded in.
    anchor_ms: u64,
    /// The last sentence spoken aloud (also shown on screen).
    said: String,
    /// How many sentences have been spoken this session.
    spoken: u64,
    /// Whether to actually use the voice (off on the headless run).
    voice: bool,
}

impl Timer {
    fn new(voice: bool) -> Self {
        Self {
            minutes: DEFAULT_MINUTES,
            remaining_ms: DEFAULT_MINUTES * MINUTE_MS,
            running: false,
            finished: false,
            anchor_ms: 0,
            said: String::new(),
            spoken: 0,
            voice,
        }
    }

    fn total_ms(&self) -> u64 {
        self.minutes * MINUTE_MS
    }

    fn idle(&self) -> bool {
        !self.running && (self.finished || self.remaining_ms == self.total_ms())
    }

    fn set_minutes(&mut self, m: u64) {
        self.minutes = m.clamp(MIN_MINUTES, MAX_MINUTES);
        self.reset();
    }

    fn plus(&mut self) {
        if !self.running {
            self.set_minutes(self.minutes + 1);
        }
    }

    fn minus(&mut self) {
        if !self.running {
            self.set_minutes(self.minutes.saturating_sub(1));
        }
    }

    fn start(&mut self, now: u64) {
        if self.running {
            return;
        }
        if self.finished {
            self.reset();
        }
        let fresh = self.remaining_ms == self.total_ms();
        self.running = true;
        self.anchor_ms = now;
        if fresh {
            let mins = self.minutes;
            self.say_minutes_left(mins);
        }
    }

    fn pause(&mut self, now: u64) {
        self.tick(now);
        self.running = false;
    }

    fn reset(&mut self) {
        self.remaining_ms = self.total_ms();
        self.running = false;
        self.finished = false;
    }

    /// Fold elapsed time into the countdown. Returns true when the display
    /// changed. Crossing a whole minute speaks the minutes left; reaching
    /// zero speaks "Time is up" and stops.
    fn tick(&mut self, now: u64) -> bool {
        if !self.running {
            return false;
        }
        let delta = now.saturating_sub(self.anchor_ms);
        if delta == 0 {
            return false;
        }
        self.anchor_ms = now;
        let before_secs = self.remaining_ms.div_ceil(1000);
        let before_min = self.remaining_ms.div_ceil(MINUTE_MS);
        self.remaining_ms = self.remaining_ms.saturating_sub(delta);
        if self.remaining_ms == 0 {
            self.running = false;
            self.finished = true;
            self.speak_str("Time is up");
            return true;
        }
        let after_min = self.remaining_ms.div_ceil(MINUTE_MS);
        if after_min < before_min {
            self.say_minutes_left(after_min);
        }
        self.remaining_ms.div_ceil(1000) != before_secs
    }

    fn say_minutes_left(&mut self, mins: u64) {
        let mut buf = [0u8; 20];
        let mut line = String::new();
        if let Ok(n) = core::str::from_utf8(u64_to_bytes(mins, &mut buf)) {
            line.push_str(n);
        }
        line.push_str(if mins == 1 {
            " minute left"
        } else {
            " minutes left"
        });
        self.speak(line);
    }

    fn speak_str(&mut self, s: &str) {
        let mut line = String::new();
        line.push_str(s);
        self.speak(line);
    }

    fn speak(&mut self, line: String) {
        if self.voice {
            let _ = synthesis::say(&line, None, None);
        }
        self.said = line;
        self.spoken += 1;
    }

    /// Fraction of the countdown already elapsed, 0.0..=1.0.
    fn progress(&self) -> f32 {
        let total = self.total_ms() as f32;
        let done = total - self.remaining_ms as f32;
        (done / total).clamp(0.0, 1.0)
    }

    fn verb(&self) -> &'static str {
        if self.running {
            "Pause"
        } else if self.idle() {
            "Start"
        } else {
            "Resume"
        }
    }

    fn status(&self) -> &'static str {
        if self.finished {
            "Time is up"
        } else if self.running {
            "Counting down"
        } else if self.idle() {
            "Ready"
        } else {
            "Paused"
        }
    }
}

// ------------------------------------------------------------------
// Component
// ------------------------------------------------------------------

struct Component;

impl krate::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize {
            width: WIDTH as u32,
            height: HEIGHT as u32,
        };
        let Ok(win) = window::create("Kitchen Timer", size) else {
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
        // A clock face: fixed design coordinates the host scales to fit.
        let _ = canvas2d::set_design_size(
            canvas,
            gfx::Size {
                width: WIDTH,
                height: HEIGHT,
            },
        );

        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");

        if quick {
            return quick_run(win, canvas);
        }

        let mut timer = Timer::new(true);
        if draw(canvas, &timer).is_err() {
            let _ = window::close(win);
            return 34;
        }

        loop {
            let event = events::wait(Some(WAIT_ROUND_MILLIS));

            // Keep the clock honest every round while running; redraw only
            // when the visible second flips.
            if timer.running && timer.tick(clock::now_millis()) {
                let _ = draw(canvas, &timer);
            }

            match event {
                Some(types::Event::Pointer(p)) if p.pressed => {
                    let now = clock::now_millis();
                    if hit(p.x, p.y, primary_rect()) {
                        if timer.running {
                            timer.pause(now);
                        } else {
                            timer.start(now);
                        }
                    } else if hit(p.x, p.y, ghost_rect()) {
                        timer.reset();
                        let _ = synthesis::stop();
                    } else if hit(p.x, p.y, minus_rect()) {
                        timer.minus();
                    } else if hit(p.x, p.y, plus_rect()) {
                        timer.plus();
                    }
                    let _ = draw(canvas, &timer);
                }
                Some(types::Event::Key(k)) if k.pressed => {
                    let now = clock::now_millis();
                    match k.key.as_str() {
                        "Space" | "Enter" => {
                            if timer.running {
                                timer.pause(now);
                            } else {
                                timer.start(now);
                            }
                        }
                        "ArrowUp" | "ArrowRight" | "+" | "=" => timer.plus(),
                        "ArrowDown" | "ArrowLeft" | "-" => timer.minus(),
                        "Escape" => timer.reset(),
                        _ => {}
                    }
                    let _ = draw(canvas, &timer);
                }
                Some(types::Event::Resized(_)) => {
                    let _ = draw(canvas, &timer);
                }
                Some(types::Event::CloseRequested(id)) if id == win => {
                    break;
                }
                _ => {}
            }
        }

        let _ = synthesis::stop();
        let _ = window::close(win);
        0
    }
}

/// The headless run: drive the real controls with synthetic time, then
/// report what the timer holds.
fn quick_run(win: u64, canvas: u64) -> i32 {
    let mut timer = Timer::new(false);

    // A 1-minute countdown run to the end: it must say "Time is up".
    timer.set_minutes(1);
    timer.start(0);
    timer.tick(30_000);
    timer.tick(60_000);
    let finished_first = timer.finished;
    let mut first_said = [0u8; 32];
    let first_len = copy_into(&mut first_said, timer.said.as_bytes());

    // Then pick 6 minutes with the buttons: + + + - from the default 5...
    timer.set_minutes(DEFAULT_MINUTES);
    timer.plus();
    timer.plus();
    timer.minus();
    timer.minus();
    timer.plus();
    // ...start it and let two and a half minutes pass.
    timer.start(1_000_000);
    let mut now = 1_000_000;
    for _ in 0..150 {
        now += 1000;
        timer.tick(now);
    }
    let _ = draw(canvas, &timer);
    let _ = window::close(win);

    let out = stdio::stdout();
    let mut buf = [0u8; 20];
    let _ = out.write(b"minutes:");
    let _ = out.write(u64_to_bytes(timer.minutes, &mut buf));
    let _ = out.write(b"\nremaining:");
    let _ = out.write(u64_to_bytes(timer.remaining_ms.div_ceil(1000), &mut buf));
    let _ = out.write(b"\nremaining_minutes:");
    let _ = out.write(u64_to_bytes(
        timer.remaining_ms.div_ceil(MINUTE_MS),
        &mut buf,
    ));
    let mut mm = [0u8; 8];
    let _ = out.write(b"\ndisplay:");
    let _ = out.write(fmt_mmss(timer.remaining_ms, &mut mm));
    let _ = out.write(b"\nrunning:");
    let _ = out.write(if timer.running { b"yes" } else { b"no" });
    let _ = out.write(b"\nspoken:");
    let _ = out.write(u64_to_bytes(timer.spoken, &mut buf));
    let _ = out.write(b"\nsaid:");
    let _ = out.write(timer.said.as_bytes());
    let _ = out.write(b"\nfirst_finished:");
    let _ = out.write(if finished_first { b"yes" } else { b"no" });
    let _ = out.write(b"\nfirst_said:");
    let _ = out.write(first_said.get(..first_len).unwrap_or(b""));
    let _ = out.write(b"\n");
    0
}

fn copy_into(dst: &mut [u8; 32], src: &[u8]) -> usize {
    let mut n = 0;
    for (d, s) in dst.iter_mut().zip(src.iter()) {
        *d = *s;
        n += 1;
    }
    n
}

// ------------------------------------------------------------------
// Layout rectangles + hit testing
// ------------------------------------------------------------------

fn rect(x: f32, y: f32, width: f32, height: f32) -> gfx::Rect {
    gfx::Rect {
        x,
        y,
        width,
        height,
    }
}

fn minus_rect() -> gfx::Rect {
    let total = PICK_BTN_W * 2.0 + PICK_VALUE_W;
    rect(CX - total * 0.5, PICK_Y, PICK_BTN_W, PICK_H)
}

fn plus_rect() -> gfx::Rect {
    let total = PICK_BTN_W * 2.0 + PICK_VALUE_W;
    rect(
        CX - total * 0.5 + PICK_BTN_W + PICK_VALUE_W,
        PICK_Y,
        PICK_BTN_W,
        PICK_H,
    )
}

fn primary_rect() -> gfx::Rect {
    let total = BTN_PRIMARY_W + BTN_GAP + BTN_GHOST_W;
    rect(CX - total * 0.5, BTN_Y, BTN_PRIMARY_W, BTN_H)
}

fn ghost_rect() -> gfx::Rect {
    let total = BTN_PRIMARY_W + BTN_GAP + BTN_GHOST_W;
    rect(
        CX - total * 0.5 + BTN_PRIMARY_W + BTN_GAP,
        BTN_Y,
        BTN_GHOST_W,
        BTN_H,
    )
}

fn hit(x: f32, y: f32, r: gfx::Rect) -> bool {
    x >= r.x && x <= r.x + r.width && y >= r.y && y <= r.y + r.height
}

// ------------------------------------------------------------------
// Rendering
// ------------------------------------------------------------------

fn style(weight: u16, spacing: f32) -> gfx::TextStyle {
    gfx::TextStyle {
        weight,
        italic: false,
        letter_spacing: spacing,
        family: gfx::FontFamily::Sans,
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

/// Draw `text` centred on `cx` with its baseline at `y`.
fn centered(
    canvas: u64,
    text: &str,
    cx: f32,
    y: f32,
    size: f32,
    ink: gfx::Color,
    st: gfx::TextStyle,
) -> Result<(), gfx::GfxError> {
    let w = match canvas2d::measure_text_styled(canvas, text, size, st) {
        Ok(m) => m.width,
        Err(_) => 0.0,
    };
    canvas2d::draw_text_styled(canvas, text, gfx::Point { x: cx - w * 0.5, y }, size, ink, st)
}

fn draw(canvas: u64, timer: &Timer) -> Result<(), gfx::GfxError> {
    canvas2d::linear_gradient(canvas, rect(0.0, 0.0, WIDTH, HEIGHT), BG_TOP, BG_BOT)?;

    let accent = if timer.finished { DONE } else { ACCENT };

    // A warm pool of light behind the ring.
    canvas2d::radial_gradient(
        canvas,
        gfx::Point { x: CX, y: RING_CY },
        RING_R + 90.0,
        tint(accent, if timer.finished { 0.16 } else { 0.08 }),
        tint(accent, 0.0),
    )?;

    centered(canvas, "KITCHEN TIMER", CX, 52.0, 13.0, INK_QUIET, style(600, 2.5))?;

    // ---- ring: track + elapsed arc from 12 o'clock, clockwise ----
    let center = gfx::Point { x: CX, y: RING_CY };
    canvas2d::stroke_circle(canvas, center, RING_R, RING_STROKE, TRACK)?;
    let p = if timer.finished { 1.0 } else { timer.progress() };
    if p > 0.0 {
        canvas2d::stroke_arc(canvas, center, RING_R, -90.0, p * 360.0, RING_STROKE, accent)?;
    }

    // ---- remaining time, big ----
    let mut buf = [0u8; 8];
    if let Ok(txt) = core::str::from_utf8(fmt_mmss(timer.remaining_ms, &mut buf)) {
        centered(canvas, txt, CX, RING_CY + 26.0, 84.0, INK, style(700, -2.0))?;
    }
    let status_ink = if timer.finished { DONE } else { INK_DIM };
    centered(canvas, timer.status(), CX, RING_CY + 66.0, 17.0, status_ink, style(500, 0.0))?;

    // ---- minute picker: [-]  N minutes  [+] ----
    let locked = timer.running;
    let btn_fill = if locked { tint(TRACK, 0.5) } else { TRACK };
    let btn_ink = if locked { INK_QUIET } else { INK };
    for (r, sign) in [(minus_rect(), "\u{2212}"), (plus_rect(), "+")] {
        canvas2d::fill_round_rect(canvas, r, radii(PICK_H * 0.5), btn_fill)?;
        centered(
            canvas,
            sign,
            r.x + r.width * 0.5,
            r.y + r.height * 0.5 + 11.0,
            32.0,
            btn_ink,
            style(500, 0.0),
        )?;
    }
    let mut nb = [0u8; 20];
    if let Ok(n) = core::str::from_utf8(u64_to_bytes(timer.minutes, &mut nb)) {
        let mut label = String::new();
        label.push_str(n);
        label.push_str(if timer.minutes == 1 { " minute" } else { " minutes" });
        centered(
            canvas,
            &label,
            CX,
            PICK_Y + PICK_H * 0.5 + 8.0,
            24.0,
            if locked { INK_DIM } else { INK },
            style(600, -0.3),
        )?;
    }

    // ---- Start/Pause + Reset ----
    let pr = primary_rect();
    canvas2d::drop_shadow_round_rect(
        canvas,
        rect(pr.x, pr.y + 4.0, pr.width, pr.height),
        radii(14.0),
        14.0,
        tint(rgb(0, 0, 0), 0.35),
    )?;
    canvas2d::fill_round_rect(canvas, pr, radii(14.0), accent)?;
    centered(
        canvas,
        timer.verb(),
        pr.x + pr.width * 0.5,
        pr.y + pr.height * 0.5 + 7.0,
        19.0,
        WHITE,
        style(700, 0.0),
    )?;

    let gr = ghost_rect();
    canvas2d::stroke_round_rect(canvas, gr, radii(14.0), 1.5, TRACK)?;
    centered(
        canvas,
        "Reset",
        gr.x + gr.width * 0.5,
        gr.y + gr.height * 0.5 + 7.0,
        19.0,
        INK_DIM,
        style(500, 0.0),
    )?;

    // ---- what the voice said last, in words too ----
    if timer.said.is_empty() {
        centered(
            canvas,
            "It will say the minutes left, out loud, every minute",
            CX,
            SAID_Y,
            14.0,
            INK_QUIET,
            style(400, 0.0),
        )?;
    } else {
        let mut line = String::new();
        line.push_str("\u{201C}");
        line.push_str(&timer.said);
        line.push_str("\u{201D}");
        centered(canvas, &line, CX, SAID_Y, 18.0, INK_DIM, style(500, 0.0))?;
    }

    canvas2d::present(canvas)?;
    Ok(())
}

// ------------------------------------------------------------------
// Numbers <-> bytes, panic-free
// ------------------------------------------------------------------

/// Render remaining milliseconds as `m:ss` / `mm:ss` (ceil to the visible
/// second, so 4:59.2 reads 5:00 until a full second has really passed).
fn fmt_mmss(ms: u64, buf: &mut [u8; 8]) -> &[u8] {
    let total_secs = ms.div_ceil(1000);
    let minutes = total_secs / 60;
    let seconds = total_secs % 60;
    let mut pos = 0usize;
    if minutes >= 10 {
        push_byte(buf, &mut pos, b'0' + ((minutes / 10) % 10) as u8);
    }
    push_byte(buf, &mut pos, b'0' + (minutes % 10) as u8);
    push_byte(buf, &mut pos, b':');
    push_byte(buf, &mut pos, b'0' + (seconds / 10) as u8);
    push_byte(buf, &mut pos, b'0' + (seconds % 10) as u8);
    buf.get(..pos).unwrap_or(b"0:00")
}

fn push_byte(buf: &mut [u8; 8], pos: &mut usize, byte: u8) {
    if let Some(slot) = buf.get_mut(*pos) {
        *slot = byte;
        *pos += 1;
    }
}

fn u64_to_bytes(value: u64, buf: &mut [u8; 20]) -> &[u8] {
    if value == 0 {
        if let Some(slot) = buf.get_mut(0) {
            *slot = b'0';
        }
        return buf.get(..1).unwrap_or(b"0");
    }
    let mut scratch = [0u8; 20];
    let mut n = value;
    let mut count = 0usize;
    while n > 0 && count < scratch.len() {
        if let Some(slot) = scratch.get_mut(count) {
            *slot = b'0' + (n % 10) as u8;
        }
        n /= 10;
        count += 1;
    }
    let mut pos = 0usize;
    let mut i = count;
    while i > 0 {
        i -= 1;
        if let (Some(src), Some(dst)) = (scratch.get(i), buf.get_mut(pos)) {
            *dst = *src;
            pos += 1;
        }
    }
    buf.get(..pos).unwrap_or(b"0")
}

// ----- widget builders (one canvas filling the window) -----

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

krate::export!(Component);
