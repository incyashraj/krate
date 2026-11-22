//! SVG: a real subset, read and rasterised in the app, so `![[diagram.svg]]`
//! shows the drawing and Lucide's icons (ISC, see LUCIDE-LICENSE.txt) draw
//! the same shapes Obsidian's do.
//!
//! Read: `svg` (width, height, viewBox), `g`, `path` (every command, arcs
//! included), `rect` (rounded), `circle`, `ellipse`, `line`, `polyline`,
//! `polygon` and `text`/`tspan`; `fill`, `stroke`, `stroke-width`,
//! `opacity`, `fill-opacity`, `stroke-opacity`, `fill-rule`, `style=""`,
//! `transform` (matrix, translate, scale, rotate, skew), and linear and
//! radial gradients by `url(#id)`. Not read: filters, masks, clip paths,
//! patterns, markers, CSS `<style>` sheets, `<use>`, embedded images.
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
    pub shapes: Vec<Shape>,
    pub texts: Vec<Text>,
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
    opacity: f32,
    fill_op: f32,
    stroke_op: f32,
    even_odd: bool,
    font: f32,
    anchor: u8,
    bold: bool,
}

fn attr<'a>(e: &'a El<'_>, k: &str) -> Option<&'a str> {
    e.attrs
        .iter()
        .find(|(a, _)| *a == k)
        .map(|(_, v)| v.as_str())
}

fn style_of(e: &El<'_>, parent: &Style) -> Style {
    let mut s = parent.clone();
    let mut set = |k: &str, v: &str| match k {
        "fill" => s.fill = String::from(v.trim()),
        "stroke" => s.stroke = String::from(v.trim()),
        "stroke-width" => s.width = num(v).unwrap_or(s.width),
        "opacity" => s.opacity *= num(v).unwrap_or(1.0),
        "fill-opacity" => s.fill_op = num(v).unwrap_or(1.0),
        "stroke-opacity" => s.stroke_op = num(v).unwrap_or(1.0),
        "fill-rule" => s.even_odd = v.trim() == "evenodd",
        "font-size" => s.font = num(v).unwrap_or(s.font),
        "text-anchor" => {
            s.anchor = match v.trim() {
                "middle" => 1,
                "end" => 2,
                _ => 0,
            }
        }
        "font-weight" => s.bold = matches!(v.trim(), "bold" | "600" | "700" | "800" | "900"),
        _ => {}
    };
    for (k, v) in &e.attrs {
        set(k, v);
    }
    if let Some(st) = attr(e, "style") {
        for decl in st.split(';') {
            if let Some((k, v)) = decl.split_once(':') {
                set(k.trim(), v);
            }
        }
    }
    if let Some(t) = attr(e, "transform") {
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

/// Read an SVG document.
pub fn parse(src: &str) -> Result<Svg, String> {
    let els = tags(src);
    let root = els
        .iter()
        .find(|e| e.name == "svg" && !e.close)
        .ok_or_else(|| String::from("This is not an SVG picture"))?;
    let vb = attr(root, "viewBox").map(numbers).filter(|v| v.len() == 4);
    let w = attr(root, "width")
        .and_then(num)
        .or(vb.as_ref().and_then(|v| v.get(2).copied()))
        .unwrap_or(300.0);
    let h = attr(root, "height")
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
    // Gradients first, wherever they are.
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
    let mut svg = Svg {
        w,
        h,
        shapes: Vec::new(),
        texts: Vec::new(),
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
        font: 16.0,
        anchor: 0,
        bold: false,
    };
    let mut stack: Vec<Style> = alloc::vec![init];
    let mut skip_depth = 0usize;
    for e in &els {
        let parent = stack.last().cloned().unwrap_or_else(|| stack[0].clone());
        if e.close {
            if matches!(e.name, "g" | "svg" | "a" | "text" | "switch") && stack.len() > 1 {
                stack.pop();
            }
            if matches!(
                e.name,
                "defs"
                    | "clipPath"
                    | "mask"
                    | "pattern"
                    | "symbol"
                    | "marker"
                    | "linearGradient"
                    | "radialGradient"
                    | "style"
            ) {
                skip_depth = skip_depth.saturating_sub(1);
            }
            continue;
        }
        if matches!(
            e.name,
            "defs"
                | "clipPath"
                | "mask"
                | "pattern"
                | "symbol"
                | "marker"
                | "linearGradient"
                | "radialGradient"
                | "style"
        ) {
            if !e.selfclose {
                skip_depth += 1;
            }
            continue;
        }
        if skip_depth > 0 {
            continue;
        }
        let st = style_of(e, &parent);
        if matches!(e.name, "g" | "svg" | "a" | "switch") {
            let is_root = e.name == "svg" && core::ptr::eq(e, root);
            if !e.selfclose && !is_root {
                stack.push(st);
            } else if is_root {
                // The root's own presentation attributes apply to all.
                let mut s = st;
                s.tf = base;
                stack.push(s);
            }
            continue;
        }
        let g = |n: &str| attr(e, n).and_then(num).unwrap_or(0.0);
        let polys: Vec<(Vec<Pt>, bool)> = match e.name {
            "path" => path(attr(e, "d").unwrap_or("")),
            "rect" => {
                let (rx, ry) = match (attr(e, "rx").and_then(num), attr(e, "ry").and_then(num)) {
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
                let v = numbers(attr(e, "points").unwrap_or(""));
                let mut pts: Vec<Pt> = v.chunks_exact(2).map(|c| (c[0], c[1])).collect();
                let closed = e.name == "polygon";
                if closed {
                    if let Some(f) = pts.first().copied() {
                        pts.push(f);
                    }
                }
                alloc::vec![(pts, closed)]
            }
            "text" | "tspan" => {
                let text = unescape(e.text.trim());
                if !text.is_empty() {
                    let (x, y) = (g("x"), g("y"));
                    let p = st.tf.apply((x, y));
                    let c = color(&st.fill).unwrap_or([0.0, 0.0, 0.0, 1.0]);
                    svg.texts.push(Text {
                        x: p.0,
                        y: p.1,
                        size: st.font * st.tf.scale(),
                        text,
                        color: [c[0], c[1], c[2], c[3] * st.opacity * st.fill_op],
                        anchor: st.anchor,
                        bold: st.bold,
                    });
                }
                if e.name == "text" && !e.selfclose {
                    stack.push(st);
                }
                continue;
            }
            _ => continue,
        };
        let polys: Vec<(Vec<Pt>, bool)> = polys
            .into_iter()
            .map(|(p, c)| (p.into_iter().map(|q| st.tf.apply(q)).collect(), c))
            .collect();
        let mut bb = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        // The bounding box in the shape's own space, for gradients.
        let inv_src: Vec<(Vec<Pt>, bool)> = match e.name {
            "path" => path(attr(e, "d").unwrap_or("")),
            _ => Vec::new(),
        };
        let pts_for_bb: Vec<Pt> = if !inv_src.is_empty() {
            inv_src
                .iter()
                .flat_map(|(p, _)| p.iter().copied())
                .collect()
        } else {
            match e.name {
                "rect" => alloc::vec![
                    (g("x"), g("y")),
                    (g("x") + g("width"), g("y") + g("height"))
                ],
                "circle" => alloc::vec![
                    (g("cx") - g("r"), g("cy") - g("r")),
                    (g("cx") + g("r"), g("cy") + g("r"))
                ],
                "ellipse" => alloc::vec![
                    (g("cx") - g("rx"), g("cy") - g("ry")),
                    (g("cx") + g("rx"), g("cy") + g("ry"))
                ],
                _ => alloc::vec![(0.0, 0.0), (1.0, 1.0)],
            }
        };
        for p in pts_for_bb {
            bb = (bb.0.min(p.0), bb.1.min(p.1), bb.2.max(p.0), bb.3.max(p.1));
        }
        let fill = paint(&st.fill, st.opacity * st.fill_op, &grads, bb, st.tf);
        let stroke = paint(&st.stroke, st.opacity * st.stroke_op, &grads, bb, st.tf);
        let fill = if matches!(e.name, "line" | "polyline")
            && attr(e, "fill").is_none()
            && !st.fill.is_empty()
            && e.name == "line"
        {
            Paint::None
        } else {
            fill
        };
        svg.shapes.push(Shape {
            polys,
            fill,
            stroke,
            width: st.width * st.tf.scale(),
            even_odd: st.even_odd,
        });
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
        Paint::None => [0.0; 4],
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

/// Paint the drawing into `w` by `h` RGBA pixels (scaled to fit), and
/// say where its text goes in those pixels.
pub fn rasterise(svg: &Svg, w: u32, h: u32) -> (Vec<u8>, Vec<Text>) {
    let (wu, hu) = (w as usize, h as usize);
    let sx = w as f32 / svg.w.max(1e-3);
    let sy = h as f32 / svg.h.max(1e-3);
    let mut px = alloc::vec![0.0f32; wu * hu * 4];
    let scale = |p: &Pt| (p.0 * sx, p.1 * sy);
    for sh in &svg.shapes {
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
            for y in 0..hu {
                for x in 0..wu {
                    let i = y * wu + x;
                    let a = cov.get(i).copied().unwrap_or(0.0).min(1.0);
                    if a <= 0.0 {
                        continue;
                    }
                    let c = paint_at(&paint, x as f32 + 0.5, y as f32 + 0.5);
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
    for sh in svg.shapes {
        for (p, _) in sh.polys {
            out.push(
                p.into_iter()
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
        assert_eq!(svg.shapes.len(), 2);
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
}
