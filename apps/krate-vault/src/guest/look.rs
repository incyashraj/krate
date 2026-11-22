//! Colours, sizes, and the small drawn icons.
//!
//! The density is the one people who keep notes in a vault know: a 44 px
//! ribbon, sidebars of about 260 px with 13 px rows, a 16 px editor at a
//! 700 px reading width, a 12 px status bar. The look is Vault's own --
//! its own accent, its own icons, drawn here from rectangles and circles.

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

pub struct Theme {
    pub bg: Rgba,
    pub side: Rgba,
    pub chrome: Rgba,
    pub border: Rgba,
    pub hover: Rgba,
    pub active: Rgba,
    pub text: Rgba,
    pub muted: Rgba,
    pub faint: Rgba,
    pub accent: Rgba,
    pub on_accent: Rgba,
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
}

pub const DARK: Theme = Theme {
    bg: hex(0x1E1E1E),
    side: hex(0x262626),
    chrome: hex(0x202020),
    border: hex(0x363636),
    hover: hex(0x303030),
    active: hex(0x3A3A3A),
    text: hex(0xDCDDDE),
    muted: hex(0x9A9A9A),
    faint: hex(0x666666),
    accent: hex(0x4C8DFF),
    on_accent: hex(0xFFFFFF),
    link: hex(0x7AA7FF),
    link_missing: hex(0x5F7FB8),
    code_bg: hex(0x2A2A2A),
    mark_bg: alpha(hex(0xFFD000), 0.32),
    tag_bg: alpha(hex(0x4C8DFF), 0.16),
    sel: alpha(hex(0x4C8DFF), 0.34),
    find: alpha(hex(0xFFD000), 0.28),
    scrim: alpha(hex(0x000000), 0.55),
    modal: hex(0x262626),
    field: hex(0x1A1A1A),
    danger: hex(0xE5484D),
    quote: hex(0x4C8DFF),
};

pub const LIGHT: Theme = Theme {
    bg: hex(0xFFFFFF),
    side: hex(0xF6F6F6),
    chrome: hex(0xEDEDED),
    border: hex(0xE0E0E0),
    hover: hex(0xEBEBEB),
    active: hex(0xE2E2E2),
    text: hex(0x222222),
    muted: hex(0x5C5C5C),
    faint: hex(0xABABAB),
    accent: hex(0x2F6FEB),
    on_accent: hex(0xFFFFFF),
    link: hex(0x2F62D1),
    link_missing: hex(0x7F9BD8),
    code_bg: hex(0xF3F3F3),
    mark_bg: alpha(hex(0xFFD000), 0.40),
    tag_bg: alpha(hex(0x2F6FEB), 0.12),
    sel: alpha(hex(0x2F6FEB), 0.22),
    find: alpha(hex(0xFFD000), 0.40),
    scrim: alpha(hex(0x000000), 0.30),
    modal: hex(0xFFFFFF),
    field: hex(0xF6F6F6),
    danger: hex(0xD93036),
    quote: hex(0x2F6FEB),
};

/// Sizes, scaled by the person's text size.
#[derive(Clone, Copy)]
pub struct Sizes {
    pub ui: f32,
    pub small: f32,
    pub body: f32,
    pub code: f32,
    pub row: f32,
    pub head: f32,
    pub ribbon: f32,
    pub tab: f32,
    pub view_head: f32,
    pub measure: f32,
    pub scale: f32,
}

impl Sizes {
    pub fn new(scale: f32) -> Sizes {
        let k = scale.clamp(0.8, 2.0);
        Sizes {
            ui: 13.0 * k,
            small: 12.0 * k,
            body: 16.0 * k,
            code: 14.0 * k,
            row: (26.0 * k).round(),
            head: (40.0 * k).round(),
            ribbon: (44.0 * k).round(),
            tab: (40.0 * k).round(),
            view_head: (38.0 * k).round(),
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

/// The icons, drawn from shapes so they are Vault's own.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Files,
    Search,
    Switcher,
    Palette,
    NewNote,
    NewFolder,
    Collapse,
    SidebarLeft,
    SidebarRight,
    Back,
    Forward,
    Reading,
    Editing,
    Backlinks,
    Outgoing,
    Tags,
    Outline,
    Theme,
    Plus,
    Close,
}

fn dot(c: u64, x: f32, y: f32, rad: f32, col: Rgba) {
    let _ = canvas2d::fill_circle(c, gfx::Point { x, y }, rad, col);
}

/// A straight line from (x0, y0) to (x1, y1), as a row of dots.
pub fn line(c: u64, x0: f32, y0: f32, x1: f32, y1: f32, w: f32, col: Rgba) {
    let dx = x1 - x0;
    let dy = y1 - y0;
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
    let len = (dx * dx + dy * dy).sqrt();
    let steps = (len / (w * 0.5)).ceil().max(2.0) as usize;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        dot(c, x0 + dx * t, y0 + dy * t, w * 0.5, col);
    }
}

/// A folder's chevron centred on (x, y): pointing right when closed,
/// down when open.
pub fn chevron(c: u64, x: f32, y: f32, open: bool, k: f32, col: Rgba) {
    let s = 1.4 * k;
    if open {
        line(c, x - 3.5 * k, y - 1.8 * k, x, y + 1.8 * k, s, col);
        line(c, x, y + 1.8 * k, x + 3.5 * k, y - 1.8 * k, s, col);
    } else {
        line(c, x - 1.8 * k, y - 3.5 * k, x + 1.8 * k, y, s, col);
        line(c, x + 1.8 * k, y, x - 1.8 * k, y + 3.5 * k, s, col);
    }
}

/// An icon centred on (x, y), about 16 px.
pub fn icon(c: u64, which: Icon, x: f32, y: f32, k: f32, col: Rgba) {
    let s = 1.5 * k;
    match which {
        Icon::Files => {
            outline(
                c,
                r(x - 7.0 * k, y - 4.0 * k, 14.0 * k, 10.0 * k),
                2.0 * k,
                s,
                col,
            );
            fill(c, r(x - 7.0 * k, y - 6.5 * k, 6.0 * k, 3.0 * k), col);
        }
        Icon::Search => {
            let _ = canvas2d::stroke_circle(
                c,
                gfx::Point {
                    x: x - 1.5 * k,
                    y: y - 1.5 * k,
                },
                5.0 * k,
                s,
                col,
            );
            line(
                c,
                x + 2.2 * k,
                y + 2.2 * k,
                x + 6.5 * k,
                y + 6.5 * k,
                s * 1.2,
                col,
            );
        }
        Icon::Switcher => {
            outline(
                c,
                r(x - 7.0 * k, y - 6.0 * k, 14.0 * k, 12.0 * k),
                2.0 * k,
                s,
                col,
            );
            line(
                c,
                x - 3.0 * k,
                y + 2.5 * k,
                x + 3.0 * k,
                y - 2.5 * k,
                s,
                col,
            );
            fill(c, r(x, y - 3.2 * k, 3.4 * k, s), col);
            fill(c, r(x + 2.4 * k, y - 3.2 * k, s, 3.4 * k), col);
        }
        Icon::Palette => {
            outline(
                c,
                r(x - 7.0 * k, y - 6.0 * k, 14.0 * k, 12.0 * k),
                2.0 * k,
                s,
                col,
            );
            line(c, x - 3.5 * k, y - 2.5 * k, x - 0.5 * k, y, s, col);
            line(c, x - 0.5 * k, y, x - 3.5 * k, y + 2.5 * k, s, col);
            fill(c, r(x + 0.5 * k, y + 2.0 * k, 3.5 * k, s), col);
        }
        Icon::NewNote | Icon::NewFolder => {
            if which == Icon::NewNote {
                outline(
                    c,
                    r(x - 6.0 * k, y - 7.0 * k, 12.0 * k, 14.0 * k),
                    2.0 * k,
                    s,
                    col,
                );
            } else {
                outline(
                    c,
                    r(x - 7.0 * k, y - 4.0 * k, 14.0 * k, 10.0 * k),
                    2.0 * k,
                    s,
                    col,
                );
                fill(c, r(x - 7.0 * k, y - 6.5 * k, 6.0 * k, 3.0 * k), col);
            }
            let cy = if which == Icon::NewNote {
                y
            } else {
                y + 1.0 * k
            };
            fill(c, r(x - 3.0 * k, cy - s * 0.5, 6.0 * k, s), col);
            fill(c, r(x - s * 0.5, cy - 3.0 * k, s, 6.0 * k), col);
        }
        Icon::Collapse => {
            line(c, x - 4.0 * k, y - 1.0 * k, x, y - 5.0 * k, s, col);
            line(c, x, y - 5.0 * k, x + 4.0 * k, y - 1.0 * k, s, col);
            line(c, x - 4.0 * k, y + 1.0 * k, x, y + 5.0 * k, s, col);
            line(c, x, y + 5.0 * k, x + 4.0 * k, y + 1.0 * k, s, col);
        }
        Icon::SidebarLeft | Icon::SidebarRight => {
            outline(
                c,
                r(x - 7.5 * k, y - 6.0 * k, 15.0 * k, 12.0 * k),
                2.0 * k,
                s,
                col,
            );
            let bx = if which == Icon::SidebarLeft {
                x - 2.5 * k
            } else {
                x + 2.5 * k
            };
            fill(c, r(bx - s * 0.5, y - 6.0 * k, s, 12.0 * k), col);
        }
        Icon::Back | Icon::Forward => {
            let d = if which == Icon::Back { 1.0 } else { -1.0 };
            line(
                c,
                x + 2.0 * k * d,
                y - 5.0 * k,
                x - 3.0 * k * d,
                y,
                s * 1.1,
                col,
            );
            line(
                c,
                x - 3.0 * k * d,
                y,
                x + 2.0 * k * d,
                y + 5.0 * k,
                s * 1.1,
                col,
            );
        }
        Icon::Reading => {
            // An open book.
            outline(
                c,
                r(x - 7.5 * k, y - 5.5 * k, 7.5 * k, 11.0 * k),
                1.5 * k,
                s,
                col,
            );
            outline(c, r(x, y - 5.5 * k, 7.5 * k, 11.0 * k), 1.5 * k, s, col);
        }
        Icon::Editing => {
            // A pencil.
            line(
                c,
                x - 5.5 * k,
                y + 5.5 * k,
                x + 4.0 * k,
                y - 4.0 * k,
                s * 1.6,
                col,
            );
            dot(c, x + 5.0 * k, y - 5.0 * k, s, col);
            fill(c, r(x - 7.0 * k, y + 6.0 * k, 4.0 * k, s), col);
        }
        Icon::Backlinks | Icon::Outgoing => {
            outline(
                c,
                r(x - 7.0 * k, y - 6.0 * k, 9.0 * k, 12.0 * k),
                2.0 * k,
                s,
                col,
            );
            let (from, to) = if which == Icon::Backlinks {
                (x + 7.0 * k, x - 1.0 * k)
            } else {
                (x - 1.0 * k, x + 7.0 * k)
            };
            fill(c, r(from.min(to), y - s * 0.5, (from - to).abs(), s), col);
            let tip = to;
            let d = if to > from { -1.0 } else { 1.0 };
            line(c, tip, y, tip + 3.0 * k * d, y - 3.0 * k, s, col);
            line(c, tip, y, tip + 3.0 * k * d, y + 3.0 * k, s, col);
        }
        Icon::Tags => {
            fill(c, r(x - 5.0 * k, y - 2.5 * k, 11.0 * k, s), col);
            fill(c, r(x - 6.0 * k, y + 2.0 * k, 11.0 * k, s), col);
            line(
                c,
                x - 1.0 * k,
                y - 6.5 * k,
                x - 3.0 * k,
                y + 6.5 * k,
                s,
                col,
            );
            line(
                c,
                x + 3.5 * k,
                y - 6.5 * k,
                x + 1.5 * k,
                y + 6.5 * k,
                s,
                col,
            );
        }
        Icon::Outline => {
            for (i, w) in [12.0f32, 9.0, 9.0, 6.0].iter().enumerate() {
                let yy = y - 5.5 * k + i as f32 * 3.7 * k;
                let xx = x - 6.0 * k + if i == 0 { 0.0 } else { 3.0 * k };
                fill(c, r(xx, yy, w * k, s), col);
            }
        }
        Icon::Theme => {
            let _ = canvas2d::stroke_circle(c, gfx::Point { x, y }, 6.0 * k, s, col);
            let _ =
                canvas2d::stroke_arc(c, gfx::Point { x, y }, 3.0 * k, 90.0, 180.0, 6.0 * k, col);
        }
        Icon::Plus => {
            fill(c, r(x - 5.5 * k, y - s * 0.5, 11.0 * k, s), col);
            fill(c, r(x - s * 0.5, y - 5.5 * k, s, 11.0 * k), col);
        }
        Icon::Close => {
            line(
                c,
                x - 4.0 * k,
                y - 4.0 * k,
                x + 4.0 * k,
                y + 4.0 * k,
                s,
                col,
            );
            line(
                c,
                x - 4.0 * k,
                y + 4.0 * k,
                x + 4.0 * k,
                y - 4.0 * k,
                s,
                col,
            );
        }
    }
}
