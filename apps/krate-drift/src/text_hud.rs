//! The 2D interface: a header bar, a centre message, and a panel.
//!
//! Until the `overlay` widget existed none of this could be written. The only
//! containers were flow containers, so a canvas holding a scene and a canvas
//! holding text were laid out one above the other and each got half the
//! window. Every number this game showed was seven-segment digits welded out
//! of triangles in `hud.rs`, and anything needing a WORD could not be said.
//!
//! The layout is three regions inside one Overlay, each sized so it hugs its
//! own content rather than claiming the window:
//!
//!   HEADER   a Grid across the top -- lap, position, clock, in fixed places
//!   CENTRE   big text in the middle, for the countdown and for results
//!   FOOT     a line at the bottom, for prompts
//!
//! A Grid is a row that WRAPS, which is the only horizontal container in the
//! widget set; Stack and its relatives are all columns. That is why the header
//! is a Grid and not a Stack.
//!
//! The polygon HUD in `hud.rs` stays for what shapes do better than text: the
//! speed bar, the countdown lights, the rev arc. They move with the frame,
//! need no font, and work identically on a machine with no GPU.

use alloc::string::String;
use alloc::vec::Vec;

use krate::ui::{tree, types};

/// Ids for the 2D layer. Well clear of the scene's 1 and 2.
const HEADER_ID: u64 = 101;
const CENTRE_ID: u64 = 102;
const FOOT_ID: u64 = 103;
/// Header cells, left to right.
const HEADER_BASE: u64 = 110;
const HEADER_CELLS: usize = 3;
/// Centre lines, top to bottom.
const CENTRE_BASE: u64 = 120;
/// Enough for the longest screen there is.
///
/// A six-car results table needs ten: a headline, a blank, six rows, a blank
/// and the best lap. At eight the last two were simply dropped, silently --
/// `show` writes as many lines as it has slots and says nothing about the
/// rest, so BEST LAP was missing from the screen with nothing to explain it.
const CENTRE_LINES: usize = 12;
const FOOT_LINE: u64 = 140;

/// How a piece of the interface is inked.
///
/// The outline is what makes any of this readable. This text sits over a 3D
/// scene, so its background is whatever the camera is pointing at -- sky one
/// moment, dark asphalt the next. The host's ordinary label colour measured
/// 3.98:1 against the sky and 1.27:1 against the road, and the second is not
/// readable. White in a dark outline is about 19:1 against its own outline
/// wherever it is, and on the dark road where the outline washes out the white
/// glyph is already high-contrast by itself (K-402).
fn ink(size: f32, bold: bool) -> types::TextStyle {
    types::TextStyle {
        color: Some(types::Color {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        }),
        outline: Some(types::Color {
            r: 8,
            g: 10,
            b: 16,
            a: 240,
        }),
        outline_width: 2.0,
        size: Some(size),
        bold,
    }
}

/// Ink for something that should read as a highlight rather than as a
/// readout: the final lap warning, a winner's name.
fn ink_warm(size: f32, bold: bool) -> types::TextStyle {
    types::TextStyle {
        color: Some(types::Color {
            r: 255,
            g: 214,
            b: 108,
            a: 255,
        }),
        ..ink(size, bold)
    }
}

/// The 2D layer, which knows what it last said so it only talks to the host
/// when something changed.
///
/// A HUD updates sixty times a second and almost nothing on it changes between
/// two frames. Sending every label every frame would put a tree round trip per
/// label into the frame budget for no visible difference.
pub struct Ui {
    shown: Vec<String>,
}

/// Every text slot, in one flat list, so `shown` can be indexed by it.
const SLOTS: usize = HEADER_CELLS + CENTRE_LINES + 1;

impl Ui {
    /// Build the whole tree once. Returns `None` if the host refuses any of
    /// it, which leaves the game running on the polygon HUD alone.
    pub fn new(win: u64, parent: u64) -> Option<Self> {
        tree::upsert_node(win, &region(HEADER_ID, parent, Region::Header)).ok()?;
        tree::upsert_node(win, &region(CENTRE_ID, parent, Region::Centre)).ok()?;
        tree::upsert_node(win, &region(FOOT_ID, parent, Region::Foot)).ok()?;

        for i in 0..HEADER_CELLS {
            tree::upsert_node(win, &header_cell(i, "")).ok()?;
        }
        for i in 0..CENTRE_LINES {
            tree::upsert_node(win, &centre_line(i, "", false)).ok()?;
        }
        tree::upsert_node(win, &foot_line("")).ok()?;

        let mut shown = Vec::with_capacity(SLOTS);
        for _ in 0..SLOTS {
            shown.push(String::new());
        }
        Some(Self { shown })
    }

    fn set(&mut self, win: u64, slot: usize, text: &str, node: types::WidgetNode) {
        if slot >= SLOTS || self.shown[slot] == text {
            return;
        }
        if tree::upsert_node(win, &node).is_ok() {
            self.shown[slot].clear();
            self.shown[slot].push_str(text);
        }
    }

    /// The header bar: three cells across the top.
    pub fn header(&mut self, win: u64, cells: [&str; HEADER_CELLS]) {
        for (i, text) in cells.iter().enumerate() {
            self.set(win, i, text, header_cell(i, text));
        }
    }

    /// The centre block. `warm` picks the highlight ink for that line.
    pub fn centre(&mut self, win: u64, lines: &[(&str, bool)]) {
        for i in 0..CENTRE_LINES {
            let (text, warm) = lines.get(i).copied().unwrap_or(("", false));
            self.set(win, HEADER_CELLS + i, text, centre_line(i, text, warm));
        }
    }

    /// The prompt at the bottom.
    pub fn foot(&mut self, win: u64, text: &str) {
        self.set(win, HEADER_CELLS + CENTRE_LINES, text, foot_line(text));
    }
}

enum Region {
    Header,
    Centre,
    Foot,
}

fn region(id: u64, parent: u64, which: Region) -> types::WidgetNode {
    // Every region hugs its content: `grow: 0.0` and no height. A child of an
    // Overlay is stretched to fill it otherwise, which would put the header's
    // text in the middle of the road.
    let (kind, padding, place) = match which {
        // A Grid is the only horizontal container in the widget set -- Stack,
        // Scroll, ListView, TreeView and Tabs are all columns.
        Region::Header => (
            types::WidgetKind::Grid,
            20.0,
            types::Placement::TopLeft,
        ),
        // Padded in from the left so the centre block sits nearer the middle
        // of the window than the corner.
        //
        // It cannot actually be CENTRED: every child of an Overlay is pinned
        // to the same cell and the widget set has no alignment, so padding is
        // the only lever there is. That is a runtime gap rather than a choice
        // -- the same one that stops the prompt sitting at the bottom -- and
        // the number here is tuned to a 1600-wide window rather than derived
        // from anything.
        // Actually centred now, rather than nudged with padding.
        //
        // This was 260 logical pixels of left padding, tuned by eye to a
        // 1600-wide window and wrong at any other size -- and padding changes
        // the PARENT'S size, so in an overlay it moved everything else in the
        // cell too. `Placement::Centre` asks for the middle and gets it at any
        // window size (K-410).
        Region::Centre => (types::WidgetKind::Stack, 0.0, types::Placement::Centre),
        // The foot sits UNDER the centre block, not at the bottom of the
        // window, and that is a runtime limit rather than a choice.
        //
        // Every child of an Overlay is pinned to the same cell, and the widget
        // set has no alignment -- a region hugs its content or fills, and
        // nothing says "put this at the bottom". Padding looked like the
        // lever and is not: 790 pixels of it GREW the overlay, which stretched
        // the scene canvas to 1604 pixels in a 900-pixel window. The scene
        // rendered offset with a white band above it, and the placement dump
        // is the only thing that said so -- on screen it looked like the
        // canvas had failed.
        //
        // Filed as the alignment gap; until then the prompt lives below the
        // centre text, which reads fine because that is where a prompt goes
        // anyway.
        // At the bottom, which padding could never do: pushing it down with
        // 790 pixels GREW the overlay and stretched the scene canvas to 1604
        // pixels in a 900-pixel window (K-410).
        Region::Foot => (
            types::WidgetKind::Stack,
            40.0,
            types::Placement::BottomCentre,
        ),
    };
    types::WidgetNode {
        id,
        parent: Some(parent),
        kind,
        label: None,
        role: None,
        style: types::Style {
            width: None,
            height: None,
            grow: 0.0,
            padding,
            text: None,
            place: Some(place),
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

fn text_node(id: u64, parent: u64, text: &str, style: types::TextStyle) -> types::WidgetNode {
    let mut label = String::new();
    label.push_str(text);
    types::WidgetNode {
        id,
        parent: Some(parent),
        kind: types::WidgetKind::Text,
        label: Some(label),
        role: Some(pure("text")),
        style: types::Style {
            width: None,
            // An UNUSED line takes no room.
            //
            // Every slot exists for the whole run so its text can be swapped
            // without rebuilding the tree, and an empty one still carried its
            // font size -- so the centre region was twelve line-boxes tall
            // whatever was in it, and centring the REGION left the one line
            // anybody could see sitting at the top of it. Height 0 on an empty
            // line makes the region as tall as its content, which is what
            // Placement::Centre needs to be worth anything.
            height: if text.is_empty() { Some(0.0) } else { None },
            grow: 0.0,
            padding: 0.0,
            text: Some(style),
            // Centre each LINE within its region as well as the region within
            // the window.
            //
            // Placing only the region centres the block, and a Stack still
            // left-aligns its children -- so the menu rows all began at the
            // same x and the block hung off to one side, with only the widest
            // line landing anywhere near the middle. Measured on the results
            // screen: two bands at x=1599 in a 3200-wide frame (dead centre)
            // and three at +66, +91 and +371.
            place: Some(types::Placement::TopCentre),
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

fn header_cell(index: usize, text: &str) -> types::WidgetNode {
    types::WidgetNode {
        style: types::Style {
            // Fixed width per cell, so LAP / POSITION / TIME stay in the same
            // places as their values change. Without it the cells shuffle
            // sideways every time a digit is added, which is the single most
            // distracting thing a readout can do.
            width: Some(300.0),
            ..header_style()
        },
        ..text_node(
            HEADER_BASE + index as u64,
            HEADER_ID,
            text,
            ink(26.0, true),
        )
    }
}

fn header_style() -> types::Style {
    types::Style {
        width: None,
        // A height as well as a width.
        //
        // Without one the Grid stretched every cell to 860 pixels -- the whole
        // window less its padding -- because a flex row makes its children
        // fill the cross axis by default. The text still drew at the top of
        // that box, so it LOOKED right while every cell was quietly claiming
        // the screen and swallowing anything behind it.
        height: Some(34.0),
        grow: 0.0,
        padding: 0.0,
        place: None,
        text: Some(ink(26.0, true)),
    }
}

fn centre_line(index: usize, text: &str, warm: bool) -> types::WidgetNode {
    // The first centre line is the headline -- the countdown number, the
    // result -- and is read at a glance; the rest are read when there is a
    // moment.
    let style = if index == 0 {
        // The headline carries the countdown digit and the result, and both
        // are read at a glance from across a room. 64px looked large in the
        // source and small on screen: the window is 1600 logical pixels wide,
        // so a 64px glyph is 4% of it. 120 is a headline.
        if warm {
            ink_warm(120.0, true)
        } else {
            ink(120.0, true)
        }
    } else if warm {
        ink_warm(26.0, false)
    } else {
        ink(26.0, false)
    };
    text_node(CENTRE_BASE + index as u64, CENTRE_ID, text, style)
}

fn foot_line(text: &str) -> types::WidgetNode {
    text_node(FOOT_LINE, FOOT_ID, text, ink(24.0, false))
}

fn pure(s: &str) -> String {
    let mut out = String::new();
    out.push_str(s);
    out
}

/// The ordinal a finishing position is announced with: 1st, 2nd, 3rd, 4th.
///
/// Hand-written because a `#![no_std]` guest has no `format!` for this shape,
/// and the field is six cars so the exceptional teens never come up.
pub fn ordinal(pos: u32, out: &mut String) {
    push_u32(pos.max(1), out);
    out.push_str(match pos {
        1 => "st",
        2 => "nd",
        3 => "rd",
        _ => "th",
    });
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

/// `m:ss.t` -- the way a lap time is read.
///
/// Tenths, not hundredths: a driver reads a lap time at a glance and the
/// hundredths column changes too fast to be read at all while racing. The
/// results screen is where hundredths belong.
pub fn push_time(seconds: f32, out: &mut String) {
    let t = if seconds.is_finite() && seconds > 0.0 {
        seconds
    } else {
        0.0
    };
    let mins = (t / 60.0) as u32;
    let secs = (t as u32) % 60;
    let tenths = ((t * 10.0) as u32) % 10;
    push_u32(mins, out);
    out.push(':');
    if secs < 10 {
        out.push('0');
    }
    push_u32(secs, out);
    out.push('.');
    push_u32(tenths, out);
}
