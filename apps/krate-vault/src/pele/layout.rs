//! Pele's layout engine (src/layout/compound.ts and components.ts in Pele
//! 1.13.0), ported function by function; the layered layout each level is
//! drawn with is `super::layered` (layered.ts).
//!
//! `compound_layout` is the entry Pele's diagrams call (`compoundLayout`,
//! named `On` in the build Obsidian ships). It lays out a graph whose nodes
//! may be nested in groups: each group is laid out on its own, in its own
//! direction, and then placed in its parent as a single box. An edge that
//! crosses a group's border passes through a port on the side facing its
//! other end.
//!
//! The test at the bottom runs every recorded call of Pele's own layout
//! (evidence/benchmarks/obsidian-vs-krate/pele-oracle) through this port and
//! wants every number back within 0.01.

// The port keeps the TypeScript's index loops, so that it reads against
// the source line by line.
#![allow(clippy::needless_range_loop)]

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

pub use super::layered::{
    ease, layered, ledge, lnode, orient, rank, Groups, Kind, LEdge, LNode, LayeredOptions,
    LayeredResult,
};
use super::layered::{jhypot, jmax, jmin, jround, or, sort_by};

/// Ports `type Dir`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    /// `TB`, top to bottom.
    Tb,
    /// `BT`, bottom to top.
    Bt,
    /// `LR`, left to right.
    Lr,
    /// `RL`, right to left.
    Rl,
}

impl Dir {
    fn vertical(self) -> bool {
        matches!(self, Dir::Tb | Dir::Bt)
    }
}

/// Ports `direction`: the direction a diagram names, or top to bottom for
/// anything else.
pub fn direction(dir: Option<&str>) -> Dir {
    match dir {
        Some("BT") => Dir::Bt,
        Some("LR") => Dir::Lr,
        Some("RL") => Dir::Rl,
        _ => Dir::Tb,
    }
}

/// Ports `interface CNode`. `parent` is -1 for none.
#[derive(Debug, Clone, PartialEq)]
pub struct CNode {
    pub w: f64,
    pub h: f64,
    pub parent: i32,
    pub is_group: bool,
    pub dir: Option<Dir>,
    pub pad_x: f64,
    pub pad_top: f64,
    pub pad_bottom: f64,
    pub min_w: f64,
    /// Declaration order; a group takes the smallest value among its members.
    pub seq: f64,
    /// How much of a side its edges may spread along, measured across the flow; 0 keeps them at its middle.
    pub span: f64,
    pub x: f64,
    pub y: f64,
}

/// Ports `interface CEdge`.
#[derive(Debug, Clone, PartialEq)]
pub struct CEdge {
    pub src: usize,
    pub dst: usize,
    pub minlen: f64,
    pub label_w: f64,
    pub label_h: f64,
    /// x, y, axis triples; axis is 0 where the edge runs vertically into the point, 1 horizontally.
    pub route: Vec<f64>,
    pub label_x: f64,
    pub label_y: f64,
}

/// Ports `interface CompoundResult`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

// `const enum Side`.
const TOP: u8 = 0;
const RIGHT: u8 = 1;
const BOTTOM: u8 = 2;
const LEFT: u8 = 3;

const MAX_PORTS: usize = 40000;
const MAX_ORDERED_PORTS: usize = 400;
const MAX_ORDERED_EDGES: usize = 500;
const FLAT_SLACK: f64 = 1.5;
const FLAT_SPARE: f64 = 4.0;

const FAN: f64 = 24.0;
const IN_LINE: f64 = 0.5;
const CURVE_PIECES: usize = 3;
const CORNER_PIECES: usize = 2;
const STEP_PIECES: usize = 3; // Math.max(3, CURVE_PIECES, CORNER_PIECES + 1)
const HANDLE: f64 = 0.62;
const MAX_INSERT: usize = 128;

/// Ports `const enum Style`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Curved,
    Stepped,
    Linear,
}

/// Ports `styleOf`.
pub fn style_of(opt: &LayeredOptions) -> Style {
    if opt.tracks {
        Style::Stepped
    } else if opt.straight {
        Style::Linear
    } else {
        Style::Curved
    }
}

/// The segments `routeCrossings` cuts the routes into (its typed arrays and
/// the `add` closure's state).
struct Segs {
    ax: Vec<f64>,
    ay: Vec<f64>,
    bx: Vec<f64>,
    by: Vec<f64>,
    low: Vec<f64>,
    high: Vec<f64>,
    owner: Vec<usize>,
    // Endpoint flags: 1 for the first segment, 2 for the last.
    ends: Vec<u8>,
    count: usize,
    top: f64,
    bottom: f64,
    from_x: f64,
    from_y: f64,
    to_x: f64,
    to_y: f64,
}

impl Segs {
    /// `add` inside `routeCrossings`.
    #[allow(clippy::too_many_arguments)]
    fn add(&mut self, ei: usize, part: u8, mut x0: f64, mut y0: f64, x1: f64, y1: f64) {
        let on = x0 == self.to_x
            && y0 == self.to_y
            && ((y1 == y0 && self.from_y == y0) || (x1 == x0 && self.from_x == x0));
        if on
            && self.count > 0
            && self.owner[self.count - 1] == ei
            && self.ends[self.count - 1] == part
            && (x0 - self.from_x) * (x1 - x0) + (y0 - self.from_y) * (y1 - y0) > 0.0
        {
            self.count -= 1;
            x0 = self.from_x;
            y0 = self.from_y;
        }
        self.from_x = x0;
        self.from_y = y0;
        self.to_x = x1;
        self.to_y = y1;
        let c = self.count;
        self.ends[c] = part;
        self.ax[c] = jmin(x0, x1);
        self.bx[c] = jmax(x0, x1);
        self.ay[c] = if x0 <= x1 { y0 } else { y1 };
        self.by[c] = if x0 <= x1 { y1 } else { y0 };
        self.low[c] = jmin(y0, y1);
        self.high[c] = jmax(y0, y1);
        if self.low[c] < self.top {
            self.top = self.low[c];
        }
        if self.high[c] > self.bottom {
            self.bottom = self.high[c];
        }
        self.owner[c] = ei;
        self.count += 1;
    }
}

/// Ports `routeCrossings`: how many times the drawn routes cross, leaving
/// out fans at shared nodes.
pub fn route_crossings(nodes: &[CNode], edges: &[CEdge], style: Style) -> usize {
    let mut size = 0usize;
    for e in edges {
        if e.src != e.dst && e.route.len() > 3 {
            size += (e.route.len() / 3 - 1) * STEP_PIECES;
        }
    }
    let mut sg = Segs {
        ax: alloc::vec![0.0; size],
        ay: alloc::vec![0.0; size],
        bx: alloc::vec![0.0; size],
        by: alloc::vec![0.0; size],
        low: alloc::vec![0.0; size],
        high: alloc::vec![0.0; size],
        owner: alloc::vec![0; size],
        ends: alloc::vec![0; size],
        count: 0,
        top: f64::INFINITY,
        bottom: f64::NEG_INFINITY,
        from_x: f64::NAN,
        from_y: f64::NAN,
        to_x: f64::NAN,
        to_y: f64::NAN,
    };
    for (ei, e) in edges.iter().enumerate() {
        if e.src == e.dst {
            continue;
        }
        let r = &e.route;
        let rl = r.len();
        if rl < 3 {
            continue;
        }
        let aligned =
            |k: usize| (r[k] - r[k - 3]).abs() < 0.01 || (r[k + 1] - r[k - 2]).abs() < 0.01;
        let length = |k: usize| (r[k] - r[k - 3]).abs() + (r[k + 1] - r[k - 2]).abs();
        let turns = |k: usize| {
            style == Style::Curved
                && k > 0
                && k + 3 < rl
                && aligned(k)
                && aligned(k + 3)
                && length(k) >= 0.01
                && length(k + 3) >= 0.01
                && (((r[k] - r[k - 3]).abs() < 0.01) != ((r[k + 3] - r[k]).abs() < 0.01))
        };
        let mut at_x = r[0];
        let mut at_y = r[1];
        let mut k = 3;
        while k < rl {
            let part = (if k == 3 { 1 } else { 0 }) | (if k == rl - 3 { 2 } else { 0 });
            let x0 = at_x;
            let y0 = at_y;
            let x1 = r[k];
            let y1 = r[k + 1];
            at_x = x1;
            at_y = y1;
            if aligned(k) || style == Style::Linear {
                if !turns(k) {
                    sg.add(ei, part, x0, y0, x1, y1);
                    k += 3;
                    continue;
                }
                // Adjacent corners share a leg, so each gets half.
                let before = if turns(k - 3) {
                    length(k) / 2.0
                } else {
                    length(k)
                };
                let after = if turns(k + 3) {
                    length(k + 3) / 2.0
                } else {
                    length(k + 3)
                };
                let ux = (x1 - r[k - 3]) / length(k);
                let uy = (y1 - r[k - 2]) / length(k);
                let vx = (r[k + 3] - x1) / length(k + 3);
                let vy = (r[k + 4] - y1) / length(k + 3);
                let sx = x1 - ux * before;
                let sy = y1 - uy * before;
                sg.add(ei, part, x0, y0, sx, sy);
                let c1x = x1 - ux * before * (1.0 - HANDLE);
                let c1y = y1 - uy * before * (1.0 - HANDLE);
                let c2x = x1 + vx * after * (1.0 - HANDLE);
                let c2y = y1 + vy * after * (1.0 - HANDLE);
                at_x = x1 + vx * after;
                at_y = y1 + vy * after;
                let turn = part | (if k + 3 == rl - 3 { 2 } else { 0 });
                let mut px = sx;
                let mut py = sy;
                for i in 1..=CORNER_PIECES {
                    let t = i as f64 / CORNER_PIECES as f64;
                    let u = 1.0 - t;
                    let x = u * u * u * sx
                        + 3.0 * u * u * t * c1x
                        + 3.0 * u * t * t * c2x
                        + t * t * t * at_x;
                    let y = u * u * u * sy
                        + 3.0 * u * u * t * c1y
                        + 3.0 * u * t * t * c2y
                        + t * t * t * at_y;
                    sg.add(ei, turn, px, py, x, y);
                    px = x;
                    py = y;
                }
                k += 3;
                continue;
            }
            if style == Style::Stepped {
                let down = r[k + 2] == 0.0;
                let mx = if down { x0 } else { (x0 + x1) / 2.0 };
                let my = if down { (y0 + y1) / 2.0 } else { y0 };
                sg.add(ei, part, x0, y0, mx, my);
                sg.add(
                    ei,
                    part,
                    mx,
                    my,
                    if down { x1 } else { mx },
                    if down { my } else { y1 },
                );
                sg.add(
                    ei,
                    part,
                    if down { x1 } else { mx },
                    if down { my } else { y1 },
                    x1,
                    y1,
                );
                k += 3;
                continue;
            }
            // Approximate the rendered S-curve with straight segments.
            let down = r[k + 2] == 0.0;
            let c = if down {
                ease(x1 - x0, y1 - y0)
            } else {
                ease(y1 - y0, x1 - x0)
            };
            let mut px = x0;
            let mut py = y0;
            for i in 1..=CURVE_PIECES {
                let t = i as f64 / CURVE_PIECES as f64;
                let u = 1.0 - t;
                let across = t * t * (3.0 - 2.0 * t);
                let along = 3.0 * c * t * u * u + 3.0 * (1.0 - c) * t * t * u + t * t * t;
                let x = x0 + (x1 - x0) * (if down { across } else { along });
                let y = y0 + (y1 - y0) * (if down { along } else { across });
                sg.add(ei, part, px, py, x, y);
                px = x;
                py = y;
            }
            k += 3;
        }
    }
    let count = sg.count;
    let (top, bottom) = (sg.top, sg.bottom);
    let Segs {
        ax,
        ay,
        bx,
        by,
        low,
        high,
        owner,
        ends,
        ..
    } = sg;
    // Count each pair only in the first band shared by both segments.
    let bands = if bottom > top {
        jround((count as f64).sqrt()) as usize
    } else {
        1
    };
    let scale = if bottom > top {
        bands as f64 / (bottom - top)
    } else {
        0.0
    };
    let band_of =
        |y: f64| -> usize { jmin((bands - 1) as f64, ((y - top) * scale).floor()) as usize };
    let mut first = alloc::vec![0usize; count];
    let mut band_start = alloc::vec![0usize; bands + 1];
    for p in 0..count {
        first[p] = band_of(low[p]);
        let last = band_of(high[p]);
        for b in first[p]..=last {
            band_start[b + 1] += 1;
        }
    }
    for b in 0..bands {
        band_start[b + 1] += band_start[b];
    }
    let mut fill = band_start[..bands].to_vec();
    let mut in_band = alloc::vec![0usize; band_start[bands]];
    for p in 0..count {
        let last = band_of(high[p]);
        for b in first[p]..=last {
            in_band[fill[b]] = p;
            fill[b] += 1;
        }
    }
    // Edges merging into a shared endpoint run do not count as crossings.
    let mut runs = alloc::vec![0.0f64; edges.len() * 8];
    let in_line = |r: &[f64], a: usize, b: usize, c: usize| -> bool {
        ((r[a] - r[b]).abs() < IN_LINE && (r[a] - r[c]).abs() < IN_LINE)
            || ((r[a + 1] - r[b + 1]).abs() < IN_LINE && (r[a + 1] - r[c + 1]).abs() < IN_LINE)
    };
    for (ei, e) in edges.iter().enumerate() {
        let r = &e.route;
        if r.len() < 6 {
            continue;
        }
        let mut k = 3;
        while k + 3 < r.len() && in_line(r, 0, k, k + 3) {
            k += 3;
        }
        let last = r.len() - 3;
        let mut j = last - 3;
        while j >= 3 && in_line(r, last, j, j - 3) {
            j -= 3;
        }
        runs[ei * 8..ei * 8 + 8].copy_from_slice(&[
            r[0],
            r[1],
            r[k],
            r[k + 1],
            r[j],
            r[j + 1],
            r[last],
            r[last + 1],
        ]);
    }
    let on_run = |ei: usize, end: u8, x: f64, y: f64| -> bool {
        let o = ei * 8 + if end == 1 { 0 } else { 4 };
        let x0 = runs[o];
        let y0 = runs[o + 1];
        let dx = runs[o + 2] - x0;
        let dy = runs[o + 3] - y0;
        let length = dx * dx + dy * dy;
        let t = if length > 0.0 {
            jmax(0.0, jmin(1.0, ((x - x0) * dx + (y - y0) * dy) / length))
        } else {
            0.0
        };
        jhypot(x - x0 - t * dx, y - y0 - t * dy) < IN_LINE
    };
    let mut total = 0usize;
    for band in 0..bands {
        let bs = band_start[band];
        let band_end = band_start[band + 1];
        if band_end - bs > MAX_INSERT {
            sort_by(&mut in_band[bs..band_end], |p, q| {
                or(ax[p] - ax[q], p as f64 - q as f64)
            });
        } else {
            for i in bs + 1..band_end {
                let p = in_band[i];
                let mut j = i;
                while j > bs && ax[in_band[j - 1]] > ax[p] {
                    in_band[j] = in_band[j - 1];
                    j -= 1;
                }
                in_band[j] = p;
            }
        }
        for s in bs..band_end {
            let p = in_band[s];
            let dx = bx[p] - ax[p];
            let dy = by[p] - ay[p];
            let here = first[p] == band;
            for &q in &in_band[s + 1..band_end] {
                if ax[q] > bx[p] {
                    break;
                }
                if high[q] < low[p]
                    || low[q] > high[p]
                    || owner[p] == owner[q]
                    || (!here && first[q] != band)
                {
                    continue;
                }
                let d1 = dx * (ay[q] - ay[p]) - dy * (ax[q] - ax[p]);
                let d2 = dx * (by[q] - ay[p]) - dy * (bx[q] - ax[p]);
                // Treat points on the line as one side to count segment-boundary crossings once.
                if (d1 > 0.0) == (d2 > 0.0) {
                    continue;
                }
                let ex = bx[q] - ax[q];
                let ey = by[q] - ay[q];
                let d3 = ex * (ay[p] - ay[q]) - ey * (ax[p] - ax[q]);
                let d4 = ex * (by[p] - ay[q]) - ey * (bx[p] - ax[q]);
                if (d3 > 0.0) == (d4 > 0.0) {
                    continue;
                }
                let ep = &edges[owner[p]];
                let eq = &edges[owner[q]];
                let at = d3 / (d3 - d4);
                let x = ax[p] + at * dx;
                let y = ay[p] + at * dy;
                // Exclude fan-in and fan-out at shared nodes.
                let fan = |v: usize, pe: u8, qe: u8| -> bool {
                    ((ends[p] & pe) != 0 && (ends[q] & qe) != 0)
                        || ((x - nodes[v].x).abs() < nodes[v].w / 2.0 + FAN
                            && (y - nodes[v].y).abs() < nodes[v].h / 2.0 + FAN)
                        || (on_run(owner[p], pe, x, y) && on_run(owner[q], qe, x, y))
                };
                if ep.src == eq.src && fan(ep.src, 1, 1) {
                    continue;
                }
                if ep.src == eq.dst && fan(ep.src, 1, 2) {
                    continue;
                }
                if ep.dst == eq.src && fan(ep.dst, 2, 1) {
                    continue;
                }
                if ep.dst == eq.dst && fan(ep.dst, 2, 2) {
                    continue;
                }
                total += 1;
            }
        }
    }
    total
}

/// Ports `inheritCrossedDirections`: as Mermaid does, groups crossed by
/// edges inherit their parent's direction.
pub fn inherit_crossed_directions(nodes: &mut [CNode], ends: &[(i32, i32)]) {
    let ancestors = |nodes: &[CNode], i: usize| -> Vec<usize> {
        let mut set: Vec<usize> = Vec::new();
        let mut p = nodes[i].parent;
        while p >= 0 && !set.contains(&(p as usize)) {
            set.push(p as usize);
            p = nodes[p as usize].parent;
        }
        set
    };
    for &(src, dst) in ends {
        if src < 0 || dst < 0 || src == dst {
            continue;
        }
        let a = ancestors(nodes, src as usize);
        let b = ancestors(nodes, dst as usize);
        for &g in &a {
            if !b.contains(&g) {
                nodes[g].dir = None;
            }
        }
        for &g in &b {
            if !a.contains(&g) {
                nodes[g].dir = None;
            }
        }
    }
}

/// Ports `shiftLayout`.
pub fn shift_layout(nodes: &mut [CNode], edges: &mut [CEdge], ox: f64, oy: f64) {
    for c in nodes.iter_mut() {
        c.x += ox;
        c.y += oy;
    }
    for e in edges.iter_mut() {
        shift_edge(e, ox, oy);
    }
}

fn shift_edge(e: &mut CEdge, ox: f64, oy: f64) {
    let mut k = 0;
    while k + 1 < e.route.len() {
        e.route[k] += ox;
        e.route[k + 1] += oy;
        k += 3;
    }
    e.label_x += ox;
    e.label_y += oy;
}

/// Ports `cnode`.
pub fn cnode(w: f64, h: f64, parent: i32) -> CNode {
    CNode {
        w,
        h,
        parent,
        is_group: false,
        dir: None,
        pad_x: 0.0,
        pad_top: 0.0,
        pad_bottom: 0.0,
        min_w: 0.0,
        seq: 0.0,
        span: 0.0,
        x: 0.0,
        y: 0.0,
    }
}

/// Ports `startSide`.
fn start_side(dir: Dir) -> u8 {
    match dir {
        Dir::Tb => TOP,
        Dir::Bt => BOTTOM,
        Dir::Lr => LEFT,
        Dir::Rl => RIGHT,
    }
}

/// Ports `opposite`.
fn opposite(side: u8) -> u8 {
    (side + 2) & 3
}

/// Ports `endpointBelow` (inside `compoundLayout`).
fn endpoint_below(item: usize, e: &CEdge, out: bool) -> bool {
    (if out { e.src } else { e.dst }) != item
}

/// Ports `interface PortEdge`.
struct PortEdge {
    e: usize,
    inner: usize,
    out: bool,
    side: u8,
    // Cross-flow position outside the group from a previous layout; NaN if unknown.
    hint: f64,
    px: f64,
    py: f64,
    route: Vec<f64>,
}

/// Ports `interface LevelEdge`.
struct LevelEdge {
    e: usize,
    a: usize,
    b: usize,
}

/// Ports `interface Level`.
struct Level {
    id: i32,
    dir: Dir,
    items: Vec<usize>,
    edges: Vec<LevelEdge>,
    ports: Vec<PortEdge>,
    port_of: BTreeMap<usize, usize>,
    routes: BTreeMap<usize, Vec<f64>>,
    x: f64,
    y: f64,
}

impl Level {
    /// `makeLevel` inside `compoundLayout` (the caller records it in `levelOf`).
    fn new(id: i32, dir: Dir) -> Level {
        Level {
            id,
            dir,
            items: Vec::new(),
            edges: Vec::new(),
            ports: Vec::new(),
            port_of: BTreeMap::new(),
            routes: BTreeMap::new(),
            x: 0.0,
            y: 0.0,
        }
    }
}

/// Ports `interface Snapshot`.
struct Snapshot {
    nodes: Vec<f64>,
    routes: Vec<Vec<f64>>,
    labels: Vec<f64>,
    levels: Vec<f64>,
    total: Size,
}

/// What `farHint` reads from the flat layout.
struct FlatHint {
    ledge_of: Vec<isize>,
    ledges: Vec<LEdge>,
    last: Vec<i32>,
}

/// One entry of `laid` (the reuse of unchanged levels across attempts).
struct Laid {
    given: Vec<f64>,
    lnodes: Vec<LNode>,
    ledges: Vec<LEdge>,
    size: LayeredResult,
}

/// What `build` returns.
struct Built {
    lnodes: Vec<LNode>,
    ledges: Vec<LEdge>,
    dummies: Vec<isize>,
}

/// The state `compoundLayout`'s closures share.
struct Compound<'a> {
    nodes: &'a mut [CNode],
    edges: &'a mut [CEdge],
    opt: &'a LayeredOptions,
    levels: Vec<Level>,
    /// `levelOf` for groups: node index to level index. The root is level 0.
    level_index: Vec<usize>,
    local: Vec<usize>,
    key: Vec<f64>,
    keyed: bool,
    flat: Option<FlatHint>,
    ordered: bool,
    laid: Vec<Vec<Laid>>,
    total: Size,
}

/// `Object.is` for numbers.
fn object_is(a: f64, b: f64) -> bool {
    (a.is_nan() && b.is_nan()) || a.to_bits() == b.to_bits()
}

impl Compound<'_> {
    fn level_of(&self, id: i32) -> usize {
        if id < 0 {
            0
        } else {
            self.level_index[id as usize]
        }
    }

    /// The port of `ei` (out or in) on the level of group `item`.
    fn child_port(&self, item: usize, ei: usize, out: bool) -> (usize, usize) {
        let cl = self.level_index[item];
        let cp = self.levels[cl].port_of[&(ei * 2 + if out { 0 } else { 1 })];
        (cl, cp)
    }

    /// `held` inside `build`.
    fn held(&self, item: usize, ei: usize, out: bool) -> bool {
        self.nodes[item].is_group && endpoint_below(item, &self.edges[ei], out)
    }

    /// `offset` inside `build`.
    fn offset(&self, sized: bool, vertical: bool, item: usize, ei: usize, out: bool) -> f64 {
        if !sized || !self.nodes[item].is_group || !endpoint_below(item, &self.edges[ei], out) {
            return 0.0;
        }
        let (cl, cp) = self.child_port(item, ei, out);
        let port = &self.levels[cl].ports[cp];
        if vertical {
            port.px - self.nodes[item].w / 2.0
        } else {
            port.py - self.nodes[item].h / 2.0
        }
    }

    /// `farHint` inside `compoundLayout`: orders outgoing ports at the
    /// group's last rank; breaks ties using the next rank.
    fn far_hint(&self, li: usize, pi: usize) -> f64 {
        let Some(fh) = &self.flat else {
            return f64::NAN;
        };
        let level = &self.levels[li];
        let port = &level.ports[pi];
        let le = fh.ledge_of[port.e];
        if le < 0 || level.id < 0 || !port.out || port.side != opposite(start_side(level.dir)) {
            return f64::NAN;
        }
        let points = &fh.ledges[le as usize].points;
        let mut inside = self.key[self.edges[port.e].src];
        let mut outside = 0.0;
        let last = fh.last[level.id as usize] as f64;
        let mut k = 0;
        while k + 1 < points.len() {
            if points[k] == last {
                inside = points[k + 1];
            }
            if points[k] == last + 1.0 {
                outside = points[k + 1];
            }
            k += 2;
        }
        inside + outside * 1e-6
    }

    /// Ports `build` inside `compoundLayout`.
    fn build(&mut self, li: usize, sized: bool) -> Built {
        for (i, &item) in self.levels[li].items.iter().enumerate() {
            self.local[item] = i;
        }
        let level = &self.levels[li];
        let vertical = level.dir.vertical();
        let start = start_side(level.dir);
        let local = &self.local;
        let mut lnodes: Vec<LNode> = Vec::new();
        let mut ledges: Vec<LEdge> = Vec::new();
        for &item in &level.items {
            let node = &self.nodes[item];
            let mut ln = if sized {
                lnode(
                    if vertical { node.w } else { node.h },
                    if vertical { node.h } else { node.w },
                    Kind::Node,
                )
            } else {
                lnode(0.0, 0.0, Kind::Node)
            };
            ln.span = if node.is_group {
                jmin(node.span, ln.w - 2.0 * node.pad_x)
            } else {
                node.span
            };
            ln.key = if self.keyed { self.key[item] } else { f64::NAN };
            lnodes.push(ln);
        }
        for le in &level.edges {
            let e = &self.edges[le.e];
            let mut edge = ledge(local[le.a], local[le.b], e.minlen, 0.0, 0.0);
            if sized {
                edge.label_w = if vertical { e.label_w } else { e.label_h };
                edge.label_h = if vertical { e.label_h } else { e.label_w };
                edge.tail_dx = self.offset(sized, vertical, le.a, le.e, true);
                edge.head_dx = self.offset(sized, vertical, le.b, le.e, false);
                edge.tail_fixed = self.held(le.a, le.e, true);
                edge.head_fixed = self.held(le.b, le.e, false);
            }
            ledges.push(edge);
        }
        let mut dummies = alloc::vec![-1isize; level.ports.len()];
        for (i, port) in level.ports.iter().enumerate() {
            if port.side == start || port.side == opposite(start) {
                let d = lnodes.len();
                let mut ln = lnode(
                    0.0,
                    0.0,
                    if port.side == start {
                        Kind::StartPort
                    } else {
                        Kind::EndPort
                    },
                );
                ln.key = if self.keyed {
                    self.key[if port.out {
                        self.edges[port.e].dst
                    } else {
                        self.edges[port.e].src
                    }]
                } else {
                    f64::NAN
                };
                ln.hint = if port.hint.is_nan() && self.keyed && self.flat.is_some() {
                    self.far_hint(li, i)
                } else {
                    port.hint
                };
                lnodes.push(ln);
                dummies[i] = d as isize;
                let mut edge = if port.out {
                    ledge(local[port.inner], d, 1.0, 0.0, 0.0)
                } else {
                    ledge(d, local[port.inner], 1.0, 0.0, 0.0)
                };
                if port.out {
                    edge.tail_dx = self.offset(sized, vertical, port.inner, port.e, true);
                    edge.tail_fixed = self.held(port.inner, port.e, true);
                } else {
                    edge.head_dx = self.offset(sized, vertical, port.inner, port.e, false);
                    edge.head_fixed = self.held(port.inner, port.e, false);
                }
                ledges.push(edge);
            } else {
                lnodes[local[port.inner]].pin = if port.side == TOP || port.side == LEFT {
                    -1.0
                } else {
                    1.0
                };
            }
        }
        Built {
            lnodes,
            ledges,
            dummies,
        }
    }

    /// Ports `layOut` inside `compoundLayout`: reuses unchanged levels across
    /// layout attempts.
    fn lay_out(
        &mut self,
        li: usize,
        mut lnodes: Vec<LNode>,
        mut ledges: Vec<LEdge>,
    ) -> (Vec<LNode>, Vec<LEdge>, LayeredResult) {
        if !self.ordered {
            let size = layered(&mut lnodes, &mut ledges, self.opt);
            return (lnodes, ledges, size);
        }
        let mut given: Vec<f64> = Vec::new();
        for ln in &lnodes {
            given.extend_from_slice(&[
                ln.w,
                ln.h,
                ln.kind as u8 as f64,
                ln.pin,
                ln.hint,
                ln.key,
                ln.span,
            ]);
        }
        for le in &ledges {
            given.extend_from_slice(&[
                le.tail as f64,
                le.head as f64,
                le.minlen,
                le.label_w,
                le.label_h,
                le.tail_dx,
                le.head_dx,
                if le.tail_fixed { 1.0 } else { 0.0 },
                if le.head_fixed { 1.0 } else { 0.0 },
            ]);
        }
        for p in &self.laid[li] {
            let same = p.given.len() == given.len()
                && p.given.iter().zip(&given).all(|(&a, &b)| object_is(a, b));
            if same {
                return (p.lnodes.clone(), p.ledges.clone(), p.size);
            }
        }
        let size = layered(&mut lnodes, &mut ledges, self.opt);
        self.laid[li].push(Laid {
            given,
            lnodes: lnodes.clone(),
            ledges: ledges.clone(),
            size,
        });
        (lnodes, ledges, size)
    }

    /// Ports `snapshot`.
    fn snapshot(&self) -> Snapshot {
        let mut at: Vec<f64> = Vec::new();
        for c in self.nodes.iter() {
            at.extend_from_slice(&[c.x, c.y, c.w, c.h]);
        }
        let mut labels: Vec<f64> = Vec::new();
        for e in self.edges.iter() {
            labels.push(e.label_x);
            labels.push(e.label_y);
        }
        let mut places: Vec<f64> = Vec::new();
        for level in &self.levels {
            places.push(level.x);
            places.push(level.y);
            for port in &level.ports {
                places.push(port.px);
                places.push(port.py);
            }
        }
        Snapshot {
            nodes: at,
            routes: self.edges.iter().map(|e| e.route.clone()).collect(),
            labels,
            levels: places,
            total: self.total,
        }
    }

    /// Ports `restore`.
    fn restore(&mut self, s: Snapshot) {
        for (i, c) in self.nodes.iter_mut().enumerate() {
            c.x = s.nodes[4 * i];
            c.y = s.nodes[4 * i + 1];
            c.w = s.nodes[4 * i + 2];
            c.h = s.nodes[4 * i + 3];
        }
        for (i, (e, route)) in self.edges.iter_mut().zip(s.routes).enumerate() {
            e.route = route;
            e.label_x = s.labels[2 * i];
            e.label_y = s.labels[2 * i + 1];
        }
        let mut k = 0;
        for level in self.levels.iter_mut() {
            level.x = s.levels[k];
            level.y = s.levels[k + 1];
            k += 2;
            for port in level.ports.iter_mut() {
                port.px = s.levels[k];
                port.py = s.levels[k + 1];
                k += 2;
            }
        }
        self.total = s.total;
    }

    /// Ports `place` inside `compoundLayout`.
    fn place(&mut self) {
        // Rank parents first to determine which group sides edges cross.
        for li in 0..self.levels.len() {
            for pi in 0..self.levels[li].ports.len() {
                let (inner, e, out, side) = {
                    let p = &self.levels[li].ports[pi];
                    (p.inner, p.e, p.out, p.side)
                };
                if self.nodes[inner].is_group && endpoint_below(inner, &self.edges[e], out) {
                    let (cl, cp) = self.child_port(inner, e, out);
                    self.levels[cl].ports[cp].side = side;
                }
            }
            if self.levels[li].edges.is_empty() {
                continue;
            }
            let mut needed = false;
            for le in &self.levels[li].edges {
                let e = &self.edges[le.e];
                if (self.nodes[le.a].is_group && e.src != le.a)
                    || (self.nodes[le.b].is_group && e.dst != le.b)
                {
                    needed = true;
                }
            }
            if !needed {
                continue;
            }
            let built = self.build(li, false);
            let mut lnodes = built.lnodes;
            let mut ledges = built.ledges;
            orient(&lnodes, &mut ledges);
            rank(&mut lnodes, &ledges, 1);
            let start = start_side(self.levels[li].dir);
            for i in 0..self.levels[li].edges.len() {
                let (ee, a, b) = {
                    let le = &self.levels[li].edges[i];
                    (le.e, le.a, le.b)
                };
                let a_above = !ledges[i].reversed;
                if self.nodes[a].is_group && self.edges[ee].src != a {
                    let (cl, cp) = self.child_port(a, ee, true);
                    self.levels[cl].ports[cp].side = if a_above { opposite(start) } else { start };
                }
                if self.nodes[b].is_group && self.edges[ee].dst != b {
                    let (cl, cp) = self.child_port(b, ee, false);
                    self.levels[cl].ports[cp].side = if a_above { start } else { opposite(start) };
                }
            }
        }

        // Lay out children first so their sizes are known.
        for q in (0..self.levels.len()).rev() {
            let id = self.levels[q].id;
            let dir = self.levels[q].dir;
            let group = if id >= 0 { Some(id as usize) } else { None };
            let vertical = dir.vertical();
            let flip = dir == Dir::Bt || dir == Dir::Rl;
            let built = self.build(q, true);
            let dummies = built.dummies;
            let (lnodes, ledges, size) = self.lay_out(q, built.lnodes, built.ledges);
            let content_w = if vertical { size.width } else { size.height };
            let content_h = if vertical { size.height } else { size.width };
            let (pad_x, pad_top, pad_bottom, min_w) = match group {
                Some(gi) => {
                    let g = &self.nodes[gi];
                    (g.pad_x, g.pad_top, g.pad_bottom, g.min_w)
                }
                None => (0.0, 0.0, 0.0, 0.0),
            };
            let box_w = jmax(content_w + 2.0 * pad_x, min_w);
            let box_h = content_h + pad_top + pad_bottom;
            let off_x = (box_w - content_w) / 2.0;
            let off_y = pad_top;
            let tx = |lx: f64, ly: f64| -> f64 {
                off_x
                    + (if vertical {
                        lx
                    } else if flip {
                        size.height - ly
                    } else {
                        ly
                    })
            };
            let ty = |lx: f64, ly: f64| -> f64 {
                off_y
                    + (if vertical {
                        if flip {
                            size.height - ly
                        } else {
                            ly
                        }
                    } else {
                        lx
                    })
            };
            let axis = if vertical { 0.0 } else { 1.0 };
            let convert = |points: &[f64]| -> Vec<f64> {
                let mut out: Vec<f64> = Vec::new();
                let mut i = 0;
                while i + 1 < points.len() {
                    out.push(tx(points[i], points[i + 1]));
                    out.push(ty(points[i], points[i + 1]));
                    out.push(axis);
                    i += 2;
                }
                out
            };

            for i in 0..self.levels[q].items.len() {
                let item = self.levels[q].items[i];
                self.nodes[item].x = tx(lnodes[i].x, lnodes[i].y);
                self.nodes[item].y = ty(lnodes[i].x, lnodes[i].y);
            }
            for i in 0..self.levels[q].edges.len() {
                let le_e = self.levels[q].edges[i].e;
                let edge = &ledges[i];
                let route = convert(&edge.points);
                self.levels[q].routes.insert(le_e, route);
                let e = &mut self.edges[le_e];
                if e.label_w > 0.0 {
                    e.label_x = tx(edge.label_x, edge.label_y);
                    e.label_y = ty(edge.label_x, edge.label_y);
                }
            }
            let mut k = self.levels[q].edges.len();
            for i in 0..self.levels[q].ports.len() {
                let (inner, pe, out, side) = {
                    let p = &self.levels[q].ports[i];
                    (p.inner, p.e, p.out, p.side)
                };
                if dummies[i] >= 0 {
                    let mut route = convert(&ledges[k].points);
                    k += 1;
                    let at = if out { route.len() - 3 } else { 0 };
                    let mut bx = route[at];
                    let mut by = route[at + 1];
                    if side == TOP {
                        by = 0.0;
                    } else if side == BOTTOM {
                        by = box_h;
                    } else if side == LEFT {
                        bx = 0.0;
                    } else {
                        bx = box_w;
                    }
                    if out {
                        route.extend_from_slice(&[bx, by, axis]);
                    } else {
                        route.splice(0..0, [bx, by, axis]);
                    }
                    let port = &mut self.levels[q].ports[i];
                    port.px = bx;
                    port.py = by;
                    port.route = route;
                    continue;
                }
                let (item_x, item_y, item_w, item_h, item_group) = {
                    let it = &self.nodes[inner];
                    (it.x, it.y, it.w, it.h, it.is_group)
                };
                let mut ix = item_x;
                let mut iy = item_y;
                if item_group && endpoint_below(inner, &self.edges[pe], out) {
                    let (cl, cp) = self.child_port(inner, pe, out);
                    let ip = &self.levels[cl].ports[cp];
                    ix += ip.px - item_w / 2.0;
                    iy += ip.py - item_h / 2.0;
                } else if side == TOP {
                    iy -= item_h / 2.0;
                } else if side == BOTTOM {
                    iy += item_h / 2.0;
                } else if side == LEFT {
                    ix -= item_w / 2.0;
                } else {
                    ix += item_w / 2.0;
                }
                let mut bx = ix;
                let mut by = iy;
                if side == TOP {
                    by = 0.0;
                } else if side == BOTTOM {
                    by = box_h;
                } else if side == LEFT {
                    bx = 0.0;
                } else {
                    bx = box_w;
                }
                let a = if side == TOP || side == BOTTOM {
                    0.0
                } else {
                    1.0
                };
                let port = &mut self.levels[q].ports[i];
                port.px = bx;
                port.py = by;
                port.route = if out {
                    alloc::vec![ix, iy, a, bx, by, a]
                } else {
                    alloc::vec![bx, by, a, ix, iy, a]
                };
            }

            match group {
                Some(gi) => {
                    self.nodes[gi].w = box_w;
                    self.nodes[gi].h = box_h;
                }
                None => {
                    self.total = Size {
                        width: box_w,
                        height: box_h,
                    };
                }
            }
        }

        for li in 0..self.levels.len() {
            let ox = self.levels[li].x;
            let oy = self.levels[li].y;
            for idx in 0..self.levels[li].items.len() {
                let item = self.levels[li].items[idx];
                let node = &mut self.nodes[item];
                node.x += ox;
                node.y += oy;
                if node.is_group {
                    let (cx, cy) = (node.x - node.w / 2.0, node.y - node.h / 2.0);
                    let cl = self.level_index[item];
                    self.levels[cl].x = cx;
                    self.levels[cl].y = cy;
                }
            }
        }

        let append = |route: &mut Vec<f64>, part: &[f64], ox: f64, oy: f64| {
            let mut i = 0;
            while i + 2 < part.len() {
                let x = part[i] + ox;
                let y = part[i + 1] + oy;
                let len = route.len();
                if len >= 3
                    && (route[len - 3] - x).abs() < 0.01
                    && (route[len - 2] - y).abs() < 0.01
                {
                    i += 3;
                    continue;
                }
                route.extend_from_slice(&[x, y, part[i + 2]]);
                i += 3;
            }
        };

        for li in 0..self.levels.len() {
            let level_id = self.levels[li].id;
            for lei in 0..self.levels[li].edges.len() {
                let le_e = self.levels[li].edges[lei].e;
                let mut route: Vec<f64> = Vec::new();
                let mut outward: Vec<usize> = Vec::new();
                let mut item = self.edges[le_e].src;
                while self.nodes[item].parent != level_id {
                    outward.push(self.level_of(self.nodes[item].parent));
                    item = self.nodes[item].parent as usize;
                }
                for &lv in &outward {
                    let level = &self.levels[lv];
                    let pi = level.port_of[&(le_e * 2)];
                    append(&mut route, &level.ports[pi].route, level.x, level.y);
                }
                {
                    let level = &self.levels[li];
                    append(&mut route, &level.routes[&le_e], level.x, level.y);
                }
                let mut inward: Vec<usize> = Vec::new();
                let mut item = self.edges[le_e].dst;
                while self.nodes[item].parent != level_id {
                    inward.push(self.level_of(self.nodes[item].parent));
                    item = self.nodes[item].parent as usize;
                }
                for &lv in inward.iter().rev() {
                    let level = &self.levels[lv];
                    let pi = level.port_of[&(le_e * 2 + 1)];
                    append(&mut route, &level.ports[pi].route, level.x, level.y);
                }
                let (lx, ly) = (self.levels[li].x, self.levels[li].y);
                let e = &mut self.edges[le_e];
                e.route = route;
                if e.label_w > 0.0 {
                    e.label_x += lx;
                    e.label_y += ly;
                }
            }
        }
    }
}

/// `descend` inside `compoundLayout`: walks the levels from `from` on,
/// giving every item its depth and every group its own level.
fn descend(
    levels: &mut Vec<Level>,
    level_index: &mut [usize],
    children: &[Vec<usize>],
    nodes: &[CNode],
    depth: &mut [i32],
    placed: &mut [u8],
    from: usize,
) {
    let mut q = from;
    while q < levels.len() {
        let id = levels[q].id;
        let dir = levels[q].dir;
        let items = levels[q].items.clone();
        for item in items {
            placed[item] = 1;
            depth[item] = if id < 0 { 0 } else { depth[id as usize] + 1 };
            if nodes[item].is_group {
                let mut child = Level::new(item as i32, nodes[item].dir.unwrap_or(dir));
                child.items = children[item].clone();
                level_index[item] = levels.len();
                levels.push(child);
            }
        }
        q += 1;
    }
}

/// Ports `compoundLayout`: lays out a graph whose nodes may be nested in
/// groups, writing every node's `x`, `y` (and a group's `w`, `h`) and every
/// edge's `route` and label point. Returns the size of the whole.
pub fn compound_layout(
    nodes: &mut [CNode],
    edges: &mut [CEdge],
    root_dir: Dir,
    opt: &LayeredOptions,
) -> Size {
    let n = nodes.len();
    let mut levels: Vec<Level> = Vec::new();
    let mut level_index = alloc::vec![usize::MAX; n];
    levels.push(Level::new(-1, root_dir));

    let mut children: Vec<Vec<usize>> = (0..n).map(|_| Vec::new()).collect();
    let mut root_items: Vec<usize> = Vec::new();
    for i in 0..n {
        let p = nodes[i].parent;
        if p >= 0 && (p as usize) < n && p as usize != i && nodes[p as usize].is_group {
            children[p as usize].push(i);
        } else {
            nodes[i].parent = -1;
            root_items.push(i);
        }
    }
    levels[0].items = root_items;
    let mut depth = alloc::vec![0i32; n];
    let mut placed = alloc::vec![0u8; n];
    descend(
        &mut levels,
        &mut level_index,
        &children,
        nodes,
        &mut depth,
        &mut placed,
        0,
    );
    // Groups that contain each other are unreachable from the root; lift them out.
    for i in 0..n {
        if placed[i] != 0 {
            continue;
        }
        let former = nodes[i].parent as usize;
        children[former].retain(|&c| c != i);
        nodes[i].parent = -1;
        levels[0].items.push(i);
        placed[i] = 1;
        depth[i] = 0;
        if nodes[i].is_group {
            let from = levels.len();
            let mut child = Level::new(i as i32, nodes[i].dir.unwrap_or(root_dir));
            child.items = children[i].clone();
            level_index[i] = levels.len();
            levels.push(child);
            descend(
                &mut levels,
                &mut level_index,
                &children,
                nodes,
                &mut depth,
                &mut placed,
                from,
            );
        }
    }

    for q in (1..levels.len()).rev() {
        let mut seq = f64::INFINITY;
        for &item in &levels[q].items {
            if nodes[item].seq < seq {
                seq = nodes[item].seq;
            }
        }
        if seq != f64::INFINITY {
            nodes[levels[q].id as usize].seq = seq;
        }
    }
    for level in levels.iter_mut() {
        sort_by(&mut level.items, |a, b| {
            or(nodes[a].seq - nodes[b].seq, a as f64 - b as f64)
        });
    }

    let parent_of = |nodes: &[CNode], i: usize| nodes[i].parent as usize;
    // Every group border an edge crosses costs a port. Past the budget, an edge is drawn between
    // the groups that hold its ends, at the level they share.
    let mut ports = 0usize;
    for e in edges.iter_mut() {
        let mut a = e.src;
        let mut b = e.dst;
        let mut crossed = 0usize;
        while depth[a] > depth[b] {
            a = parent_of(nodes, a);
            crossed += 1;
        }
        while depth[b] > depth[a] {
            b = parent_of(nodes, b);
            crossed += 1;
        }
        while a != b && nodes[a].parent != nodes[b].parent {
            a = parent_of(nodes, a);
            b = parent_of(nodes, b);
            crossed += 2;
        }
        if a == b {
            continue;
        }
        if ports + crossed > MAX_PORTS {
            e.src = a;
            e.dst = b;
        } else {
            ports += crossed;
        }
    }

    let level_of =
        |level_index: &[usize], id: i32| if id < 0 { 0 } else { level_index[id as usize] };
    let mut loops: Vec<usize> = Vec::new();
    let mut stray: Vec<usize> = Vec::new();
    for ei in 0..edges.len() {
        let e = &mut edges[ei];
        e.route = Vec::new();
        if e.src == e.dst {
            loops.push(ei);
            continue;
        }
        let mut a = e.src;
        let mut b = e.dst;
        let mut a_path: Vec<usize> = Vec::new();
        let mut b_path: Vec<usize> = Vec::new();
        while depth[a] > depth[b] {
            a_path.push(a);
            a = parent_of(nodes, a);
        }
        while depth[b] > depth[a] {
            b_path.push(b);
            b = parent_of(nodes, b);
        }
        if a == b {
            stray.push(ei);
            continue;
        }
        while nodes[a].parent != nodes[b].parent {
            a_path.push(a);
            b_path.push(b);
            a = parent_of(nodes, a);
            b = parent_of(nodes, b);
        }
        let li = level_of(&level_index, nodes[a].parent);
        levels[li].edges.push(LevelEdge { e: ei, a, b });
        for (path, out) in [(&a_path, true), (&b_path, false)] {
            for &inner in path {
                let li = level_of(&level_index, nodes[inner].parent);
                let pi = levels[li].ports.len();
                levels[li].ports.push(PortEdge {
                    e: ei,
                    inner,
                    out,
                    side: TOP,
                    hint: f64::NAN,
                    px: 0.0,
                    py: 0.0,
                    route: Vec::new(),
                });
                levels[li]
                    .port_of
                    .insert(ei * 2 + if out { 0 } else { 1 }, pi);
            }
        }
    }

    let ordered = ports > 1 && ports <= MAX_ORDERED_PORTS && edges.len() <= MAX_ORDERED_EDGES;

    // Use a flat layout to coordinate node and port order across groups.
    let mut key = alloc::vec![f64::NAN; n];
    let mut flat_crossings = 0usize;
    let mut flat_hint: Option<FlatHint> = None;
    let mut flat = levels.len() > 1 && ordered;
    for level in &levels {
        if level.dir != root_dir {
            flat = false;
        }
    }
    if flat {
        let vertical = root_dir.vertical();
        let mut leaf = alloc::vec![-1isize; n];
        let mut group = alloc::vec![-1i32; n];
        let mut lnodes: Vec<LNode> = Vec::new();
        let mut of: Vec<i32> = Vec::new();
        let mut parent: Vec<i32> = Vec::new();
        for i in 0..n {
            if !nodes[i].is_group {
                continue;
            }
            group[i] = parent.len() as i32;
            parent.push(-1);
        }
        for i in 0..n {
            let p = nodes[i].parent;
            if nodes[i].is_group {
                parent[group[i] as usize] = if p >= 0 { group[p as usize] } else { -1 };
            } else {
                leaf[i] = lnodes.len() as isize;
                lnodes.push(lnode(
                    if vertical { nodes[i].w } else { nodes[i].h },
                    if vertical { nodes[i].h } else { nodes[i].w },
                    Kind::Node,
                ));
                of.push(if p >= 0 { group[p as usize] } else { -1 });
            }
        }
        let mut ledges: Vec<LEdge> = Vec::new();
        let mut ledge_of = alloc::vec![-1isize; edges.len()];
        for (ei, e) in edges.iter().enumerate() {
            if e.src == e.dst || leaf[e.src] < 0 || leaf[e.dst] < 0 {
                continue;
            }
            ledge_of[ei] = ledges.len() as isize;
            ledges.push(ledge(
                leaf[e.src] as usize,
                leaf[e.dst] as usize,
                e.minlen,
                if vertical { e.label_w } else { e.label_h },
                if vertical { e.label_h } else { e.label_w },
            ));
        }
        let mut fopt = opt.clone();
        fopt.groups = Some(Groups { of, parent });
        fopt.order_only = true;
        let flat_layout = layered(&mut lnodes, &mut ledges, &fopt);
        flat_crossings = flat_layout
            .crossings
            .unwrap_or(0)
            .min(flat_layout.declared.unwrap_or(0));
        let mut sum = alloc::vec![0.0f64; n];
        let mut count = alloc::vec![0i32; n];
        // Last rank occupied by each group.
        let mut last = alloc::vec![-1i32; n];
        for i in 0..n {
            if leaf[i] < 0 {
                continue;
            }
            let lf = leaf[i] as usize;
            key[i] = lnodes[lf].x;
            let mut p = nodes[i].parent;
            while p >= 0 {
                let pu = p as usize;
                sum[pu] += key[i];
                count[pu] += 1;
                last[pu] = last[pu].max(lnodes[lf].rank);
                p = nodes[pu].parent;
            }
        }
        for i in 0..n {
            if count[i] > 0 {
                key[i] = sum[i] / count[i] as f64;
            }
        }
        flat_hint = Some(FlatHint {
            ledge_of,
            ledges,
            last,
        });
    }

    let level_count = levels.len();
    let mut c = Compound {
        nodes,
        edges,
        opt,
        levels,
        level_index,
        local: alloc::vec![0usize; n],
        key,
        keyed: false,
        flat: flat_hint,
        ordered,
        laid: (0..level_count).map(|_| Vec::new()).collect(),
        total: Size::default(),
    };

    // The flat layout ignores group boxes, so independent ordering may yield fewer crossings.
    let style = style_of(opt);
    c.keyed = flat;
    c.place();
    let mut crossings: i64 = -1;
    if flat {
        crossings = route_crossings(c.nodes, c.edges, style) as i64;
        if crossings as f64 > flat_crossings as f64 * FLAT_SLACK + FLAT_SPARE {
            let first = c.snapshot();
            c.keyed = false;
            c.place();
            let own = route_crossings(c.nodes, c.edges, style) as i64;
            if own >= crossings {
                c.keyed = true;
                c.restore(first);
            } else {
                crossings = own;
            }
        }
    }

    // Refine port order using routes outside each group. Skip this if independent ordering won.
    if c.keyed == flat && c.ordered {
        let mut again = false;
        for li in 0..c.levels.len() {
            let id = c.levels[li].id;
            if id < 0 {
                continue;
            }
            let (box_x, box_y, box_w, box_h) = {
                let b = &c.nodes[id as usize];
                (b.x, b.y, b.w, b.h)
            };
            let dir = c.levels[li].dir;
            let vertical = dir.vertical();
            let end = opposite(start_side(dir));
            let mut seen: Vec<(f64, f64)> = Vec::new();
            for pi in 0..c.levels[li].ports.len() {
                let (pe, out, side, px, py) = {
                    let p = &c.levels[li].ports[pi];
                    (p.e, p.out, p.side, p.px, p.py)
                };
                if side != end {
                    continue;
                }
                // Skip the straight run out of the port to find where the edge turns.
                let route = &c.edges[pe].route;
                let bx = c.levels[li].x + px;
                let by = c.levels[li].y + py;
                let mut at: isize = -1;
                let mut k = 0;
                while k < route.len() && (at < 0 || out) {
                    if (route[k] - bx).abs() < 0.01 && (route[k + 1] - by).abs() < 0.01 {
                        at = k as isize;
                    }
                    k += 3;
                }
                if at < 0 {
                    continue;
                }
                let a: isize = if vertical { 0 } else { 1 };
                let rl = route.len() as isize;
                let stepk: isize = if out { 3 } else { -3 };
                let mut next = at + stepk;
                while next >= 0
                    && next < rl
                    && (route[(next + a) as usize] - route[(at + a) as usize]).abs() < IN_LINE
                {
                    next += stepk;
                }
                if next < 0 || next >= rl {
                    continue;
                }
                let hint = route[(next + a) as usize]
                    - (if vertical {
                        box_x - box_w / 2.0
                    } else {
                        box_y - box_h / 2.0
                    });
                c.levels[li].ports[pi].hint = hint;
                seen.push((if vertical { px } else { py }, hint));
            }
            sort_by(&mut seen, |p, q| p.0 - q.0);
            for i in 1..seen.len() {
                if seen[i - 1].1 > seen[i].1 {
                    again = true;
                }
            }
        }
        if again {
            let before = if crossings >= 0 {
                crossings
            } else {
                route_crossings(c.nodes, c.edges, style) as i64
            };
            if before > 0 {
                let first = c.snapshot();
                c.place();
                if route_crossings(c.nodes, c.edges, style) as i64 >= before {
                    c.restore(first);
                }
            }
        }
    }

    let total = c.total;
    let (nodes, edges) = (c.nodes, c.edges);
    for &ei in &stray {
        let (ax, ay) = (nodes[edges[ei].src].x, nodes[edges[ei].src].y);
        let (bx, by) = (nodes[edges[ei].dst].x, nodes[edges[ei].dst].y);
        let e = &mut edges[ei];
        e.route = alloc::vec![ax, ay, 0.0, bx, by, 0.0];
        e.label_x = (ax + bx) / 2.0;
        e.label_y = (ay + by) / 2.0;
    }
    for &ei in &loops {
        let node = &nodes[edges[ei].src];
        let (nx, ny, nw) = (node.x, node.y, node.w);
        let e = &mut edges[ei];
        e.label_x = nx + nw / 2.0 + 28.0 + e.label_w / 2.0;
        e.label_y = ny;
    }

    total
}

/// Ports `interface Part` (components.ts).
struct Part {
    nodes: Vec<usize>,
    edges: Vec<usize>,
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

/// `include` inside `wrapComponents`.
fn include(p: &mut Part, x: f64, y: f64, w: f64, h: f64) {
    p.left = jmin(p.left, x - w / 2.0);
    p.top = jmin(p.top, y - h / 2.0);
    p.right = jmax(p.right, x + w / 2.0);
    p.bottom = jmax(p.bottom, y + h / 2.0);
}

/// Ports `wrapComponents` (components.ts): moves disconnected parts into
/// rows without changing the layout inside a part.
pub fn wrap_components(
    nodes: &mut [CNode],
    edges: &mut [CEdge],
    width: f64,
    gap: f64,
) -> Option<Size> {
    let n = nodes.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    fn join(parent: &mut [usize], a: usize, b: usize) {
        let ra = root(parent, a);
        let rb = root(parent, b);
        parent[ra] = rb;
    }
    for i in 0..n {
        if nodes[i].parent >= 0 {
            join(&mut parent, i, nodes[i].parent as usize);
        }
    }
    for e in edges.iter() {
        join(&mut parent, e.src, e.dst);
    }

    let mut parts: Vec<Part> = Vec::new();
    let mut slot = alloc::vec![usize::MAX; n];
    for i in 0..n {
        let id = root(&mut parent, i);
        if slot[id] == usize::MAX {
            slot[id] = parts.len();
            parts.push(Part {
                nodes: Vec::new(),
                edges: Vec::new(),
                left: f64::INFINITY,
                top: f64::INFINITY,
                right: f64::NEG_INFINITY,
                bottom: f64::NEG_INFINITY,
            });
        }
        let part = &mut parts[slot[id]];
        let c = &nodes[i];
        part.nodes.push(i);
        include(part, c.x, c.y, c.w, c.h);
    }
    if parts.len() < 2 {
        return None;
    }
    for (ei, e) in edges.iter().enumerate() {
        let id = root(&mut parent, e.src);
        let part = &mut parts[slot[id]];
        part.edges.push(ei);
        let mut i = 0;
        while i + 1 < e.route.len() {
            include(part, e.route[i], e.route[i + 1], 0.0, 0.0);
            i += 3;
        }
        if e.label_w > 0.0 {
            include(part, e.label_x, e.label_y, e.label_w, e.label_h);
        }
    }

    let (mut x, mut y, mut row_height, mut right) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let (mut from, mut to) = (f64::INFINITY, f64::NEG_INFINITY);
    let mut shifts: Vec<f64> = Vec::new();
    for part in &parts {
        let w = part.right - part.left;
        let h = part.bottom - part.top;
        if x > 0.0 && x + w > width {
            y += row_height + gap;
            x = 0.0;
            row_height = 0.0;
        }
        shifts.push(x - part.left);
        shifts.push(y - part.top);
        from = jmin(from, part.left);
        to = jmax(to, part.right);
        right = jmax(right, x + w);
        row_height = jmax(row_height, h);
        x += w + gap;
    }
    if right >= to - from {
        return None;
    }
    for (k, part) in parts.iter().enumerate() {
        let (ox, oy) = (shifts[2 * k], shifts[2 * k + 1]);
        for &i in &part.nodes {
            nodes[i].x += ox;
            nodes[i].y += oy;
        }
        for &ei in &part.edges {
            shift_edge(&mut edges[ei], ox, oy);
        }
    }
    Some(Size {
        width: right,
        height: y + row_height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::{self, Value};
    use alloc::format;
    use alloc::string::String;

    /// A number as cap.js wrote it: non-finite numbers are the strings
    /// "Infinity", "-Infinity" and "NaN".
    fn num(v: Option<&Value>) -> f64 {
        match v {
            Some(Value::Num(x)) => *x,
            Some(Value::Str(s)) => match s.as_str() {
                "Infinity" => f64::INFINITY,
                "-Infinity" => f64::NEG_INFINITY,
                _ => f64::NAN,
            },
            Some(Value::Bool(b)) => {
                if *b {
                    1.0
                } else {
                    0.0
                }
            }
            _ => f64::NAN,
        }
    }

    fn flag(v: &Value, k: &str) -> bool {
        v.get_bool(k).unwrap_or(false)
    }

    fn dir_of(s: Option<&str>) -> Option<Dir> {
        s.map(|s| direction(Some(s)))
    }

    fn input(call: &Value) -> (Vec<CNode>, Vec<CEdge>, Dir, LayeredOptions) {
        let inp = call.get("input").expect("input");
        let (nodes, edges) = graph_of(inp);
        let dir = direction(inp.get_str("dir"));
        let o = inp.get("opts").expect("opts");
        let opt = LayeredOptions {
            node_sep: num(o.get("nodeSep")),
            edge_sep: num(o.get("edgeSep")),
            rank_sep: num(o.get("rankSep")),
            port_sep: num(o.get("portSep")),
            tracks: flag(o, "tracks"),
            straight: flag(o, "straight"),
            apart: flag(o, "apart"),
            head_room: o.get("headRoom").map(|v| num(Some(v))).unwrap_or(0.0),
            groups: None,
            order_only: flag(o, "orderOnly"),
        };
        (nodes, edges, dir, opt)
    }

    /// `wrap` cases (written by a capture of Pele's `wrapComponents`): the
    /// first value that differs in each.
    fn check_wrap(name: &str, doc: &Value) -> Vec<String> {
        let mut bad: Vec<String> = Vec::new();
        for (ci, case) in doc
            .get("wrap")
            .map(|c| c.arr())
            .unwrap_or(&[])
            .iter()
            .enumerate()
        {
            let inp = case.get("input").expect("input");
            let (mut nodes, mut edges) = graph_of(inp);
            let got = wrap_components(
                &mut nodes,
                &mut edges,
                num(inp.get("width")),
                num(inp.get("gap")),
            );
            let out = case.get("output").expect("output");
            let want = match out.get("result") {
                Some(Value::Obj(_)) => {
                    let r = out.get("result").expect("result");
                    Some(Size {
                        width: num(r.get("width")),
                        height: num(r.get("height")),
                    })
                }
                _ => None,
            };
            let first = (|| -> Option<String> {
                match (got, want) {
                    (None, None) => {}
                    (Some(g), Some(w)) if close(g.width, w.width) && close(g.height, w.height) => {}
                    _ => return Some(format!("result: got {got:?}, want {want:?}")),
                }
                for (i, (n, w)) in nodes.iter().zip(out.get("nodes")?.arr()).enumerate() {
                    if !close(n.x, num(w.get("x"))) || !close(n.y, num(w.get("y"))) {
                        return Some(format!("node {i}: got {},{}", n.x, n.y));
                    }
                }
                for (i, (e, w)) in edges.iter().zip(out.get("edges")?.arr()).enumerate() {
                    let route: Vec<f64> =
                        w.get("route")?.arr().iter().map(|v| num(Some(v))).collect();
                    if route.len() != e.route.len()
                        || e.route.iter().zip(&route).any(|(&a, &b)| !close(a, b))
                        || !close(e.label_x, num(w.get("labelX")))
                        || !close(e.label_y, num(w.get("labelY")))
                    {
                        return Some(format!("edge {i}: got {:?}, want {:?}", e.route, route));
                    }
                }
                None
            })();
            if let Some(msg) = first {
                bad.push(format!("{name} wrap {ci}: {msg}"));
            }
        }
        bad
    }

    fn graph_of(inp: &Value) -> (Vec<CNode>, Vec<CEdge>) {
        let nodes = inp
            .get("nodes")
            .expect("nodes")
            .arr()
            .iter()
            .map(|n| CNode {
                w: num(n.get("w")),
                h: num(n.get("h")),
                parent: num(n.get("parent")) as i32,
                is_group: flag(n, "isGroup"),
                dir: dir_of(n.get_str("dir")),
                pad_x: num(n.get("padX")),
                pad_top: num(n.get("padTop")),
                pad_bottom: num(n.get("padBottom")),
                min_w: num(n.get("minW")),
                seq: num(n.get("seq")),
                span: num(n.get("span")),
                x: num(n.get("x")),
                y: num(n.get("y")),
            })
            .collect();
        let edges = inp
            .get("edges")
            .expect("edges")
            .arr()
            .iter()
            .map(|e| CEdge {
                src: num(e.get("src")) as usize,
                dst: num(e.get("dst")) as usize,
                minlen: num(e.get("minlen")),
                label_w: num(e.get("labelW")),
                label_h: num(e.get("labelH")),
                route: e
                    .get("route")
                    .map(|r| r.arr().iter().map(|v| num(Some(v))).collect())
                    .unwrap_or_default(),
                label_x: num(e.get("labelX")),
                label_y: num(e.get("labelY")),
            })
            .collect();
        (nodes, edges)
    }

    fn close(a: f64, b: f64) -> bool {
        (a.is_nan() && b.is_nan()) || a == b || (a - b).abs() <= 0.01
    }

    /// Runs one oracle file; returns the first value that differs in each
    /// call, if any.
    fn check(name: &str, text: &str) -> Vec<String> {
        let doc = json::parse(text).unwrap_or_else(|| panic!("{name}: not JSON"));
        let mut bad: Vec<String> = check_wrap(name, &doc);
        for (ci, call) in doc
            .get("calls")
            .map(|c| c.arr())
            .unwrap_or(&[])
            .iter()
            .enumerate()
        {
            let (mut nodes, mut edges, dir, opt) = input(call);
            let size = compound_layout(&mut nodes, &mut edges, dir, &opt);
            let out = call.get("output").expect("output");
            let first = (|| -> Option<String> {
                let s = out.get("size")?;
                let (w, h) = (num(s.get("width")), num(s.get("height")));
                if !close(size.width, w) || !close(size.height, h) {
                    return Some(format!(
                        "size: got {}x{}, want {}x{}",
                        size.width, size.height, w, h
                    ));
                }
                for (i, (n, want)) in nodes.iter().zip(out.get("nodes")?.arr()).enumerate() {
                    for (field, got, exp) in [
                        ("x", n.x, num(want.get("x"))),
                        ("y", n.y, num(want.get("y"))),
                        ("w", n.w, num(want.get("w"))),
                        ("h", n.h, num(want.get("h"))),
                    ] {
                        if !close(got, exp) {
                            return Some(format!("node {i}.{field}: got {got}, want {exp}"));
                        }
                    }
                }
                let want_edges = out.get("edges")?.arr();
                if want_edges.len() != edges.len() {
                    return Some(format!(
                        "edge count: got {}, want {}",
                        edges.len(),
                        want_edges.len()
                    ));
                }
                for (i, (e, want)) in edges.iter().zip(want_edges).enumerate() {
                    let route: Vec<f64> = want
                        .get("route")?
                        .arr()
                        .iter()
                        .map(|v| num(Some(v)))
                        .collect();
                    if route.len() != e.route.len() {
                        return Some(format!(
                            "edge {i} route length: got {} {:?}, want {} {:?}",
                            e.route.len(),
                            e.route,
                            route.len(),
                            route
                        ));
                    }
                    for (k, (&got, &exp)) in e.route.iter().zip(&route).enumerate() {
                        if !close(got, exp) {
                            return Some(format!(
                                "edge {i} route[{k}]: got {got}, want {exp} (got {:?}, want {:?})",
                                e.route, route
                            ));
                        }
                    }
                    for (field, got, exp) in [
                        ("labelX", e.label_x, num(want.get("labelX"))),
                        ("labelY", e.label_y, num(want.get("labelY"))),
                        ("src", e.src as f64, num(want.get("src"))),
                        ("dst", e.dst as f64, num(want.get("dst"))),
                    ] {
                        if !close(got, exp) {
                            return Some(format!("edge {i}.{field}: got {got}, want {exp}"));
                        }
                    }
                }
                None
            })();
            if let Some(msg) = first {
                bad.push(format!("{name} call {ci}: {msg}"));
            }
        }
        bad
    }

    macro_rules! oracle {
        ($($name:literal),* $(,)?) => {
            [$(($name, include_str!(concat!(
                "../../../../evidence/benchmarks/obsidian-vs-krate/pele-oracle/",
                $name,
                ".json"
            )))),*]
        };
    }

    #[test]
    fn compound_layout_matches_pele() {
        let cases = oracle!(
            "class",
            "er",
            "flow-back-edge",
            "flow-bt",
            "flow-edges",
            "flow-multiline",
            "flow-rl-nested",
            "flow-shapes",
            "flow-subgraphs",
            "flow-wide",
            "requirement",
            "state",
        );
        let mut bad: Vec<String> = Vec::new();
        let mut calls = 0;
        for (name, text) in cases {
            let doc = json::parse(text).expect("oracle JSON");
            let n = doc.get("calls").map(|c| c.arr().len()).unwrap_or(0);
            assert!(n > 0, "{name}: the oracle has no layout calls");
            calls += n;
            let b = check(name, text);
            if b.is_empty() {
                std::println!("{name}: {n} call(s) match");
            }
            bad.extend(b);
        }
        for b in &bad {
            std::println!("MISMATCH {b}");
        }
        assert!(
            bad.is_empty(),
            "{} of {} calls differ from Pele:\n{}",
            bad.len(),
            calls,
            bad.join("\n")
        );
    }

    /// Checks every oracle file in `$PELE_ORACLE_DIR`, when it is set: the
    /// way to try the port against diagrams beyond the checked-in corpus
    /// (`node cap.js some.mmd $PELE_ORACLE_DIR/some.json`).
    #[test]
    fn compound_layout_matches_pele_in_oracle_dir() {
        let Ok(dir) = std::env::var("PELE_ORACLE_DIR") else {
            return;
        };
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .expect("PELE_ORACLE_DIR")
            .filter_map(|e| e.ok())
            .map(|e| e.path().to_string_lossy().into_owned())
            .filter(|p| p.ends_with(".json"))
            .collect();
        names.sort();
        let mut bad: Vec<String> = Vec::new();
        for path in &names {
            let text = std::fs::read_to_string(path).expect("oracle file");
            let b = check(path, &text);
            if b.is_empty() {
                std::println!("ok {path}");
            }
            bad.extend(b);
        }
        for b in &bad {
            std::println!("MISMATCH {b}");
        }
        assert!(
            bad.is_empty(),
            "{} mismatches across {} files",
            bad.len(),
            names.len()
        );
    }

    #[test]
    fn js_number_rules() {
        assert_eq!(jround(2.5), 3.0);
        assert_eq!(jround(-2.5), -2.0);
        assert_eq!(jround(0.49999999999999994), 0.0);
        assert!(jmin(f64::NAN, 1.0).is_nan());
        assert!(jmax(-0.0, 0.0).is_sign_positive());
        assert!(jmin(0.0, -0.0).is_sign_negative());
        assert_eq!(jhypot(3.0, 4.0), 5.0);
        assert_eq!(super::super::layered::to_i32(4294967297.0), 1);
        assert_eq!(super::super::layered::to_i32(-1.5), -1);
        let mut v = [3, 1, 2, 1, 0];
        sort_by(&mut v, |a, b| a as f64 - b as f64);
        assert_eq!(v, [0, 1, 1, 2, 3]);
        let mut big: Vec<(i32, usize)> = (0..100).map(|i| ((i * 37) % 7, i as usize)).collect();
        sort_by(&mut big, |a, b| (a.0 - b.0) as f64);
        for w in big.windows(2) {
            assert!(w[0].0 < w[1].0 || (w[0].0 == w[1].0 && w[0].1 < w[1].1));
        }
    }

    #[test]
    fn wrap_components_rows_disconnected_parts() {
        let mut nodes = alloc::vec![cnode(100.0, 20.0, -1), cnode(100.0, 20.0, -1)];
        nodes[0].x = 50.0;
        nodes[0].y = 10.0;
        nodes[1].x = 250.0;
        nodes[1].y = 10.0;
        let mut edges: Vec<CEdge> = Vec::new();
        // Side by side they are 300 wide; at width 150 they wrap into two rows.
        let size = wrap_components(&mut nodes, &mut edges, 150.0, 10.0).expect("wrapped");
        assert_eq!(
            size,
            Size {
                width: 100.0,
                height: 50.0
            }
        );
        assert_eq!((nodes[1].x, nodes[1].y), (50.0, 40.0));
    }
}
