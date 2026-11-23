//! Ports svg/edges.ts: an edge's path from its route, its markers, the
//! casing drawn under an edge that crosses a fill, its label and a node's
//! self-loops.

#![allow(clippy::collapsible_match)]

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use super::builder::{esc_text, label_svg, num};
use super::ink::{ink, style_fill};
use crate::pele::js::{self, jhypot, jmax, jmin, jround, or, sign};
use crate::pele::layout::ease;
use crate::pele::text::label::{Icons, Label};

pub const ARROW: f64 = 8.0;

/// Ports `interface EdgePath`. `tokens` keeps the path's commands as
/// numbers, so that checking fills need not parse `d`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EdgePath {
    pub d: String,
    pub tokens: Option<Vec<f64>>,
    pub sx: f64,
    pub sy: f64,
    pub sdx: f64,
    pub sdy: f64,
    pub ex: f64,
    pub ey: f64,
    pub edx: f64,
    pub edy: f64,
}

/// The most a corner is rounded by in a stepped route.
const CORNER: f64 = 12.0;
/// How far the handles of a wide turn reach along its two legs.
const HANDLE: f64 = 0.62;
/// How far an edge runs straight before a marker at its end, where it has the room.
pub const LEAD: f64 = 6.0;

/// Ports `sideReach()`: how far from each side of each node the edges
/// that meet it start to turn.
#[derive(Debug, Clone, Default)]
pub struct SideReach {
    reach: BTreeMap<i64, f64>,
}

impl SideReach {
    pub fn new() -> SideReach {
        SideReach::default()
    }

    /// `add`: a node, the side of it, and the marker of an end there.
    pub fn add(&mut self, node: usize, side: i32, kind: &str) {
        let trim = marker_trim(kind);
        let key = node as i64 * 4 + side as i64;
        if side >= 0 && trim > 0.0 && trim + LEAD > self.reach.get(&key).copied().unwrap_or(0.0) {
            self.reach.insert(key, trim + LEAD);
        }
    }

    /// `of`.
    pub fn of(&self, node: usize, side: i32) -> f64 {
        if side >= 0 {
            self.reach
                .get(&(node as i64 * 4 + side as i64))
                .copied()
                .unwrap_or(0.0)
        } else {
            0.0
        }
    }
}

fn near(ax: f64, ay: f64, bx: f64, by: f64) -> bool {
    (ax - bx).abs() < 0.01 && (ay - by).abs() < 0.01
}

/// `xs[i]` with JS's `undefined` (NaN in arithmetic) past the ends.
fn at(v: &[f64], i: isize) -> f64 {
    if i < 0 {
        return f64::NAN;
    }
    v.get(i as usize).copied().unwrap_or(f64::NAN)
}

struct Route {
    xs: Vec<f64>,
    ys: Vec<f64>,
    axes: Vec<f64>,
    linear: bool,
}

impl Route {
    fn aligned(&self, i: isize) -> bool {
        (at(&self.xs, i) - at(&self.xs, i - 1)).abs() < 0.01
            || (at(&self.ys, i) - at(&self.ys, i - 1)).abs() < 0.01
    }
    fn straight(&self, i: isize) -> bool {
        self.linear || self.aligned(i)
    }
    /// The way the route heads from point i to point j: along the flow where an S joins them.
    fn unit(&self, i: isize, j: isize, axis: f64) -> (f64, f64) {
        if !self.straight(i.max(j)) {
            return if axis == 0.0 {
                (0.0, or(sign(at(&self.ys, j) - at(&self.ys, i)), 1.0))
            } else {
                (or(sign(at(&self.xs, j) - at(&self.xs, i)), 1.0), 0.0)
            };
        }
        let dx = at(&self.xs, j) - at(&self.xs, i);
        let dy = at(&self.ys, j) - at(&self.ys, i);
        let len = or(jhypot(dx, dy), 1.0);
        (dx / len, dy / len)
    }
    fn length(&self, i: isize) -> f64 {
        (at(&self.xs, i) - at(&self.xs, i - 1)).abs()
            + (at(&self.ys, i) - at(&self.ys, i - 1)).abs()
    }
    /// Whether the route turns a corner at point i, from running one way to running the other.
    fn turns(&self, i: isize, last: isize) -> bool {
        !self.linear
            && i > 0
            && i < last
            && self.aligned(i)
            && self.aligned(i + 1)
            && self.length(i) >= 0.01
            && self.length(i + 1) >= 0.01
            && ((at(&self.xs, i) - at(&self.xs, i - 1)).abs() < 0.01)
                != ((at(&self.xs, i + 1) - at(&self.xs, i)).abs() < 0.01)
    }
    fn along(&self, i: isize, ux: f64, uy: f64) -> f64 {
        (at(&self.xs, i) - at(&self.xs, i - 1)).abs() * ux.abs()
            + (at(&self.ys, i) - at(&self.ys, i - 1)).abs() * uy.abs()
    }
}

/// `lead` inside `routePath`.
fn lead(reach: f64, trim: f64, run: f64) -> f64 {
    let room = jmin(jmax(0.0, reach - LEAD - trim), run / 4.0);
    room + jmin(reach - trim - room, (run - room) / 4.0)
}

/// Ports `written`: match the value parsed from num() output.
fn written(v: f64) -> f64 {
    let n = jround(v * 100.0);
    if n == 0.0 {
        0.0
    } else {
        n / 100.0
    }
}

/// The default `startReach`/`endReach` of `routePath`.
pub fn default_reach(trim: f64) -> f64 {
    if trim > 0.0 {
        trim + LEAD
    } else {
        0.0
    }
}

/// Ports `routePath`: builds the path for a route of x, y, axis triples,
/// trimming both ends to leave room for markers. `start_reach` and
/// `end_reach` say how far from its nodes the edge starts to turn (pass
/// [`default_reach`] of the trim for Pele's default).
pub fn route_path(
    route: &[f64],
    curve: Option<&str>,
    start_trim: f64,
    end_trim: f64,
    start_reach: f64,
    end_reach: f64,
) -> EdgePath {
    let linear = curve == Some("linear");
    let stepped = matches!(curve, Some("step") | Some("stepBefore") | Some("stepAfter"));
    let mut xs: Vec<f64> = Vec::new();
    let mut ys: Vec<f64> = Vec::new();
    let mut axes: Vec<f64> = Vec::new();
    let mut i = 0;
    while i < route.len() {
        let x = route[i];
        let y = at(route, i as isize + 1);
        let axis = at(route, i as isize + 2);
        // A stepped route turns at right angles where a smooth one would curve.
        if stepped {
            if let (Some(&px), Some(&py)) = (xs.last(), ys.last()) {
                if (x - px).abs() >= 0.01 && (y - py).abs() >= 0.01 {
                    let my = match curve {
                        Some("stepBefore") => py,
                        Some("stepAfter") => y,
                        _ => (py + y) / 2.0,
                    };
                    let mx = match curve {
                        Some("stepBefore") => px,
                        Some("stepAfter") => x,
                        _ => (px + x) / 2.0,
                    };
                    let turns = if axis == 0.0 {
                        [px, my, x, my]
                    } else {
                        [mx, py, mx, y]
                    };
                    let mut k = 0;
                    while k < 4 {
                        if !(near(turns[k], turns[k + 1], px, py)
                            || near(turns[k], turns[k + 1], x, y))
                        {
                            xs.push(turns[k]);
                            ys.push(turns[k + 1]);
                            axes.push(axis);
                        }
                        k += 2;
                    }
                }
            }
        }
        xs.push(x);
        ys.push(y);
        axes.push(axis);
        i += 3;
    }
    let mut r = Route {
        xs,
        ys,
        axes,
        linear,
    };
    let mut last = r.xs.len() as isize - 1;
    let (sdx, sdy) = r.unit(0, 1, at(&r.axes, 1));
    let (edx, edy) = r.unit(last - 1, last, at(&r.axes, last));
    let sx = r.xs[0];
    let sy = r.ys[0];
    let ex = r.xs[last as usize];
    let ey = r.ys[last as usize];
    r.xs[0] += sdx * start_trim;
    r.ys[0] += sdy * start_trim;
    r.xs[last as usize] -= edx * end_trim;
    r.ys[last as usize] -= edy * end_trim;
    let arrives = if end_reach > end_trim && (!r.straight(last) || r.turns(last - 1, last)) {
        lead(end_reach, end_trim, r.along(last, edx, edy))
    } else {
        0.0
    };
    let leaves = if start_reach > start_trim && (!r.straight(1) || r.turns(1, last)) {
        lead(start_reach, start_trim, r.along(1, sdx, sdy))
    } else {
        0.0
    };
    if arrives >= 1.0 {
        let l = last as usize;
        let nx = r.xs[l] - edx * arrives;
        let ny = r.ys[l] - edy * arrives;
        let na = r.axes[l];
        r.xs.insert(l, nx);
        r.ys.insert(l, ny);
        r.axes.insert(l, na);
        last += 1;
    }
    if leaves >= 1.0 {
        let nx = r.xs[0] + sdx * leaves;
        let ny = r.ys[0] + sdy * leaves;
        let na = at(&r.axes, 1);
        r.xs.insert(1, nx);
        r.ys.insert(1, ny);
        r.axes.insert(1, na);
        last += 1;
    }

    // Keep tokens to avoid parsing d when checking fills.
    let mut tokens: Vec<f64> = Vec::new();
    let mut d = String::new();
    let tok = |tokens: &mut Vec<f64>, d: &mut String, v: f64| {
        tokens.push(written(v));
        d.push_str(&num(v));
    };
    tokens.push(77.0);
    tokens.push(2.0);
    d.push('M');
    tok(&mut tokens, &mut d, r.xs[0]);
    d.push(',');
    tok(&mut tokens, &mut d, r.ys[0]);
    let mut at_x = r.xs[0];
    let mut at_y = r.ys[0];
    let line_to =
        |tokens: &mut Vec<f64>, d: &mut String, at_x: &mut f64, at_y: &mut f64, x: f64, y: f64| {
            if near(x, y, *at_x, *at_y) {
                return;
            }
            tokens.push(76.0);
            tokens.push(2.0);
            d.push('L');
            tok(tokens, d, x);
            d.push(',');
            tok(tokens, d, y);
            *at_x = x;
            *at_y = y;
        };
    let mut i: isize = 1;
    while i <= last {
        let iu = i as usize;
        let x = r.xs[iu];
        let y = r.ys[iu];
        if !r.straight(i) {
            let down = r.axes[iu] == 0.0;
            let along = if down { y - at_y } else { x - at_x };
            let k = along.abs() * ease(if down { x - at_x } else { y - at_y }, along);
            let ux = if down { 0.0 } else { sign(along) * k };
            let uy = if down { sign(along) * k } else { 0.0 };
            tokens.push(67.0);
            tokens.push(6.0);
            d.push('C');
            tok(&mut tokens, &mut d, at_x + ux);
            d.push(',');
            tok(&mut tokens, &mut d, at_y + uy);
            d.push(' ');
            tok(&mut tokens, &mut d, x - ux);
            d.push(',');
            tok(&mut tokens, &mut d, y - uy);
            d.push(' ');
            tok(&mut tokens, &mut d, x);
            d.push(',');
            tok(&mut tokens, &mut d, y);
            at_x = x;
            at_y = y;
            i += 1;
            continue;
        }
        if !r.turns(i, last) {
            // A point part of the way along one straight run adds nothing to the path.
            if i < last
                && r.aligned(i)
                && r.aligned(i + 1)
                && (x - r.xs[iu - 1]) * (r.xs[iu + 1] - x) + (y - r.ys[iu - 1]) * (r.ys[iu + 1] - y)
                    > 0.0
            {
                i += 1;
                continue;
            }
            line_to(&mut tokens, &mut d, &mut at_x, &mut at_y, x, y);
            i += 1;
            continue;
        }
        // How much of the leg before the turn and of the leg after it the turn takes.
        let mut before = if r.turns(i - 1, last) {
            r.length(i) / 2.0
        } else {
            r.length(i)
        };
        let mut after = if r.turns(i + 1, last) {
            r.length(i + 1) / 2.0
        } else {
            r.length(i + 1)
        };
        if stepped {
            let m = jmin(jmin(CORNER, before), after);
            before = m;
            after = m;
        }
        let (ux, uy) = r.unit(i - 1, i, r.axes[iu]);
        let (vx, vy) = r.unit(i, i + 1, at(&r.axes, i + 1));
        line_to(
            &mut tokens,
            &mut d,
            &mut at_x,
            &mut at_y,
            x - ux * before,
            y - uy * before,
        );
        at_x = x + vx * after;
        at_y = y + vy * after;
        if stepped {
            tokens.push(81.0);
            tokens.push(4.0);
            d.push('Q');
            tok(&mut tokens, &mut d, x);
            d.push(',');
            tok(&mut tokens, &mut d, y);
            d.push(' ');
            tok(&mut tokens, &mut d, at_x);
            d.push(',');
            tok(&mut tokens, &mut d, at_y);
        } else {
            tokens.push(67.0);
            tokens.push(6.0);
            d.push('C');
            tok(&mut tokens, &mut d, x - ux * before * (1.0 - HANDLE));
            d.push(',');
            tok(&mut tokens, &mut d, y - uy * before * (1.0 - HANDLE));
            d.push(' ');
            tok(&mut tokens, &mut d, x + vx * after * (1.0 - HANDLE));
            d.push(',');
            tok(&mut tokens, &mut d, y + vy * after * (1.0 - HANDLE));
            d.push(' ');
            tok(&mut tokens, &mut d, at_x);
            d.push(',');
            tok(&mut tokens, &mut d, at_y);
        }
        i += 1;
    }
    EdgePath {
        d,
        tokens: Some(tokens),
        sx,
        sy,
        sdx: -sdx,
        sdy: -sdy,
        ex,
        ey,
        edx,
        edy,
    }
}

/// Ports `markerTrim`.
pub fn marker_trim(kind: &str) -> f64 {
    match kind {
        "arrow_point" => ARROW - 1.0,
        "arrow_circle" => ARROW,
        "arrow_cross" => 4.0,
        _ => 0.0,
    }
}

/// Ports `marker`: draws a marker whose tip is at (x, y), pointing along
/// (dx, dy). Marker types are Mermaid's: arrow_point, arrow_circle,
/// arrow_cross; anything else draws nothing.
#[allow(clippy::too_many_arguments)]
pub fn marker(kind: &str, x: f64, y: f64, dx: f64, dy: f64, color: &str, stroke: &str) -> String {
    let nx = -dy;
    let ny = dx;
    match kind {
        "arrow_point" => {
            let bx = x - dx * ARROW;
            let by = y - dy * ARROW;
            let half = ARROW * 0.42;
            format!(
                "<path class=\"pele-marker\" d=\"M{},{}L{},{}L{},{}Z\" fill=\"{}\" stroke=\"{}\"/>",
                num(x),
                num(y),
                num(bx + nx * half),
                num(by + ny * half),
                num(bx - nx * half),
                num(by - ny * half),
                color,
                stroke
            )
        }
        "arrow_circle" => format!(
            "<circle class=\"pele-marker\" cx=\"{}\" cy=\"{}\" r=\"3.5\" fill=\"{}\" stroke=\"{}\"/>",
            num(x - dx * 4.0),
            num(y - dy * 4.0),
            color,
            stroke
        ),
        "arrow_cross" => {
            let cx = x - dx * 4.0;
            let cy = y - dy * 4.0;
            let a = 3.5;
            let ux = (dx + nx) * a;
            let uy = (dy + ny) * a;
            let vx = (dx - nx) * a;
            let vy = (dy - ny) * a;
            format!(
                "<path class=\"pele-marker\" d=\"M{},{}L{},{}M{},{}L{},{}\"/>",
                num(cx - ux),
                num(cy - uy),
                num(cx + ux),
                num(cy + uy),
                num(cx - vx),
                num(cy - vy),
                num(cx + vx),
                num(cy + vy)
            )
        }
        _ => String::new(),
    }
}

/// `TENS`: 1e0 to 1e22, each exact.
const TENS: [f64; 23] = [
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
    1e17, 1e18, 1e19, 1e20, 1e21, 1e22,
];

/// Ports `tokenize` (of a path's `d`): each command stores its character
/// code, argument count, and arguments.
fn tokenize(d: &str) -> Vec<f64> {
    let b = d.as_bytes();
    let code = |j: usize| -> i32 { b.get(j).map(|c| *c as i32).unwrap_or(-1) };
    let mut out: Vec<f64> = Vec::new();
    let mut at_i: isize = -1;
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as i32;
        if (65..=90).contains(&c) && c != 69 {
            at_i = out.len() as isize + 1;
            out.push(c as f64);
            out.push(0.0);
            i += 1;
        } else if c == 45 || c == 46 || (48..=57).contains(&c) {
            // Scale exact integers to match Number() rounding; otherwise use Number().
            let mut j = if c == 45 { i + 1 } else { i };
            let mut m = 0.0f64;
            let mut places: i32 = -1;
            loop {
                let k = code(j);
                if !((48..=57).contains(&k) || (k == 46 && places < 0)) {
                    break;
                }
                if k == 46 {
                    places = 0;
                } else {
                    m = m * 10.0 + k as f64 - 48.0;
                    if places >= 0 {
                        places += 1;
                    }
                }
                j += 1;
            }
            let v;
            if (code(j) | 32) == 101 || m > 9007199254740991.0 || places > 22 {
                loop {
                    let k = code(j);
                    let ok = k == 46
                        || k == 101
                        || k == 69
                        || (48..=57).contains(&k)
                        || ((k == 45 || k == 43) && j > 0 && (code(j - 1) | 32) == 101);
                    if !ok {
                        break;
                    }
                    j += 1;
                }
                v = js::number(&d[i..j]);
            } else {
                let mut x = if places > 0 {
                    m / TENS[places as usize]
                } else {
                    m
                };
                if c == 45 {
                    x = -x;
                }
                v = x;
            }
            if at_i >= 0 {
                out.push(v);
                out[at_i as usize] += 1.0;
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// Ports `pathPoints`: samples curves and arcs; NaN separates subpaths.
fn path_points(d: &str) -> Vec<f64> {
    sample(&tokenize(d))
}

/// Ports `sample`.
fn sample(tokens: &[f64]) -> Vec<f64> {
    let mut out: Vec<f64> = Vec::new();
    let mut x = 0.0f64;
    let mut y = 0.0f64;
    let tk = |k: usize| tokens.get(k).copied().unwrap_or(f64::NAN);
    let mut i = 0usize;
    while i < tokens.len() {
        let cmd = tokens[i] as i32;
        let count = tk(i + 1) as usize;
        let s = i + 2;
        let e = s + count;
        i = e;
        if cmd == 72 || cmd == 86 {
            for k in s..e {
                if cmd == 72 {
                    x = tk(k);
                } else {
                    y = tk(k);
                }
                out.push(x);
                out.push(y);
            }
        } else if cmd == 67 || cmd == 81 {
            let cubic = cmd == 67;
            let step = if cubic { 6 } else { 4 };
            let mut k = s;
            while k + step <= e {
                let x1 = tk(k);
                let y1 = tk(k + 1);
                let x2 = tk(k + 2);
                let y2 = tk(k + 3);
                let x3 = if cubic { tk(k + 4) } else { x2 };
                let y3 = if cubic { tk(k + 5) } else { y2 };
                let mut t = 0.125f64;
                while t <= 1.0 {
                    // Preserve the original De Casteljau operation order to keep rounding unchanged.
                    let mut ax = x + (x1 - x) * t;
                    let mut ay = y + (y1 - y) * t;
                    let mut bx = x1 + (x2 - x1) * t;
                    let mut by = y1 + (y2 - y1) * t;
                    if cubic {
                        let cx = x2 + (x3 - x2) * t;
                        let cy = y2 + (y3 - y2) * t;
                        ax += (bx - ax) * t;
                        ay += (by - ay) * t;
                        bx += (cx - bx) * t;
                        by += (cy - by) * t;
                    }
                    out.push(ax + (bx - ax) * t);
                    out.push(ay + (by - ay) * t);
                    t += 0.125;
                }
                x = x3;
                y = y3;
                k += step;
            }
        } else if cmd == 65 {
            let mut k = s;
            while k + 7 <= e {
                arc(
                    &mut out,
                    x,
                    y,
                    tk(k),
                    tk(k + 1),
                    tk(k + 3),
                    tk(k + 4),
                    tk(k + 5),
                    tk(k + 6),
                );
                x = tk(k + 5);
                y = tk(k + 6);
                k += 7;
            }
        } else {
            let mut k = s;
            while k + 2 <= e {
                if cmd == 77 && k == s && !out.is_empty() {
                    out.push(f64::NAN);
                    out.push(f64::NAN);
                }
                x = tk(k);
                y = tk(k + 1);
                out.push(x);
                out.push(y);
                k += 2;
            }
        }
    }
    out
}

/// Ports `arc`: samples an unrotated arc from (x1, y1), after SVG's
/// conversion from end points to a centre.
#[allow(clippy::too_many_arguments)]
fn arc(
    out: &mut Vec<f64>,
    x1: f64,
    y1: f64,
    mut rx: f64,
    mut ry: f64,
    large: f64,
    sweep: f64,
    x2: f64,
    y2: f64,
) {
    let hx = (x1 - x2) / 2.0;
    let hy = (y1 - y2) / 2.0;
    if rx == 0.0 || ry == 0.0 || (hx == 0.0 && hy == 0.0) {
        out.push(x2);
        out.push(y2);
        return;
    }
    let scale = jmax(1.0, ((hx * hx) / (rx * rx) + (hy * hy) / (ry * ry)).sqrt());
    rx *= scale;
    ry *= scale;
    let k = (if large == sweep { -1.0 } else { 1.0 })
        * jmax(
            0.0,
            (rx * rx * ry * ry - rx * rx * hy * hy - ry * ry * hx * hx)
                / (rx * rx * hy * hy + ry * ry * hx * hx),
        )
        .sqrt();
    let cx = (k * rx * hy) / ry + (x1 + x2) / 2.0;
    let cy = (-k * ry * hx) / rx + (y1 + y2) / 2.0;
    let from = ((y1 - cy) / ry).atan2((x1 - cx) / rx);
    let mut turn = ((y2 - cy) / ry).atan2((x2 - cx) / rx) - from;
    let sweep_on = sweep != 0.0 && !sweep.is_nan();
    if sweep_on && turn < 0.0 {
        turn += 2.0 * core::f64::consts::PI;
    } else if !sweep_on && turn > 0.0 {
        turn -= 2.0 * core::f64::consts::PI;
    }
    for i in 1..=8 {
        let a = from + (turn * i as f64) / 8.0;
        out.push(cx + rx * a.cos());
        out.push(cy + ry * a.sin());
    }
}

/// Ports `clips`: segment overlap with the box, by Liang-Barsky clipping.
#[allow(clippy::too_many_arguments)]
fn clips(x: f64, y: f64, dx: f64, dy: f64, l: f64, t: f64, r: f64, b: f64) -> bool {
    let mut lo = 0.0f64;
    let mut hi = 1.0f64;
    for side in 0..4 {
        let p = match side {
            0 => -dx,
            1 => dx,
            2 => -dy,
            _ => dy,
        };
        let q = match side {
            0 => x - l,
            1 => r - x,
            2 => y - t,
            _ => b - y,
        };
        if p == 0.0 {
            if q < 0.0 {
                hi = -1.0;
            }
        } else if p < 0.0 {
            lo = jmax(lo, q / p);
        } else {
            hi = jmin(hi, q / p);
        }
    }
    lo < hi
}

/// Ports `enters`.
fn enters(points: &[f64], l: f64, t: f64, r: f64, b: f64) -> bool {
    let mut k = 0;
    while k + 2 < points.len() {
        let x = points[k];
        let y = points[k + 1];
        let x2 = points[k + 2];
        let y2 = at(points, k as isize + 3);
        k += 2;
        if (x < l && x2 < l) || (x > r && x2 > r) || (y < t && y2 < t) || (y > b && y2 > b) {
            continue;
        }
        if clips(x, y, x2 - x, y2 - y, l, t, r, b) {
            return true;
        }
    }
    false
}

/// Ports `interface Fill`: bounds are [left, top, right, bottom].
#[derive(Debug, Clone, PartialEq)]
pub struct Fill {
    pub bx: [f64; 4],
    pub shape: String,
}

/// What `fillUnder` finds: the innermost fill holding the whole edge,
/// a crossing of a border (`''`), or no fill (`undefined`).
#[derive(Debug, Clone, PartialEq)]
pub enum Under<'a> {
    On(&'a Fill),
    Crosses,
    Off,
}

/// Ports `fillUnder`: the innermost fill holding the whole edge and its
/// marker tips, Crosses when it crosses a border, or Off off fills.
pub fn fill_under<'a>(
    d: &str,
    tips: &[f64],
    fills: &'a [Fill],
    tokens: Option<&[f64]>,
) -> Under<'a> {
    if fills.is_empty() {
        return Under::Off;
    }
    let points = match tokens {
        Some(t) => sample(t),
        None => path_points(d),
    };
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    let mut k = 0;
    while k < points.len() + tips.len() {
        let x = if k < points.len() {
            points[k]
        } else {
            tips[k - points.len()]
        };
        let y = if k < points.len() {
            at(&points, k as isize + 1)
        } else {
            at(tips, (k + 1 - points.len()) as isize)
        };
        if x < min_x {
            min_x = x;
        }
        if x > max_x {
            max_x = x;
        }
        if y < min_y {
            min_y = y;
        }
        if y > max_y {
            max_y = y;
        }
        k += 2;
    }
    let mut holder: Option<&Fill> = None;
    let mut least = f64::INFINITY;
    for fill in fills {
        let [l, t, r, b] = fill.bx;
        if max_x < l + 0.5 || min_x > r - 0.5 || max_y < t + 0.5 || min_y > b - 0.5 {
            continue;
        }
        let mut touched = enters(&points, l + 0.5, t + 0.5, r - 0.5, b - 0.5);
        let mut k = 0;
        while k < tips.len() && !touched {
            touched = tips[k] > l + 0.5
                && tips[k] < r - 0.5
                && tips[k + 1] > t + 0.5
                && tips[k + 1] < b - 0.5;
            k += 2;
        }
        if !touched {
            continue;
        }
        if min_x < l || max_x > r || min_y < t || max_y > b {
            return Under::Crosses;
        }
        let area = (r - l) * (b - t);
        if area < least {
            least = area;
            holder = Some(fill);
        }
    }
    match holder {
        Some(f) => Under::On(f),
        None => Under::Off,
    }
}

/// `/[";]stroke:/.test(line)`.
fn sets_stroke(line: &str) -> bool {
    line.contains("\"stroke:") || line.contains(";stroke:")
}

/// Ports `edgeInk`: the fill's ink for an edge on it, or "" for a casing
/// when unknown or when the line style sets a stroke.
pub fn edge_ink(fill: &Fill, line: &str) -> String {
    if sets_stroke(line) {
        return String::new();
    }
    ink(&fill.shape, "").unwrap_or("").to_string()
}

/// Ports `hollowOn`: markers with the page colour inside them, with the
/// fill's colour there instead.
pub fn hollow_on(markers: &str, fill: &Fill) -> String {
    match style_fill(&fill.shape).map(js::trim) {
        Some(color) if !color.is_empty() => {
            markers.replace(" fill=\"var(--_bg)\"", &format!(" fill=\"{}\"", color))
        }
        _ => markers.to_string(),
    }
}

/// Ports `overFills`: the ink of the innermost fill that holds the whole
/// edge, Some("") for a casing where it crosses a border, or None off fills.
pub fn over_fills(
    d: &str,
    tips: &[f64],
    fills: &[Fill],
    line: &str,
    tokens: Option<&[f64]>,
) -> Option<String> {
    match fill_under(d, tips, fills, tokens) {
        Under::On(fill) => Some(edge_ink(fill, line)),
        Under::Crosses => Some(String::new()),
        Under::Off => None,
    }
}

const CASE: f64 = 4.0;

/// `new RegExp(`[";]${prop}:([^;"]+);`).exec(line)?.[1].trim()`.
fn decl<'a>(line: &'a str, prop: &str) -> Option<&'a str> {
    if !line.contains(prop) {
        return None;
    }
    let b = line.as_bytes();
    for i in 0..b.len() {
        if (b[i] == b'"' || b[i] == b';')
            && line[i + 1..].starts_with(prop)
            && line[i + 1 + prop.len()..].starts_with(':')
        {
            let s = i + 2 + prop.len();
            let e = line[s..]
                .find([';', '"'])
                .map(|k| s + k)
                .unwrap_or(line.len());
            if e > s && b.get(e) == Some(&b';') {
                return Some(js::trim(&line[s..e]));
            }
        }
    }
    None
}

/// `/^[\d.]+(?:px)?$/.test(width)`.
fn plain_px(width: &str) -> bool {
    let body = width.strip_suffix("px").unwrap_or(width);
    !body.is_empty() && body.bytes().all(|b| b.is_ascii_digit() || b == b'.')
}

/// Ports `casing`: a wider page-coloured stroke under an edge, as faint as
/// it; markers drop their class so marker styles skip it.
pub fn casing(d: &str, markers: &str, line: &str, thick: bool) -> String {
    let stroke = decl(line, "stroke");
    if stroke == Some("none") || stroke == Some("transparent") {
        return String::new();
    }
    let width = if thick {
        Some("2.5")
    } else {
        decl(line, "stroke-width")
    };
    let mut attrs = String::new();
    let mut style = String::new();
    match width {
        Some(w) if plain_px(w) => {
            if js::parse_float(w) != 1.0 {
                attrs = format!(" stroke-width=\"{}\"", num(js::parse_float(w) + CASE));
            }
        }
        Some(w) => style.push_str(&format!(
            "stroke-width:calc({} + {}px);",
            w,
            js::num_str(CASE)
        )),
        None => {}
    }
    for prop in ["opacity", "stroke-opacity"] {
        if let Some(value) = decl(line, prop) {
            style.push_str(&format!("{}:{};", prop, value));
        }
    }
    let body = format!("<path d=\"{}\"/>", d) + &markers.replace(" class=\"pele-marker\"", "");
    if !attrs.is_empty() || !style.is_empty() {
        let s = if style.is_empty() {
            String::new()
        } else {
            format!(" style=\"{}\"", style)
        };
        format!("<g{}{}>{}</g>", attrs, s, body)
    } else {
        body
    }
}

/// Ports `edgeCasing`.
pub fn edge_casing(
    path: &EdgePath,
    start_type: &str,
    end_type: &str,
    line: &str,
    thick: bool,
) -> String {
    casing(
        &path.d,
        &(marker(
            start_type,
            path.sx,
            path.sy,
            path.sdx,
            path.sdy,
            "var(--_bg)",
            "var(--_bg)",
        ) + &marker(
            end_type,
            path.ex,
            path.ey,
            path.edx,
            path.edy,
            "var(--_bg)",
            "var(--_bg)",
        )),
        line,
        thick,
    )
}

/// Ports `casingsSvg`.
pub fn casings_svg(casings: &str) -> String {
    if casings.is_empty() {
        return String::new();
    }
    format!(
        "<g class=\"pele-casings\" fill=\"none\" stroke=\"var(--_bg)\" stroke-width=\"{}\" stroke-linecap=\"round\" stroke-linejoin=\"round\" pointer-events=\"none\">{}</g>",
        js::num_str(1.0 + CASE),
        casings
    )
}

/// Ports `edgeLabelSvg`: an edge label on a patch of the background, so
/// the line does not run through the text.
pub fn edge_label_svg(
    id: &str,
    label: &Label,
    x: f64,
    y: f64,
    attrs: &str,
    icons: Icons,
) -> String {
    let w = label.width + 8.0;
    let h = label.height;
    format!(
        "<g class=\"pele-edge-label\" data-id=\"{}\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"3\" fill=\"var(--_bg)\"/>{}</g>",
        esc_text(id),
        num(x - w / 2.0),
        num(y - h / 2.0),
        num(w),
        num(h),
        label_svg(label, x, y, attrs, icons)
    )
}

/// The label fields of an edge `loopPath` writes (`e`).
pub struct LoopLabel<'a> {
    pub label_w: f64,
    pub label_h: f64,
    pub label_x: &'a mut f64,
    pub label_y: &'a mut f64,
}

/// Ports `loopPath`: a self-loop beside a node of drawn size w by h, out
/// of the side that lies across the flow and back in. `k` counts the
/// loops already on the node. Also places the edge's label beyond the loop.
#[allow(clippy::too_many_arguments)]
pub fn loop_path(
    cx: f64,
    cy: f64,
    w: f64,
    h: f64,
    k: f64,
    sideways: bool,
    base: f64,
    trim: f64,
    e: LoopLabel,
) -> EdgePath {
    let half = (if sideways { w } else { h }) / 2.0;
    let spread = jmin(half - 4.0, 8.0 + k * 6.0);
    let reach = base + k * 8.0;
    let len = or(jhypot(reach, spread), 1.0);
    // Local frame: `out` points away from the node, `along` runs along its side.
    let pt = |out: f64, along: f64| -> String {
        if sideways {
            format!("{},{}", num(cx + along), num(cy + h / 2.0 + out))
        } else {
            format!("{},{}", num(cx + w / 2.0 + out), num(cy + along))
        }
    };
    let tx = (reach / len) * trim;
    let ty = (spread / len) * trim;
    if sideways {
        *e.label_x = cx;
        *e.label_y = cy + h / 2.0 + reach + 4.0 + e.label_h / 2.0;
    } else {
        *e.label_x = cx + w / 2.0 + reach + 4.0 + e.label_w / 2.0;
        *e.label_y = cy;
    }
    EdgePath {
        d: format!(
            "M{}C{} {} {}",
            pt(0.0, -spread),
            pt(reach, -spread * 2.0),
            pt(reach, spread * 2.0),
            pt(tx, spread + ty)
        ),
        tokens: None,
        sx: if sideways { cx - spread } else { cx + w / 2.0 },
        sy: if sideways { cy + h / 2.0 } else { cy - spread },
        sdx: if sideways { 0.0 } else { -1.0 },
        sdy: if sideways { -1.0 } else { 0.0 },
        ex: if sideways { cx + spread } else { cx + w / 2.0 },
        ey: if sideways { cy + h / 2.0 } else { cy + spread },
        edx: if sideways {
            -spread / len
        } else {
            -reach / len
        },
        edy: if sideways {
            -reach / len
        } else {
            -spread / len
        },
    }
}
