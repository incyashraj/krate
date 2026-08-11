//! Krate clip — a modern clipboard tool over the system clipboard.
//!
//! The wall it tests: can an app hand text to the rest of the machine and get
//! it back? Copy and paste is the oldest bridge between programs; if an app
//! cannot reach the system clipboard, it lives on an island. This app writes a
//! known marker string with `clipboard::write-text`, reads it straight back
//! with `clipboard::read-text`, and shows the round-trip in a small window.
//!
//! The whole UI is drawn on one `gfx.canvas2d`: a considered dark ground, a
//! bold title, a framed text area showing the payload, two rounded buttons --
//! Copy and Paste -- that are drawn and hit-tested by hand, and a status line
//! that confirms the round-trip or reports the exact failure. Native widgets
//! cannot be styled by the app, so every pixel is painted here instead.
//!
//! On exit it prints one line so a script can judge the round-trip without
//! looking at pixels: `clip:ok` when the read-back matched the write,
//! `clip:mismatch` when the clipboard returned different bytes, and
//! `clip:error` when either call was denied or unsupported by the host. The
//! comparison is on raw bytes -- no `format!`, no `==` on owned strings, only
//! `krate:*` imports and no reachable panic.

#[allow(warnings)]
mod bindings;

use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::ui::{clipboard, events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const MARKER: &str = "krate-clip round-trip 12345";
const WIDTH: f32 = 600.0;
const HEIGHT: f32 = 430.0;

const QUICK_ROUNDS: u32 = 4;
const MAX_ROUNDS: u32 = 600;
const ROUND_MILLIS: u32 = 50;

/// Payload buffer: the text shown in the area and copied to the clipboard.
const PAYLOAD_CAP: usize = 128;

/// What the last clipboard action produced, for the status line.
#[derive(Clone, Copy, PartialEq)]
enum Status {
    /// Nothing done yet.
    Idle,
    /// Wrote the payload to the clipboard.
    Copied,
    /// Read back and it matched the payload.
    PastedOk,
    /// Read back and it differed.
    PastedMismatch,
    /// A clipboard call was refused or unsupported.
    Error,
}

/// The two drawn buttons.
#[derive(Clone, Copy, PartialEq)]
enum Button {
    Copy,
    Paste,
}

struct Component;

struct App {
    /// The text payload, held in a fixed byte buffer (no growing Vec).
    payload: [u8; PAYLOAD_CAP],
    payload_len: usize,
    /// The most recent read-back, for the preview line.
    readback: [u8; PAYLOAD_CAP],
    readback_len: usize,
    status: Status,
    /// Which button is under the pointer, for a hover lift.
    hover: Option<Button>,
    /// Which button is pressed, for a press-down state.
    pressed: Option<Button>,
}

impl App {
    fn new() -> Self {
        let mut payload = [0u8; PAYLOAD_CAP];
        let len = copy_into(&mut payload, MARKER.as_bytes());
        App {
            payload,
            payload_len: len,
            readback: [0u8; PAYLOAD_CAP],
            readback_len: 0,
            status: Status::Idle,
            hover: None,
            pressed: None,
        }
    }

    fn payload_bytes(&self) -> &[u8] {
        self.payload.get(..self.payload_len).unwrap_or(&[])
    }
    fn readback_bytes(&self) -> &[u8] {
        self.readback.get(..self.readback_len).unwrap_or(&[])
    }

    /// Write the payload to the clipboard.
    fn do_copy(&mut self, out: &stdio::OutputStream) {
        if let Ok(text) = core::str::from_utf8(self.payload_bytes()) {
            if clipboard::write_text(text).is_ok() {
                self.status = Status::Copied;
                return;
            }
        }
        self.status = Status::Error;
        let _ = out.write(b"clip:error\n");
    }

    /// Read the clipboard back and compare with the payload.
    fn do_paste(&mut self, out: &stdio::OutputStream) {
        match clipboard::read_text() {
            Ok(text) => {
                let bytes = text.as_bytes();
                self.readback_len = copy_into(&mut self.readback, bytes);
                if bytes == self.payload_bytes() {
                    self.status = Status::PastedOk;
                    let _ = out.write(b"clip:ok\n");
                } else {
                    self.status = Status::PastedMismatch;
                    let _ = out.write(b"clip:mismatch\n");
                }
            }
            Err(_) => {
                self.status = Status::Error;
                let _ = out.write(b"clip:error\n");
            }
        }
    }
}

impl bindings::Guest for Component {
    fn run() -> i32 {
        let out = stdio::stdout();

        let size = types::WindowSize {
            width: WIDTH as u32,
            height: HEIGHT as u32,
        };
        let Ok(win) = window::create("Clipboard", size) else {
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
        // The app's own coordinate system: keep drawing in these numbers
        // and the host scales them to any window, centred, never stretched
        // out of proportion (K-096).
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

        let mut app = App::new();

        // For the automated shot, perform the whole round-trip up front so the
        // first captured frame already shows a confirmed copy+paste, not an
        // idle window. Interactive runs start Idle and react to clicks.
        if quick {
            app.do_copy(&out);
            app.do_paste(&out);
        }

        let _ = draw(canvas, &app);

        // A real session ends when the person closes the window, never on a
        // round count (K-092). `quick` keeps its bound so a headless check
        // cannot hang.
        let rounds = if quick { QUICK_ROUNDS } else { u32::MAX };
        let mut r = 0u32;
        while r < rounds {
            match events::wait(Some(ROUND_MILLIS)) {
                Some(types::Event::CloseRequested(id)) if id == win => break,
                Some(types::Event::Pointer(p)) => {
                    let hit = hit_button(p.x, p.y);
                    if p.pressed {
                        app.pressed = hit;
                        app.hover = hit;
                    } else {
                        // Release: if released over the same button, fire it.
                        if let Some(b) = hit {
                            if app.pressed == Some(b) {
                                match b {
                                    Button::Copy => app.do_copy(&out),
                                    Button::Paste => app.do_paste(&out),
                                }
                            }
                        }
                        app.pressed = None;
                        app.hover = hit;
                    }
                    let _ = draw(canvas, &app);
                }
                _ => {
                    let _ = draw(canvas, &app);
                }
            }
            r += 1;
        }

        let _ = window::close(win);
        0
    }
}

// ------------------------------------------------------------------
// Layout constants (shared by draw and hit-test)
// ------------------------------------------------------------------

const AREA_X: f32 = 32.0;
const AREA_Y: f32 = 118.0;
const AREA_W: f32 = WIDTH - 64.0;
const AREA_H: f32 = 96.0;

const BTN_W: f32 = 254.0;
const BTN_H: f32 = 54.0;
const BTN_Y: f32 = 246.0;
const BTN_GAP: f32 = 16.0;
/// Left edge of the Copy button; Paste sits to its right.
const COPY_X: f32 = 32.0;
const PASTE_X: f32 = COPY_X + BTN_W + BTN_GAP;

fn hit_button(x: f32, y: f32) -> Option<Button> {
    if y >= BTN_Y && y <= BTN_Y + BTN_H {
        if x >= COPY_X && x <= COPY_X + BTN_W {
            return Some(Button::Copy);
        }
        if x >= PASTE_X && x <= PASTE_X + BTN_W {
            return Some(Button::Paste);
        }
    }
    None
}

// ------------------------------------------------------------------
// Rendering
// ------------------------------------------------------------------

fn draw(canvas: u64, app: &App) -> Result<(), gfx::GfxError> {
    // Considered dark ground with a soft top glow.
    canvas2d::linear_gradient(
        canvas,
        gfx::Rect {
            x: 0.0,
            y: 0.0,
            width: WIDTH,
            height: HEIGHT,
        },
        color(0.086, 0.098, 0.157, 1.0),
        color(0.043, 0.051, 0.086, 1.0),
    )?;
    canvas2d::radial_gradient(
        canvas,
        gfx::Point { x: WIDTH * 0.5, y: 20.0 },
        320.0,
        color(0.30, 0.50, 0.95, 0.14),
        color(0.30, 0.50, 0.95, 0.0),
    )?;

    // ---- header ----
    draw_text(canvas, "Clipboard", 32.0, 58.0, 32.0, color(0.96, 0.97, 1.0, 1.0))?;
    draw_text(
        canvas,
        "Copy this text out, paste it back in",
        32.0,
        86.0,
        14.0,
        color(0.58, 0.66, 0.84, 1.0),
    )?;

    // ---- text area ----
    // A framed panel showing the payload, like a real input field.
    round_rect(
        canvas,
        AREA_X,
        AREA_Y,
        AREA_W,
        AREA_H,
        14.0,
        color(0.129, 0.145, 0.216, 1.0),
    )?;
    // Inner top hairline for a lit edge: an opaque, slightly lighter strip
    // inset from the rounded corners.
    canvas2d::fill_rect(
        canvas,
        gfx::Rect {
            x: AREA_X + 14.0,
            y: AREA_Y,
            width: AREA_W - 28.0,
            height: 1.5,
        },
        color(0.20, 0.22, 0.30, 1.0),
    )?;
    // A little accent bar on the left, like a focused field.
    canvas2d::fill_rect(
        canvas,
        gfx::Rect {
            x: AREA_X + 4.0,
            y: AREA_Y + 14.0,
            width: 3.0,
            height: AREA_H - 28.0,
        },
        color(0.36, 0.72, 1.0, 0.9),
    )?;
    if let Ok(txt) = core::str::from_utf8(app.payload_bytes()) {
        draw_text(canvas, txt, AREA_X + 22.0, AREA_Y + 40.0, 18.0, color(0.92, 0.95, 1.0, 1.0))?;
    }
    draw_text(
        canvas,
        "payload on the clipboard",
        AREA_X + 22.0,
        AREA_Y + 70.0,
        12.0,
        color(0.50, 0.58, 0.76, 1.0),
    )?;

    // ---- buttons ----
    // Copy: a filled accent (blue) button. Paste: a tinted outline button.
    draw_button(
        canvas,
        COPY_X,
        BTN_Y,
        BTN_W,
        BTN_H,
        "Copy",
        (0.24, 0.55, 1.0),
        true,
        app.hover == Some(Button::Copy),
        app.pressed == Some(Button::Copy),
    )?;
    draw_button(
        canvas,
        PASTE_X,
        BTN_Y,
        BTN_W,
        BTN_H,
        "Paste",
        (0.32, 0.80, 0.68),
        false,
        app.hover == Some(Button::Paste),
        app.pressed == Some(Button::Paste),
    )?;

    // ---- status line ----
    draw_status(canvas, app)?;

    canvas2d::present(canvas)?;
    Ok(())
}

/// A rounded button. `filled` picks a solid accent fill vs a tinted outline;
/// hover lifts the fill, press darkens it, so the control reacts to the pointer.
#[allow(clippy::too_many_arguments)]
fn draw_button(
    canvas: u64,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    label: &str,
    tint: (f32, f32, f32),
    filled: bool,
    hover: bool,
    pressed: bool,
) -> Result<(), gfx::GfxError> {
    let (tr, tg, tb) = tint;
    // Soft drop shadow beneath: OPAQUE and near-ground so it reads as a subtle
    // lift without the doubled-alpha corners a translucent rounded fill causes.
    round_rect(canvas, x, y + 3.0, w, h, 14.0, color(0.020, 0.025, 0.045, 1.0))?;

    if filled {
        // Press darkens, hover brightens a touch. Opaque throughout.
        let k = if pressed {
            0.80
        } else if hover {
            1.12
        } else {
            1.0
        };
        round_rect(
            canvas,
            x,
            y,
            w,
            h,
            14.0,
            color((tr * k).min(1.0), (tg * k).min(1.0), (tb * k).min(1.0), 1.0),
        )?;
    } else {
        // "Outlined": an OPAQUE dark-tinted body (the tint mixed a little way up
        // from the ground), brighter on hover. Opaque keeps the corners clean.
        let m = if pressed {
            0.22
        } else if hover {
            0.34
        } else {
            0.26
        };
        let bg = (0.078, 0.090, 0.145);
        round_rect(
            canvas,
            x,
            y,
            w,
            h,
            14.0,
            color(
                bg.0 + (tr - bg.0) * m,
                bg.1 + (tg - bg.1) * m,
                bg.2 + (tb - bg.2) * m,
                1.0,
            ),
        )?;
    }

    // Centered-ish label. Estimate width from glyph count to center it.
    let approx = label_width(canvas, label, 18.0);
    let lx = x + (w - approx) * 0.5;
    let ink = if filled {
        color(1.0, 1.0, 1.0, 1.0)
    } else {
        color((tr + 0.35).min(1.0), (tg + 0.20).min(1.0), (tb + 0.25).min(1.0), 1.0)
    };
    draw_text(canvas, label, lx, y + h * 0.5 + 6.0, 18.0, ink)?;
    Ok(())
}

fn draw_status(canvas: u64, app: &App) -> Result<(), gfx::GfxError> {
    let sy = 344.0;
    // A status dot whose color reflects the outcome.
    let (dot, msg): (gfx::Color, &str) = match app.status {
        Status::Idle => (
            color(0.5, 0.58, 0.75, 1.0),
            "Click Copy, then Paste to prove the round-trip.",
        ),
        Status::Copied => (
            color(0.36, 0.72, 1.0, 1.0),
            "Copied to the clipboard. Now click Paste.",
        ),
        Status::PastedOk => (
            color(0.42, 0.90, 0.55, 1.0),
            "Round-trip matched: same text came back.",
        ),
        Status::PastedMismatch => (
            color(0.98, 0.72, 0.35, 1.0),
            "The clipboard returned different text.",
        ),
        Status::Error => (
            color(0.98, 0.45, 0.55, 1.0),
            "A clipboard call was refused by the host.",
        ),
    };
    disc(canvas, 38.0, sy - 5.0, 5.0, dot)?;
    draw_text(canvas, msg, 52.0, sy, 14.0, color(0.86, 0.90, 0.98, 1.0))?;

    // When a paste happened, show the read-back preview beneath.
    if app.status == Status::PastedOk || app.status == Status::PastedMismatch {
        if let Ok(rb) = core::str::from_utf8(app.readback_bytes()) {
            let mut buf = [0u8; PAYLOAD_CAP + 16];
            let text = join(b"read back: ", rb.as_bytes(), &mut buf);
            if let Ok(t) = core::str::from_utf8(text) {
                draw_text(canvas, t, 52.0, sy + 24.0, 12.0, color(0.52, 0.60, 0.78, 1.0))?;
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------------
// Canvas helpers
// ------------------------------------------------------------------

/// A filled rounded rectangle assembled from NON-overlapping pieces, so a
/// translucent color composites evenly (no doubled-alpha corner blobs): a full-
/// width center band, a top and bottom band between the corners, and four
/// corner discs. Nothing is painted twice.
fn round_rect(
    canvas: u64,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
    c: gfx::Color,
) -> Result<(), gfx::GfxError> {
    // One host call, antialiased on the curve. The bands-plus-corner-discs
    // version this replaced showed seams where the pieces met.
    canvas2d::fill_round_rect(
        canvas,
        gfx::Rect { x, y, width: w, height: h },
        gfx::CornerRadii { top_left: r, top_right: r, bottom_right: r, bottom_left: r },
        c,
    )
}

fn disc(canvas: u64, cx: f32, cy: f32, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_circle(canvas, gfx::Point { x: cx, y: cy }, r, c)
}

fn draw_text(
    canvas: u64,
    text: &str,
    x: f32,
    y: f32,
    size: f32,
    c: gfx::Color,
) -> Result<(), gfx::GfxError> {
    canvas2d::draw_text(canvas, text, gfx::Point { x, y }, size, c)
}

fn color(r: f32, g: f32, b: f32, a: f32) -> gfx::Color {
    gfx::Color { r, g, b, a }
}

/// Rendered width of a string at a given font size, measured by the host with
/// the same font layout `draw_text` draws with.
///
/// This used to be character count times an invented constant, with a comment
/// claiming the host face was monospace or near-monospace. It is not: it is
/// proportional, and `i` and `W` differ about four times in real width. So a
/// centred label was not centred and a right-aligned number did not line up.
/// `measure_text` is the true answer.
fn label_width(canvas: u64, s: &str, size: f32) -> f32 {
    match canvas2d::measure_text(canvas, s, size) {
        Ok(m) => m.width,
        Err(_) => 0.0,
    }
}

// ------------------------------------------------------------------
// Byte helpers, panic-free
// ------------------------------------------------------------------

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

fn join<'a>(prefix: &[u8], value: &[u8], buf: &'a mut [u8]) -> &'a [u8] {
    let mut pos = 0usize;
    for byte in prefix {
        if let Some(slot) = buf.get_mut(pos) {
            *slot = *byte;
            pos += 1;
        }
    }
    for byte in value {
        if let Some(slot) = buf.get_mut(pos) {
            *slot = *byte;
            pos += 1;
        }
    }
    buf.get(..pos).unwrap_or(b"")
}

// ------------------------------------------------------------------
// Widget builders (root stack + one canvas filling it)
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
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

bindings::export!(Component with_types_in bindings);
