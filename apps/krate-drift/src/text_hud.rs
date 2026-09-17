//! The text half of the HUD, sitting over the 3D scene in an Overlay.
//!
//! Until the `overlay` widget existed this file could not have been written.
//! The only containers were flow containers: a canvas holding the scene and a
//! canvas holding text were laid out one above the other and each got half the
//! window, so a 3D app could have a scene or a HUD and not both. Every number
//! this game showed was seven-segment digits welded out of triangles in
//! `hud.rs`, and anything that needed a WORD -- "LAP", "FINISHED", the name of
//! the car that won -- could not be said at all. `build_hud` stood a chequered
//! band in for a title and a row of bars in for a results table.
//!
//! An Overlay gives every child the whole container, so these labels sit on
//! top of the running race at their own positions.
//!
//! The polygon HUD stays. The speedometer, the speed bar and the big digits
//! are drawn in the scene where they belong -- they move with the frame, they
//! need no font, and they work identically on a machine with no GPU. Text is
//! for what shapes cannot say.

use alloc::string::String;
use alloc::vec::Vec;

use krate::ui::{tree, types};

use crate::car::MAX_LAPS;

/// Ids for the text layer. Well clear of the scene's 1 and 2.
pub const PANEL_ID: u64 = 100;
const LINE_BASE: u64 = 101;

/// How many lines the panel can hold. Six: title, subtitle, and four result
/// rows at most.
const LINES: usize = 8;

/// The text layer, which knows what it last said so it only talks to the host
/// when something changed.
///
/// A HUD updates sixty times a second and almost nothing on it changes between
/// two frames. Sending eight unchanged labels every frame would put a tree
/// round trip per line into the frame budget for no visible difference.
pub struct TextHud {
    shown: Vec<String>,
}

impl TextHud {
    /// Build the panel and its lines, all initially empty.
    ///
    /// Every line is created up front rather than as needed: a node that
    /// appears mid-race would relayout the panel, and an empty label occupies
    /// no visible space anyway.
    pub fn new(win: u64, parent: u64) -> Option<Self> {
        tree::upsert_node(win, &panel_node(parent)).ok()?;
        let mut shown = Vec::with_capacity(LINES);
        for i in 0..LINES {
            tree::upsert_node(win, &line_node(i, "")).ok()?;
            shown.push(String::new());
        }
        Some(Self { shown })
    }

    /// Set one line, skipping the host call when it already says this.
    fn set(&mut self, win: u64, i: usize, text: &str) {
        if i >= LINES || self.shown[i] == text {
            return;
        }
        if tree::upsert_node(win, &line_node(i, text)).is_ok() {
            self.shown[i].clear();
            self.shown[i].push_str(text);
        }
    }

    /// Write the whole panel for this frame.
    ///
    /// `lines` is what the panel should say, top to bottom. Anything past the
    /// end is blanked, so a results screen does not leave the countdown's
    /// words behind it.
    pub fn show(&mut self, win: u64, lines: &[&str]) {
        for i in 0..LINES {
            self.set(win, i, lines.get(i).copied().unwrap_or(""));
        }
    }
}

fn panel_node(parent: u64) -> types::WidgetNode {
    types::WidgetNode {
        id: PANEL_ID,
        parent: Some(parent),
        kind: types::WidgetKind::Stack,
        label: None,
        role: None,
        // `grow: 0.0` and no height, so the panel hugs its lines instead of
        // claiming the window. It is a child of an Overlay, which would
        // otherwise stretch it over the whole scene and put the text in the
        // middle of the road.
        style: types::Style {
            width: None,
            height: None,
            grow: 0.0,
            padding: 18.0,
            // The panel paints nothing itself; each line carries its own ink.
            text: None,
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

/// How a HUD line is inked: white, in a near-black outline, large.
///
/// The outline is doing the real work. This text sits over a 3D scene, so its
/// background is whatever the camera is pointing at -- sky one moment, dark
/// asphalt the next. The host's ordinary label colour measured 3.98:1 against
/// the sky and 1.27:1 against the road, and the second of those is not
/// readable. White in a dark outline reads at about 19:1 against its own
/// outline wherever it is, and on the dark road where the outline itself
/// disappears the white glyph is already high-contrast by itself.
fn hud_text_style(size: f32, bold: bool) -> types::TextStyle {
    types::TextStyle {
        color: Some(types::Color {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        }),
        outline: Some(types::Color {
            r: 10,
            g: 12,
            b: 18,
            a: 235,
        }),
        outline_width: 2.0,
        size: Some(size),
        bold,
    }
}

fn line_node(index: usize, text: &str) -> types::WidgetNode {
    let mut label = String::new();
    label.push_str(text);
    // The first line is the headline -- the lap, the title, the result -- and
    // is read at a glance at speed; the rest are read when there is a moment.
    let (size, bold) = if index == 0 { (30.0, true) } else { (20.0, false) };
    types::WidgetNode {
        id: LINE_BASE + index as u64,
        parent: Some(PANEL_ID),
        kind: types::WidgetKind::Text,
        label: Some(label),
        role: Some(pure("text")),
        style: types::Style {
            width: None,
            height: None,
            grow: 0.0,
            padding: 0.0,
            text: Some(hud_text_style(size, bold)),
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

fn pure(s: &str) -> String {
    let mut out = String::new();
    out.push_str(s);
    out
}

/// The ordinal a finishing position is announced with: 1st, 2nd, 3rd, 4th.
///
/// Small and hand-written because a `#![no_std]` guest has no `format!` for
/// this shape and the field is six cars, so the exceptional teens never come
/// up.
pub fn ordinal(pos: u32, out: &mut String) {
    let mut digits = [0u8; 3];
    let mut n = pos.max(1);
    let mut len = 0;
    while n > 0 && len < 3 {
        digits[len] = b'0' + (n % 10) as u8;
        n /= 10;
        len += 1;
    }
    for i in (0..len).rev() {
        out.push(digits[i] as char);
    }
    out.push_str(match pos {
        1 => "st",
        2 => "nd",
        3 => "rd",
        _ => "th",
    });
}

/// "LAP 2 OF 3", spelled out.
///
/// The polygon HUD shows this as two numbers with a leaning slash between
/// them, which it has to: it has no letters. That slash was read as the digit
/// 1 until it was tilted. Words do not have that problem.
pub fn lap_line(lap: u32, out: &mut String) {
    out.push_str("LAP ");
    push_u32(lap.min(MAX_LAPS), out);
    out.push_str(" OF ");
    push_u32(MAX_LAPS, out);
}

pub fn push_u32(mut v: u32, out: &mut String) {
    let mut digits = [0u8; 10];
    let mut len = 0;
    loop {
        digits[len] = b'0' + (v % 10) as u8;
        v /= 10;
        len += 1;
        if v == 0 || len == 10 {
            break;
        }
    }
    for i in (0..len).rev() {
        out.push(digits[i] as char);
    }
}
