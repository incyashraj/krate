//! Ports diagrams/block/render.ts: lays out and draws a parsed block
//! diagram.

#![allow(clippy::needless_range_loop, clippy::too_many_arguments)]

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::f64::consts::{FRAC_1_SQRT_2, PI};
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use super::arrows::{arrow_axis, arrow_mask, arrow_size, draw_arrow, OVERHANG};
use super::db::BlockDb;
use super::layout::{layout, Cell, HasCell};
use super::types::Block;
use crate::json::Value;
use crate::pele::common::fit_width::{tighten, title_room};
use crate::pele::common::ordered::OrderedMap;
use crate::pele::core::{RenderOptions, Rendered};
use crate::pele::errors::PeleError;
use crate::pele::js::{self, jhypot, jmax, jmin, jround, or, sign};
use crate::pele::svg::builder::{esc_text, label_svg, num};
use crate::pele::svg::edges::{
    casings_svg, edge_casing, edge_label_svg, marker, marker_trim, over_fills, EdgePath, Fill,
    ARROW,
};
use crate::pele::svg::ink::{ink_fill, paints};
use crate::pele::svg::root::{svg_document, Accessible};
use crate::pele::svg::shapes::{draw_shape, shape_filled, shape_inset, shape_size};
use crate::pele::svg::theme::{class_names, resolve_style, rounded, sets_fill, ResolvedStyle};
use crate::pele::svg::title::title_label;
use crate::pele::text::label::{layout_label, Label};
use crate::pele::text::measurer::{style, Measure, METRICS};

const GAP: f64 = 12.0;
const WRAP: f64 = 200.0;
const CORNER: f64 = 8.0;
const SHAPE_ATTRS: &str = " fill=\"var(--_s)\" stroke=\"var(--_b)\"";
const LINE_ATTRS: &str = " stroke=\"var(--_b)\"";
/// Checking every edge against every block is quadratic, so large
/// diagrams route without it.
const OBSTACLE_BUDGET: f64 = 2_000_000.0;
const LABEL_GAP: f64 = 4.0;

/// Ports `SHAPES`.
fn shape_of(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "square" => "rect",
        "round" => "rounded",
        "circle" => "circle",
        "doublecircle" => "dbl-circ",
        "diamond" => "diam",
        "hexagon" => "hex",
        "stadium" => "stadium",
        "subroutine" => "fr-rect",
        "cylinder" => "cyl",
        "rect_left_inv_arrow" => "odd",
        "lean_right" => "lean-r",
        "lean_left" => "lean-l",
        "trapezoid" => "trap-b",
        "inv_trapezoid" => "trap-t",
        _ => return None,
    })
}

/// Ports `FIXED`: shapes that keep their own size in a larger cell.
fn fixed(shape: &str) -> bool {
    matches!(shape, "circle" | "dbl-circ" | "diam")
}

/// Ports `WIDE`: shapes that only widen, because their slants and curves
/// are set by their height.
fn wide(shape: &str) -> bool {
    matches!(
        shape,
        "stadium" | "hex" | "odd" | "lean-r" | "lean-l" | "trap-b" | "trap-t"
    )
}

/// Ports `const enum Kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Leaf,
    Arrow,
    Composite,
    Space,
}

/// Ports `interface View`. `block` is a place in the db's arena.
#[derive(Debug, Clone)]
struct View {
    cell: Cell,
    block: usize,
    kind: Kind,
    index: usize,
    shape: &'static str,
    label: Label,
    style: ResolvedStyle,
    mask: u32,
    dx: f64,
    dy: f64,
    /// Size as drawn, which for a fixed shape is smaller than its cell.
    bw: f64,
    bh: f64,
}

impl HasCell for View {
    fn cell(&self) -> &Cell {
        &self.cell
    }
    fn cell_mut(&mut self) -> &mut Cell {
        &mut self.cell
    }
}

/// Ports `interface Box`, for what a label must not cover: a view (by
/// place), or a container's title (`view` None).
#[derive(Debug, Clone, Copy)]
struct Ob {
    view: Option<usize>,
    x: f64,
    y: f64,
    bw: f64,
    bh: f64,
}

/// Ports `interface Route`. `edge` is a place in the db's arena.
#[derive(Debug, Clone)]
struct Route {
    edge: usize,
    /// Two points for a straight line, three for a bend, none for a self-loop.
    points: Vec<f64>,
    looped: usize,
    label: Label,
    lx: f64,
    ly: f64,
}

/// Ports `isRound`.
fn is_round(v: &View) -> bool {
    v.kind == Kind::Leaf && (v.shape == "circle" || v.shape == "dbl-circ")
}

/// Ports `edgeY`: where a vertical line at x meets the outline, below the
/// center (sign 1) or above it (sign -1).
fn edge_y(v: &View, x: f64, sgn: f64) -> f64 {
    let hw = v.bw / 2.0;
    let hh = v.bh / 2.0;
    let dx = (x - v.cell.x).abs();
    let mut reach = hh;
    if is_round(v) {
        reach = jmax(hw * hw - dx * dx, 0.0).sqrt();
    } else if v.kind == Kind::Leaf && v.shape == "diam" {
        reach = hh * jmax(1.0 - dx / hw, 0.0);
    } else if v.kind == Kind::Leaf {
        reach = hh - shape_inset(v.shape, v.bw, v.bh, if sgn > 0.0 { 2 } else { 0 }, 0.0);
    } else if v.kind == Kind::Arrow && arrow_axis(v.mask) == "x" {
        reach = hh - OVERHANG;
    }
    v.cell.y + sgn * reach
}

/// Ports `edgeX`: where a horizontal line at y meets the outline, right of
/// the center (sign 1) or left of it (sign -1).
fn edge_x(v: &View, y: f64, sgn: f64) -> f64 {
    let hw = v.bw / 2.0;
    let hh = v.bh / 2.0;
    let dy = (y - v.cell.y).abs();
    let mut reach = hw;
    if is_round(v) {
        reach = jmax(hh * hh - dy * dy, 0.0).sqrt();
    } else if v.kind == Kind::Leaf && v.shape == "diam" {
        reach = hw * jmax(1.0 - dy / hh, 0.0);
    } else if v.kind == Kind::Leaf {
        reach = hw - shape_inset(v.shape, v.bw, v.bh, if sgn > 0.0 { 1 } else { 3 }, 0.0);
    } else if v.kind == Kind::Arrow && arrow_axis(v.mask) == "y" {
        reach = hw - OVERHANG;
    }
    v.cell.x + sgn * reach
}

/// Ports `border`: where the line from the center toward (tx, ty) leaves
/// the outline.
fn border(v: &View, tx: f64, ty: f64, out: &mut Vec<f64>) {
    let dx = tx - v.cell.x;
    let dy = ty - v.cell.y;
    let hw = v.bw / 2.0;
    let hh = v.bh / 2.0;
    let mut k = if is_round(v) {
        hw / or(jhypot(dx, dy), 1.0)
    } else if v.kind == Kind::Leaf && v.shape == "diam" {
        1.0 / or(dx.abs() / hw + dy.abs() / hh, 1.0)
    } else {
        jmin(
            if dx == 0.0 {
                f64::INFINITY
            } else {
                hw / dx.abs()
            },
            if dy == 0.0 {
                f64::INFINITY
            } else {
                hh / dy.abs()
            },
        )
    };
    if k == f64::INFINITY {
        k = 0.0;
    }
    out.push(v.cell.x + dx * k);
    out.push(v.cell.y + dy * k);
}

/// Ports `crossesBox`.
fn crosses_box(x1: f64, y1: f64, x2: f64, y2: f64, v: &Ob) -> bool {
    let l = v.x - v.bw / 2.0 + 1.0;
    let r = v.x + v.bw / 2.0 - 1.0;
    let t = v.y - v.bh / 2.0 + 1.0;
    let b = v.y + v.bh / 2.0 - 1.0;
    if (x1 < l && x2 < l) || (x1 > r && x2 > r) || (y1 < t && y2 < t) || (y1 > b && y2 > b) {
        return false;
    }
    if x1 == x2 || y1 == y2 {
        return true;
    }
    // The segment is diagonal and its bounding box overlaps: the box is hit unless all four corners are on one side.
    let dx = x2 - x1;
    let dy = y2 - y1;
    let side = |x: f64, y: f64| -> f64 { sign(dx * (y - y1) - dy * (x - x1)) };
    let first = side(l, t);
    first == 0.0 || side(r, t) != first || side(r, b) != first || side(l, b) != first
}

/// Ports `hits`.
fn hits(points: &[f64], s: usize, t: usize, obstacles: &[Ob]) -> usize {
    let mut count = 0;
    for v in obstacles {
        if v.view == Some(s) || v.view == Some(t) {
            continue;
        }
        let mut i = 0;
        while i + 3 < points.len() {
            if crosses_box(points[i], points[i + 1], points[i + 2], points[i + 3], v) {
                count += 1;
                break;
            }
            i += 2;
        }
    }
    count
}

/// Ports `routeEdge`: a straight line where the two blocks share a row or
/// a column, otherwise one bend between facing sides.
fn route_edge(
    views: &[View],
    si: usize,
    ti: usize,
    shift: f64,
    obstacles: Option<&[Ob]>,
) -> Vec<f64> {
    let s = &views[si];
    let t = &views[ti];
    let sl = s.cell.x - s.bw / 2.0;
    let sr = s.cell.x + s.bw / 2.0;
    let st = s.cell.y - s.bh / 2.0;
    let sb = s.cell.y + s.bh / 2.0;
    let tl = t.cell.x - t.bw / 2.0;
    let tr = t.cell.x + t.bw / 2.0;
    let tt = t.cell.y - t.bh / 2.0;
    let tb = t.cell.y + t.bh / 2.0;
    let x1 = jmax(sl, tl);
    let x2 = jmin(sr, tr);
    let y1 = jmax(st, tt);
    let y2 = jmin(sb, tb);
    let over_x = x2 - x1 > 6.0;
    let over_y = y2 - y1 > 6.0;

    if over_x && over_y {
        // One block is inside the other: join the inner one to the nearest side of the outer one.
        let inner_is_s = s.bw * s.bh <= t.bw * t.bh;
        let (inner, outer) = if inner_is_s { (s, t) } else { (t, s) };
        let up = inner.cell.y - inner.bh / 2.0 - (outer.cell.y - outer.bh / 2.0);
        let down = outer.cell.y + outer.bh / 2.0 - (inner.cell.y + inner.bh / 2.0);
        let left = inner.cell.x - inner.bw / 2.0 - (outer.cell.x - outer.bw / 2.0);
        let right = outer.cell.x + outer.bw / 2.0 - (inner.cell.x + inner.bw / 2.0);
        let least = jmin(jmin(up, down), jmin(left, right));
        let points = if least == down {
            alloc::vec![
                inner.cell.x,
                edge_y(inner, inner.cell.x, 1.0),
                inner.cell.x,
                outer.cell.y + outer.bh / 2.0
            ]
        } else if least == up {
            alloc::vec![
                inner.cell.x,
                edge_y(inner, inner.cell.x, -1.0),
                inner.cell.x,
                outer.cell.y - outer.bh / 2.0
            ]
        } else if least == right {
            alloc::vec![
                edge_x(inner, inner.cell.y, 1.0),
                inner.cell.y,
                outer.cell.x + outer.bw / 2.0,
                inner.cell.y
            ]
        } else {
            alloc::vec![
                edge_x(inner, inner.cell.y, -1.0),
                inner.cell.y,
                outer.cell.x - outer.bw / 2.0,
                inner.cell.y
            ]
        };
        return if inner_is_s {
            points
        } else {
            alloc::vec![points[2], points[3], points[0], points[1]]
        };
    }
    if over_x {
        let margin = jmin(6.0, (x2 - x1) / 2.0);
        let x = jmin(jmax((x1 + x2) / 2.0 + shift, x1 + margin), x2 - margin);
        let down = if t.cell.y > s.cell.y { 1.0 } else { -1.0 };
        return alloc::vec![x, edge_y(s, x, down), x, edge_y(t, x, -down)];
    }
    if over_y {
        let margin = jmin(6.0, (y2 - y1) / 2.0);
        let y = jmin(jmax((y1 + y2) / 2.0 + shift, y1 + margin), y2 - margin);
        let right = if t.cell.x > s.cell.x { 1.0 } else { -1.0 };
        return alloc::vec![edge_x(s, y, right), y, edge_x(t, y, -right), y];
    }

    let right = if t.cell.x > s.cell.x { 1.0 } else { -1.0 };
    let down = if t.cell.y > s.cell.y { 1.0 } else { -1.0 };
    let across = alloc::vec![
        edge_x(s, s.cell.y, right),
        s.cell.y,
        t.cell.x,
        s.cell.y,
        t.cell.x,
        edge_y(t, t.cell.x, -down)
    ];
    let along = alloc::vec![
        s.cell.x,
        edge_y(s, s.cell.x, down),
        s.cell.x,
        t.cell.y,
        edge_x(t, t.cell.y, -right),
        t.cell.y
    ];
    let is_wide = (t.cell.x - s.cell.x).abs() >= (t.cell.y - s.cell.y).abs();
    let (mut best, other) = if is_wide {
        (across, along)
    } else {
        (along, across)
    };
    let Some(obstacles) = obstacles else {
        return best;
    };
    let mut least = hits(&best, si, ti, obstacles);
    if least == 0 {
        return best;
    }
    let count = hits(&other, si, ti, obstacles);
    if count < least {
        best = other;
        least = count;
    }
    if least == 0 {
        return best;
    }
    let mut direct: Vec<f64> = Vec::new();
    border(s, t.cell.x, t.cell.y, &mut direct);
    border(t, s.cell.x, s.cell.y, &mut direct);
    if hits(&direct, si, ti, obstacles) < least {
        direct
    } else {
        best
    }
}

/// Ports `trace`: builds the path through two or three points, rounding
/// the bend and leaving room for markers.
fn trace(p: &[f64], mut start_trim: f64, mut end_trim: f64) -> EdgePath {
    let n = p.len();
    let sx = p[0];
    let sy = p[1];
    let ex = p[n - 2];
    let ey = p[n - 1];
    let mut ax = p[2] - sx;
    let mut ay = p[3] - sy;
    let first = jhypot(ax, ay);
    ax /= or(first, 1.0);
    ay /= or(first, 1.0);
    let mut bx = ex - p[n - 4];
    let mut by = ey - p[n - 3];
    let last = jhypot(bx, by);
    bx /= or(last, 1.0);
    by /= or(last, 1.0);
    if n == 4 && start_trim + end_trim > first {
        let scale = first / (start_trim + end_trim);
        start_trim *= scale;
        end_trim *= scale;
    }
    start_trim = jmin(start_trim, first);
    end_trim = jmin(end_trim, last);
    let mut d = format!(
        "M{},{}",
        num(sx + ax * start_trim),
        num(sy + ay * start_trim)
    );
    if n == 6 {
        let r = jmax(0.0, jmin(jmin(CORNER, first - start_trim), last - end_trim));
        d.push_str(&format!(
            "L{},{}Q{},{} {},{}",
            num(p[2] - ax * r),
            num(p[3] - ay * r),
            num(p[2]),
            num(p[3]),
            num(p[2] + bx * r),
            num(p[3] + by * r)
        ));
    }
    d.push_str(&format!(
        "L{},{}",
        num(ex - bx * end_trim),
        num(ey - by * end_trim)
    ));
    EdgePath {
        d,
        tokens: None,
        sx,
        sy,
        sdx: -ax,
        sdy: -ay,
        ex,
        ey,
        edx: bx,
        edy: by,
    }
}

/// Ports `loopArc`: a self-loop is an arc around the top right corner of
/// the block, or around the matching point on a round or diamond outline.
/// Gives the arc's center and radius and the angles where it leaves and
/// returns.
fn loop_arc(v: &View, k: usize) -> [f64; 5] {
    let hw = v.bw / 2.0;
    let hh = v.bh / 2.0;
    let r = jmax(1.0, jmin(jmin(10.0 + 5.0 * (k as f64 - 1.0), hw), hh));
    let (x, y) = (v.cell.x, v.cell.y);
    if is_round(v) {
        let px = x + hw * FRAC_1_SQRT_2;
        let py = y - hw * FRAC_1_SQRT_2;
        let half = 2.0 * jmin(1.0, r / (2.0 * hw)).asin();
        let angle = |at: f64| -> f64 { (y - hw * at.sin() - py).atan2(x + hw * at.cos() - px) };
        return [px, py, r, angle(PI / 4.0 + half), angle(PI / 4.0 - half)];
    }
    if v.kind == Kind::Leaf && v.shape == "diam" {
        let slope = hh.atan2(hw);
        return [x + hw / 2.0, y - hh / 2.0, r, slope + PI, slope];
    }
    [x + hw, y - hh, r, PI, PI / 2.0]
}

/// Ports `loopPath`.
fn loop_path(v: &View, k: usize, start_trim: f64, end_trim: f64) -> EdgePath {
    let [px, py, r, a1, a2] = loop_arc(v, k);
    let from = a1 + start_trim / r;
    let to = a2 - end_trim / r;
    let mut sweep = (to - from) % (2.0 * PI);
    if sweep <= 0.0 {
        sweep += 2.0 * PI;
    }
    let x1 = px + r * from.cos();
    let y1 = py + r * from.sin();
    let x2 = px + r * to.cos();
    let y2 = py + r * to.sin();
    let sx = px + r * a1.cos();
    let sy = py + r * a1.sin();
    let ex = px + r * a2.cos();
    let ey = py + r * a2.sin();
    // Each marker lies along the chord it covers, so it follows the curve instead of the tangent at its tip.
    let sl = jhypot(sx - x1, sy - y1);
    let el = jhypot(ex - x2, ey - y2);
    EdgePath {
        d: format!(
            "M{},{}A{},{} 0 {} 1 {},{}",
            num(x1),
            num(y1),
            num(r),
            num(r),
            if sweep > PI { 1 } else { 0 },
            num(x2),
            num(y2)
        ),
        tokens: None,
        sx,
        sy,
        sdx: if sl > 0.0 { (sx - x1) / sl } else { a1.sin() },
        sdy: if sl > 0.0 { (sy - y1) / sl } else { -a1.cos() },
        ex,
        ey,
        edx: if el > 0.0 { (ex - x2) / el } else { -a2.sin() },
        edy: if el > 0.0 { (ey - y2) / el } else { a2.cos() },
    }
}

const SPOTS: [f64; 5] = [0.5, 0.3, 0.7, 0.15, 0.85];

/// Ports `covers`.
fn covers(x: f64, y: f64, w: f64, h: f64, s: usize, t: usize, obstacles: Option<&[Ob]>) -> bool {
    let Some(obstacles) = obstacles else {
        return false;
    };
    for v in obstacles {
        if v.view != Some(s)
            && v.view != Some(t)
            && (x - v.x).abs() < (w + v.bw) / 2.0
            && (y - v.y).abs() < (h + v.bh) / 2.0
        {
            return true;
        }
    }
    false
}

/// Ports `labelSpot`: picks the point on a leg where a label of the given
/// size covers no block other than the two it joins.
fn label_spot(
    p: &[f64],
    at: usize,
    w: f64,
    h: f64,
    s: usize,
    t: usize,
    obstacles: Option<&[Ob]>,
) -> f64 {
    if obstacles.is_none() {
        return 0.5;
    }
    for spot in SPOTS {
        let x = p[at] + (p[at + 2] - p[at]) * spot;
        let y = p[at + 1] + (p[at + 3] - p[at + 1]) * spot;
        if !covers(x, y, w, h, s, t, obstacles) {
            return spot;
        }
    }
    0.5
}

/// Ports `stackLabels`: stacks overlapping labels along their shared line,
/// if they fit between its markers and cover no other block or title.
/// Returns the extra length needed to fit the labels.
fn stack_labels(
    stack: &[usize],
    routes: &mut [Route],
    legs: &[Vec<f64>],
    s: usize,
    t: usize,
    obstacles: Option<&[Ob]>,
) -> f64 {
    let vertical = legs[stack[0]][0] == legs[stack[0]][2];
    let mut lo = f64::NEG_INFINITY;
    let mut hi = f64::INFINITY;
    let mut need = -LABEL_GAP;
    let mut crowded = false;
    for a in 0..stack.len() {
        let leg = &legs[stack[a]];
        if if vertical {
            leg[0] != leg[2]
        } else {
            leg[1] != leg[3]
        } {
            return 0.0;
        }
        lo = jmax(
            lo,
            if vertical {
                jmin(leg[1], leg[3])
            } else {
                jmin(leg[0], leg[2])
            },
        );
        hi = jmin(
            hi,
            if vertical {
                jmax(leg[1], leg[3])
            } else {
                jmax(leg[0], leg[2])
            },
        );
        let p = &routes[stack[a]];
        need += (if vertical {
            p.label.height
        } else {
            p.label.width + 8.0
        }) + LABEL_GAP;
        for b in 0..a {
            let q = &routes[stack[b]];
            if (p.lx - q.lx).abs() < (p.label.width + q.label.width) / 2.0 + 8.0
                && (p.ly - q.ly).abs() < (p.label.height + q.label.height) / 2.0
            {
                crowded = true;
            }
        }
    }
    if !crowded {
        return 0.0;
    }
    if need > hi - lo - 2.0 * ARROW {
        return need - (hi - lo - 2.0 * ARROW);
    }
    let mut along: Vec<f64> = Vec::new();
    let mut next = (lo + hi - need) / 2.0;
    for &i in stack {
        let (label, lx, ly) = (&routes[i].label, routes[i].lx, routes[i].ly);
        let size = if vertical {
            label.height
        } else {
            label.width + 8.0
        };
        let at = next + size / 2.0;
        if covers(
            if vertical { lx } else { at },
            if vertical { at } else { ly },
            label.width + 8.0,
            label.height,
            s,
            t,
            obstacles,
        ) {
            return 0.0;
        }
        along.push(at);
        next += size + LABEL_GAP;
    }
    for (k, &i) in stack.iter().enumerate() {
        if vertical {
            routes[i].ly = along[k];
        } else {
            routes[i].lx = along[k];
        }
    }
    0.0
}

/// Ports `widen`: widens the gap in the blocks' nearest shared container.
/// `base` keeps the gaps as they were when the labels were measured, by
/// container and axis.
fn widen(
    views: &mut [View],
    parents: &[Option<usize>],
    s: usize,
    t: usize,
    vertical: bool,
    short: f64,
    base: &mut BTreeMap<(usize, bool), Vec<f64>>,
) -> bool {
    let mut chain: Vec<usize> = Vec::new();
    let mut v = Some(s);
    while let Some(k) = v {
        chain.push(k);
        v = parents[k];
    }
    let mut tb: Option<usize> = None;
    let mut bx: Option<usize> = Some(t);
    while let Some(k) = bx {
        if chain.contains(&k) {
            break;
        }
        tb = Some(k);
        bx = parents[k];
    }
    let (Some(bx), Some(tb)) = (bx, tb) else {
        return false;
    };
    if bx == s {
        return false;
    }
    let mut sb = s;
    while parents[sb] != Some(bx) {
        sb = parents[sb].unwrap();
    }
    let at: usize = if vertical {
        if views[sb].cell.row == views[tb].cell.row {
            return false;
        }
        (jmax(views[sb].cell.row, views[tb].cell.row) - 1.0) as usize
    } else {
        let (l, r) = if views[sb].cell.col < views[tb].cell.col {
            (sb, tb)
        } else {
            (tb, sb)
        };
        if views[r].cell.col < views[l].cell.col + views[l].cell.span {
            return false;
        }
        (views[r].cell.col - 1.0) as usize
    };
    let gaps = if vertical {
        &mut views[bx].cell.row_gaps
    } else {
        &mut views[bx].cell.col_gaps
    };
    let before = base.entry((bx, vertical)).or_insert_with(|| gaps.clone());
    let was = before.get(at).copied().unwrap_or(0.0);
    if gaps.len() <= at {
        gaps.resize(at + 1, 0.0);
    }
    gaps[at] = jmax(gaps[at], was + short);
    true
}

/// Ports `renderBlock`.
pub fn render_block(
    model: &BlockDb,
    config: &Value,
    options: &RenderOptions,
    palette_style: &str,
) -> Result<Rendered, PeleError> {
    tighten(options, |tight| {
        draw(model, config, options, palette_style, tight)
    })
}

/// The views a label or a line must keep clear of.
fn obs_of(views: &[View], list: &[usize]) -> Vec<Ob> {
    list.iter()
        .map(|&k| Ob {
            view: Some(k),
            x: views[k].cell.x,
            y: views[k].cell.y,
            bw: views[k].bw,
            bh: views[k].bh,
        })
        .collect()
}

/// The state `place` and `route` share in `draw`.
struct Scene<'a> {
    views: Vec<View>,
    parents: Vec<Option<usize>>,
    widths: Vec<f64>,
    obstacles: Vec<usize>,
    titles: Vec<Ob>,
    gap: f64,
    model: &'a BlockDb,
    ends: Vec<(usize, usize)>,
    drawn: Vec<usize>,
    pairs: BTreeMap<usize, usize>,
    labelled: alloc::collections::BTreeSet<usize>,
    check: bool,
    routes: Vec<Route>,
    stacks: OrderedMap<Vec<usize>>,
    legs: Vec<Vec<f64>>,
    edge_size: f64,
    measurer: &'a dyn Measure,
    icons: crate::pele::text::label::Icons<'a>,
}

impl Scene<'_> {
    /// Ports `place`.
    fn place(&mut self) -> Result<(f64, f64), PeleError> {
        for v in self.views.iter_mut() {
            v.cell.nw = self.widths[v.index];
        }
        let content = layout(&mut self.views, 0, self.gap)?;
        self.obstacles.clear();
        self.titles.clear();
        for k in 0..self.views.len() {
            let v = &mut self.views[k];
            if v.kind == Kind::Leaf && fixed(v.shape) {
                v.bw = v.cell.nw;
                v.bh = v.cell.nh;
            } else if v.kind == Kind::Arrow {
                v.bw = if v.cell.span > 1.0 {
                    v.cell.w
                } else {
                    v.cell.nw
                };
                v.bh = v.cell.nh;
            } else {
                v.bw = v.cell.w;
                v.bh = if v.kind == Kind::Leaf && wide(v.shape) {
                    v.cell.nh
                } else {
                    v.cell.h
                };
            }
            if v.kind == Kind::Leaf && v.shape == "cyl" {
                v.dy = jmin(12.0, jmax(6.0, v.bw / 14.0)) / 2.0;
            }
            if v.kind == Kind::Leaf || v.kind == Kind::Arrow {
                self.obstacles.push(k);
            } else if v.cell.head > 0.0 {
                self.titles.push(Ob {
                    view: None,
                    x: v.cell.x,
                    y: v.cell.y - v.cell.h / 2.0 + (v.cell.head + self.gap) / 2.0 + 2.0,
                    bw: v.label.width,
                    bh: v.label.height,
                });
            }
        }
        Ok(content)
    }

    fn key(&self, s: usize, t: usize) -> usize {
        s.min(t) * self.views.len() + s.max(t)
    }

    /// Ports `route`.
    fn route(&mut self) {
        let mut seen: BTreeMap<usize, usize> = BTreeMap::new();
        self.routes.clear();
        self.stacks.clear();
        let check = if self.check {
            Some(obs_of(&self.views, &self.obstacles))
        } else {
            None
        };
        for i in 0..self.drawn.len() {
            let (s, t) = self.ends[i];
            let key = self.key(s, t);
            let total = self.pairs.get(&key).copied().unwrap_or(1);
            let k = seen.get(&key).copied().unwrap_or(0);
            seen.insert(key, k + 1);
            let edge = &self.model.arena[self.drawn[i]];
            let label = layout_label(
                edge.label.as_deref(),
                false,
                self.measurer,
                self.edge_size,
                WRAP,
                0,
                self.icons,
            );
            let points: Vec<f64>;
            let lx: f64;
            let ly: f64;
            if s == t {
                let [px, py, r, _, _] = loop_arc(&self.views[s], k + 1);
                points = Vec::new();
                lx = px + r + 6.0 + label.width / 2.0;
                ly = py - r / 2.0;
            } else {
                let step = if self.labelled.contains(&key) {
                    jround(self.edge_size * 1.5) + 2.0
                } else {
                    14.0
                };
                points = route_edge(
                    &self.views,
                    s,
                    t,
                    (k as f64 - (total as f64 - 1.0) / 2.0) * step,
                    check.as_deref(),
                );
                let n = points.len();
                // The label goes on the longer leg.
                let first_leg = (points[2] - points[0]).abs() + (points[3] - points[1]).abs();
                let last_leg =
                    (points[n - 2] - points[n - 4]).abs() + (points[n - 1] - points[n - 3]).abs();
                let at = if n == 6 && last_leg > first_leg { 2 } else { 0 };
                let spot = if label.width > 0.0 {
                    label_spot(
                        &points,
                        at,
                        label.width + 8.0,
                        label.height,
                        s,
                        t,
                        check.as_deref(),
                    )
                } else {
                    0.5
                };
                lx = points[at] + (points[at + 2] - points[at]) * spot;
                ly = points[at + 1] + (points[at + 3] - points[at + 1]) * spot;
                if label.width > 0.0 && total > 1 {
                    self.legs[i] = points[at..at + 4].to_vec();
                    let skey = js::num_str(key as f64);
                    match self.stacks.get_mut(&skey) {
                        Some(stack) => stack.push(i),
                        None => self.stacks.insert(&skey, alloc::vec![i]),
                    }
                }
            }
            self.routes.push(Route {
                edge: self.drawn[i],
                points,
                looped: if s == t { k + 1 } else { 0 },
                label,
                lx,
                ly,
            });
        }
    }

    /// Ports `settle`.
    fn settle(&mut self, grow: bool, tight: bool) -> bool {
        self.route();
        let blocked: Option<Vec<Ob>> = if self.check {
            let mut list = obs_of(&self.views, &self.obstacles);
            list.extend_from_slice(&self.titles);
            Some(list)
        } else {
            None
        };
        let mut base: BTreeMap<(usize, bool), Vec<f64>> = BTreeMap::new();
        let mut grown = false;
        let stacks: Vec<Vec<usize>> = self.stacks.values().cloned().collect();
        for stack in stacks {
            if stack.len() < 2 {
                continue;
            }
            let (s, t) = self.ends[stack[0]];
            let short = stack_labels(
                &stack,
                &mut self.routes,
                &self.legs,
                s,
                t,
                blocked.as_deref(),
            );
            let vertical = self.legs[stack[0]][0] == self.legs[stack[0]][2];
            // Keep columns narrow to fit the host in tight layouts.
            if grow
                && short > 0.0
                && (vertical || !tight)
                && widen(
                    &mut self.views,
                    &self.parents,
                    s,
                    t,
                    vertical,
                    short.ceil(),
                    &mut base,
                )
            {
                grown = true;
            }
        }
        grown
    }

    /// Ports `overlap`.
    fn overlap(&self) -> f64 {
        let mut area = 0.0;
        for i in 0..self.routes.len() {
            let p = &self.routes[i];
            for j in i + 1..self.routes.len() {
                let q = &self.routes[j];
                let pw = (p.label.width + 8.0) / 2.0;
                let qw = (q.label.width + 8.0) / 2.0;
                let w = jmin(p.lx + pw, q.lx + qw) - jmax(p.lx - pw, q.lx - qw);
                let h = jmin(p.ly + p.label.height / 2.0, q.ly + q.label.height / 2.0)
                    - jmax(p.ly - p.label.height / 2.0, q.ly - q.label.height / 2.0);
                if p.label.width > 0.0 && q.label.width > 0.0 && w > 0.0 && h > 0.0 {
                    area += w * h;
                }
            }
        }
        area
    }

    /// Ports `bound`: the drawing's extent, as min x, min y, max x, max y.
    fn bound(&self, content: (f64, f64)) -> [f64; 4] {
        let mut b = [0.0, 0.0, content.0, content.1];
        let mut grow = |x: f64, y: f64, w: f64, h: f64| {
            if x - w / 2.0 < b[0] {
                b[0] = x - w / 2.0;
            }
            if x + w / 2.0 > b[2] {
                b[2] = x + w / 2.0;
            }
            if y - h / 2.0 < b[1] {
                b[1] = y - h / 2.0;
            }
            if y + h / 2.0 > b[3] {
                b[3] = y + h / 2.0;
            }
        };
        for i in 0..self.routes.len() {
            let route = &self.routes[i];
            if route.looped > 0 {
                let [px, py, r, _, _] = loop_arc(&self.views[self.ends[i].0], route.looped);
                grow(px, py, 2.0 * r + 2.0, 2.0 * r + 2.0);
            }
            if route.label.width > 0.0 {
                grow(
                    route.lx,
                    route.ly,
                    route.label.width + 8.0,
                    route.label.height,
                );
            }
        }
        b
    }
}

/// Ports `styleOf`.
fn style_of(model: &BlockDb, block: &Block) -> ResolvedStyle {
    let mut declarations: Vec<String> = Vec::new();
    // A style list is split on commas only, so one entry can still hold several declarations.
    let mut add = |styles: &[String]| {
        for s in styles {
            for part in s.split(';') {
                declarations.push(part.into());
            }
        }
    };
    let default = [String::from("default")];
    let names: &[String] = match &block.classes {
        Some(c) if !c.is_empty() => c,
        _ => &default,
    };
    for name in names {
        if let Some(def) = model.classes.get(name) {
            add(&def.styles);
        }
    }
    if let Some(styles) = &block.styles {
        add(styles);
    }
    resolve_style(&declarations)
}

/// Ports `draw`.
fn draw(
    model: &BlockDb,
    config: &Value,
    options: &RenderOptions,
    palette_style: &str,
    tight: bool,
) -> Result<Rendered, PeleError> {
    let size = options.font_size.unwrap_or(16.0);
    let edge_size = jround(size * 0.875);
    let measurer: &dyn Measure = options.measurer.unwrap_or(&METRICS);
    let pad = options.padding.unwrap_or(8.0);
    let icons = options.icons;
    let configured = config.get("block").and_then(|b| b.get("padding"));
    let gap = jmin(
        match configured {
            Some(Value::Num(v)) if *v >= 0.0 => jmin(*v, 200.0),
            _ => GAP,
        },
        if tight { 6.0 } else { f64::INFINITY },
    );
    let wrap = if tight { 80.0 } else { WRAP };

    let view = |b: usize, index: usize| -> View {
        let block = &model.arena[b];
        let kind_str = block.kind.as_deref();
        let st = if kind_str == Some("space") {
            ResolvedStyle::default()
        } else {
            style_of(model, block)
        };
        let text_style =
            (if st.bold { style::BOLD } else { 0 }) | (if st.italic { style::ITALIC } else { 0 });
        let font_size = st.font_size.unwrap_or(size);
        let mut kind = Kind::Leaf;
        let mut shape = "";
        let label: Label;
        let mut nw = 0.0;
        let mut nh = 0.0;
        let mut head = 0.0;
        let mut mask = 0;
        let mut dx = 0.0;
        let mut dy = 0.0;
        if kind_str == Some("space") {
            kind = Kind::Space;
            label = layout_label(None, false, measurer, font_size, wrap, 0, icons);
        } else if kind_str == Some("composite") {
            kind = Kind::Composite;
            label = layout_label(
                block.label.as_deref(),
                false,
                measurer,
                font_size,
                if tight { title_room(options) } else { 4000.0 },
                text_style,
                icons,
            );
            head = if label.height > 0.0 {
                label.height + 4.0
            } else {
                0.0
            };
            nw = label.width + 2.0 * gap;
            nh = head + 2.0 * gap;
        } else if kind_str == Some("block_arrow") {
            kind = Kind::Arrow;
            label = layout_label(
                block.label.as_deref(),
                false,
                measurer,
                font_size,
                wrap,
                text_style,
                icons,
            );
            let blank = !label
                .lines
                .iter()
                .flatten()
                .any(|part| part.icon.is_some() || !js::trim(&part.text).is_empty());
            mask = arrow_mask(block.directions.as_deref());
            let s = if blank {
                arrow_size(mask, 0.0, 0.0)
            } else {
                arrow_size(mask, label.width, label.height)
            };
            nw = s.w;
            nh = s.h;
            dx = s.dx;
            dy = s.dy;
        } else {
            shape = shape_of(kind_str.unwrap_or("")).unwrap_or("rect");
            label = layout_label(
                block.label.as_deref(),
                false,
                measurer,
                font_size,
                wrap,
                text_style,
                icons,
            );
            let s = shape_size(shape, label.width, label.height);
            nw = s.w;
            // A cylinder's caps deepen as it widens, so it is sized for the deepest ones.
            nh = if shape == "cyl" {
                jmax(s.h, label.height + 40.0)
            } else {
                s.h
            };
            dy = s.dy;
        }
        View {
            cell: Cell {
                children: Vec::new(),
                columns: if kind == Kind::Composite {
                    block.columns.unwrap_or(-1.0)
                } else {
                    -1.0
                },
                span: if kind == Kind::Space {
                    1.0
                } else {
                    block.width_in_columns.unwrap_or(1.0)
                },
                space: kind == Kind::Space,
                head,
                nw,
                nh,
                ..Cell::default()
            },
            block: b,
            kind,
            index,
            shape,
            label,
            style: st,
            mask,
            dx,
            dy,
            bw: 0.0,
            bh: 0.0,
        }
    };

    let mut root = view(model.root, 0);
    root.cell.nw = 0.0;
    root.cell.nh = 0.0;
    let mut views: Vec<View> = alloc::vec![root];
    let mut parents: Vec<Option<usize>> = alloc::vec![None];
    let mut by_id: BTreeMap<String, usize> = BTreeMap::new();
    let mut i = 0;
    while i < views.len() {
        if views[i].kind == Kind::Composite {
            let children = model.arena[views[i].block]
                .children
                .clone()
                .unwrap_or_default();
            for child in children {
                let index = views.len();
                let v = view(child, index);
                views[i].cell.children.push(index);
                parents.push(Some(i));
                let space = v.kind == Kind::Space;
                views.push(v);
                if !space {
                    by_id.insert(model.arena[child].id.clone().unwrap_or_default(), index);
                }
            }
        }
        i += 1;
    }

    // Layout keeps the widest a container has been, so each run starts from the widths it needs.
    let widths: Vec<f64> = views.iter().map(|v| v.cell.nw).collect();
    let mut scene = Scene {
        views,
        parents,
        widths,
        obstacles: Vec::new(),
        titles: Vec::new(),
        gap,
        model,
        ends: Vec::new(),
        drawn: Vec::new(),
        pairs: BTreeMap::new(),
        labelled: alloc::collections::BTreeSet::new(),
        check: false,
        routes: Vec::new(),
        stacks: OrderedMap::new(),
        legs: Vec::new(),
        edge_size,
        measurer,
        icons,
    };
    let mut content = scene.place()?;

    // Edges between the same two blocks are spread apart instead of drawn on top of each other.
    for &e in &model.edges {
        let edge = &model.arena[e];
        let s = by_id.get(edge.start.as_deref().unwrap_or("")).copied();
        let t = by_id.get(edge.end.as_deref().unwrap_or("")).copied();
        let (Some(s), Some(t)) = (s, t) else {
            continue;
        };
        let key = scene.key(s, t);
        *scene.pairs.entry(key).or_insert(0) += 1;
        if edge.label.as_deref().is_some_and(|l| !l.is_empty()) {
            scene.labelled.insert(key);
        }
        scene.ends.push((s, t));
        scene.drawn.push(e);
    }
    scene.legs = alloc::vec![Vec::new(); scene.drawn.len()];
    scene.check = (scene.drawn.len() * scene.obstacles.len()) as f64 <= OBSTACLE_BUDGET;

    let n_drawn = scene.drawn.len() as f64;
    if scene.settle(n_drawn * n_drawn <= OBSTACLE_BUDGET, tight) {
        let before = scene.overlap();
        let b = scene.bound(content);
        let narrow = b[2] - b[0];
        // A container stretched by its row gives some of a new gap back, so the gap may take a few rounds.
        let mut grown = true;
        let mut round = 0;
        while round < 3 && grown {
            content = scene.place()?;
            grown = scene.settle(round < 2, tight);
            round += 1;
        }
        // The drawing only grows if that leaves less of the labels covering each other, and within maxWidth.
        let b = scene.bound(content);
        let wide_w = b[2] - b[0];
        let room = match options.max_width {
            Some(m) if m > 0.0 => m,
            _ => f64::INFINITY,
        };
        if scene.overlap() > before - 1.0 || (wide_w > narrow && (wide_w + 2.0 * pad).ceil() > room)
        {
            for v in scene.views.iter_mut() {
                v.cell.row_gaps = Vec::new();
                v.cell.col_gaps = Vec::new();
            }
            content = scene.place()?;
            scene.settle(false, tight);
        }
    }

    let [min_x, min_y, max_x, max_y] = scene.bound(content);

    let title = title_label(
        model.title.as_deref(),
        measurer,
        size,
        if tight { title_room(options) } else { 4000.0 },
    );
    let title_height = if title.height > 0.0 {
        title.height + 12.0
    } else {
        0.0
    };
    let inner = jmax(max_x - min_x, title.width);
    let ox = pad - min_x + (inner - (max_x - min_x)) / 2.0;
    let oy = pad + title_height - min_y;
    for v in scene.views.iter_mut() {
        v.cell.x += ox;
        v.cell.y += oy;
    }
    let width = (inner + 2.0 * pad).ceil();
    let height = (max_y - min_y + title_height + 2.0 * pad).ceil();

    let mut clusters = String::new();
    let mut nodes = String::new();
    let mut fills: Vec<Fill> = Vec::new();
    for v in scene.views.iter().skip(1) {
        if v.kind == Kind::Space {
            continue;
        }
        let block = &model.arena[v.block];
        let classes = class_names(&block.classes.clone().unwrap_or_default().join(" "));
        let id = esc_text(block.id.as_deref().unwrap_or(""));
        let c = &v.cell;
        if v.kind == Kind::Composite {
            let top = c.y - c.h / 2.0;
            if paints(&v.style.shape) {
                fills.push(Fill {
                    bx: [c.x - c.w / 2.0, top, c.x + c.w / 2.0, top + c.h],
                    shape: v.style.shape.clone(),
                });
            }
            clusters.push_str(&format!(
                "<g class=\"pele-cluster{}\" data-id=\"{}\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"var(--_a)\"{} stroke=\"var(--_b)\"{}/>{}</g>",
                classes,
                id,
                num(c.x - c.w / 2.0),
                num(top),
                num(c.w),
                num(c.h),
                if sets_fill(&v.style.shape) {
                    ""
                } else {
                    " fill-opacity=\"0.5\""
                },
                rounded(&v.style.shape),
                label_svg(
                    &v.label,
                    c.x,
                    top + (c.head + gap) / 2.0 + 2.0,
                    &format!(
                        " class=\"pele-cluster-label\"{}{}",
                        ink_fill(&v.style.shape, &v.style.text, Some("var(--_m)")),
                        v.style.text
                    ),
                    icons
                )
            ));
            continue;
        }
        let arrow = v.kind == Kind::Arrow;
        let mut label = v.label.clone();
        // Reuse a wider cell without changing the node size or the routes computed from it.
        if c.span > 1.0
            && v.label.width > 0.0
            && v.label.lines.len() > 1
            && v.bw > c.nw
            && (arrow || !fixed(v.shape))
        {
            let text_style = (if v.style.bold { style::BOLD } else { 0 })
                | (if v.style.italic { style::ITALIC } else { 0 });
            let room = v.label.width + (v.bw - c.nw);
            label = layout_label(
                block.label.as_deref(),
                false,
                measurer,
                v.label.size,
                room,
                text_style,
                icons,
            );
        }
        let ink = if arrow || shape_filled(v.shape) {
            ink_fill(&v.style.shape, &v.style.text, None)
        } else {
            String::new()
        };
        nodes.push_str(&format!(
            "<g class=\"pele-node pele-shape-{}{}\" data-id=\"{}\" transform=\"translate({},{})\">{}{}</g>",
            if arrow { "block-arrow" } else { v.shape },
            classes,
            id,
            num(c.x),
            num(c.y),
            if arrow {
                draw_arrow(v.mask, v.bw, v.bh, &format!("{}{}", SHAPE_ATTRS, v.style.shape))
            } else {
                draw_shape(
                    v.shape,
                    v.bw,
                    v.bh,
                    &format!("{}{}", SHAPE_ATTRS, v.style.shape),
                    &format!("{}{}", LINE_ATTRS, v.style.line),
                )
            },
            label_svg(
                &label,
                v.dx,
                v.dy,
                &format!(" class=\"pele-label\"{}{}", ink, v.style.text),
                icons
            )
        ));
    }

    let mut casings = String::new();
    let mut edges = String::new();
    let mut labels = String::new();
    for i in 0..scene.routes.len() {
        let route = &scene.routes[i];
        let edge = &model.arena[route.edge];
        let start_type = edge.arrow_type_start.as_deref().unwrap_or("");
        let end_type = edge.arrow_type_end.as_deref().unwrap_or("");
        let path = if route.looped > 0 {
            loop_path(
                &scene.views[scene.ends[i].0],
                route.looped,
                marker_trim(start_type),
                marker_trim(end_type),
            )
        } else {
            let mut points = route.points.clone();
            let mut k = 0;
            while k < points.len() {
                points[k] += ox;
                points[k + 1] += oy;
                k += 2;
            }
            trace(&points, marker_trim(start_type), marker_trim(end_type))
        };
        let thick = edge.thickness.as_deref() == Some("thick");
        let over = over_fills(
            &path.d,
            &[path.sx, path.sy, path.ex, path.ey],
            &fills,
            "",
            None,
        );
        if over.as_deref() == Some("") {
            casings.push_str(&edge_casing(&path, start_type, end_type, "", thick));
        }
        let over_s = over.as_deref().unwrap_or("");
        let color = if over_s.is_empty() {
            "var(--_l)"
        } else {
            over_s
        };
        let id = esc_text(edge.id.as_deref().unwrap_or(""));
        edges.push_str(&format!(
            "<g class=\"pele-edge\" data-id=\"{}\"{}><path d=\"{}\"{}{}/>{}{}</g>",
            id,
            if over_s.is_empty() {
                String::new()
            } else {
                format!(" stroke=\"{}\"", over_s)
            },
            path.d,
            if thick { " stroke-width=\"2.5\"" } else { "" },
            if edge.pattern.as_deref() == Some("dotted") {
                " stroke-dasharray=\"3 4\""
            } else {
                ""
            },
            marker(start_type, path.sx, path.sy, path.sdx, path.sdy, color, "none"),
            marker(end_type, path.ex, path.ey, path.edx, path.edy, color, "none")
        ));
        if route.label.width > 0.0 {
            let x = route.lx + ox;
            let y = route.ly + oy;
            labels.push_str(&edge_label_svg(
                edge.id.as_deref().unwrap_or(""),
                &route.label,
                x,
                y,
                "",
                icons,
            ));
        }
    }

    let mut content_out = label_svg(
        &title,
        width / 2.0,
        pad + title.height / 2.0,
        " class=\"pele-title\" font-weight=\"var(--_tw)\"",
        None,
    );
    if !clusters.is_empty() {
        content_out.push_str(&format!("<g class=\"pele-clusters\">{}</g>", clusters));
    }
    content_out.push_str(&casings_svg(&casings));
    if !edges.is_empty() {
        content_out.push_str(&format!(
            "<g class=\"pele-edges\" fill=\"none\" stroke=\"var(--_l)\" stroke-linecap=\"round\">{}</g>",
            edges
        ));
    }
    content_out.push_str(&format!("<g class=\"pele-nodes\">{}</g>", nodes));
    if !labels.is_empty() {
        content_out.push_str(&format!(
            "<g class=\"pele-edge-labels\" font-size=\"{}\">{}</g>",
            js::num_str(edge_size),
            labels
        ));
    }
    let svg = svg_document(
        "block",
        width,
        height,
        size,
        options,
        palette_style,
        &Accessible::default(),
        &content_out,
    );
    Ok(Rendered {
        svg,
        width,
        height,
        links: Vec::new(),
    })
}
