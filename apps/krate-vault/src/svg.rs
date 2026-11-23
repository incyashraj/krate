//! SVG: a real subset, read and rasterised in the app, so `![[diagram.svg]]`
//! shows the drawing and Lucide's icons (ISC, see LUCIDE-LICENSE.txt) draw
//! the same shapes Obsidian's do.
//!
//! Read: `svg` (width, height, viewBox), `g`, `path` (every command, arcs
//! included), `rect` (rounded), `circle`, `ellipse`, `line`, `polyline`,
//! `polygon` and `text`/`tspan`; `fill`, `stroke`, `stroke-width`,
//! `opacity`, `fill-opacity`, `stroke-opacity`, `fill-rule`, `style=""`,
//! `transform` (matrix, translate, scale, rotate, skew), and linear and
//! radial gradients by `url(#id)`; `<defs>`, `<use>` and `<symbol>`;
//! `<style>` sheets (type, class, id and descendant selectors); clip
//! paths, masks and group opacity, each group drawn on a layer of its own;
//! and the common filters (feGaussianBlur, feOffset, feColorMatrix,
//! feDropShadow, feFlood, feComposite, feMerge, feBlend). Not read:
//! patterns, markers, embedded images, the other filter primitives.
//! Text is not rasterised here (there is no font in the app): it comes
//! back as runs the caller draws with the canvas's own text.

use alloc::string::String;
use alloc::vec::Vec;
// Float methods (sqrt, sin, cos) in no_std, from the SDK's prelude.
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

pub type Pt = (f32, f32);

/// A 2D affine transform (a b c d e f), as SVG's `matrix()`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tf(pub [f32; 6]);

impl Tf {
    pub const ID: Tf = Tf([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    pub fn then(self, o: Tf) -> Tf {
        // self applied first, then o.
        let [a, b, c, d, e, f] = self.0;
        let [a2, b2, c2, d2, e2, f2] = o.0;
        Tf([
            a2 * a + c2 * b,
            b2 * a + d2 * b,
            a2 * c + c2 * d,
            b2 * c + d2 * d,
            a2 * e + c2 * f + e2,
            b2 * e + d2 * f + f2,
        ])
    }

    pub fn apply(&self, p: Pt) -> Pt {
        let [a, b, c, d, e, f] = self.0;
        (a * p.0 + c * p.1 + e, b * p.0 + d * p.1 + f)
    }

    pub fn invert(&self) -> Tf {
        let [a, b, c, d, e, f] = self.0;
        let det = a * d - b * c;
        if det.abs() < 1e-12 {
            return Tf::ID;
        }
        let (ia, ib, ic, id) = (d / det, -b / det, -c / det, a / det);
        Tf([ia, ib, ic, id, -(ia * e + ic * f), -(ib * e + id * f)])
    }

    fn scale(&self) -> f32 {
        let [a, b, c, d, ..] = self.0;
        ((a * a + b * b).sqrt() + (c * c + d * d).sqrt()) * 0.5
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Paint {
    None,
    Color([f32; 4]),
    /// A gradient: stops, linear (x1 y1 x2 y2) or radial (cx cy r), in the
    /// shape's own user space after its transform.
    Linear(Vec<(f32, [f32; 4])>, Pt, Pt),
    Radial(Vec<(f32, [f32; 4])>, Pt, f32),
    /// A `<pattern>`: its contents drawn once into a tile (x, y, w, h in
    /// pattern space) repeated; `tf` maps pattern space to the document.
    Pattern {
        items: Vec<Item>,
        tile: (f32, f32, f32, f32),
        tf: Tf,
        op: f32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub text: String,
    pub color: [f32; 4],
    /// 0 start, 1 middle, 2 end.
    pub anchor: u8,
    pub bold: bool,
    /// SVG text is set in a serif face unless its font-family names a
    /// sans-serif or monospace one.
    pub serif: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    /// Closed or open polylines, in document coordinates.
    pub polys: Vec<(Vec<Pt>, bool)>,
    pub fill: Paint,
    pub stroke: Paint,
    pub width: f32,
    pub even_odd: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Svg {
    pub w: f32,
    pub h: f32,
    pub items: Vec<Item>,
    pub texts: Vec<Text>,
}

impl Svg {
    /// Every shape, groups opened, in drawing order.
    pub fn shapes(&self) -> Vec<&Shape> {
        fn go<'a>(items: &'a [Item], out: &mut Vec<&'a Shape>) {
            for it in items {
                match it {
                    Item::Shape(s) => out.push(s),
                    Item::Group(g) => go(&g.items, out),
                    Item::Image { .. } => {}
                }
            }
        }
        let mut out = Vec::new();
        go(&self.items, &mut out);
        out
    }
}

// ---- numbers and colours ------------------------------------------------------------------

fn num(s: &str) -> Option<f32> {
    let t = s.trim().trim_end_matches("px").trim_end_matches('%');
    t.parse().ok()
}

fn hexval(s: &str) -> Option<u32> {
    u32::from_str_radix(s, 16).ok()
}

pub fn color(s: &str) -> Option<[f32; 4]> {
    let s = s.trim();
    if let Some(h) = s.strip_prefix('#') {
        let v = hexval(h)?;
        return Some(match h.len() {
            3 => [
                ((v >> 8) & 15) as f32 / 15.0,
                ((v >> 4) & 15) as f32 / 15.0,
                (v & 15) as f32 / 15.0,
                1.0,
            ],
            6 => [
                ((v >> 16) & 255) as f32 / 255.0,
                ((v >> 8) & 255) as f32 / 255.0,
                (v & 255) as f32 / 255.0,
                1.0,
            ],
            8 => [
                ((v >> 24) & 255) as f32 / 255.0,
                ((v >> 16) & 255) as f32 / 255.0,
                ((v >> 8) & 255) as f32 / 255.0,
                (v & 255) as f32 / 255.0,
            ],
            _ => return None,
        });
    }
    if let Some(inner) = s.strip_prefix("rgba(").or_else(|| s.strip_prefix("rgb(")) {
        let parts: Vec<f32> = inner
            .trim_end_matches(')')
            .split([',', ' '])
            .filter(|x| !x.is_empty())
            .filter_map(|x| {
                if let Some(p) = x.strip_suffix('%') {
                    p.parse::<f32>().ok().map(|v| v * 2.55)
                } else {
                    x.parse().ok()
                }
            })
            .collect();
        let g = |i: usize| parts.get(i).copied().unwrap_or(0.0);
        let a = if parts.len() > 3 { g(3) } else { 1.0 };
        return Some([g(0) / 255.0, g(1) / 255.0, g(2) / 255.0, a]);
    }
    let named: u32 = match s.to_ascii_lowercase().as_str() {
        "black" | "currentcolor" => 0x000000,
        "white" => 0xffffff,
        "red" => 0xff0000,
        "green" => 0x008000,
        "lime" => 0x00ff00,
        "blue" => 0x0000ff,
        "yellow" => 0xffff00,
        "orange" => 0xffa500,
        "purple" => 0x800080,
        "gray" | "grey" => 0x808080,
        "silver" => 0xc0c0c0,
        "navy" => 0x000080,
        "teal" => 0x008080,
        "maroon" => 0x800000,
        "olive" => 0x808000,
        "aqua" | "cyan" => 0x00ffff,
        "fuchsia" | "magenta" => 0xff00ff,
        "pink" => 0xffc0cb,
        "brown" => 0xa52a2a,
        "gold" => 0xffd700,
        "lightgray" | "lightgrey" => 0xd3d3d3,
        "darkgray" | "darkgrey" => 0xa9a9a9,
        "steelblue" => 0x4682b4,
        "tomato" => 0xff6347,
        "skyblue" => 0x87ceeb,
        _ => return None,
    };
    Some([
        ((named >> 16) & 255) as f32 / 255.0,
        ((named >> 8) & 255) as f32 / 255.0,
        (named & 255) as f32 / 255.0,
        1.0,
    ])
}

fn numbers(s: &str) -> Vec<f32> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b.get(i).copied().unwrap_or(0);
        if c.is_ascii_digit() || c == b'-' || c == b'+' || c == b'.' {
            let start = i;
            let mut seen_dot = c == b'.';
            let mut seen_e = false;
            i += 1;
            while i < b.len() {
                let d = b.get(i).copied().unwrap_or(0);
                if d.is_ascii_digit() {
                    i += 1;
                } else if d == b'.' && !seen_dot && !seen_e {
                    seen_dot = true;
                    i += 1;
                } else if (d == b'e' || d == b'E') && !seen_e {
                    seen_e = true;
                    i += 1;
                    if matches!(b.get(i), Some(b'-' | b'+')) {
                        i += 1;
                    }
                } else {
                    break;
                }
            }
            if let Some(v) = s.get(start..i).and_then(|t| t.parse().ok()) {
                out.push(v);
            }
        } else {
            i += 1;
        }
    }
    out
}

pub fn transform(s: &str) -> Tf {
    let mut t = Tf::ID;
    let mut rest = s;
    while let Some(open) = rest.find('(') {
        let name = rest
            .get(..open)
            .unwrap_or("")
            .trim()
            .trim_start_matches(',')
            .trim();
        let close = rest
            .get(open..)
            .and_then(|r| r.find(')'))
            .map_or(rest.len(), |c| open + c);
        let v = numbers(rest.get(open + 1..close).unwrap_or(""));
        let g = |i: usize, d: f32| v.get(i).copied().unwrap_or(d);
        let m = match name {
            "matrix" => Tf([
                g(0, 1.0),
                g(1, 0.0),
                g(2, 0.0),
                g(3, 1.0),
                g(4, 0.0),
                g(5, 0.0),
            ]),
            "translate" => Tf([1.0, 0.0, 0.0, 1.0, g(0, 0.0), g(1, 0.0)]),
            "scale" => {
                let sx = g(0, 1.0);
                Tf([sx, 0.0, 0.0, g(1, sx), 0.0, 0.0])
            }
            "rotate" => {
                let a = g(0, 0.0).to_radians();
                let (s, c) = (a.sin(), a.cos());
                let r = Tf([c, s, -s, c, 0.0, 0.0]);
                if v.len() >= 3 {
                    let (cx, cy) = (g(1, 0.0), g(2, 0.0));
                    Tf([1.0, 0.0, 0.0, 1.0, -cx, -cy])
                        .then(r)
                        .then(Tf([1.0, 0.0, 0.0, 1.0, cx, cy]))
                } else {
                    r
                }
            }
            "skewX" => Tf([1.0, 0.0, g(0, 0.0).to_radians().tan(), 1.0, 0.0, 0.0]),
            "skewY" => Tf([1.0, g(0, 0.0).to_radians().tan(), 0.0, 1.0, 0.0, 0.0]),
            _ => Tf::ID,
        };
        // Listed transforms apply right to left to a point.
        t = m.then(t);
        rest = rest.get(close + 1..).unwrap_or("");
    }
    t
}

// ---- paths --------------------------------------------------------------------------------

fn flatten_cubic(out: &mut Vec<Pt>, p0: Pt, p1: Pt, p2: Pt, p3: Pt) {
    let len = ((p1.0 - p0.0).abs() + (p1.1 - p0.1).abs())
        + ((p2.0 - p1.0).abs() + (p2.1 - p1.1).abs())
        + ((p3.0 - p2.0).abs() + (p3.1 - p2.1).abs());
    let n = ((len / 2.0) as usize).clamp(4, 64);
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let u = 1.0 - t;
        let a = u * u * u;
        let b = 3.0 * u * u * t;
        let c = 3.0 * u * t * t;
        let d = t * t * t;
        out.push((
            a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0,
            a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1,
        ));
    }
}

#[allow(clippy::too_many_arguments)]
fn flatten_arc(
    out: &mut Vec<Pt>,
    p0: Pt,
    rx: f32,
    ry: f32,
    phi_deg: f32,
    large: bool,
    sweep: bool,
    p1: Pt,
) {
    if (p0.0 - p1.0).abs() < 1e-6 && (p0.1 - p1.1).abs() < 1e-6 {
        return;
    }
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx < 1e-6 || ry < 1e-6 {
        out.push(p1);
        return;
    }
    let phi = phi_deg.to_radians();
    let (sp, cp) = (phi.sin(), phi.cos());
    let dx = (p0.0 - p1.0) * 0.5;
    let dy = (p0.1 - p1.1) * 0.5;
    let x1 = cp * dx + sp * dy;
    let y1 = -sp * dx + cp * dy;
    let lam = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lam > 1.0 {
        let s = lam.sqrt();
        rx *= s;
        ry *= s;
    }
    let num = (rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1).max(0.0);
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut co = if den > 0.0 { (num / den).sqrt() } else { 0.0 };
    if large == sweep {
        co = -co;
    }
    let cx1 = co * rx * y1 / ry;
    let cy1 = -co * ry * x1 / rx;
    let cx = cp * cx1 - sp * cy1 + (p0.0 + p1.0) * 0.5;
    let cy = sp * cx1 + cp * cy1 + (p0.1 + p1.1) * 0.5;
    let ang = |ux: f32, uy: f32, vx: f32, vy: f32| -> f32 {
        (ux * vy - uy * vx).atan2(ux * vx + uy * vy)
    };
    let t1 = ang(1.0, 0.0, (x1 - cx1) / rx, (y1 - cy1) / ry);
    let mut dt = ang(
        (x1 - cx1) / rx,
        (y1 - cy1) / ry,
        (-x1 - cx1) / rx,
        (-y1 - cy1) / ry,
    );
    let tau = core::f32::consts::PI * 2.0;
    if !sweep && dt > 0.0 {
        dt -= tau;
    } else if sweep && dt < 0.0 {
        dt += tau;
    }
    let n = ((dt.abs() * rx.max(ry) / 2.0) as usize).clamp(4, 96);
    for i in 1..=n {
        let t = t1 + dt * i as f32 / n as f32;
        let (st, ct) = (t.sin(), t.cos());
        out.push((
            cp * rx * ct - sp * ry * st + cx,
            sp * rx * ct + cp * ry * st + cy,
        ));
    }
}

/// A path's `d` as polylines, each with whether it was closed.
pub fn path(d: &str) -> Vec<(Vec<Pt>, bool)> {
    let mut out: Vec<(Vec<Pt>, bool)> = Vec::new();
    let mut cur: Vec<Pt> = Vec::new();
    let mut p: Pt = (0.0, 0.0);
    let mut start: Pt = (0.0, 0.0);
    let mut last_ctrl: Option<Pt> = None;
    let mut last_q: Option<Pt> = None;
    let b = d.as_bytes();
    let mut i = 0;
    let mut cmd = b'M';
    let flush = |cur: &mut Vec<Pt>, out: &mut Vec<(Vec<Pt>, bool)>, closed: bool| {
        if cur.len() > 1 {
            out.push((core::mem::take(cur), closed));
        } else {
            cur.clear();
        }
    };
    while i < b.len() {
        let c = b.get(i).copied().unwrap_or(0);
        if c.is_ascii_alphabetic() && c != b'e' && c != b'E' {
            cmd = c;
            i += 1;
            if cmd == b'Z' || cmd == b'z' {
                if !cur.is_empty() {
                    cur.push(start);
                }
                flush(&mut cur, &mut out, true);
                p = start;
                last_ctrl = None;
                last_q = None;
            }
            continue;
        }
        if !(c.is_ascii_digit() || c == b'-' || c == b'.' || c == b'+') {
            i += 1;
            continue;
        }
        // The arguments of one command, read with the arc flags allowed to
        // run together ("a1 1 0 011 1").
        let need = match cmd.to_ascii_uppercase() {
            b'M' | b'L' | b'T' => 2,
            b'H' | b'V' => 1,
            b'S' | b'Q' => 4,
            b'C' => 6,
            b'A' => 7,
            _ => 0,
        };
        if need == 0 {
            i += 1;
            continue;
        }
        let mut args: Vec<f32> = Vec::new();
        while args.len() < need && i < b.len() {
            let c2 = b.get(i).copied().unwrap_or(0);
            if c2 == b' ' || c2 == b',' || c2 == b'\n' || c2 == b'\t' || c2 == b'\r' {
                i += 1;
                continue;
            }
            if cmd.eq_ignore_ascii_case(&b'A')
                && (args.len() == 3 || args.len() == 4)
                && (c2 == b'0' || c2 == b'1')
            {
                args.push(f32::from(c2 - b'0'));
                i += 1;
                continue;
            }
            let start_i = i;
            let mut j = i;
            if matches!(b.get(j), Some(b'-' | b'+')) {
                j += 1;
            }
            let mut dot = false;
            let mut exp = false;
            while let Some(&d2) = b.get(j) {
                if d2.is_ascii_digit() {
                    j += 1;
                } else if d2 == b'.' && !dot && !exp {
                    dot = true;
                    j += 1;
                } else if (d2 == b'e' || d2 == b'E') && !exp {
                    exp = true;
                    j += 1;
                    if matches!(b.get(j), Some(b'-' | b'+')) {
                        j += 1;
                    }
                } else {
                    break;
                }
            }
            if j == start_i {
                break;
            }
            match d.get(start_i..j).and_then(|t| t.parse::<f32>().ok()) {
                Some(v) => args.push(v),
                None => break,
            }
            i = j;
        }
        if args.len() < need {
            break;
        }
        let rel = cmd.is_ascii_lowercase();
        let a = |k: usize| args.get(k).copied().unwrap_or(0.0);
        let at = |x: f32, y: f32| if rel { (p.0 + x, p.1 + y) } else { (x, y) };
        match cmd.to_ascii_uppercase() {
            b'M' => {
                flush(&mut cur, &mut out, false);
                p = at(a(0), a(1));
                start = p;
                cur.push(p);
                // Further pairs after a move are lines.
                cmd = if rel { b'l' } else { b'L' };
                last_ctrl = None;
                last_q = None;
            }
            b'L' => {
                if cur.is_empty() {
                    cur.push(p);
                }
                p = at(a(0), a(1));
                cur.push(p);
                last_ctrl = None;
                last_q = None;
            }
            b'H' => {
                if cur.is_empty() {
                    cur.push(p);
                }
                p = (if rel { p.0 + a(0) } else { a(0) }, p.1);
                cur.push(p);
                last_ctrl = None;
                last_q = None;
            }
            b'V' => {
                if cur.is_empty() {
                    cur.push(p);
                }
                p = (p.0, if rel { p.1 + a(0) } else { a(0) });
                cur.push(p);
                last_ctrl = None;
                last_q = None;
            }
            b'C' | b'S' => {
                if cur.is_empty() {
                    cur.push(p);
                }
                let (c1, c2, e) = if cmd.eq_ignore_ascii_case(&b'C') {
                    (at(a(0), a(1)), at(a(2), a(3)), at(a(4), a(5)))
                } else {
                    let c1 = last_ctrl.map_or(p, |lc| (2.0 * p.0 - lc.0, 2.0 * p.1 - lc.1));
                    (c1, at(a(0), a(1)), at(a(2), a(3)))
                };
                flatten_cubic(&mut cur, p, c1, c2, e);
                last_ctrl = Some(c2);
                last_q = None;
                p = e;
            }
            b'Q' | b'T' => {
                if cur.is_empty() {
                    cur.push(p);
                }
                let (q, e) = if cmd.eq_ignore_ascii_case(&b'Q') {
                    (at(a(0), a(1)), at(a(2), a(3)))
                } else {
                    let q = last_q.map_or(p, |lq| (2.0 * p.0 - lq.0, 2.0 * p.1 - lq.1));
                    (q, at(a(0), a(1)))
                };
                let c1 = (p.0 + 2.0 / 3.0 * (q.0 - p.0), p.1 + 2.0 / 3.0 * (q.1 - p.1));
                let c2 = (e.0 + 2.0 / 3.0 * (q.0 - e.0), e.1 + 2.0 / 3.0 * (q.1 - e.1));
                flatten_cubic(&mut cur, p, c1, c2, e);
                last_q = Some(q);
                last_ctrl = None;
                p = e;
            }
            b'A' => {
                if cur.is_empty() {
                    cur.push(p);
                }
                let e = at(a(5), a(6));
                flatten_arc(&mut cur, p, a(0), a(1), a(2), a(3) != 0.0, a(4) != 0.0, e);
                p = e;
                last_ctrl = None;
                last_q = None;
            }
            _ => {}
        }
    }
    flush(&mut cur, &mut out, false);
    out
}

fn ellipse(cx: f32, cy: f32, rx: f32, ry: f32) -> Vec<Pt> {
    let n = ((rx.max(ry) * 1.5) as usize).clamp(12, 128);
    (0..=n)
        .map(|i| {
            let t = core::f32::consts::PI * 2.0 * i as f32 / n as f32;
            (cx + rx * t.cos(), cy + ry * t.sin())
        })
        .collect()
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, rx: f32, ry: f32) -> Vec<Pt> {
    let rx = rx.min(w * 0.5).max(0.0);
    let ry = ry.min(h * 0.5).max(0.0);
    if rx <= 0.0 || ry <= 0.0 {
        return alloc::vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h), (x, y)];
    }
    let mut p = Vec::new();
    let corner = |p: &mut Vec<Pt>, cx: f32, cy: f32, a0: f32| {
        for i in 0..=6 {
            let t = (a0 + 90.0 * i as f32 / 6.0).to_radians();
            p.push((cx + rx * t.cos(), cy + ry * t.sin()));
        }
    };
    corner(&mut p, x + w - rx, y + ry, -90.0);
    corner(&mut p, x + w - rx, y + h - ry, 0.0);
    corner(&mut p, x + rx, y + h - ry, 90.0);
    corner(&mut p, x + rx, y + ry, 180.0);
    if let Some(f) = p.first().copied() {
        p.push(f);
    }
    p
}

// ---- reading the document ---------------------------------------------------------------

#[derive(Debug, Clone)]
struct El<'a> {
    name: &'a str,
    attrs: Vec<(&'a str, String)>,
    close: bool,
    selfclose: bool,
    text: &'a str,
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// The tags of an XML document, in order, each with the text after it.
fn tags(src: &str) -> Vec<El<'_>> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(o) = src.get(i..).and_then(|r| r.find('<')) {
        let start = i + o;
        let rest = src.get(start..).unwrap_or("");
        if rest.starts_with("<!--") {
            i = start + rest.find("-->").map_or(rest.len(), |e| e + 3);
            continue;
        }
        if rest.starts_with("<?") || rest.starts_with("<!") {
            i = start + rest.find('>').map_or(rest.len(), |e| e + 1);
            continue;
        }
        let Some(end) = rest.find('>') else {
            break;
        };
        let inner = rest.get(1..end).unwrap_or("");
        let close = inner.starts_with('/');
        let selfclose = inner.ends_with('/');
        let body = inner.trim_start_matches('/').trim_end_matches('/');
        let name_end = body.find(|c: char| c.is_whitespace()).unwrap_or(body.len());
        let name = body.get(..name_end).unwrap_or("");
        let name = name.rsplit(':').next().unwrap_or(name);
        let mut attrs = Vec::new();
        let ab = body.get(name_end..).unwrap_or("");
        let mut k = 0;
        while let Some(eq) = ab.get(k..).and_then(|r| r.find('=')) {
            let key = ab.get(k..k + eq).unwrap_or("").trim();
            let after = ab.get(k + eq + 1..).unwrap_or("").trim_start();
            let skipped = ab.get(k + eq + 1..).unwrap_or("").len() - after.len();
            let q = after.chars().next().unwrap_or('"');
            if q != '"' && q != '\'' {
                break;
            }
            let vend = after
                .get(1..)
                .and_then(|r| r.find(q))
                .map_or(after.len(), |e| e + 1);
            let val = after.get(1..vend).unwrap_or("");
            let key = key.rsplit(':').next().unwrap_or(key);
            attrs.push((key, unescape(val)));
            k = k + eq + 1 + skipped + vend + 1;
        }
        let after = start + end + 1;
        let next = src
            .get(after..)
            .and_then(|r| r.find('<'))
            .map_or(src.len(), |n| after + n);
        out.push(El {
            name,
            attrs,
            close,
            selfclose,
            text: src.get(after..next).unwrap_or(""),
        });
        i = after;
    }
    out
}

#[derive(Debug, Clone)]
struct Style {
    tf: Tf,
    fill: String,
    stroke: String,
    width: f32,
    /// What the element's ancestors' opacity left for it, when they were
    /// not drawn as a group of their own.
    opacity: f32,
    fill_op: f32,
    stroke_op: f32,
    even_odd: bool,
    clip_even_odd: bool,
    font: f32,
    anchor: u8,
    bold: bool,
    serif: bool,
    marker_start: Option<String>,
    marker_mid: Option<String>,
    marker_end: Option<String>,
    // Not inherited: read afresh for each element.
    own_op: f32,
    clip: Option<String>,
    mask: Option<String>,
    filter: Option<String>,
    hidden: bool,
}

/// A node of the document: an element with its children, by index.
struct Node<'a> {
    name: &'a str,
    attrs: Vec<(&'a str, String)>,
    kids: Vec<usize>,
    text: &'a str,
}

/// The document as a tree; node 0 holds the top-level elements.
fn tree<'a>(els: &[El<'a>]) -> Vec<Node<'a>> {
    let mut nodes = alloc::vec![Node {
        name: "",
        attrs: Vec::new(),
        kids: Vec::new(),
        text: "",
    }];
    let mut open: Vec<usize> = alloc::vec![0];
    for e in els {
        if e.close {
            if let Some(at) = open.iter().rposition(|i| nodes[*i].name == e.name) {
                if at > 0 {
                    open.truncate(at);
                }
            }
            continue;
        }
        let i = nodes.len();
        nodes.push(Node {
            name: e.name,
            attrs: e.attrs.clone(),
            kids: Vec::new(),
            text: e.text,
        });
        let parent = open.last().copied().unwrap_or(0);
        nodes[parent].kids.push(i);
        if !e.selfclose {
            open.push(i);
        }
    }
    nodes
}

fn attr<'a>(e: &'a El<'_>, k: &str) -> Option<&'a str> {
    e.attrs
        .iter()
        .find(|(a, _)| *a == k)
        .map(|(_, v)| v.as_str())
}

fn nattr<'a>(n: &'a Node<'_>, k: &str) -> Option<&'a str> {
    n.attrs
        .iter()
        .find(|(a, _)| *a == k)
        .map(|(_, v)| v.as_str())
}

// ---- CSS ----------------------------------------------------------------------------------

/// One compound selector: `tag.class#id`, any part optional.
#[derive(Debug, Clone, PartialEq)]
struct Compound {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
}

#[derive(Debug, Clone)]
struct Rule {
    /// Compounds left to right, each an ancestor of the next.
    sel: Vec<Compound>,
    spec: u32,
    order: usize,
    decls: Vec<(String, String)>,
}

fn compound(s: &str) -> Option<Compound> {
    let mut c = Compound {
        tag: None,
        id: None,
        classes: Vec::new(),
    };
    let mut cur = String::new();
    let mut kind = 't';
    let flush = |c: &mut Compound, kind: char, cur: &mut String| {
        if !cur.is_empty() {
            match kind {
                '.' => c.classes.push(core::mem::take(cur)),
                '#' => c.id = Some(core::mem::take(cur)),
                _ if cur.as_str() == "*" => cur.clear(),
                _ => c.tag = Some(core::mem::take(cur)),
            }
        }
    };
    for ch in s.chars() {
        match ch {
            '.' | '#' => {
                flush(&mut c, kind, &mut cur);
                kind = ch;
            }
            ':' | '[' => return None,
            _ => cur.push(ch),
        }
    }
    flush(&mut c, kind, &mut cur);
    Some(c)
}

/// The rules of `<style>` sheets: type, class and id selectors, compound
/// and descendant; anything else (pseudo-classes, attributes, `>` and `+`)
/// is skipped rather than guessed at.
fn css(sheet: &str, first: usize) -> Vec<Rule> {
    let mut out = Vec::new();
    // Comments out.
    let mut text = String::new();
    let mut rest = sheet;
    while let Some(i) = rest.find("/*") {
        text.push_str(rest.get(..i).unwrap_or(""));
        rest = rest
            .get(i..)
            .and_then(|r| r.find("*/").map(|e| r.get(e + 2..).unwrap_or("")))
            .unwrap_or("");
    }
    text.push_str(rest);
    let mut order = first;
    let mut rest = text.as_str();
    while let Some(open) = rest.find('{') {
        let head = rest.get(..open).unwrap_or("").trim();
        let close = rest
            .get(open..)
            .and_then(|r| r.find('}'))
            .map_or(rest.len(), |c| open + c);
        let body = rest.get(open + 1..close).unwrap_or("");
        rest = rest.get(close + 1..).unwrap_or("");
        if head.starts_with('@') {
            continue;
        }
        let decls: Vec<(String, String)> = body
            .split(';')
            .filter_map(|d| d.split_once(':'))
            .map(|(k, v)| {
                let v = v.trim().trim_end_matches("!important").trim();
                (String::from(k.trim()), String::from(v))
            })
            .collect();
        for sel in head.split(',') {
            if sel.contains(['>', '+', '~']) {
                continue;
            }
            let parts: Option<Vec<Compound>> = sel.split_whitespace().map(compound).collect();
            let Some(parts) = parts.filter(|p| !p.is_empty()) else {
                continue;
            };
            let spec = parts
                .iter()
                .map(|c| {
                    u32::from(c.id.is_some()) * 100
                        + c.classes.len() as u32 * 10
                        + u32::from(c.tag.is_some())
                })
                .sum();
            out.push(Rule {
                sel: parts,
                spec,
                order,
                decls: decls.clone(),
            });
            order += 1;
        }
    }
    out
}

fn matches_compound(c: &Compound, n: &Node<'_>) -> bool {
    if c.tag.as_deref().is_some_and(|t| t != n.name) {
        return false;
    }
    if let Some(id) = &c.id {
        if nattr(n, "id") != Some(id.as_str()) {
            return false;
        }
    }
    let classes = nattr(n, "class").unwrap_or("");
    c.classes
        .iter()
        .all(|k| classes.split_whitespace().any(|x| x == k))
}

fn matches(rule: &Rule, nodes: &[Node<'_>], at: usize, anc: &[usize]) -> bool {
    let Some((last, before)) = rule.sel.split_last() else {
        return false;
    };
    if !matches_compound(last, &nodes[at]) {
        return false;
    }
    // Each earlier compound on some ancestor, outermost first.
    let mut a = anc.len();
    for c in before.iter().rev() {
        loop {
            if a == 0 {
                return false;
            }
            a -= 1;
            if matches_compound(c, &nodes[anc[a]]) {
                break;
            }
        }
    }
    true
}

/// The text of every `<style>` sheet, CDATA unwrapped.
fn sheets(src: &str) -> String {
    let mut out = String::new();
    let mut rest = src;
    while let Some(i) = rest.find("<style") {
        let after = rest.get(i..).unwrap_or("");
        let Some(gt) = after.find('>') else {
            break;
        };
        let body = after.get(gt + 1..).unwrap_or("");
        let end = body.find("</style").unwrap_or(body.len());
        let sheet = body.get(..end).unwrap_or("");
        out.push_str(&sheet.replace("<![CDATA[", "").replace("]]>", ""));
        out.push('\n');
        rest = body.get(end..).unwrap_or("");
    }
    out
}

// ---- styles -------------------------------------------------------------------------------

fn set_prop(s: &mut Style, k: &str, v: &str) {
    match k {
        "fill" => s.fill = String::from(v.trim()),
        "stroke" => s.stroke = String::from(v.trim()),
        "stroke-width" => s.width = num(v).unwrap_or(s.width),
        "opacity" => s.own_op = num(v).unwrap_or(1.0).clamp(0.0, 1.0),
        "fill-opacity" => s.fill_op = num(v).unwrap_or(1.0),
        "stroke-opacity" => s.stroke_op = num(v).unwrap_or(1.0),
        "fill-rule" => s.even_odd = v.trim() == "evenodd",
        "clip-rule" => s.clip_even_odd = v.trim() == "evenodd",
        "font-size" => s.font = num(v).unwrap_or(s.font),
        // The CSS shorthand: its size is the token in px, its family the rest.
        "font" => {
            for tok in v.split_whitespace() {
                if tok.ends_with("px") {
                    s.font = num(tok).unwrap_or(s.font);
                }
            }
            if let Some(fam) = v.split_whitespace().last() {
                set_prop(s, "font-family", fam);
            }
        }
        "text-anchor" => {
            s.anchor = match v.trim() {
                "middle" => 1,
                "end" => 2,
                _ => 0,
            }
        }
        "font-weight" => s.bold = matches!(v.trim(), "bold" | "600" | "700" | "800" | "900"),
        "font-family" => {
            let f = v.to_ascii_lowercase();
            s.serif = ![
                "sans",
                "helvetica",
                "arial",
                "system-ui",
                "mono",
                "inter",
                "verdana",
                "segoe",
            ]
            .iter()
            .any(|w| f.contains(w));
        }
        "clip-path" => s.clip = Some(String::from(v.trim())),
        "mask" => s.mask = Some(String::from(v.trim())),
        "filter" => s.filter = Some(String::from(v.trim())),
        "marker-start" => s.marker_start = Some(String::from(v.trim())),
        "marker-mid" => s.marker_mid = Some(String::from(v.trim())),
        "marker-end" => s.marker_end = Some(String::from(v.trim())),
        "marker" => {
            let m = Some(String::from(v.trim()));
            s.marker_start = m.clone();
            s.marker_mid = m.clone();
            s.marker_end = m;
        }
        "display" => s.hidden |= v.trim() == "none",
        "visibility" => s.hidden |= v.trim() == "hidden",
        _ => {}
    }
}

/// An element's style: its parent's, then its attributes, then the style
/// sheets' rules in specificity order, then its own `style=""`.
fn style_of(cx: &Cx<'_, '_>, at: usize, anc: &[usize], parent: &Style) -> Style {
    let n = &cx.nodes[at];
    let mut s = parent.clone();
    s.own_op = 1.0;
    s.clip = None;
    s.mask = None;
    s.filter = None;
    s.hidden = false;
    for (k, v) in &n.attrs {
        set_prop(&mut s, k, v);
    }
    let mut hits: Vec<&Rule> = cx
        .rules
        .iter()
        .filter(|r| matches(r, cx.nodes, at, anc))
        .collect();
    hits.sort_by_key(|r| (r.spec, r.order));
    for r in hits {
        for (k, v) in &r.decls {
            set_prop(&mut s, k, v);
        }
    }
    if let Some(st) = nattr(n, "style") {
        for decl in st.split(';') {
            if let Some((k, v)) = decl.split_once(':') {
                set_prop(&mut s, k.trim(), v);
            }
        }
    }
    if let Some(t) = nattr(n, "transform") {
        s.tf = transform(t).then(parent.tf);
    }
    s
}

struct Grad {
    id: String,
    stops: Vec<(f32, [f32; 4])>,
    radial: bool,
    a: Pt,
    b: Pt,
    r: f32,
    /// Coordinates in the shape's bounding box (0..1), the default.
    bbox: bool,
}

fn paint(spec: &str, op: f32, grads: &[Grad], bbox: (f32, f32, f32, f32), tf: Tf) -> Paint {
    let spec = spec.trim();
    if spec.is_empty() || spec == "none" || spec == "transparent" {
        return Paint::None;
    }
    if let Some(id) = spec.strip_prefix("url(#").and_then(|r| r.split(')').next()) {
        let Some(g) = grads.iter().find(|g| g.id == id) else {
            return Paint::Color([0.0, 0.0, 0.0, op]);
        };
        let stops: Vec<(f32, [f32; 4])> = g
            .stops
            .iter()
            .map(|(o, c)| (*o, [c[0], c[1], c[2], c[3] * op]))
            .collect();
        let map = |p: Pt| -> Pt {
            let q = if g.bbox {
                (
                    bbox.0 + p.0 * (bbox.2 - bbox.0),
                    bbox.1 + p.1 * (bbox.3 - bbox.1),
                )
            } else {
                p
            };
            tf.apply(q)
        };
        return if g.radial {
            let r = if g.bbox {
                g.r * (bbox.2 - bbox.0).max(bbox.3 - bbox.1)
            } else {
                g.r
            } * tf.scale();
            Paint::Radial(stops, map(g.a), r)
        } else {
            Paint::Linear(stops, map(g.a), map(g.b))
        };
    }
    match color(spec) {
        Some(c) => Paint::Color([c[0], c[1], c[2], c[3] * op]),
        None => Paint::Color([0.0, 0.0, 0.0, op]),
    }
}

fn gradients(els: &[El<'_>]) -> Vec<Grad> {
    let mut grads: Vec<Grad> = Vec::new();
    let mut k = 0;
    while k < els.len() {
        let e = &els[k];
        if !e.close && (e.name == "linearGradient" || e.name == "radialGradient") {
            let radial = e.name == "radialGradient";
            let user = attr(e, "gradientUnits") == Some("userSpaceOnUse");
            let f = |n: &str, d: f32| -> f32 {
                attr(e, n).map_or(d, |v| {
                    let p = v.trim().ends_with('%');
                    num(v).map_or(d, |x| if p { x / 100.0 } else { x })
                })
            };
            let mut g = Grad {
                id: String::from(attr(e, "id").unwrap_or("")),
                stops: Vec::new(),
                radial,
                a: if radial {
                    (f("cx", 0.5), f("cy", 0.5))
                } else {
                    (f("x1", 0.0), f("y1", 0.0))
                },
                b: (f("x2", 1.0), f("y2", 0.0)),
                r: f("r", 0.5),
                bbox: !user,
            };
            if let Some(href) = attr(e, "href").and_then(|h| h.strip_prefix('#')) {
                if let Some(src) = grads.iter().find(|x| x.id == href) {
                    g.stops = src.stops.clone();
                }
            }
            k += 1;
            while k < els.len() && !(els[k].close && els[k].name == e.name) && !e.selfclose {
                let s = &els[k];
                if s.name == "stop" && !s.close {
                    let off = attr(s, "offset").map_or(0.0, |v| {
                        let p = v.trim().ends_with('%');
                        num(v).map_or(0.0, |x| if p { x / 100.0 } else { x })
                    });
                    let mut col = String::from(attr(s, "stop-color").unwrap_or("black"));
                    let mut op = attr(s, "stop-opacity").and_then(num).unwrap_or(1.0);
                    if let Some(st) = attr(s, "style") {
                        for decl in st.split(';') {
                            if let Some((kk, v)) = decl.split_once(':') {
                                match kk.trim() {
                                    "stop-color" => col = String::from(v.trim()),
                                    "stop-opacity" => op = num(v).unwrap_or(1.0),
                                    _ => {}
                                }
                            }
                        }
                    }
                    let c = color(&col).unwrap_or([0.0, 0.0, 0.0, 1.0]);
                    g.stops
                        .push((off.clamp(0.0, 1.0), [c[0], c[1], c[2], c[3] * op]));
                }
                k += 1;
            }
            grads.push(g);
        }
        k += 1;
    }
    grads
}

// ---- groups: clip paths, masks, filters ---------------------------------------------------

/// What is drawn: shapes, and groups drawn on their own and then put
/// down through their clip path, mask, filter and opacity.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Shape(Shape),
    Group(Group),
    /// A picture (`<image>`): straight RGBA pixels and the transform from
    /// its pixel grid to the document.
    Image {
        w: u32,
        h: u32,
        rgba: Vec<u8>,
        tf: Tf,
        op: f32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub items: Vec<Item>,
    pub opacity: f32,
    /// Shapes that clip it, in document coordinates.
    pub clip: Option<(Vec<Vec<Pt>>, bool)>,
    /// Drawn and used by its luminance.
    pub mask: Option<Vec<Item>>,
    pub filter: Option<Vec<Prim>>,
    /// Whether the filter works in linear light (`linearRGB`, the
    /// default) rather than sRGB.
    pub linear: bool,
    /// The filter region (x0, y0, x1, y1), in document coordinates:
    /// nothing a filter draws reaches past it.
    pub region: Option<(f32, f32, f32, f32)>,
}

/// A filter's input or output: the group's own drawing, its alpha alone,
/// the step before, or a named result.
#[derive(Debug, Clone, PartialEq)]
pub enum Src {
    Graphic,
    Alpha,
    Last,
    Named(String),
}

/// One filter primitive, lengths already in document units.
#[derive(Debug, Clone, PartialEq)]
pub enum Prim {
    Blur {
        input: Src,
        sx: f32,
        sy: f32,
        out: String,
    },
    Offset {
        input: Src,
        dx: f32,
        dy: f32,
        out: String,
    },
    /// A 4 by 5 matrix over un-premultiplied RGBA.
    Matrix {
        input: Src,
        m: [f32; 20],
        out: String,
    },
    Flood {
        color: [f32; 4],
        out: String,
    },
    /// Porter-Duff: over, in, out, atop, xor; or arithmetic k1..k4.
    Composite {
        a: Src,
        b: Src,
        op: String,
        k: [f32; 4],
        out: String,
    },
    Merge {
        inputs: Vec<Src>,
        out: String,
    },
    Shadow {
        input: Src,
        dx: f32,
        dy: f32,
        sx: f32,
        sy: f32,
        color: [f32; 4],
        out: String,
    },
}

fn src_of(v: Option<&str>) -> Src {
    match v.map(str::trim) {
        None | Some("") => Src::Last,
        Some("SourceGraphic") => Src::Graphic,
        Some("SourceAlpha") => Src::Alpha,
        Some(n) => Src::Named(String::from(n)),
    }
}

fn hue_matrix(deg: f32) -> [f32; 20] {
    let (s, c) = (deg.to_radians().sin(), deg.to_radians().cos());
    [
        0.213 + c * 0.787 - s * 0.213,
        0.715 - c * 0.715 - s * 0.715,
        0.072 - c * 0.072 + s * 0.928,
        0.0,
        0.0,
        0.213 - c * 0.213 + s * 0.143,
        0.715 + c * 0.285 + s * 0.140,
        0.072 - c * 0.072 - s * 0.283,
        0.0,
        0.0,
        0.213 - c * 0.213 - s * 0.787,
        0.715 - c * 0.715 + s * 0.715,
        0.072 + c * 0.928 + s * 0.072,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
        0.0,
    ]
}

fn color_matrix(n: &Node<'_>) -> [f32; 20] {
    let vals = numbers(nattr(n, "values").unwrap_or(""));
    let id = [
        1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0,
        1.0, 0.0,
    ];
    match nattr(n, "type").unwrap_or("matrix") {
        "saturate" => {
            let s = vals.first().copied().unwrap_or(1.0);
            [
                0.213 + 0.787 * s,
                0.715 - 0.715 * s,
                0.072 - 0.072 * s,
                0.0,
                0.0,
                0.213 - 0.213 * s,
                0.715 + 0.285 * s,
                0.072 - 0.072 * s,
                0.0,
                0.0,
                0.213 - 0.213 * s,
                0.715 - 0.715 * s,
                0.072 + 0.928 * s,
                0.0,
                0.0,
                0.0,
                0.0,
                0.0,
                1.0,
                0.0,
            ]
        }
        "hueRotate" => hue_matrix(vals.first().copied().unwrap_or(0.0)),
        "luminanceToAlpha" => [
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.2125,
            0.7154, 0.0721, 0.0, 0.0,
        ],
        _ if vals.len() == 20 => {
            let mut m = [0.0; 20];
            m.copy_from_slice(&vals);
            m
        }
        _ => id,
    }
}

/// A `<filter>`'s primitives, lengths scaled by the user space's `k`.
fn filter_prims(cx: &Cx<'_, '_>, at: usize, tf: Tf) -> Vec<Prim> {
    let k = tf.scale();
    let mut out = Vec::new();
    for &i in &cx.nodes[at].kids {
        let n = &cx.nodes[i];
        let f = |a: &str, d: f32| nattr(n, a).and_then(num).unwrap_or(d);
        let res = String::from(nattr(n, "result").unwrap_or(""));
        let input = src_of(nattr(n, "in"));
        let flood = || {
            let c =
                color(nattr(n, "flood-color").unwrap_or("black")).unwrap_or([0.0, 0.0, 0.0, 1.0]);
            [c[0], c[1], c[2], c[3] * f("flood-opacity", 1.0)]
        };
        let sd = || {
            let v = numbers(nattr(n, "stdDeviation").unwrap_or("0"));
            let a = v.first().copied().unwrap_or(0.0);
            (a * k, v.get(1).copied().unwrap_or(a) * k)
        };
        let shift = |dx: f32, dy: f32| {
            let [a, b, c, d, ..] = tf.0;
            (a * dx + c * dy, b * dx + d * dy)
        };
        match n.name {
            "feGaussianBlur" => {
                let (sx, sy) = sd();
                out.push(Prim::Blur {
                    input,
                    sx,
                    sy,
                    out: res,
                });
            }
            "feOffset" => {
                let (dx, dy) = shift(f("dx", 0.0), f("dy", 0.0));
                out.push(Prim::Offset {
                    input,
                    dx,
                    dy,
                    out: res,
                });
            }
            "feColorMatrix" => out.push(Prim::Matrix {
                input,
                m: color_matrix(n),
                out: res,
            }),
            "feFlood" => out.push(Prim::Flood {
                color: flood(),
                out: res,
            }),
            "feComposite" => out.push(Prim::Composite {
                a: input,
                b: src_of(nattr(n, "in2")),
                op: String::from(nattr(n, "operator").unwrap_or("over")),
                k: [f("k1", 0.0), f("k2", 0.0), f("k3", 0.0), f("k4", 0.0)],
                out: res,
            }),
            "feBlend" => out.push(Prim::Composite {
                a: input,
                b: src_of(nattr(n, "in2")),
                op: String::from("over"),
                k: [0.0; 4],
                out: res,
            }),
            "feMerge" => out.push(Prim::Merge {
                inputs: cx.nodes[i]
                    .kids
                    .iter()
                    .filter(|j| cx.nodes[**j].name == "feMergeNode")
                    .map(|j| src_of(nattr(&cx.nodes[*j], "in")))
                    .collect(),
                out: res,
            }),
            "feDropShadow" => {
                let (dx, dy) = shift(f("dx", 2.0), f("dy", 2.0));
                let v = numbers(nattr(n, "stdDeviation").unwrap_or("2"));
                let a = v.first().copied().unwrap_or(2.0);
                out.push(Prim::Shadow {
                    input,
                    dx,
                    dy,
                    sx: a * k,
                    sy: v.get(1).copied().unwrap_or(a) * k,
                    color: flood(),
                    out: res,
                });
            }
            _ => {}
        }
    }
    out
}

// ---- reading ------------------------------------------------------------------------------

/// Reads a file an `<image>` names.
type Fetch<'a> = dyn FnMut(&str) -> Option<Vec<u8>> + 'a;

/// What a walk of the document needs.
struct Cx<'n, 'a> {
    nodes: &'n [Node<'a>],
    rules: Vec<Rule>,
    grads: Vec<Grad>,
    /// Reads a file an `<image>` names, beside the SVG.
    fetch: core::cell::RefCell<&'n mut Fetch<'n>>,
    /// The root's style: what markers and patterns inherit.
    root: Option<Style>,
}

impl Cx<'_, '_> {
    fn by_id(&self, id: &str) -> Option<usize> {
        self.nodes.iter().position(|n| nattr(n, "id") == Some(id))
    }

    fn by_url(&self, spec: &str) -> Option<usize> {
        let id = spec
            .trim()
            .strip_prefix("url(")?
            .trim_start_matches(['"', '\'']);
        let id = id.strip_prefix('#')?;
        let id = id.split([')', '"', '\'']).next()?;
        self.by_id(id)
    }
}

/// Elements that are never drawn where they stand.
fn is_def(name: &str) -> bool {
    matches!(
        name,
        "defs"
            | "clipPath"
            | "mask"
            | "pattern"
            | "symbol"
            | "marker"
            | "linearGradient"
            | "radialGradient"
            | "style"
            | "filter"
            | "title"
            | "desc"
            | "metadata"
            | "script"
    )
}

fn bbox_of(items: &[Item], bb: &mut (f32, f32, f32, f32)) {
    for it in items {
        match it {
            Item::Shape(s) => {
                for (p, _) in &s.polys {
                    for q in p {
                        *bb = (bb.0.min(q.0), bb.1.min(q.1), bb.2.max(q.0), bb.3.max(q.1));
                    }
                }
            }
            Item::Group(g) => bbox_of(&g.items, bb),
            Item::Image { w, h, tf, .. } => {
                for q in [
                    (0.0, 0.0),
                    (*w as f32, 0.0),
                    (0.0, *h as f32),
                    (*w as f32, *h as f32),
                ] {
                    let q = tf.apply(q);
                    *bb = (bb.0.min(q.0), bb.1.min(q.1), bb.2.max(q.0), bb.3.max(q.1));
                }
            }
        }
    }
}

fn flat_polys(items: &[Item], out: &mut Vec<Vec<Pt>>) {
    for it in items {
        match it {
            Item::Shape(s) => out.extend(s.polys.iter().map(|(p, _)| p.clone())),
            Item::Group(g) => flat_polys(&g.items, out),
            Item::Image { .. } => {}
        }
    }
}

const MAX_DEPTH: usize = 24;

/// Draw node `at` and its children into `out`.
fn walk(
    cx: &Cx<'_, '_>,
    at: usize,
    anc: &mut Vec<usize>,
    parent: &Style,
    out: &mut Vec<Item>,
    texts: &mut Vec<Text>,
    depth: usize,
) {
    if depth > MAX_DEPTH {
        return;
    }
    let n = &cx.nodes[at];
    if is_def(n.name) {
        return;
    }
    let st = style_of(cx, at, anc, parent);
    if st.hidden {
        return;
    }
    let g = |k: &str| nattr(n, k).and_then(num).unwrap_or(0.0);
    let container = matches!(n.name, "g" | "svg" | "a" | "switch" | "use" | "symbol");
    // A group of its own when something applies to it as a whole.
    let grouped = st.clip.is_some()
        || st.mask.is_some()
        || st.filter.is_some()
        || (container && st.own_op < 1.0);
    let mut inner_style = st.clone();
    if grouped {
        // The group's opacity is applied once, to the group.
        if container {
            inner_style.own_op = 1.0;
        }
    } else if container {
        inner_style.opacity *= st.own_op;
    }
    let mut items: Vec<Item> = Vec::new();
    {
        let target = if grouped { &mut items } else { &mut *out };
        match n.name {
            "g" | "a" | "switch" => {
                anc.push(at);
                for &k in &n.kids {
                    walk(cx, k, anc, &inner_style, target, texts, depth + 1);
                }
                anc.pop();
            }
            "svg" | "symbol" => {
                // A nested viewport: its x and y, and its viewBox fitted to
                // its width and height.
                let mut s = inner_style.clone();
                let place = Tf([1.0, 0.0, 0.0, 1.0, g("x"), g("y")]);
                let vb = nattr(n, "viewBox").map(numbers).filter(|v| v.len() == 4);
                let fit = match (
                    &vb,
                    nattr(n, "width").and_then(num),
                    nattr(n, "height").and_then(num),
                ) {
                    (Some(v), Some(w), Some(h)) => {
                        let k = (w / v[2].max(1e-3)).min(h / v[3].max(1e-3));
                        Tf([k, 0.0, 0.0, k, -v[0] * k, -v[1] * k])
                    }
                    _ => Tf::ID,
                };
                s.tf = fit.then(place).then(inner_style.tf);
                anc.push(at);
                for &k in &n.kids {
                    walk(cx, k, anc, &s, target, texts, depth + 1);
                }
                anc.pop();
            }
            "use" => {
                let href = nattr(n, "href").and_then(|h| h.strip_prefix('#'));
                if let Some(t) = href.and_then(|h| cx.by_id(h)) {
                    let mut s = inner_style.clone();
                    s.tf = Tf([1.0, 0.0, 0.0, 1.0, g("x"), g("y")]).then(inner_style.tf);
                    let tn = &cx.nodes[t];
                    anc.push(at);
                    if tn.name == "symbol" {
                        // A symbol is drawn at the use's width and height.
                        let ts = style_of(cx, t, anc, &s);
                        let vb = nattr(tn, "viewBox").map(numbers).filter(|v| v.len() == 4);
                        let (w, h) = (
                            nattr(n, "width").and_then(num),
                            nattr(n, "height").and_then(num),
                        );
                        let fit = match (&vb, w, h) {
                            (Some(v), Some(w), Some(h)) => {
                                let k = (w / v[2].max(1e-3)).min(h / v[3].max(1e-3));
                                Tf([k, 0.0, 0.0, k, -v[0] * k, -v[1] * k])
                            }
                            _ => Tf::ID,
                        };
                        let mut ss = ts;
                        ss.tf = fit.then(s.tf);
                        anc.push(t);
                        for &k in &tn.kids {
                            walk(cx, k, anc, &ss, target, texts, depth + 1);
                        }
                        anc.pop();
                    } else if !anc.contains(&t) {
                        walk_used(cx, t, anc, &s, target, texts, depth + 1);
                    }
                    anc.pop();
                }
            }
            "text" | "tspan" => {
                shape(cx, n, &st, target, texts);
                anc.push(at);
                for &k in &n.kids {
                    walk(cx, k, anc, &st, target, texts, depth + 1);
                }
                anc.pop();
            }
            _ => shape(cx, n, &st, target, texts),
        }
    }
    if !grouped {
        return;
    }
    let mut bb = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    bbox_of(&items, &mut bb);
    let unit = Tf([bb.2 - bb.0, 0.0, 0.0, bb.3 - bb.1, bb.0, bb.1]);
    let clip = st.clip.as_deref().and_then(|c| cx.by_url(c)).map(|ci| {
        let cn = &cx.nodes[ci];
        let mut cs = st.clone();
        cs.own_op = 1.0;
        cs.clip = None;
        cs.tf = if nattr(cn, "clipPathUnits") == Some("objectBoundingBox") {
            unit
        } else {
            st.tf
        };
        if let Some(t) = nattr(cn, "transform") {
            cs.tf = transform(t).then(cs.tf);
        }
        let mut shapes = Vec::new();
        let mut tx = Vec::new();
        anc.push(ci);
        for &k in &cn.kids {
            walk(cx, k, anc, &cs, &mut shapes, &mut tx, depth + 1);
        }
        anc.pop();
        let even_odd = cn
            .kids
            .first()
            .map(|k| style_of(cx, *k, anc, &cs).clip_even_odd)
            .unwrap_or(false);
        let mut polys = Vec::new();
        flat_polys(&shapes, &mut polys);
        (polys, even_odd)
    });
    let mask = st.mask.as_deref().and_then(|m| cx.by_url(m)).map(|mi| {
        let mn = &cx.nodes[mi];
        let mut ms = st.clone();
        ms.own_op = 1.0;
        ms.opacity = 1.0;
        ms.mask = None;
        if nattr(mn, "maskContentUnits") == Some("objectBoundingBox") {
            ms.tf = unit;
        }
        let mut m = Vec::new();
        let mut tx = Vec::new();
        anc.push(mi);
        for &k in &mn.kids {
            walk(cx, k, anc, &ms, &mut m, &mut tx, depth + 1);
        }
        anc.pop();
        m
    });
    let fnode = st.filter.as_deref().and_then(|f| cx.by_url(f));
    let filter = fnode.map(|fi| filter_prims(cx, fi, st.tf));
    let linear = fnode.is_none_or(|fi| {
        let n = &cx.nodes[fi];
        let said = nattr(n, "color-interpolation-filters").or_else(|| {
            nattr(n, "style").and_then(|st| {
                st.split(';')
                    .filter_map(|d| d.split_once(':'))
                    .find(|(k, _)| k.trim() == "color-interpolation-filters")
                    .map(|(_, v)| v.trim())
            })
        });
        said.map(str::trim) != Some("sRGB")
    });
    // The filter region: by default the shape's box and a tenth more on
    // every side, as Chromium clips Obsidian's.
    let region = fnode.map(|fi| {
        let fnn = &cx.nodes[fi];
        let frac = |k: &str, d: f32| {
            nattr(fnn, k).map_or(d, |v| {
                let pc = v.trim().ends_with('%');
                num(v).map_or(d, |x| if pc { x / 100.0 } else { x })
            })
        };
        let (bw, bh) = (bb.2 - bb.0, bb.3 - bb.1);
        if nattr(fnn, "filterUnits") == Some("userSpaceOnUse") {
            let a = st.tf.apply((frac("x", 0.0), frac("y", 0.0)));
            let b = st.tf.apply((
                frac("x", 0.0) + frac("width", 0.0),
                frac("y", 0.0) + frac("height", 0.0),
            ));
            (a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1))
        } else {
            let (x, y) = (bb.0 + frac("x", -0.1) * bw, bb.1 + frac("y", -0.1) * bh);
            (
                x,
                y,
                x + frac("width", 1.2) * bw,
                y + frac("height", 1.2) * bh,
            )
        }
    });
    out.push(Item::Group(Group {
        items,
        opacity: if container { st.own_op } else { 1.0 },
        clip,
        mask,
        filter,
        linear,
        region,
    }));
}

/// The element a `<use>` points at, drawn in the use's place.
fn walk_used(
    cx: &Cx<'_, '_>,
    t: usize,
    anc: &mut Vec<usize>,
    s: &Style,
    out: &mut Vec<Item>,
    texts: &mut Vec<Text>,
    depth: usize,
) {
    let tn = &cx.nodes[t];
    if is_def(tn.name) && tn.name != "symbol" {
        // A use of something inside <defs> is fine; of <defs> itself, not.
        return;
    }
    walk(cx, t, anc, s, out, texts, depth);
}

/// A basic shape or text, in `st`'s user space.
fn shape(cx: &Cx<'_, '_>, n: &Node<'_>, st: &Style, out: &mut Vec<Item>, texts: &mut Vec<Text>) {
    let g = |k: &str| nattr(n, k).and_then(num).unwrap_or(0.0);
    let polys: Vec<(Vec<Pt>, bool)> = match n.name {
        "path" => path(nattr(n, "d").unwrap_or("")),
        "rect" => {
            let (rx, ry) = match (nattr(n, "rx").and_then(num), nattr(n, "ry").and_then(num)) {
                (Some(a), Some(b)) => (a, b),
                (Some(a), None) | (None, Some(a)) => (a, a),
                _ => (0.0, 0.0),
            };
            alloc::vec![(
                rounded_rect(g("x"), g("y"), g("width"), g("height"), rx, ry),
                true
            )]
        }
        "circle" => alloc::vec![(ellipse(g("cx"), g("cy"), g("r"), g("r")), true)],
        "ellipse" => alloc::vec![(ellipse(g("cx"), g("cy"), g("rx"), g("ry")), true)],
        "line" => alloc::vec![(alloc::vec![(g("x1"), g("y1")), (g("x2"), g("y2"))], false)],
        "polyline" | "polygon" => {
            let v = numbers(nattr(n, "points").unwrap_or(""));
            let mut pts: Vec<Pt> = v.chunks_exact(2).map(|c| (c[0], c[1])).collect();
            let closed = n.name == "polygon";
            if closed {
                if let Some(f) = pts.first().copied() {
                    pts.push(f);
                }
            }
            alloc::vec![(pts, closed)]
        }
        "image" => {
            image(cx, n, st, out);
            return;
        }
        "text" | "tspan" => {
            let text = unescape(n.text.trim());
            if !text.is_empty() {
                let p = st.tf.apply((g("x"), g("y")));
                let c = color(&st.fill).unwrap_or([0.0, 0.0, 0.0, 1.0]);
                texts.push(Text {
                    x: p.0,
                    y: p.1,
                    size: st.font * st.tf.scale(),
                    text,
                    color: [c[0], c[1], c[2], c[3] * st.opacity * st.own_op * st.fill_op],
                    anchor: st.anchor,
                    bold: st.bold,
                    serif: st.serif,
                });
            }
            return;
        }
        _ => return,
    };
    // The bounding box in the shape's own space, for gradients.
    let mut bb = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (p, _) in &polys {
        for q in p {
            bb = (bb.0.min(q.0), bb.1.min(q.1), bb.2.max(q.0), bb.3.max(q.1));
        }
    }
    let polys: Vec<(Vec<Pt>, bool)> = polys
        .into_iter()
        .map(|(p, c)| (p.into_iter().map(|q| st.tf.apply(q)).collect(), c))
        .collect();
    let op = st.opacity * st.own_op;
    let fill = if n.name == "line" {
        Paint::None
    } else {
        pattern(cx, &st.fill, op * st.fill_op, bb, st.tf)
            .unwrap_or_else(|| paint(&st.fill, op * st.fill_op, &cx.grads, bb, st.tf))
    };
    let stroke = pattern(cx, &st.stroke, op * st.stroke_op, bb, st.tf)
        .unwrap_or_else(|| paint(&st.stroke, op * st.stroke_op, &cx.grads, bb, st.tf));
    out.push(Item::Shape(Shape {
        polys: polys.clone(),
        fill,
        stroke,
        width: st.width * st.tf.scale(),
        even_odd: st.even_odd,
    }));
    if matches!(n.name, "path" | "line" | "polyline" | "polygon") {
        markers(cx, n, st, out);
    }
}

/// A `fill` or `stroke` that names a `<pattern>`: its tile drawn and
/// repeated, in the pattern's units.
fn pattern(
    cx: &Cx<'_, '_>,
    spec: &str,
    op: f32,
    bb: (f32, f32, f32, f32),
    tf: Tf,
) -> Option<Paint> {
    let pi = cx.by_url(spec)?;
    let mut pn = &cx.nodes[pi];
    if pn.name != "pattern" {
        return None;
    }
    // A pattern may take its tile and contents from another by href.
    let mut content = pn;
    for _ in 0..4 {
        if !content.kids.is_empty() {
            break;
        }
        match nattr(content, "href")
            .and_then(|h| h.strip_prefix('#'))
            .and_then(|h| cx.by_id(h))
        {
            Some(o) => content = &cx.nodes[o],
            None => break,
        }
    }
    let attr_of = |k: &str| nattr(pn, k).or_else(|| nattr(content, k));
    let units_bb = attr_of("patternUnits") != Some("userSpaceOnUse");
    let frac = |k: &str| -> f32 {
        attr_of(k).map_or(0.0, |v| {
            let pc = v.trim().ends_with('%');
            num(v).map_or(0.0, |x| if pc { x / 100.0 } else { x })
        })
    };
    let (bw, bh) = (bb.2 - bb.0, bb.3 - bb.1);
    let (x, y, w, h) = if units_bb {
        (
            bb.0 + frac("x") * bw,
            bb.1 + frac("y") * bh,
            frac("width") * bw,
            frac("height") * bh,
        )
    } else {
        (frac("x"), frac("y"), frac("width"), frac("height"))
    };
    if w <= 0.0 || h <= 0.0 {
        return Some(Paint::None);
    }
    // Contents: in the tile's own space (origin at its corner), scaled by
    // the viewBox or by the box when patternContentUnits says so.
    let vb = attr_of("viewBox").map(numbers).filter(|v| v.len() == 4);
    let inner = match vb {
        Some(v) => {
            let k = (w / v[2].max(1e-3)).min(h / v[3].max(1e-3));
            Tf([k, 0.0, 0.0, k, -v[0] * k, -v[1] * k])
        }
        None if attr_of("patternContentUnits") == Some("objectBoundingBox") => {
            Tf([bw, 0.0, 0.0, bh, 0.0, 0.0])
        }
        None => Tf::ID,
    };
    let mut cs = cx.root.clone()?;
    cs.tf = inner;
    let mut items = Vec::new();
    let mut tx = Vec::new();
    let mut anc = alloc::vec![pi];
    for &k in &content.kids {
        walk(cx, k, &mut anc, &cs, &mut items, &mut tx, 4);
    }
    let ptf = attr_of("patternTransform").map(transform).unwrap_or(Tf::ID);
    pn = &cx.nodes[pi];
    let _ = pn;
    Some(Paint::Pattern {
        items,
        tile: (x, y, w, h),
        tf: ptf.then(tf),
        op,
    })
}

/// The markers at a line's start, its corners and its end.
fn markers(cx: &Cx<'_, '_>, n: &Node<'_>, st: &Style, out: &mut Vec<Item>) {
    if st.marker_start.is_none() && st.marker_mid.is_none() && st.marker_end.is_none() {
        return;
    }
    let g = |k: &str| nattr(n, k).and_then(num).unwrap_or(0.0);
    // The vertices as written (a path's flattened curves are not corners:
    // its mid markers go on its straight segments' ends only).
    let pts: Vec<Pt> = match n.name {
        "line" => alloc::vec![(g("x1"), g("y1")), (g("x2"), g("y2"))],
        "polyline" | "polygon" => {
            let v = numbers(nattr(n, "points").unwrap_or(""));
            let mut p: Vec<Pt> = v.chunks_exact(2).map(|c| (c[0], c[1])).collect();
            if n.name == "polygon" {
                if let Some(f) = p.first().copied() {
                    p.push(f);
                }
            }
            p
        }
        _ => path(nattr(n, "d").unwrap_or(""))
            .into_iter()
            .flat_map(|(p, _)| p)
            .collect(),
    };
    if pts.len() < 2 {
        return;
    }
    let ang = |a: Pt, b: Pt| (b.1 - a.1).atan2(b.0 - a.0);
    let last = pts.len() - 1;
    let mut place: Vec<(&Option<String>, Pt, f32, bool)> = Vec::new();
    place.push((&st.marker_start, pts[0], ang(pts[0], pts[1]), true));
    if n.name != "path" {
        for i in 1..last {
            let a = (ang(pts[i - 1], pts[i]) + ang(pts[i], pts[i + 1])) * 0.5;
            place.push((&st.marker_mid, pts[i], a, false));
        }
    }
    place.push((
        &st.marker_end,
        pts[last],
        ang(pts[last - 1], pts[last]),
        false,
    ));
    for (spec, at, angle, start) in place {
        let Some(mi) = spec.as_deref().and_then(|m| cx.by_url(m)) else {
            continue;
        };
        let mn = &cx.nodes[mi];
        if mn.name != "marker" {
            continue;
        }
        let f = |k: &str, d: f32| nattr(mn, k).and_then(num).unwrap_or(d);
        let (mw, mh) = (f("markerWidth", 3.0), f("markerHeight", 3.0));
        let rot = match nattr(mn, "orient").map(str::trim) {
            Some("auto") => angle,
            Some("auto-start-reverse") => {
                if start {
                    angle + core::f32::consts::PI
                } else {
                    angle
                }
            }
            Some(v) => num(v).unwrap_or(0.0).to_radians(),
            None => 0.0,
        };
        let units = if nattr(mn, "markerUnits") == Some("userSpaceOnUse") {
            1.0
        } else {
            st.width
        };
        let vb = nattr(mn, "viewBox").map(numbers).filter(|v| v.len() == 4);
        let (k, ox, oy) = match vb {
            Some(v) => ((mw / v[2].max(1e-3)).min(mh / v[3].max(1e-3)), v[0], v[1]),
            None => (1.0, 0.0, 0.0),
        };
        let (rx, ry) = (f("refX", 0.0), f("refY", 0.0));
        let (sn, cs) = (rot.sin(), rot.cos());
        let place_tf = Tf([1.0, 0.0, 0.0, 1.0, -rx - 0.0, -ry])
            .then(Tf([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]))
            .then(Tf([k, 0.0, 0.0, k, 0.0, 0.0]))
            .then(Tf([units, 0.0, 0.0, units, 0.0, 0.0]))
            .then(Tf([cs, sn, -sn, cs, 0.0, 0.0]))
            .then(Tf([1.0, 0.0, 0.0, 1.0, at.0, at.1]))
            .then(st.tf);
        let _ = (ox, oy);
        let Some(mut ms) = cx.root.clone() else {
            continue;
        };
        ms.tf = place_tf;
        let mut tx = Vec::new();
        let mut anc = alloc::vec![mi];
        for &c in &mn.kids {
            walk(cx, c, &mut anc, &ms, out, &mut tx, 6);
        }
    }
}

/// Standard base64, as a `data:` address carries it.
fn base64(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => continue,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

/// An `<image>`: a `data:` address or a file beside the SVG, decoded and
/// fitted to its box as `preserveAspectRatio` says (default: centred,
/// whole).
fn image(cx: &Cx<'_, '_>, n: &Node<'_>, st: &Style, out: &mut Vec<Item>) {
    let Some(href) = nattr(n, "href") else {
        return;
    };
    let bytes = if let Some(rest) = href.trim().strip_prefix("data:") {
        let (meta, data) = rest.split_once(',').unwrap_or(("", rest));
        if meta.ends_with(";base64") {
            base64(data)
        } else {
            unescape(data).into_bytes()
        }
    } else {
        match (cx.fetch.borrow_mut())(href.trim()) {
            Some(b) => b,
            None => return,
        }
    };
    let Ok(pic) = crate::image::decode(&bytes) else {
        return;
    };
    let g = |k: &str| nattr(n, k).and_then(num);
    let (iw, ih) = (pic.width as f32, pic.height as f32);
    let (lw, lh) = (pic.logical.0 as f32, pic.logical.1 as f32);
    let (x, y) = (g("x").unwrap_or(0.0), g("y").unwrap_or(0.0));
    let w = g("width").unwrap_or(lw);
    let h = g("height").unwrap_or(lh);
    let par = nattr(n, "preserveAspectRatio").unwrap_or("xMidYMid meet");
    let (kx, ky) = if par.trim() == "none" {
        (w / iw, h / ih)
    } else {
        let k = if par.contains("slice") {
            (w / iw).max(h / ih)
        } else {
            (w / iw).min(h / ih)
        };
        (k, k)
    };
    let (dw, dh) = (iw * kx, ih * ky);
    let ax = if par.contains("xMin") {
        0.0
    } else if par.contains("xMax") {
        1.0
    } else {
        0.5
    };
    let ay = if par.contains("YMin") {
        0.0
    } else if par.contains("YMax") {
        1.0
    } else {
        0.5
    };
    let tf = Tf([kx, 0.0, 0.0, ky, x + (w - dw) * ax, y + (h - dh) * ay]).then(st.tf);
    out.push(Item::Image {
        w: pic.width,
        h: pic.height,
        rgba: pic.rgba,
        tf,
        op: st.opacity * st.own_op,
    });
}

/// Read an SVG document.
pub fn parse(src: &str) -> Result<Svg, String> {
    parse_with(src, &mut |_| None)
}

/// Read an SVG document whose `<image>`s may name files: `fetch` reads one
/// by the name written, relative to the SVG.
pub fn parse_with(
    src: &str,
    fetch: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
) -> Result<Svg, String> {
    let els = tags(src);
    let nodes = tree(&els);
    let root = nodes
        .iter()
        .position(|n| n.name == "svg")
        .ok_or_else(|| String::from("This is not an SVG picture"))?;
    let rn = &nodes[root];
    let vb = nattr(rn, "viewBox").map(numbers).filter(|v| v.len() == 4);
    let w = nattr(rn, "width")
        .and_then(num)
        .or(vb.as_ref().and_then(|v| v.get(2).copied()))
        .unwrap_or(300.0);
    let h = nattr(rn, "height")
        .and_then(num)
        .or(vb.as_ref().and_then(|v| v.get(3).copied()))
        .unwrap_or(150.0);
    let base = match &vb {
        Some(v) => {
            let (vx, vy, vw, vh) = (v[0], v[1], v[2].max(1e-3), v[3].max(1e-3));
            let s = (w / vw).min(h / vh);
            let ox = (w - vw * s) * 0.5;
            let oy = (h - vh * s) * 0.5;
            Tf([s, 0.0, 0.0, s, ox - vx * s, oy - vy * s])
        }
        None => Tf::ID,
    };
    let mut cx = Cx {
        nodes: &nodes,
        rules: css(&sheets(src), 0),
        grads: gradients(&els),
        fetch: core::cell::RefCell::new(fetch),
        root: None,
    };
    let init = Style {
        tf: base,
        fill: String::from("black"),
        stroke: String::from("none"),
        width: 1.0,
        opacity: 1.0,
        fill_op: 1.0,
        stroke_op: 1.0,
        even_odd: false,
        clip_even_odd: false,
        font: 16.0,
        anchor: 0,
        bold: false,
        serif: true,
        marker_start: None,
        marker_mid: None,
        marker_end: None,
        own_op: 1.0,
        clip: None,
        mask: None,
        filter: None,
        hidden: false,
    };
    let mut svg = Svg {
        w,
        h,
        items: Vec::new(),
        texts: Vec::new(),
    };
    // The root's own presentation attributes and styles apply to all; its
    // viewBox is the base transform.
    let mut anc = Vec::new();
    let mut rs = style_of(&cx, root, &anc, &init);
    rs.tf = base;
    let mut rstyle = rs.clone();
    rstyle.own_op = 1.0;
    rstyle.clip = None;
    rstyle.mask = None;
    rstyle.filter = None;
    cx.root = Some(rstyle);
    let grouped = rs.clip.is_some() || rs.mask.is_some() || rs.filter.is_some() || rs.own_op < 1.0;
    anc.push(root);
    if grouped {
        // Rare: the root drawn as one group.
        let mut items = Vec::new();
        let mut inner = rs.clone();
        inner.own_op = 1.0;
        inner.clip = None;
        inner.mask = None;
        inner.filter = None;
        for &k in &rn.kids {
            walk(&cx, k, &mut anc, &inner, &mut items, &mut svg.texts, 1);
        }
        svg.items.push(Item::Group(Group {
            items,
            opacity: rs.own_op,
            clip: None,
            mask: None,
            filter: rs
                .filter
                .as_deref()
                .and_then(|f| cx.by_url(f))
                .map(|fi| filter_prims(&cx, fi, base)),
            linear: true,
            region: None,
        }));
    } else {
        for &k in &rn.kids {
            walk(&cx, k, &mut anc, &rs, &mut svg.items, &mut svg.texts, 1);
        }
    }
    Ok(svg)
}

// ---- rasterising --------------------------------------------------------------------------

fn paint_at(p: &Paint, x: f32, y: f32) -> [f32; 4] {
    let lerp = |stops: &[(f32, [f32; 4])], t: f32| -> [f32; 4] {
        let t = t.clamp(0.0, 1.0);
        let Some(first) = stops.first() else {
            return [0.0; 4];
        };
        if t <= first.0 {
            return first.1;
        }
        for w in stops.windows(2) {
            let (a, b) = (w[0], w[1]);
            if t <= b.0 {
                let f = if b.0 > a.0 {
                    (t - a.0) / (b.0 - a.0)
                } else {
                    0.0
                };
                let mut c = [0.0; 4];
                for (i, ci) in c.iter_mut().enumerate() {
                    *ci = a.1[i] + (b.1[i] - a.1[i]) * f;
                }
                return c;
            }
        }
        stops.last().map_or([0.0; 4], |s| s.1)
    };
    match p {
        // Drawn from its tile by the shape's own sampler.
        Paint::None | Paint::Pattern { .. } => [0.0; 4],
        Paint::Color(c) => *c,
        Paint::Linear(stops, a, b) => {
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let l2 = (dx * dx + dy * dy).max(1e-6);
            lerp(stops, ((x - a.0) * dx + (y - a.1) * dy) / l2)
        }
        Paint::Radial(stops, c, r) => {
            let d = ((x - c.0) * (x - c.0) + (y - c.1) * (y - c.1)).sqrt();
            lerp(stops, d / r.max(1e-3))
        }
    }
}

/// Anti-aliased coverage of closed polygons, 4 sub-rows a pixel.
fn coverage(polys: &[Vec<Pt>], w: usize, h: usize, even_odd: bool) -> Vec<f32> {
    let mut cov = alloc::vec![0.0f32; w * h];
    let mut edges: Vec<(f32, f32, f32, f32, i32)> = Vec::new();
    for poly in polys {
        for seg in poly.windows(2) {
            let (a, b) = (seg[0], seg[1]);
            if (a.1 - b.1).abs() < 1e-6 {
                continue;
            }
            let dir = if b.1 > a.1 { 1 } else { -1 };
            let (top, bot) = if a.1 < b.1 { (a, b) } else { (b, a) };
            edges.push((top.0, top.1, bot.0, bot.1, dir));
        }
        // An open sub-path fills as if closed.
        if let (Some(f), Some(l)) = (poly.first(), poly.last()) {
            // (A closing edge that is level adds nothing.)
            if (f.1 - l.1).abs() > 1e-6 {
                let dir = if f.1 > l.1 { 1 } else { -1 };
                let (top, bot) = if l.1 < f.1 { (*l, *f) } else { (*f, *l) };
                edges.push((top.0, top.1, bot.0, bot.1, dir));
            }
        }
    }
    const SUB: usize = 4;
    let mut xs: Vec<(f32, i32)> = Vec::new();
    for py in 0..h {
        for s in 0..SUB {
            let y = py as f32 + (s as f32 + 0.5) / SUB as f32;
            xs.clear();
            for &(x0, y0, x1, y1, dir) in &edges {
                if y >= y0 && y < y1 {
                    let t = (y - y0) / (y1 - y0);
                    xs.push((x0 + (x1 - x0) * t, dir));
                }
            }
            if xs.is_empty() {
                continue;
            }
            xs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(core::cmp::Ordering::Equal));
            let mut wind = 0i32;
            for k in 0..xs.len().saturating_sub(1) {
                wind += xs[k].1;
                let inside = if even_odd {
                    (k + 1) % 2 == 1
                } else {
                    wind != 0
                };
                if !inside {
                    continue;
                }
                let (xa, xb) = (xs[k].0.max(0.0), xs[k + 1].0.min(w as f32));
                if xb <= xa {
                    continue;
                }
                let row = py * w;
                let ia = xa as usize;
                let ib = (xb as usize).min(w.saturating_sub(1));
                let add = 1.0 / SUB as f32;
                if ia == ib {
                    if let Some(c) = cov.get_mut(row + ia) {
                        *c += (xb - xa) * add;
                    }
                    continue;
                }
                if let Some(c) = cov.get_mut(row + ia) {
                    *c += (ia as f32 + 1.0 - xa) * add;
                }
                for x in ia + 1..ib {
                    if let Some(c) = cov.get_mut(row + x) {
                        *c += add;
                    }
                }
                if let Some(c) = cov.get_mut(row + ib) {
                    *c += (xb - ib as f32) * add;
                }
            }
        }
    }
    cov
}

/// The outline of a stroke: a quad per segment and a round end at every
/// point (round joins and caps, as Lucide draws).
fn stroke_polys(polys: &[(Vec<Pt>, bool)], width: f32) -> Vec<Vec<Pt>> {
    let r = (width * 0.5).max(0.35);
    let mut out = Vec::new();
    for (p, _) in polys {
        for seg in p.windows(2) {
            let (a, b) = (seg[0], seg[1]);
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1e-4 {
                continue;
            }
            let (nx, ny) = (-dy / len * r, dx / len * r);
            out.push(alloc::vec![
                (a.0 + nx, a.1 + ny),
                (b.0 + nx, b.1 + ny),
                (b.0 - nx, b.1 - ny),
                (a.0 - nx, a.1 - ny),
                (a.0 + nx, a.1 + ny),
            ]);
        }
        for q in p {
            out.push(ellipse(q.0, q.1, r, r));
        }
    }
    // One winding for every piece, so overlaps add up instead of cancelling.
    for poly in out.iter_mut() {
        let area: f32 = poly
            .windows(2)
            .map(|w| w[0].0 * w[1].1 - w[1].0 * w[0].1)
            .sum();
        if area < 0.0 {
            poly.reverse();
        }
    }
    out
}

/// Draw one shape into premultiplied RGBA `px` (`wu` by `hu`), with the
/// document scaled by (sx, sy).
fn draw_shape(sh: &Shape, px: &mut [f32], wu: usize, hu: usize, sx: f32, sy: f32) {
    let scale = |p: &Pt| (p.0 * sx, p.1 * sy);
    let polys: Vec<(Vec<Pt>, bool)> = sh
        .polys
        .iter()
        .map(|(p, c)| (p.iter().map(scale).collect(), *c))
        .collect();
    let scale_paint = |p: &Paint| -> Paint {
        match p {
            Paint::Linear(s, a, b) => Paint::Linear(s.clone(), scale(a), scale(b)),
            Paint::Radial(s, c, r) => Paint::Radial(s.clone(), scale(c), r * (sx + sy) * 0.5),
            o => o.clone(),
        }
    };
    let mut layers: Vec<(Vec<f32>, Paint)> = Vec::new();
    if sh.fill != Paint::None {
        let fills: Vec<Vec<Pt>> = polys.iter().map(|(p, _)| p.clone()).collect();
        layers.push((coverage(&fills, wu, hu, sh.even_odd), scale_paint(&sh.fill)));
    }
    if sh.stroke != Paint::None && sh.width > 0.0 {
        let outline = stroke_polys(&polys, sh.width * (sx + sy) * 0.5);
        layers.push((coverage(&outline, wu, hu, false), scale_paint(&sh.stroke)));
    }
    for (cov, paint) in layers {
        // A pattern's tile, drawn once at this scale.
        let tile = match &paint {
            Paint::Pattern {
                items,
                tile,
                tf,
                op,
            } => {
                let k = tf.scale() * (sx + sy) * 0.5;
                let tw = ((tile.2 * k).ceil() as usize).clamp(1, 1024);
                let th = ((tile.3 * k).ceil() as usize).clamp(1, 1024);
                let mut buf = alloc::vec![0.0f32; tw * th * 4];
                render(
                    items,
                    &mut buf,
                    tw,
                    th,
                    tw as f32 / tile.2,
                    th as f32 / tile.3,
                );
                Some((buf, tw, th, *tile, tf.invert(), *op))
            }
            _ => None,
        };
        for y in 0..hu {
            for x in 0..wu {
                let i = y * wu + x;
                let a = cov.get(i).copied().unwrap_or(0.0).min(1.0);
                if a <= 0.0 {
                    continue;
                }
                let c = match &tile {
                    Some((buf, tw, th, t, inv, op)) => {
                        let p = inv.apply(((x as f32 + 0.5) / sx, (y as f32 + 0.5) / sy));
                        let u = ((p.0 - t.0).rem_euclid(t.2) / t.2 * *tw as f32) as usize;
                        let v = ((p.1 - t.1).rem_euclid(t.3) / t.3 * *th as f32) as usize;
                        let j = (v.min(th - 1) * tw + u.min(tw - 1)) * 4;
                        let pa = buf[j + 3];
                        if pa <= 0.0 {
                            continue;
                        }
                        [buf[j] / pa, buf[j + 1] / pa, buf[j + 2] / pa, pa * op]
                    }
                    None => paint_at(&paint, x as f32 + 0.5, y as f32 + 0.5),
                };
                let alpha = a * c[3];
                if let Some(d) = px.get_mut(i * 4..i * 4 + 4) {
                    // Premultiplied "over".
                    d[0] = c[0] * alpha + d[0] * (1.0 - alpha);
                    d[1] = c[1] * alpha + d[1] * (1.0 - alpha);
                    d[2] = c[2] * alpha + d[2] * (1.0 - alpha);
                    d[3] = alpha + d[3] * (1.0 - alpha);
                }
            }
        }
    }
}

/// A picture put down through its transform, sampled between pixels.
#[allow(clippy::too_many_arguments)]
fn draw_image(
    w: u32,
    h: u32,
    rgba: &[u8],
    tf: Tf,
    op: f32,
    px: &mut [f32],
    wu: usize,
    hu: usize,
    sx: f32,
    sy: f32,
) {
    let (iw, ih) = (w as usize, h as usize);
    if iw == 0 || ih == 0 || rgba.len() < iw * ih * 4 {
        return;
    }
    let to_px = tf.then(Tf([sx, 0.0, 0.0, sy, 0.0, 0.0]));
    let inv = to_px.invert();
    let corners = [
        (0.0, 0.0),
        (w as f32, 0.0),
        (0.0, h as f32),
        (w as f32, h as f32),
    ]
    .map(|q| to_px.apply(q));
    let x0 = corners
        .iter()
        .map(|c| c.0)
        .fold(f32::MAX, f32::min)
        .floor()
        .max(0.0) as usize;
    let x1 = (corners.iter().map(|c| c.0).fold(f32::MIN, f32::max).ceil() as usize).min(wu);
    let y0 = corners
        .iter()
        .map(|c| c.1)
        .fold(f32::MAX, f32::min)
        .floor()
        .max(0.0) as usize;
    let y1 = (corners.iter().map(|c| c.1).fold(f32::MIN, f32::max).ceil() as usize).min(hu);
    let at = |x: usize, y: usize, c: usize| f32::from(rgba[(y * iw + x) * 4 + c]) / 255.0;
    for y in y0..y1 {
        for x in x0..x1 {
            let p = inv.apply((x as f32 + 0.5, y as f32 + 0.5));
            if p.0 < 0.0 || p.1 < 0.0 || p.0 >= w as f32 || p.1 >= h as f32 {
                continue;
            }
            // Bilinear, on premultiplied colour.
            let (fx, fy) = ((p.0 - 0.5).max(0.0), (p.1 - 0.5).max(0.0));
            let (ax, ay) = ((fx as usize).min(iw - 1), (fy as usize).min(ih - 1));
            let (bx, by) = ((ax + 1).min(iw - 1), (ay + 1).min(ih - 1));
            let (tx, ty) = (fx - ax as f32, fy - ay as f32);
            let mut c = [0.0f32; 4];
            for (qx, qy, wgt) in [
                (ax, ay, (1.0 - tx) * (1.0 - ty)),
                (bx, ay, tx * (1.0 - ty)),
                (ax, by, (1.0 - tx) * ty),
                (bx, by, tx * ty),
            ] {
                let a = at(qx, qy, 3);
                c[0] += at(qx, qy, 0) * a * wgt;
                c[1] += at(qx, qy, 1) * a * wgt;
                c[2] += at(qx, qy, 2) * a * wgt;
                c[3] += a * wgt;
            }
            let i = (y * wu + x) * 4;
            let k = 1.0 - c[3] * op;
            px[i] = c[0] * op + px[i] * k;
            px[i + 1] = c[1] * op + px[i + 1] * k;
            px[i + 2] = c[2] * op + px[i + 2] * k;
            px[i + 3] = c[3] * op + px[i + 3] * k;
        }
    }
}

/// `top` over `under`, both premultiplied.
fn over(top: &[f32], under: &mut [f32]) {
    for (d, s) in under.chunks_exact_mut(4).zip(top.chunks_exact(4)) {
        let k = 1.0 - s[3];
        d[0] = s[0] + d[0] * k;
        d[1] = s[1] + d[1] * k;
        d[2] = s[2] + d[2] * k;
        d[3] = s[3] + d[3] * k;
    }
}

/// Three box blurs, close to a Gaussian of `sigma` pixels, along x or y.
fn blur_axis(px: &mut [f32], wu: usize, hu: usize, sigma: f32, along_x: bool) {
    if sigma < 0.3 {
        return;
    }
    // Box widths for three passes (W3C's filter-effects approximation).
    let d = (sigma * 3.0 * (2.0 * core::f32::consts::PI).sqrt() / 4.0 + 0.5).floor() as usize;
    let d = d.max(1);
    let (lines, len) = if along_x { (hu, wu) } else { (wu, hu) };
    let idx = |line: usize, i: usize| {
        if along_x {
            (line * wu + i) * 4
        } else {
            (i * wu + line) * 4
        }
    };
    let mut tmp = alloc::vec![0.0f32; len * 4];
    for pass in 0..3 {
        // Even widths shift by half a pixel, alternately, as the spec says.
        let (lo, hi) = if d % 2 == 1 {
            (d / 2, d / 2)
        } else if pass == 0 {
            (d / 2, d / 2 - 1)
        } else if pass == 1 {
            (d / 2 - 1, d / 2)
        } else {
            (d / 2, d / 2)
        };
        let width = (lo + hi + 1) as f32;
        for line in 0..lines {
            let mut acc = [0.0f32; 4];
            // Running sum over [i - lo, i + hi].
            for j in 0..=hi.min(len.saturating_sub(1)) {
                let k = idx(line, j);
                for c in 0..4 {
                    acc[c] += px[k + c];
                }
            }
            for i in 0..len {
                for c in 0..4 {
                    tmp[i * 4 + c] = acc[c] / width;
                }
                let add = i + hi + 1;
                if add < len {
                    let k = idx(line, add);
                    for c in 0..4 {
                        acc[c] += px[k + c];
                    }
                }
                if i >= lo {
                    let k = idx(line, i - lo);
                    for c in 0..4 {
                        acc[c] -= px[k + c];
                    }
                }
            }
            for i in 0..len {
                let k = idx(line, i);
                px[k..k + 4].copy_from_slice(&tmp[i * 4..i * 4 + 4]);
            }
        }
    }
}

fn blur(px: &[f32], wu: usize, hu: usize, sx: f32, sy: f32) -> Vec<f32> {
    let mut out = px.to_vec();
    blur_axis(&mut out, wu, hu, sx, true);
    blur_axis(&mut out, wu, hu, sy, false);
    out
}

fn offset(px: &[f32], wu: usize, hu: usize, dx: f32, dy: f32) -> Vec<f32> {
    let (ox, oy) = (dx.round() as isize, dy.round() as isize);
    let mut out = alloc::vec![0.0f32; px.len()];
    for y in 0..hu as isize {
        let fy = y - oy;
        if fy < 0 || fy >= hu as isize {
            continue;
        }
        for x in 0..wu as isize {
            let fx = x - ox;
            if fx < 0 || fx >= wu as isize {
                continue;
            }
            let d = ((y as usize) * wu + x as usize) * 4;
            let s = ((fy as usize) * wu + fx as usize) * 4;
            out[d..d + 4].copy_from_slice(&px[s..s + 4]);
        }
    }
    out
}

fn matrix(px: &[f32], m: &[f32; 20]) -> Vec<f32> {
    let mut out = alloc::vec![0.0f32; px.len()];
    for (d, s) in out.chunks_exact_mut(4).zip(px.chunks_exact(4)) {
        let a = s[3];
        let (r, g, b) = if a > 0.0 {
            (s[0] / a, s[1] / a, s[2] / a)
        } else {
            (0.0, 0.0, 0.0)
        };
        let row = |i: usize| {
            (m[i] * r + m[i + 1] * g + m[i + 2] * b + m[i + 3] * a + m[i + 4]).clamp(0.0, 1.0)
        };
        let na = row(15);
        d[0] = row(0) * na;
        d[1] = row(5) * na;
        d[2] = row(10) * na;
        d[3] = na;
    }
    out
}

fn composite(a: &[f32], b: &[f32], op: &str, k: [f32; 4]) -> Vec<f32> {
    let mut out = alloc::vec![0.0f32; a.len()];
    for ((d, s), u) in out
        .chunks_exact_mut(4)
        .zip(a.chunks_exact(4))
        .zip(b.chunks_exact(4))
    {
        let (aa, ab) = (s[3], u[3]);
        for c in 0..4 {
            d[c] = match op {
                "in" => s[c] * ab,
                "out" => s[c] * (1.0 - ab),
                "atop" => s[c] * ab + u[c] * (1.0 - aa),
                "xor" => s[c] * (1.0 - ab) + u[c] * (1.0 - aa),
                "arithmetic" => {
                    (k[0] * s[c] * u[c] + k[1] * s[c] + k[2] * u[c] + k[3]).clamp(0.0, 1.0)
                }
                _ => s[c] + u[c] * (1.0 - aa),
            };
        }
        if op == "arithmetic" {
            // Keep premultiplied colour within its alpha.
            for c in 0..3 {
                d[c] = d[c].min(d[3]);
            }
        }
    }
    out
}

/// sRGB to linear light and back, on premultiplied pixels.
fn to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn to_srgb(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

fn light(px: &mut [f32], into_linear: bool) {
    for p in px.chunks_exact_mut(4) {
        let a = p[3];
        if a <= 0.0 {
            continue;
        }
        for c in p.iter_mut().take(3) {
            let v = (*c / a).clamp(0.0, 1.0);
            *c = if into_linear {
                to_linear(v)
            } else {
                to_srgb(v)
            } * a;
        }
    }
}

fn colour_in(c: [f32; 4], linear: bool) -> [f32; 4] {
    if linear {
        [to_linear(c[0]), to_linear(c[1]), to_linear(c[2]), c[3]]
    } else {
        c
    }
}

/// Run a filter's primitives over a group's drawing (`linear`: the
/// drawing is in linear light, and so must the flood colours be).
fn filter(
    prims: &[Prim],
    graphic: Vec<f32>,
    wu: usize,
    hu: usize,
    sx: f32,
    sy: f32,
    linear: bool,
) -> Vec<f32> {
    let alpha: Vec<f32> = graphic
        .chunks_exact(4)
        .flat_map(|p| [0.0, 0.0, 0.0, p[3]])
        .collect();
    let mut named: Vec<(String, Vec<f32>)> = Vec::new();
    let mut last = graphic.clone();
    let get = |s: &Src, last: &Vec<f32>, named: &Vec<(String, Vec<f32>)>| -> Vec<f32> {
        match s {
            Src::Graphic => graphic.clone(),
            Src::Alpha => alpha.clone(),
            Src::Last => last.clone(),
            Src::Named(n) => named
                .iter()
                .rev()
                .find(|(k, _)| k == n)
                .map_or_else(|| last.clone(), |(_, v)| v.clone()),
        }
    };
    for p in prims {
        let (res, name) = match p {
            Prim::Blur {
                input,
                sx: bx,
                sy: by,
                out,
            } => (
                blur(&get(input, &last, &named), wu, hu, bx * sx, by * sy),
                out,
            ),
            Prim::Offset { input, dx, dy, out } => (
                offset(&get(input, &last, &named), wu, hu, dx * sx, dy * sy),
                out,
            ),
            Prim::Matrix { input, m, out } => (matrix(&get(input, &last, &named), m), out),
            Prim::Flood { color, out } => {
                let color = colour_in(*color, linear);
                let a = color[3];
                let px = [color[0] * a, color[1] * a, color[2] * a, a];
                (px.iter().copied().cycle().take(wu * hu * 4).collect(), out)
            }
            Prim::Composite { a, b, op, k, out } => (
                composite(&get(a, &last, &named), &get(b, &last, &named), op, *k),
                out,
            ),
            Prim::Merge { inputs, out } => {
                let mut acc = alloc::vec![0.0f32; wu * hu * 4];
                for i in inputs {
                    let layer = get(i, &last, &named);
                    over(&layer, &mut acc);
                }
                (acc, out)
            }
            Prim::Shadow {
                input,
                dx,
                dy,
                sx: bx,
                sy: by,
                color,
                out,
            } => {
                let color = colour_in(*color, linear);
                let src = get(input, &last, &named);
                let a = color[3];
                let shade: Vec<f32> = src
                    .chunks_exact(4)
                    .flat_map(|p| {
                        let k = p[3] * a;
                        [color[0] * k, color[1] * k, color[2] * k, k]
                    })
                    .collect();
                let mut sh = offset(
                    &blur(&shade, wu, hu, bx * sx, by * sy),
                    wu,
                    hu,
                    dx * sx,
                    dy * sy,
                );
                over(&src, &mut sh);
                (sh, out)
            }
        };
        if !name.is_empty() {
            named.push((name.clone(), res.clone()));
        }
        last = res;
    }
    last
}

/// Draw items into `px`: shapes in order; a group into a layer of its
/// own, then through its filter, clip path, mask and opacity, then over.
fn render(items: &[Item], px: &mut [f32], wu: usize, hu: usize, sx: f32, sy: f32) {
    for it in items {
        match it {
            Item::Shape(sh) => draw_shape(sh, px, wu, hu, sx, sy),
            Item::Image { w, h, rgba, tf, op } => {
                draw_image(*w, *h, rgba, *tf, *op, px, wu, hu, sx, sy)
            }
            Item::Group(g) => {
                let mut layer = alloc::vec![0.0f32; wu * hu * 4];
                render(&g.items, &mut layer, wu, hu, sx, sy);
                if let Some(prims) = &g.filter {
                    if g.linear {
                        light(&mut layer, true);
                        layer = filter(prims, layer, wu, hu, sx, sy, true);
                        light(&mut layer, false);
                    } else {
                        layer = filter(prims, layer, wu, hu, sx, sy, false);
                    }
                    if let Some((x0, y0, x1, y1)) = g.region {
                        let (x0, x1) = (x0 * sx, x1 * sx);
                        let (y0, y1) = (y0 * sy, y1 * sy);
                        for (i, p) in layer.chunks_exact_mut(4).enumerate() {
                            let (x, y) = ((i % wu) as f32 + 0.5, (i / wu) as f32 + 0.5);
                            if x < x0 || x > x1 || y < y0 || y > y1 {
                                p.iter_mut().for_each(|v| *v = 0.0);
                            }
                        }
                    }
                }
                if let Some((polys, even_odd)) = &g.clip {
                    let scaled: Vec<Vec<Pt>> = polys
                        .iter()
                        .map(|p| p.iter().map(|q| (q.0 * sx, q.1 * sy)).collect())
                        .collect();
                    let cov = coverage(&scaled, wu, hu, *even_odd);
                    for (p, c) in layer.chunks_exact_mut(4).zip(cov.iter()) {
                        let k = c.min(1.0);
                        p.iter_mut().for_each(|v| *v *= k);
                    }
                }
                if let Some(mask) = &g.mask {
                    let mut m = alloc::vec![0.0f32; wu * hu * 4];
                    render(mask, &mut m, wu, hu, sx, sy);
                    for (p, q) in layer.chunks_exact_mut(4).zip(m.chunks_exact(4)) {
                        // Luminance of the premultiplied mask: colour times alpha.
                        let k = (0.2125 * q[0] + 0.7154 * q[1] + 0.0721 * q[2]).clamp(0.0, 1.0);
                        p.iter_mut().for_each(|v| *v *= k);
                    }
                }
                if g.opacity < 1.0 {
                    layer.iter_mut().for_each(|v| *v *= g.opacity);
                }
                over(&layer, px);
            }
        }
    }
}

/// Paint the drawing into `w` by `h` RGBA pixels (scaled to fit), and
/// say where its text goes in those pixels.
pub fn rasterise(svg: &Svg, w: u32, h: u32) -> (Vec<u8>, Vec<Text>) {
    let (wu, hu) = (w as usize, h as usize);
    let sx = w as f32 / svg.w.max(1e-3);
    let sy = h as f32 / svg.h.max(1e-3);
    let mut px = alloc::vec![0.0f32; wu * hu * 4];
    render(&svg.items, &mut px, wu, hu, sx, sy);
    let mut rgba = Vec::with_capacity(wu * hu * 4);
    for p in px.chunks_exact(4) {
        let a = p[3];
        let un = |v: f32| {
            if a > 0.0 {
                (v / a).clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        rgba.push((un(p[0]) * 255.0 + 0.5) as u8);
        rgba.push((un(p[1]) * 255.0 + 0.5) as u8);
        rgba.push((un(p[2]) * 255.0 + 0.5) as u8);
        rgba.push((a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
    }
    let texts = svg
        .texts
        .iter()
        .map(|t| Text {
            x: t.x * sx,
            y: t.y * sy,
            size: t.size * (sx + sy) * 0.5,
            ..t.clone()
        })
        .collect();
    (rgba, texts)
}

/// An icon's elements (Lucide's 24-unit grid) as polylines to stroke,
/// scaled to `size` and placed with its centre at (x, y).
pub fn icon_lines(elements: &str, x: f32, y: f32, size: f32) -> Vec<Vec<Pt>> {
    let mut doc = String::from(
        "<svg viewBox=\"0 0 24 24\" width=\"24\" height=\"24\" fill=\"none\" stroke=\"black\">",
    );
    doc.push_str(elements);
    doc.push_str("</svg>");
    let Ok(svg) = parse(&doc) else {
        return Vec::new();
    };
    let s = size / 24.0;
    let mut out = Vec::new();
    for sh in svg.shapes() {
        for (p, _) in &sh.polys {
            out.push(
                p.iter()
                    .copied()
                    .map(|(px, py)| (x + (px - 12.0) * s, y + (py - 12.0) * s))
                    .collect(),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha_at(rgba: &[u8], w: usize, x: usize, y: usize) -> u8 {
        rgba[(y * w + x) * 4 + 3]
    }

    #[test]
    fn paths_flatten_every_command() {
        let p = path("M2 2 L10 2 h5 v5 H2 V2 Z");
        assert_eq!(p.len(), 1);
        assert!(p[0].1, "closed");
        assert_eq!(p[0].0[2], (15.0, 2.0));
        let c = path("m4 14a1 1 0 0 1-.78-1.63l9.9-10.2");
        assert!(c[0].0.len() > 4, "the arc is flattened");
        let q = path("M0 0 Q 5 10 10 0 T 20 0 C 1 2 3 4 5 6 S 7 8 9 10");
        assert!(q[0].0.len() > 20);
        // Arc flags packed together, as minifiers write them.
        let packed = path("M0 0a5 5 0 105 5");
        assert!(packed[0].0.len() > 4);
        assert_eq!(
            transform("translate(10,5) scale(2)").apply((1.0, 1.0)),
            (12.0, 7.0)
        );
        assert_eq!(color("#f00"), Some([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(color("rgb(0, 255, 0)"), Some([0.0, 1.0, 0.0, 1.0]));
    }

    #[test]
    fn shapes_fill_strokes_and_gradients() {
        let svg = parse(
            r##"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" width="40" height="20" viewBox="0 0 40 20">
            <defs><linearGradient id="g" x1="0" x2="1"><stop offset="0" stop-color="#000"/><stop offset="100%" stop-color="#fff"/></linearGradient></defs>
            <rect x="0" y="0" width="20" height="20" fill="url(#g)"/>
            <g transform="translate(20,0)"><circle cx="10" cy="10" r="8" fill="none" stroke="red" stroke-width="2"/></g>
            <text x="20" y="18" font-size="6" text-anchor="middle" fill="blue">Hi &amp; bye</text>
            </svg>"##,
        )
        .unwrap();
        assert_eq!((svg.w, svg.h), (40.0, 20.0));
        assert_eq!(svg.shapes().len(), 2);
        assert_eq!(svg.texts[0].text, "Hi & bye");
        assert_eq!(svg.texts[0].anchor, 1);
        let (rgba, texts) = rasterise(&svg, 80, 40);
        assert_eq!(rgba.len(), 80 * 40 * 4);
        // The gradient runs dark to light.
        let i0 = (20 * 80 + 2) * 4;
        let i1 = (20 * 80 + 37) * 4;
        assert!(rgba[i0] < 40 && rgba[i1] > 200);
        // The ring is drawn, its middle is empty.
        assert!(alpha_at(&rgba, 80, 60, 4) > 200);
        assert_eq!(alpha_at(&rgba, 80, 60, 20), 0);
        assert_eq!(texts[0].x, 40.0);
        assert!(parse("<html></html>").is_err());
    }

    #[test]
    fn lucide_icons_become_lines() {
        let lines = icon_lines(
            r#"<circle cx="12" cy="12" r="10" /><path d="m9 12 2 2 4-4" />"#,
            8.0,
            8.0,
            16.0,
        );
        assert_eq!(lines.len(), 2);
        assert!(lines[0]
            .iter()
            .all(|p| (p.0 - 8.0).abs() <= 6.8 && (p.1 - 8.0).abs() <= 6.8));
        let list = icon_lines(
            r#"<line x1="8" x2="21" y1="6" y2="6" /><polyline points="15 17 20 12 15 7" />"#,
            0.0,
            0.0,
            24.0,
        );
        assert_eq!(list[1].len(), 3);
    }

    fn px(rgba: &[u8], w: usize, x: usize, y: usize) -> [u8; 4] {
        let i = (y * w + x) * 4;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    }

    #[test]
    fn use_symbols_and_style_sheets() {
        let svg = parse(
            r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="40" height="20">
            <style><![CDATA[ .red { fill: #f00 } #b { fill: blue } g.k rect { fill: lime } ]]></style>
            <defs><rect id="sq" width="10" height="10"/>
              <symbol id="dot" viewBox="0 0 2 2"><circle cx="1" cy="1" r="1"/></symbol></defs>
            <use xlink:href="#sq" x="0" y="0" class="red"/>
            <use href="#sq" x="10" y="0" id="b"/>
            <g class="k"><rect x="20" y="0" width="10" height="10" fill="black"/></g>
            <use href="#dot" x="30" y="0" width="10" height="10" fill="#ff0"/>
            <rect x="0" y="10" width="10" height="10" style="display:none"/>
            </svg>"##,
        )
        .unwrap();
        let (rgba, _) = rasterise(&svg, 40, 20);
        assert_eq!(
            px(&rgba, 40, 5, 5),
            [255, 0, 0, 255],
            "class rule on the use reaches the rect"
        );
        assert_eq!(px(&rgba, 40, 15, 5), [0, 0, 255, 255], "id rule");
        assert_eq!(
            px(&rgba, 40, 25, 5),
            [0, 255, 0, 255],
            "descendant rule beats the attribute"
        );
        assert_eq!(
            px(&rgba, 40, 35, 5),
            [255, 255, 0, 255],
            "symbol fitted to the use's box"
        );
        assert_eq!(px(&rgba, 40, 5, 15)[3], 0, "display:none draws nothing");
    }

    #[test]
    fn clip_paths_masks_and_group_opacity() {
        let svg = parse(
            r##"<svg width="40" height="20">
            <defs>
              <clipPath id="c"><rect x="0" y="0" width="5" height="20"/></clipPath>
              <mask id="m"><rect x="10" y="0" width="10" height="20" fill="white"/><rect x="15" y="0" width="5" height="20" fill="black"/></mask>
            </defs>
            <rect x="0" y="0" width="10" height="20" fill="red" clip-path="url(#c)"/>
            <rect x="10" y="0" width="10" height="20" fill="blue" mask="url(#m)"/>
            <g opacity="0.5"><rect x="20" y="0" width="20" height="20" fill="#000"/><rect x="20" y="0" width="20" height="20" fill="#000"/></g>
            </svg>"##,
        )
        .unwrap();
        let (rgba, _) = rasterise(&svg, 40, 20);
        assert_eq!(px(&rgba, 40, 2, 10)[3], 255, "inside the clip");
        assert_eq!(px(&rgba, 40, 7, 10)[3], 0, "outside the clip");
        assert_eq!(px(&rgba, 40, 12, 10), [0, 0, 255, 255], "white mask shows");
        assert_eq!(px(&rgba, 40, 17, 10)[3], 0, "black mask hides");
        // Two overlapping shapes in a half-opaque group: half, not three quarters.
        let a = px(&rgba, 40, 30, 10)[3];
        assert!((126..=129).contains(&a), "{a}");
    }

    #[test]
    fn filters_blur_offset_and_shadow() {
        let svg = parse(
            r##"<svg width="60" height="20">
            <filter id="s" x="-100%" width="300%"><feDropShadow dx="4" dy="0" stdDeviation="0" flood-color="#000" flood-opacity="1"/></filter>
            <filter id="b"><feGaussianBlur stdDeviation="2"/></filter>
            <filter id="g"><feColorMatrix type="saturate" values="0"/></filter>
            <filter id="o" x="-100%" width="300%"><feOffset in="SourceGraphic" dx="3" dy="0" result="x"/><feMerge><feMergeNode in="x"/></feMerge></filter>
            <rect x="0" y="5" width="6" height="10" fill="#f00" filter="url(#s)"/>
            <rect x="20" y="0" width="10" height="20" fill="#000" filter="url(#b)"/>
            <rect x="40" y="0" width="5" height="20" fill="#f00" filter="url(#g)"/>
            <rect x="50" y="0" width="5" height="20" fill="#000" filter="url(#o)"/>
            </svg>"##,
        )
        .unwrap();
        let (rgba, _) = rasterise(&svg, 60, 20);
        assert_eq!(
            px(&rgba, 60, 2, 10),
            [255, 0, 0, 255],
            "the shape stays on top"
        );
        assert_eq!(
            px(&rgba, 60, 8, 10),
            [0, 0, 0, 255],
            "its shadow shows beside it"
        );
        let edge = px(&rgba, 60, 19, 10)[3];
        assert!(
            edge > 20 && edge < 200,
            "blur spreads past the edge: {edge}"
        );
        let grey = px(&rgba, 60, 42, 10);
        assert!(
            grey[0] == grey[1] && grey[1] == grey[2] && grey[0] > 30,
            "saturate 0 greys: {grey:?}"
        );
        assert_eq!(px(&rgba, 60, 51, 10)[3], 0, "moved off");
        assert_eq!(px(&rgba, 60, 56, 10)[3], 255, "moved on");
    }

    #[test]
    fn patterns_markers_and_pictures() {
        const RED: &str = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAEUlEQVR4nGP4z8DwH4QZYAwAR8oH+WdZbrcAAAAASUVORK5CYII=";
        let src = alloc::format!(
            r##"<svg width="60" height="20">
            <defs>
              <pattern id="p" width="4" height="4" patternUnits="userSpaceOnUse"><rect width="2" height="4" fill="#00f"/></pattern>
              <marker id="m" markerWidth="4" markerHeight="4" refX="0" refY="2" orient="auto" markerUnits="userSpaceOnUse"><rect width="4" height="4" fill="#0f0"/></marker>
            </defs>
            <rect x="0" y="0" width="20" height="20" fill="url(#p)"/>
            <line x1="22" y1="10" x2="34" y2="10" stroke="#000" marker-end="url(#m)"/>
            <image x="40" y="0" width="10" height="10" href="data:image/png;base64,{RED}"/>
            <image x="50" y="0" width="10" height="10" href="dot.png"/>
            </svg>"##
        );
        let mut asked = Vec::new();
        let svg = parse_with(&src, &mut |h| {
            asked.push(String::from(h));
            Some(base64(RED))
        })
        .unwrap();
        assert_eq!(
            asked,
            ["dot.png"],
            "a file beside the SVG is asked for by name"
        );
        let (rgba, _) = rasterise(&svg, 60, 20);
        assert_eq!(px(&rgba, 60, 1, 5), [0, 0, 255, 255], "the tile's stripe");
        assert_eq!(px(&rgba, 60, 3, 5)[3], 0, "the tile's gap");
        assert_eq!(px(&rgba, 60, 5, 5), [0, 0, 255, 255], "repeated");
        assert_eq!(
            px(&rgba, 60, 36, 10),
            [0, 255, 0, 255],
            "the marker past the line's end"
        );
        assert_eq!(px(&rgba, 60, 45, 5), [255, 0, 0, 255], "a data: picture");
        assert_eq!(
            px(&rgba, 60, 55, 5),
            [255, 0, 0, 255],
            "a picture read beside it"
        );
        assert_eq!(px(&rgba, 60, 45, 15)[3], 0, "only in its box");
    }

    #[test]
    fn filters_mix_in_linear_light() {
        // Red beside blue, blurred: where they meet, linear light mixes to
        // a brighter purple (188) than sRGB arithmetic does (128).
        let doc = |cif: &str| {
            alloc::format!(
                r##"<svg width="40" height="10"><filter id="b" x="0" y="0" width="1" height="1"{cif}><feGaussianBlur stdDeviation="4 0"/></filter>
                <g filter="url(#b)"><rect x="0" y="0" width="20" height="10" fill="#f00"/><rect x="20" y="0" width="20" height="10" fill="#00f"/></g></svg>"##
            )
        };
        let (a, _) = rasterise(&parse(&doc("")).unwrap(), 40, 10);
        let (b, _) = rasterise(
            &parse(&doc(" color-interpolation-filters=\"sRGB\"")).unwrap(),
            40,
            10,
        );
        let (la, sa) = (px(&a, 40, 20, 5), px(&b, 40, 20, 5));
        assert!(la[0] > 160 && la[2] > 160, "linear: {la:?}");
        assert!(sa[0] < 150 && sa[2] < 150, "sRGB: {sa:?}");
    }
}
