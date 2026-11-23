//! Colours, sizes, and the drawn icons.
//!
//! The look is the well-known notes app's default theme, dark and light,
//! value for value (its CSS variables: `--color-base-00` to `-100`, the
//! accent `hsl(258 88% 66%)`, 40 px headers, 13 px interface text, 300 px
//! sidebars). The icons are Lucide-style line icons drawn here from lines,
//! arcs and rounded rectangles -- the app's own logo is not among them.

use krate::gfx::{canvas2d, types as gfx};
use krate::layout::Rect;
#[allow(unused_imports)]
use krate::prelude::*;

pub type Rgba = gfx::Color;

pub const fn hex(v: u32) -> Rgba {
    gfx::Color {
        r: ((v >> 16) & 0xff) as f32 / 255.0,
        g: ((v >> 8) & 0xff) as f32 / 255.0,
        b: (v & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

pub const fn alpha(c: Rgba, a: f32) -> Rgba {
    gfx::Color {
        r: c.r,
        g: c.g,
        b: c.b,
        a,
    }
}

/// `0xRRGGBB` of a colour (alpha dropped), for the graph's own picture.
pub fn rgb(c: Rgba) -> u32 {
    let b = |x: f32| ((x.clamp(0.0, 1.0) * 255.0).round() as u32) & 0xff;
    (b(c.r) << 16) | (b(c.g) << 8) | b(c.b)
}

/// `a` over `b`, as one solid colour.
pub fn over(a: Rgba, b: Rgba) -> Rgba {
    gfx::Color {
        r: b.r + (a.r - b.r) * a.a,
        g: b.g + (a.g - b.g) * a.a,
        b: b.b + (a.b - b.b) * a.a,
        a: 1.0,
    }
}

pub struct Theme {
    /// `--background-primary`: notes, the active tab.
    pub bg: Rgba,
    /// `--background-secondary`: sidebars, the tab bar, the status bar.
    pub side: Rgba,
    /// `--background-modifier-border`.
    pub border: Rgba,
    /// `--background-modifier-border-hover`.
    pub border_hover: Rgba,
    /// `--background-modifier-border-focus`.
    pub border_focus: Rgba,
    /// `--background-modifier-hover` (over the background it sits on).
    pub hover: Rgba,
    /// A selected row.
    pub active: Rgba,
    pub text: Rgba,
    pub muted: Rgba,
    pub faint: Rgba,
    /// `--interactive-accent`.
    pub accent: Rgba,
    pub on_accent: Rgba,
    /// `--text-accent`: links.
    pub link: Rgba,
    pub link_missing: Rgba,
    pub code_bg: Rgba,
    pub mark_bg: Rgba,
    pub tag_bg: Rgba,
    pub sel: Rgba,
    pub find: Rgba,
    pub scrim: Rgba,
    pub modal: Rgba,
    pub field: Rgba,
    pub danger: Rgba,
    pub quote: Rgba,
    /// `--interactive-normal`: buttons.
    pub button: Rgba,
    pub graph_line: Rgba,
    pub canvas_edge: Rgba,
    pub dark: bool,
}

pub const DARK: Theme = Theme {
    bg: hex(0x1C1C1C),
    side: hex(0x282828),
    border: hex(0x333333),
    border_hover: hex(0x3F3F3F),
    border_focus: hex(0x555555),
    hover: alpha(hex(0xFFFFFF), 0.067),
    active: alpha(hex(0xFFFFFF), 0.067),
    text: hex(0xDADADA),
    muted: hex(0xB3B3B3),
    faint: hex(0x666666),
    accent: hex(0x8A5CF5),
    on_accent: hex(0xFFFFFF),
    link: hex(0xA68AF9),
    link_missing: alpha(hex(0xA68AF9), 0.7),
    code_bg: hex(0x232323),
    mark_bg: alpha(hex(0xFFD000), 0.4),
    tag_bg: alpha(hex(0x8A5CF5), 0.1),
    sel: alpha(hex(0x8A5CF5), 0.33),
    find: alpha(hex(0xFFD000), 0.4),
    scrim: alpha(hex(0x0A0A0A), 0.4),
    modal: hex(0x1C1C1C),
    field: hex(0x2E2E2E),
    danger: hex(0xFB464C),
    quote: hex(0x8A5CF5),
    button: hex(0x333333),
    graph_line: hex(0x3F3F3F),
    canvas_edge: hex(0x7E7E7E),
    dark: true,
};

pub const LIGHT: Theme = Theme {
    bg: hex(0xFFFFFF),
    side: hex(0xF6F6F6),
    border: hex(0xE4E4E4),
    border_hover: hex(0xDADADA),
    border_focus: hex(0xBDBDBD),
    hover: alpha(hex(0x000000), 0.067),
    active: alpha(hex(0x000000), 0.067),
    text: hex(0x222222),
    muted: hex(0x5C5C5C),
    faint: hex(0xABABAB),
    accent: hex(0x8A5CF5),
    on_accent: hex(0xFFFFFF),
    link: hex(0x8A5CF5),
    link_missing: alpha(hex(0x8A5CF5), 0.7),
    code_bg: hex(0xFAFAFA),
    mark_bg: alpha(hex(0xFFD000), 0.4),
    tag_bg: alpha(hex(0x8A5CF5), 0.1),
    sel: alpha(hex(0x8A5CF5), 0.2),
    find: alpha(hex(0xFFD000), 0.4),
    scrim: alpha(hex(0xDCDCDC), 0.4),
    modal: hex(0xFFFFFF),
    field: hex(0xFFFFFF),
    danger: hex(0xE93147),
    quote: hex(0x8A5CF5),
    button: hex(0xFFFFFF),
    graph_line: hex(0xDADADA),
    canvas_edge: hex(0xABABAB),
    dark: false,
};

/// Sizes, scaled by the person's text size.
#[derive(Clone, Copy)]
pub struct Sizes {
    /// `--font-ui-small`.
    pub ui: f32,
    /// `--font-ui-smaller`.
    pub small: f32,
    /// `--font-ui-medium`.
    pub medium: f32,
    /// The note's text (`baseFontSize`).
    pub body: f32,
    pub code: f32,
    /// A row of the file tree.
    pub row: f32,
    /// `--header-height`.
    pub head: f32,
    pub ribbon: f32,
    pub tab: f32,
    pub view_head: f32,
    /// `--file-line-width`.
    pub measure: f32,
    pub scale: f32,
}

impl Sizes {
    pub fn new(scale: f32) -> Sizes {
        Sizes::with_body(scale, 16.0)
    }

    pub fn with_body(scale: f32, body: f32) -> Sizes {
        let k = scale.clamp(0.8, 2.0);
        Sizes {
            ui: 13.0 * k,
            small: 12.0 * k,
            medium: 15.0 * k,
            body: body * k,
            code: body * 0.875 * k,
            row: (27.0 * k).round(),
            head: (40.0 * k).round(),
            ribbon: (44.0 * k).round(),
            tab: (40.0 * k).round(),
            view_head: (40.0 * k).round(),
            measure: 700.0 * k,
            scale: k,
        }
    }
}

pub fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::new(x, y, w.max(0.0), h.max(0.0))
}

pub fn g(rc: Rect) -> gfx::Rect {
    gfx::Rect {
        x: rc.x,
        y: rc.y,
        width: rc.w,
        height: rc.h,
    }
}

pub fn radii(v: f32) -> gfx::CornerRadii {
    gfx::CornerRadii {
        top_left: v,
        top_right: v,
        bottom_right: v,
        bottom_left: v,
    }
}

pub fn style(weight: u16, italic: bool, mono: bool) -> gfx::TextStyle {
    gfx::TextStyle {
        weight,
        italic,
        letter_spacing: 0.0,
        family: if mono {
            gfx::FontFamily::Mono
        } else {
            gfx::FontFamily::Sans
        },
    }
}

pub fn fill(c: u64, rc: Rect, col: Rgba) {
    let _ = canvas2d::fill_rect(c, g(rc), col);
}

pub fn round(c: u64, rc: Rect, rad: f32, col: Rgba) {
    let _ = canvas2d::fill_round_rect(c, g(rc), radii(rad), col);
}

/// Rounded on the top corners only (an active tab).
pub fn round_top(c: u64, rc: Rect, rad: f32, col: Rgba) {
    let _ = canvas2d::fill_round_rect(
        c,
        g(rc),
        gfx::CornerRadii {
            top_left: rad,
            top_right: rad,
            bottom_right: 0.0,
            bottom_left: 0.0,
        },
        col,
    );
}

pub fn outline(c: u64, rc: Rect, rad: f32, w: f32, col: Rgba) {
    let _ = canvas2d::stroke_round_rect(c, g(rc), radii(rad), w, col);
}

pub fn text(c: u64, s: &str, x: f32, base: f32, size: f32, weight: u16, col: Rgba) {
    let _ = canvas2d::draw_text_styled(
        c,
        s,
        gfx::Point { x, y: base },
        size,
        col,
        style(weight, false, false),
    );
}

pub fn text_styled(c: u64, s: &str, x: f32, base: f32, size: f32, st: gfx::TextStyle, col: Rgba) {
    let _ = canvas2d::draw_text_styled(c, s, gfx::Point { x, y: base }, size, col, st);
}

/// Spaced small capitals (a file tag like CANVAS).
pub fn caps(c: u64, s: &str, x: f32, base: f32, size: f32, col: Rgba) -> f32 {
    let st = gfx::TextStyle {
        weight: 600,
        italic: false,
        letter_spacing: size * 0.08,
        family: gfx::FontFamily::Sans,
    };
    let _ = canvas2d::draw_text_styled(c, s, gfx::Point { x, y: base }, size, col, st);
    canvas2d::measure_text_styled(c, s, size, st).map_or(0.0, |m| m.width)
}

pub fn caps_width(c: u64, s: &str, size: f32) -> f32 {
    let st = gfx::TextStyle {
        weight: 600,
        italic: false,
        letter_spacing: size * 0.08,
        family: gfx::FontFamily::Sans,
    };
    canvas2d::measure_text_styled(c, s, size, st).map_or(0.0, |m| m.width)
}

pub fn width(c: u64, s: &str, size: f32, weight: u16) -> f32 {
    if s.is_empty() {
        return 0.0;
    }
    canvas2d::measure_text_styled(c, s, size, style(weight, false, false)).map_or(0.0, |m| m.width)
}

pub fn width_styled(c: u64, s: &str, size: f32, st: gfx::TextStyle) -> f32 {
    if s.is_empty() {
        return 0.0;
    }
    canvas2d::measure_text_styled(c, s, size, st).map_or(0.0, |m| m.width)
}

/// Draw `s` in at most `room` px, cut with an ellipsis. Returns the width.
#[allow(clippy::too_many_arguments)]
pub fn fit(
    c: u64,
    s: &str,
    x: f32,
    base: f32,
    size: f32,
    weight: u16,
    col: Rgba,
    room: f32,
) -> f32 {
    if room <= 4.0 || s.is_empty() {
        return 0.0;
    }
    let full = width(c, s, size, weight);
    if full <= room {
        text(c, s, x, base, size, weight, col);
        return full;
    }
    let ell = width(c, "\u{2026}", size, weight);
    let (mut lo, mut hi) = (0usize, s.len());
    while lo < hi {
        let mut mid = (lo + hi).div_ceil(2);
        while mid > lo && !s.is_char_boundary(mid) {
            mid -= 1;
        }
        if mid == lo {
            break;
        }
        if width(c, s.get(..mid).unwrap_or(""), size, weight) + ell <= room {
            lo = mid;
        } else {
            hi = mid - 1;
            while hi > lo && !s.is_char_boundary(hi) {
                hi -= 1;
            }
        }
    }
    let part = s.get(..lo).unwrap_or("").trim_end();
    let pw = width(c, part, size, weight);
    text(c, part, x, base, size, weight, col);
    text(c, "\u{2026}", x + pw, base, size, weight, col);
    pw + ell
}

/// A straight line from (x0, y0) to (x1, y1), `w` wide: one stretched,
/// turned pixel, so a line is one host call however long it is.
pub fn line(c: u64, x0: f32, y0: f32, x1: f32, y1: f32, w: f32, col: Rgba) {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 0.01 {
        return;
    }
    if dx.abs() < 0.01 || dy.abs() < 0.01 {
        let (x, y) = (x0.min(x1), y0.min(y1));
        fill(
            c,
            r(
                x - if dx.abs() < 0.01 { w * 0.5 } else { 0.0 },
                y - if dy.abs() < 0.01 { w * 0.5 } else { 0.0 },
                dx.abs().max(w),
                dy.abs().max(w),
            ),
            col,
        );
        return;
    }
    let px = [
        (col.r * 255.0) as u8,
        (col.g * 255.0) as u8,
        (col.b * 255.0) as u8,
        (col.a * 255.0) as u8,
    ];
    let _ = canvas2d::draw_sprite(
        c,
        gfx::Point {
            x: (x0 + x1) * 0.5,
            y: (y0 + y1) * 0.5,
        },
        gfx::Size {
            width: len,
            height: w,
        },
        dy.atan2(dx),
        1,
        1,
        &px,
    );
}

/// A line with round ends, as icon strokes are drawn.
pub fn stroke(c: u64, x0: f32, y0: f32, x1: f32, y1: f32, w: f32, col: Rgba) {
    line(c, x0, y0, x1, y1, w, col);
    let _ = canvas2d::fill_circle(c, gfx::Point { x: x0, y: y0 }, w * 0.5, col);
    let _ = canvas2d::fill_circle(c, gfx::Point { x: x1, y: y1 }, w * 0.5, col);
}

/// A folder's chevron centred on (x, y): pointing right when closed,
/// down when open.
pub fn chevron(c: u64, x: f32, y: f32, open: bool, k: f32, col: Rgba) {
    icon(
        c,
        if open {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        },
        x,
        y,
        k * 0.875,
        col,
    );
}

/// The icons, in the Lucide set's shapes (24-unit grid).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Folder,
    Search,
    Bookmark,
    SidebarLeft,
    SidebarRight,
    SquarePen,
    FolderPlus,
    Sort,
    Reveal,
    ChevronsUpDown,
    ChevronRight,
    ChevronDown,
    Close,
    Plus,
    Minus,
    ArrowLeft,
    ArrowRight,
    BookOpen,
    Pencil,
    More,
    Help,
    Gear,
    Backlinks,
    Outgoing,
    Tags,
    Archive,
    List,
    File,
    FileText,
    Graph,
    Canvas,
    Table,
    LayoutList,
    Calendar,
    Terminal,
    Pin,
    Link,
    Refresh,
    Maximize,
    Undo,
    Redo,
    Image,
    Filter,
    History,
    Hash,
    Info,
    Text,
    Check,
    Headphones,
    ArrowUpDown,
    Rows,
    Binary,
    SquareCheck,
    ChevronUp,
    Kanban,
    LayoutGrid,
    Trash,
    Code,
    Dice,
    FilePlus,
    ZoomIn,
    ZoomOut,
    Globe,
    RotateCw,
    Glasses,
    Mic,
    Layout,
    FileSearch,
    Files,
}

/// An icon centred on (x, y), 16 px at `k` = 1.
pub fn icon(c: u64, which: Icon, x: f32, y: f32, k: f32, col: Rgba) {
    let s = 16.0 / 24.0 * k;
    let ox = x - 12.0 * s;
    let oy = y - 12.0 * s;
    let w = (1.75 * 16.0 / 18.0 * k).max(1.0);
    let p = |a: f32, b: f32| (ox + a * s, oy + b * s);
    let seg = |a: (f32, f32), b: (f32, f32)| {
        let (x0, y0) = p(a.0, a.1);
        let (x1, y1) = p(b.0, b.1);
        stroke(c, x0, y0, x1, y1, w, col);
    };
    let poly = |pts: &[(f32, f32)]| {
        for pair in pts.windows(2) {
            seg(pair[0], pair[1]);
        }
    };
    let rect = |x0: f32, y0: f32, ww: f32, hh: f32, rad: f32| {
        let (a, b) = p(x0, y0);
        let _ = canvas2d::stroke_round_rect(c, g(r(a, b, ww * s, hh * s)), radii(rad * s), w, col);
    };
    let circle = |cx: f32, cy: f32, rad: f32| {
        let (a, b) = p(cx, cy);
        let _ = canvas2d::stroke_circle(c, gfx::Point { x: a, y: b }, rad * s, w, col);
    };
    let arc = |cx: f32, cy: f32, rad: f32, start: f32, sweep: f32| {
        let (a, b) = p(cx, cy);
        let _ = canvas2d::stroke_arc(c, gfx::Point { x: a, y: b }, rad * s, start, sweep, w, col);
    };
    let dot = |cx: f32, cy: f32, rad: f32| {
        let (a, b) = p(cx, cy);
        let _ = canvas2d::fill_circle(c, gfx::Point { x: a, y: b }, rad * s, col);
    };
    let folder = || {
        poly(&[
            (2.5, 18.0),
            (2.5, 5.0),
            (4.0, 3.5),
            (8.0, 3.5),
            (10.0, 6.0),
            (20.0, 6.0),
            (21.5, 7.5),
            (21.5, 18.5),
            (20.0, 20.0),
            (4.0, 20.0),
            (2.5, 18.5),
        ]);
    };
    match which {
        Icon::Folder => {
            folder();
            seg((2.5, 10.0), (21.5, 10.0));
        }
        Icon::FolderPlus => {
            folder();
            seg((12.0, 10.0), (12.0, 16.0));
            seg((9.0, 13.0), (15.0, 13.0));
        }
        Icon::Search => {
            circle(11.0, 11.0, 7.5);
            seg((21.0, 21.0), (16.5, 16.5));
        }
        Icon::Bookmark => poly(&[
            (19.0, 21.0),
            (12.0, 17.0),
            (5.0, 21.0),
            (5.0, 5.0),
            (7.0, 3.0),
            (17.0, 3.0),
            (19.0, 5.0),
            (19.0, 21.0),
        ]),
        Icon::SidebarLeft => {
            rect(3.0, 3.0, 18.0, 18.0, 2.0);
            seg((9.0, 3.0), (9.0, 21.0));
        }
        Icon::SidebarRight => {
            rect(3.0, 3.0, 18.0, 18.0, 2.0);
            seg((15.0, 3.0), (15.0, 21.0));
        }
        Icon::SquarePen => {
            poly(&[
                (12.0, 3.0),
                (5.0, 3.0),
                (3.0, 5.0),
                (3.0, 19.0),
                (5.0, 21.0),
                (19.0, 21.0),
                (21.0, 19.0),
                (21.0, 12.0),
            ]);
            poly(&[
                (18.4, 2.6),
                (21.4, 5.6),
                (12.4, 14.6),
                (8.6, 15.6),
                (9.5, 11.6),
                (18.4, 2.6),
            ]);
        }
        Icon::Sort => {
            poly(&[(3.0, 8.0), (7.0, 4.0), (11.0, 8.0)]);
            seg((7.0, 4.0), (7.0, 20.0));
            seg((11.0, 12.0), (15.0, 12.0));
            seg((11.0, 16.0), (18.0, 16.0));
            seg((11.0, 20.0), (21.0, 20.0));
        }
        Icon::Reveal => {
            rect(3.0, 8.0, 18.0, 13.0, 2.0);
            seg((6.0, 4.0), (18.0, 4.0));
        }
        Icon::ChevronsUpDown => {
            poly(&[(7.0, 15.0), (12.0, 20.0), (17.0, 15.0)]);
            poly(&[(7.0, 9.0), (12.0, 4.0), (17.0, 9.0)]);
        }
        Icon::ChevronRight => poly(&[(9.0, 18.0), (15.0, 12.0), (9.0, 6.0)]),
        Icon::ChevronDown => poly(&[(6.0, 9.0), (12.0, 15.0), (18.0, 9.0)]),
        Icon::Close => {
            seg((18.0, 6.0), (6.0, 18.0));
            seg((6.0, 6.0), (18.0, 18.0));
        }
        Icon::Plus => {
            seg((5.0, 12.0), (19.0, 12.0));
            seg((12.0, 5.0), (12.0, 19.0));
        }
        Icon::Minus => seg((5.0, 12.0), (19.0, 12.0)),
        Icon::ArrowLeft => {
            poly(&[(12.0, 19.0), (5.0, 12.0), (12.0, 5.0)]);
            seg((19.0, 12.0), (5.0, 12.0));
        }
        Icon::ArrowRight => {
            poly(&[(12.0, 5.0), (19.0, 12.0), (12.0, 19.0)]);
            seg((5.0, 12.0), (19.0, 12.0));
        }
        Icon::BookOpen => {
            seg((12.0, 7.0), (12.0, 21.0));
            poly(&[
                (12.0, 7.0),
                (10.5, 4.3),
                (8.0, 3.0),
                (3.0, 3.0),
                (2.0, 4.0),
                (2.0, 17.0),
                (3.0, 18.0),
                (9.0, 18.0),
                (11.2, 19.2),
                (12.0, 21.0),
            ]);
            poly(&[
                (12.0, 7.0),
                (13.5, 4.3),
                (16.0, 3.0),
                (21.0, 3.0),
                (22.0, 4.0),
                (22.0, 17.0),
                (21.0, 18.0),
                (15.0, 18.0),
                (12.8, 19.2),
                (12.0, 21.0),
            ]);
        }
        Icon::Pencil => {
            poly(&[
                (17.0, 3.0),
                (21.0, 7.0),
                (8.0, 20.0),
                (3.0, 21.0),
                (4.0, 16.0),
                (17.0, 3.0),
            ]);
            seg((15.0, 5.0), (19.0, 9.0));
        }
        Icon::More => {
            dot(5.0, 12.0, 1.6);
            dot(12.0, 12.0, 1.6);
            dot(19.0, 12.0, 1.6);
        }
        Icon::Help => {
            circle(12.0, 12.0, 10.0);
            arc(12.0, 9.5, 2.9, 180.0, 230.0);
            seg((12.0, 12.4), (12.0, 13.5));
            dot(12.0, 17.0, 1.1);
        }
        Icon::Gear => {
            circle(12.0, 12.0, 3.0);
            circle(12.0, 12.0, 7.6);
            for i in 0..8 {
                let a = i as f32 * core::f32::consts::FRAC_PI_4;
                seg(
                    (12.0 + a.cos() * 7.8, 12.0 + a.sin() * 7.8),
                    (12.0 + a.cos() * 10.0, 12.0 + a.sin() * 10.0),
                );
            }
        }
        Icon::Backlinks | Icon::Outgoing => {
            // Two links of a chain, and an arrow in or out.
            rect(3.0, 9.0, 9.0, 6.0, 3.0);
            rect(12.0, 9.0, 9.0, 6.0, 3.0);
            seg((9.0, 12.0), (15.0, 12.0));
            if which == Icon::Backlinks {
                poly(&[(8.0, 19.0), (5.0, 21.5), (8.0, 24.0)]);
            } else {
                poly(&[(16.0, 0.0), (19.0, 2.5), (16.0, 5.0)]);
            }
        }
        Icon::Tags => {
            poly(&[
                (3.0, 3.0),
                (11.0, 3.0),
                (19.5, 11.5),
                (19.5, 12.5),
                (12.5, 19.5),
                (11.5, 19.5),
                (3.0, 11.0),
                (3.0, 3.0),
            ]);
            dot(7.5, 7.5, 1.3);
            poly(&[(15.0, 3.0), (22.0, 10.0), (22.0, 12.0), (17.0, 17.0)]);
        }
        Icon::Archive => {
            rect(2.0, 3.0, 20.0, 5.0, 1.0);
            poly(&[
                (4.0, 8.0),
                (4.0, 19.0),
                (6.0, 21.0),
                (18.0, 21.0),
                (20.0, 19.0),
                (20.0, 8.0),
            ]);
            seg((10.0, 12.0), (14.0, 12.0));
        }
        Icon::List => {
            for y in [6.0, 12.0, 18.0] {
                seg((8.0, y), (21.0, y));
                dot(3.5, y, 1.1);
            }
        }
        Icon::File | Icon::FileText => {
            poly(&[
                (15.0, 2.0),
                (6.0, 2.0),
                (4.0, 4.0),
                (4.0, 20.0),
                (6.0, 22.0),
                (18.0, 22.0),
                (20.0, 20.0),
                (20.0, 7.0),
                (15.0, 2.0),
            ]);
            poly(&[(14.0, 2.0), (14.0, 8.0), (20.0, 8.0)]);
            if which == Icon::FileText {
                seg((8.0, 13.0), (16.0, 13.0));
                seg((8.0, 17.0), (16.0, 17.0));
                seg((8.0, 9.0), (10.0, 9.0));
            }
        }
        Icon::Graph => {
            circle(12.0, 18.0, 3.0);
            circle(6.0, 6.0, 3.0);
            circle(18.0, 6.0, 3.0);
            poly(&[
                (6.0, 9.0),
                (6.0, 11.0),
                (7.0, 12.0),
                (17.0, 12.0),
                (18.0, 11.0),
                (18.0, 9.0),
            ]);
            seg((12.0, 12.0), (12.0, 15.0));
        }
        Icon::Canvas => {
            rect(3.0, 3.0, 7.0, 9.0, 1.0);
            rect(14.0, 3.0, 7.0, 5.0, 1.0);
            rect(14.0, 12.0, 7.0, 9.0, 1.0);
            rect(3.0, 16.0, 7.0, 5.0, 1.0);
        }
        Icon::Table => {
            rect(3.0, 3.0, 18.0, 18.0, 2.0);
            seg((3.0, 9.0), (21.0, 9.0));
            seg((3.0, 15.0), (21.0, 15.0));
            seg((12.0, 3.0), (12.0, 21.0));
        }
        Icon::Headphones => {
            arc(12.0, 12.0, 9.0, 180.0, 180.0);
            seg((3.0, 12.0), (3.0, 19.0));
            seg((21.0, 12.0), (21.0, 19.0));
            rect(3.0, 14.0, 5.0, 7.0, 2.0);
            rect(16.0, 14.0, 5.0, 7.0, 2.0);
        }
        Icon::ArrowUpDown => {
            poly(&[(21.0, 16.0), (17.0, 20.0), (13.0, 16.0)]);
            seg((17.0, 20.0), (17.0, 4.0));
            poly(&[(3.0, 8.0), (7.0, 4.0), (11.0, 8.0)]);
            seg((7.0, 4.0), (7.0, 20.0));
        }
        Icon::Rows => {
            rect(3.0, 4.0, 18.0, 7.0, 2.0);
            rect(3.0, 13.0, 18.0, 7.0, 2.0);
        }
        Icon::Binary => {
            rect(14.0, 14.0, 4.0, 6.0, 2.0);
            rect(6.0, 4.0, 4.0, 6.0, 2.0);
            seg((6.0, 20.0), (10.0, 20.0));
            seg((14.0, 10.0), (18.0, 10.0));
            poly(&[(6.0, 14.0), (8.0, 14.0), (8.0, 20.0)]);
            poly(&[(14.0, 4.0), (16.0, 4.0), (16.0, 10.0)]);
        }
        Icon::SquareCheck => {
            rect(3.0, 3.0, 18.0, 18.0, 2.0);
            poly(&[(9.0, 12.0), (11.0, 14.0), (15.0, 10.0)]);
        }
        Icon::ChevronUp => poly(&[(18.0, 15.0), (12.0, 9.0), (6.0, 15.0)]),
        Icon::Kanban => {
            rect(3.0, 3.0, 18.0, 18.0, 2.0);
            seg((8.0, 7.0), (8.0, 14.0));
            seg((12.0, 7.0), (12.0, 11.0));
            seg((16.0, 7.0), (16.0, 16.0));
        }
        Icon::Globe => {
            // lucide globe-2
            circle(12.0, 12.0, 10.0);
            poly(&[(21.54, 15.0), (17.0, 15.0), (15.0, 17.0), (15.0, 21.54)]);
            poly(&[
                (7.0, 3.34),
                (7.0, 5.0),
                (9.0, 7.0),
                (10.0, 7.0),
                (12.0, 9.0),
                (12.0, 11.0),
            ]);
            poly(&[
                (12.0, 11.0),
                (14.0, 13.0),
                (16.0, 11.0),
                (17.5, 9.5),
                (20.0, 9.5),
                (20.0, 7.0),
            ]);
            poly(&[
                (11.0, 21.95),
                (11.0, 18.0),
                (9.0, 16.0),
                (8.0, 14.0),
                (6.0, 13.0),
                (2.05, 11.0),
            ]);
        }
        Icon::RotateCw => {
            arc(12.0, 12.0, 9.0, 300.0, 300.0);
            poly(&[(21.0, 3.0), (21.0, 8.0), (16.0, 8.0)]);
        }
        Icon::Glasses => {
            circle(6.0, 15.0, 4.0);
            circle(18.0, 15.0, 4.0);
            arc(12.0, 15.0, 2.0, 180.0, 180.0);
            seg((2.5, 13.0), (5.0, 4.0));
            seg((21.5, 13.0), (19.0, 4.0));
        }
        Icon::Mic => {
            rect(9.0, 2.0, 6.0, 13.0, 3.0);
            arc(12.0, 10.0, 7.0, 0.0, 180.0);
            seg((19.0, 10.0), (19.0, 10.0));
            seg((12.0, 17.0), (12.0, 22.0));
        }
        Icon::FileSearch => {
            // lucide file-search: the page, its corner, a lens on it.
            poly(&[
                (14.0, 2.0),
                (6.0, 2.0),
                (4.0, 4.0),
                (4.0, 20.0),
                (6.0, 22.0),
                (18.0, 22.0),
                (20.0, 20.0),
                (20.0, 8.0),
                (14.0, 2.0),
            ]);
            poly(&[(14.0, 2.0), (14.0, 8.0), (20.0, 8.0)]);
            circle(11.5, 14.5, 2.5);
            seg((13.3, 16.3), (15.0, 18.0));
        }
        Icon::Files => {
            // lucide files: a page with one behind it.
            poly(&[
                (15.0, 2.0),
                (10.0, 2.0),
                (8.0, 4.0),
                (8.0, 16.0),
                (10.0, 18.0),
                (19.0, 18.0),
                (21.0, 16.0),
                (21.0, 8.0),
                (15.0, 2.0),
            ]);
            poly(&[(15.0, 2.0), (15.0, 8.0), (21.0, 8.0)]);
            poly(&[(4.0, 7.0), (4.0, 20.0), (6.0, 22.0), (16.0, 22.0)]);
        }
        Icon::Layout => {
            rect(3.0, 3.0, 18.0, 18.0, 2.0);
            seg((3.0, 9.0), (21.0, 9.0));
            seg((9.0, 21.0), (9.0, 9.0));
        }
        Icon::Dice => {
            rect(3.0, 3.0, 18.0, 18.0, 2.0);
            dot(8.0, 8.0, 1.5);
            dot(16.0, 8.0, 1.5);
            dot(12.0, 12.0, 1.5);
            dot(8.0, 16.0, 1.5);
            dot(16.0, 16.0, 1.5);
        }
        Icon::FilePlus => {
            poly(&[
                (15.0, 2.0),
                (6.0, 2.0),
                (4.0, 4.0),
                (4.0, 20.0),
                (6.0, 22.0),
                (18.0, 22.0),
                (20.0, 20.0),
                (20.0, 7.0),
                (15.0, 2.0),
            ]);
            poly(&[(14.0, 2.0), (14.0, 8.0), (20.0, 8.0)]);
            seg((12.0, 12.0), (12.0, 18.0));
            seg((9.0, 15.0), (15.0, 15.0));
        }
        Icon::Trash => {
            seg((3.0, 6.0), (21.0, 6.0));
            poly(&[
                (19.0, 6.0),
                (19.0, 20.0),
                (17.0, 22.0),
                (7.0, 22.0),
                (5.0, 20.0),
                (5.0, 6.0),
            ]);
            poly(&[
                (8.0, 6.0),
                (8.0, 4.0),
                (10.0, 2.0),
                (14.0, 2.0),
                (16.0, 4.0),
                (16.0, 6.0),
            ]);
        }
        Icon::Code => {
            poly(&[(18.0, 16.0), (22.0, 12.0), (18.0, 8.0)]);
            poly(&[(6.0, 8.0), (2.0, 12.0), (6.0, 16.0)]);
            seg((14.5, 4.0), (9.5, 20.0));
        }
        Icon::LayoutGrid => {
            rect(3.0, 3.0, 7.0, 7.0, 1.0);
            rect(14.0, 3.0, 7.0, 7.0, 1.0);
            rect(14.0, 14.0, 7.0, 7.0, 1.0);
            rect(3.0, 14.0, 7.0, 7.0, 1.0);
        }
        Icon::Check => poly(&[(20.0, 6.0), (9.0, 17.0), (4.0, 12.0)]),
        Icon::ZoomIn | Icon::ZoomOut => {
            circle(11.0, 11.0, 8.0);
            seg((21.0, 21.0), (16.65, 16.65));
            seg((8.0, 11.0), (14.0, 11.0));
            if which == Icon::ZoomIn {
                seg((11.0, 8.0), (11.0, 14.0));
            }
        }
        Icon::LayoutList => {
            rect(3.0, 3.0, 7.0, 7.0, 1.0);
            rect(3.0, 14.0, 7.0, 7.0, 1.0);
            seg((14.0, 4.0), (21.0, 4.0));
            seg((14.0, 9.0), (21.0, 9.0));
            seg((14.0, 15.0), (21.0, 15.0));
            seg((14.0, 20.0), (21.0, 20.0));
        }
        Icon::Calendar => {
            rect(3.0, 4.0, 18.0, 18.0, 2.0);
            seg((16.0, 2.0), (16.0, 6.0));
            seg((8.0, 2.0), (8.0, 6.0));
            seg((3.0, 10.0), (21.0, 10.0));
        }
        Icon::Terminal => {
            poly(&[(4.0, 17.0), (10.0, 11.0), (4.0, 5.0)]);
            seg((12.0, 19.0), (20.0, 19.0));
        }
        Icon::Pin => {
            seg((12.0, 17.0), (12.0, 22.0));
            poly(&[
                (9.0, 3.0),
                (15.0, 3.0),
                (15.0, 10.8),
                (17.9, 12.5),
                (19.0, 14.3),
                (19.0, 17.0),
                (5.0, 17.0),
                (5.0, 14.3),
                (6.1, 12.5),
                (9.0, 10.8),
                (9.0, 3.0),
            ]);
        }
        Icon::Link => {
            rect(2.0, 8.0, 11.0, 8.0, 4.0);
            rect(11.0, 8.0, 11.0, 8.0, 4.0);
        }
        Icon::Refresh => {
            arc(12.0, 12.0, 8.5, 200.0, 290.0);
            poly(&[(20.5, 3.5), (20.5, 8.5), (15.5, 8.5)]);
        }
        Icon::Maximize => {
            poly(&[(8.0, 3.0), (3.0, 3.0), (3.0, 8.0)]);
            poly(&[(16.0, 3.0), (21.0, 3.0), (21.0, 8.0)]);
            poly(&[(21.0, 16.0), (21.0, 21.0), (16.0, 21.0)]);
            poly(&[(3.0, 16.0), (3.0, 21.0), (8.0, 21.0)]);
        }
        Icon::Undo | Icon::Redo => {
            let m = |x: f32| if which == Icon::Undo { x } else { 24.0 - x };
            poly(&[(m(9.0), 14.0), (m(4.0), 9.0), (m(9.0), 4.0)]);
            poly(&[
                (m(4.0), 9.0),
                (m(14.5), 9.0),
                (m(18.0), 10.5),
                (m(20.0), 14.5),
                (m(18.0), 18.5),
                (m(14.5), 20.0),
                (m(11.0), 20.0),
            ]);
        }
        Icon::Image => {
            rect(3.0, 3.0, 18.0, 18.0, 2.0);
            circle(9.0, 9.0, 2.0);
            poly(&[(21.0, 15.0), (17.0, 11.0), (6.0, 21.0)]);
        }
        Icon::Filter => {
            seg((3.0, 6.0), (21.0, 6.0));
            seg((7.0, 12.0), (17.0, 12.0));
            seg((10.0, 18.0), (14.0, 18.0));
        }
        Icon::History => {
            arc(12.0, 12.0, 9.0, 160.0, 330.0);
            poly(&[(3.0, 3.0), (3.0, 8.0), (8.0, 8.0)]);
            poly(&[(12.0, 7.0), (12.0, 12.0), (16.0, 14.0)]);
        }
        Icon::Hash => {
            seg((4.0, 9.0), (20.0, 9.0));
            seg((4.0, 15.0), (20.0, 15.0));
            seg((10.0, 3.0), (8.0, 21.0));
            seg((16.0, 3.0), (14.0, 21.0));
        }
        Icon::Info => {
            circle(12.0, 12.0, 10.0);
            seg((12.0, 16.0), (12.0, 12.0));
            dot(12.0, 8.0, 1.1);
        }
        Icon::Text => {
            seg((17.0, 6.1), (3.0, 6.1));
            seg((21.0, 12.0), (3.0, 12.0));
            seg((15.1, 18.0), (3.0, 18.0));
        }
    }
}
