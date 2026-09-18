//! Cards -- a styled interface built from REAL WIDGETS, not a canvas.
//!
//! The point this app exists to make: until `box-style`, an app that wanted a
//! card with a background and a rounded corner had to abandon the widget set
//! and hand-draw its whole interface on a canvas. `apps/krate-board` does
//! exactly that and looks modern; `apps/krate-settings` uses real widgets and
//! looks like a 2005 dialog. Same runtime, same day.
//!
//! That choice cost everything the widget set gives: real controls, native
//! text editing, host scrolling, accessibility, and a layout that reflows when
//! the window resizes. A rounded corner is not worth any of that.
//!
//! Everything here is a `stack`, a `text` or a `button`. Nothing is drawn by
//! this app -- the boxes, borders and corners are the host's, asked for
//! through `style.box`. Resize the window and it reflows, because taffy is
//! still doing the layout.
//!
//! `#![no_std]`, so it stays `krate:*`-only.

#![no_std]

extern crate alloc;

use alloc::string::String;
use krate::gfx::types as gfx;
use krate::ui::{events, tree, types, window};

const WIDTH: u32 = 900;
const HEIGHT: u32 = 620;

// Ids. Laid out in bands so a card's children are easy to place by hand.
const ROOT: u64 = 1;
const HEADER: u64 = 2;
const TITLE: u64 = 3;
const SUBTITLE: u64 = 4;
const ROW: u64 = 5;
/// Each card takes a block of ten ids: the card, then its children.
const CARD_BASE: u64 = 100;
const FOOT: u64 = 900;
const FOOT_TEXT: u64 = 901;

/// ui colours are 0-255 channels, unlike gfx's 0.0-1.0 floats.
const fn rgb(r: u8, g: u8, b: u8) -> types::Color {
    types::Color { r, g, b, a: 255 }
}

const INK: types::Color = rgb(237, 240, 247);
const INK_QUIET: types::Color = rgb(148, 158, 178);
const PAGE: types::Color = rgb(14, 16, 22);
const CARD: types::Color = rgb(26, 29, 38);
const CARD_EDGE: types::Color = rgb(48, 54, 66);

/// One card's accent, and what it says.
struct Card {
    tag: &'static str,
    tag_ink: types::Color,
    tag_fill: types::Color,
    title: &'static str,
    body: &'static str,
}

const CARDS: [Card; 3] = [
    Card {
        tag: " DESIGN ",
        tag_ink: rgb(204, 178, 255),
        tag_fill: rgb(56, 43, 92),
        title: "Onboarding flow",
        body: "Three screens, one decision each.",
    },
    Card {
        tag: " BUILD ",
        tag_ink: rgb(140, 204, 255),
        tag_fill: rgb(33, 64, 102),
        title: "Keyboard shortcuts",
        body: "Every action reachable without a mouse.",
    },
    Card {
        tag: " SHIP ",
        tag_ink: rgb(140, 235, 184),
        tag_fill: rgb(28, 76, 56),
        title: "Release notes",
        body: "Written as the work lands, not after.",
    },
];

fn owned(s: &str) -> String {
    let mut out = String::new();
    out.push_str(s);
    out
}

/// A node with no box and no text style: the plain default.
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

fn ink(color: types::Color, size: f32, bold: bool) -> types::TextStyle {
    types::TextStyle {
        color: Some(color),
        outline: None,
        outline_width: 0.0,
        size: Some(size),
        bold,
    }
}

fn boxed(
    background: Option<types::Color>,
    border: Option<types::Color>,
    border_width: f32,
    corner_radius: f32,
) -> types::BoxStyle {
    types::BoxStyle {
        background,
        border,
        border_width,
        corner_radius,
    }
}

fn label(
    id: u64,
    parent: u64,
    text: &str,
    style: types::TextStyle,
    box_: Option<types::BoxStyle>,
) -> types::WidgetNode {
    let mut n = node(id, Some(parent), types::WidgetKind::Text);
    n.label = Some(owned(text));
    n.style.text = Some(style);
    n.style.box_ = box_;
    n
}

fn build(win: u64) -> bool {
    // The page. A stack with a background is the whole reason this app is
    // legible: before `box`, a container painted nothing and the window was
    // the host's default grey whatever the app wanted.
    let mut root = node(ROOT, None, types::WidgetKind::Stack);
    root.style.grow = 1.0;
    root.style.padding = 28.0;
    root.style.box_ = Some(boxed(Some(PAGE), None, 0.0, 0.0));
    if tree::set_root(win, &root).is_err() {
        return false;
    }

    let header = {
        let mut n = node(HEADER, Some(ROOT), types::WidgetKind::Stack);
        n.style.padding = 4.0;
        n
    };
    if tree::upsert_node(win, &header).is_err() {
        return false;
    }
    let title = label(TITLE, HEADER, "Launch board", ink(INK, 30.0, true), None);
    let subtitle = label(
        SUBTITLE,
        HEADER,
        "Real widgets. The boxes are the host's.",
        ink(INK_QUIET, 14.0, false),
        None,
    );
    if tree::upsert_node(win, &title).is_err() || tree::upsert_node(win, &subtitle).is_err() {
        return false;
    }

    // A Grid is the only container that lays out as a ROW; every other one is
    // a column. Three cards side by side need a grid.
    let mut row = node(ROW, Some(ROOT), types::WidgetKind::Grid);
    row.style.grow = 1.0;
    row.style.padding = 14.0;
    if tree::upsert_node(win, &row).is_err() {
        return false;
    }

    for (i, card) in CARDS.iter().enumerate() {
        let base = CARD_BASE + (i as u64) * 10;
        // The card itself: filled, bordered, rounded. None of these three
        // words could be said about a widget before this change.
        let mut holder = node(base, Some(ROW), types::WidgetKind::Stack);
        holder.style.grow = 1.0;
        holder.style.padding = 18.0;
        holder.style.box_ = Some(boxed(Some(CARD), Some(CARD_EDGE), 1.0, 14.0));
        if tree::upsert_node(win, &holder).is_err() {
            return false;
        }

        // The tag is a pill: a text widget with its own fill and a radius
        // larger than half its height, which the host clamps to a lozenge.
        let tag = label(
            base + 1,
            base,
            card.tag,
            ink(card.tag_ink, 11.0, true),
            Some(boxed(Some(card.tag_fill), None, 0.0, 20.0)),
        );
        let heading = label(base + 2, base, card.title, ink(INK, 17.0, true), None);
        let body = label(base + 3, base, card.body, ink(INK_QUIET, 13.0, false), None);
        if tree::upsert_node(win, &tag).is_err()
            || tree::upsert_node(win, &heading).is_err()
            || tree::upsert_node(win, &body).is_err()
        {
            return false;
        }

        // A real button, restyled. It still hovers, still presses, still
        // reports an action -- the host washes the app's own fill rather than
        // replacing it.
        let mut button = node(base + 4, Some(base), types::WidgetKind::Button);
        button.label = Some(owned("Open"));
        button.style.text = Some(ink(INK, 13.0, false));
        button.style.box_ = Some(boxed(Some(card.tag_fill), Some(card.tag_ink), 1.0, 8.0));
        if tree::upsert_node(win, &button).is_err() {
            return false;
        }
    }

    // A foot strip, to show a bordered band as well as a filled one.
    let mut foot = node(FOOT, Some(ROOT), types::WidgetKind::Stack);
    foot.style.padding = 12.0;
    foot.style.box_ = Some(boxed(None, Some(CARD_EDGE), 1.0, 10.0));
    if tree::upsert_node(win, &foot).is_err() {
        return false;
    }
    let foot_text = label(
        FOOT_TEXT,
        FOOT,
        "Every box, border and corner here is style.box -- this app draws nothing itself.",
        ink(INK_QUIET, 12.0, false),
        None,
    );
    tree::upsert_node(win, &foot_text).is_ok()
}

struct Component;

impl krate::Guest for Component {
    fn run() -> i32 {
        let win = match window::create(
            "Cards",
            types::WindowSize {
                width: WIDTH,
                height: HEIGHT,
            },
        ) {
            Ok(win) => win,
            Err(_) => return 1,
        };
        if !build(win) {
            return 1;
        }

        loop {
            match events::wait(Some(120)) {
                Some(types::Event::CloseRequested(_)) => break,
                _ => continue,
            }
        }
        let _ = window::close(win);
        0
    }
}

krate::export!(Component);

// Keep the gfx import referenced: the app asks for no canvas, and saying so
// here stops the unused-import warning from suggesting it should.
const _: Option<gfx::Color> = None;
