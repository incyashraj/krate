//! Pele's layered (Sugiyama-style) layout of one flat graph
//! (src/layout/layered.ts in Pele 1.13.0), ported function by function.
//! Ranks run down the y axis; callers rotate the result for other
//! directions.
//!
//! The port keeps Pele's numbers exactly: f64 throughout, the floating
//! operations in the order the TypeScript writes them, `Int32Array` as `i32`
//! (or `usize` where a value is only ever an index), JS `Math.min`/`max`
//! with their NaN and signed-zero rules, `Math.round` rounding halves up and
//! `Math.hypot` as V8 computes it. Every JS sort that takes a comparator is
//! stable, so the port sorts with a stable merge sort that reads a
//! comparator's NaN as "equal", as V8 does.
//!
//! What the layout of edges aims for is set out at the top of layered.ts;
//! each function here names the TypeScript function it ports.

// The port keeps the TypeScript's index loops, so that it reads against
// the source line by line.
#![allow(clippy::needless_range_loop)]

use alloc::collections::BTreeSet;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

// ---- JS number semantics -------------------------------------------------

/// JS `Math.min(a, b)`: NaN if either is NaN, and -0 below +0.
pub(crate) fn jmin(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        return f64::NAN;
    }
    if a < b {
        a
    } else if b < a {
        b
    } else if a == 0.0 && (a.is_sign_negative() || b.is_sign_negative()) {
        -0.0
    } else {
        a
    }
}

/// JS `Math.max(a, b)`: NaN if either is NaN, and +0 above -0.
pub(crate) fn jmax(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        return f64::NAN;
    }
    if a > b {
        a
    } else if b > a {
        b
    } else if a == 0.0 && (a.is_sign_positive() || b.is_sign_positive()) {
        0.0
    } else {
        a
    }
}

/// JS `Math.round`: halves go toward +infinity.
pub(crate) fn jround(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let f = x.floor();
    if x - f >= 0.5 {
        f + 1.0
    } else {
        f
    }
}

/// JS `Math.hypot(x, y)` as V8 computes it: scaled by the larger, the
/// squares summed with Kahan's compensation.
pub(crate) fn jhypot(x: f64, y: f64) -> f64 {
    let mut max = 0.0f64;
    let mut nan = false;
    let vals = [x, y];
    let mut abs = [0.0f64; 2];
    for (i, &v) in vals.iter().enumerate() {
        if v.is_nan() {
            nan = true;
        } else {
            abs[i] = v.abs();
            if abs[i] > max {
                max = abs[i];
            }
        }
    }
    if max == f64::INFINITY {
        return f64::INFINITY;
    }
    if nan {
        return f64::NAN;
    }
    if max == 0.0 {
        return 0.0;
    }
    let mut sum = 0.0f64;
    let mut compensation = 0.0f64;
    for a in abs {
        let n = a / max;
        let summand = n * n - compensation;
        let preliminary = sum + summand;
        compensation = (preliminary - sum) - summand;
        sum = preliminary;
    }
    sum.sqrt() * max
}

/// JS ToInt32, what assigning a number into an `Int32Array` does.
pub(crate) fn to_i32(x: f64) -> i32 {
    if !x.is_finite() {
        return 0;
    }
    let m = x.trunc() % 4294967296.0;
    (m as i64 as u32) as i32
}

/// JS `a || b` for numbers: `b` when `a` is 0, -0 or NaN.
#[inline]
pub(crate) fn or(a: f64, b: f64) -> f64 {
    if a != 0.0 && !a.is_nan() {
        a
    } else {
        b
    }
}

/// A stable sort by a JS comparator: `cmp(a, b) < 0` puts `a` first; zero
/// or NaN keeps the two in the order they came. For the consistent
/// comparators Pele passes, this is exactly what V8's `sort` gives.
pub(crate) fn sort_by<T: Copy>(v: &mut [T], mut cmp: impl FnMut(T, T) -> f64) {
    let n = v.len();
    if n < 2 {
        return;
    }
    const RUN: usize = 16;
    let mut s = 0;
    while s < n {
        let end = (s + RUN).min(n);
        for i in s + 1..end {
            let item = v[i];
            let mut j = i;
            while j > s && cmp(item, v[j - 1]) < 0.0 {
                v[j] = v[j - 1];
                j -= 1;
            }
            v[j] = item;
        }
        s += RUN;
    }
    if n <= RUN {
        return;
    }
    let mut buf = v.to_vec();
    let mut in_v = true;
    let mut width = RUN;
    while width < n {
        {
            let (src, dst): (&[T], &mut [T]) = if in_v {
                (&*v, &mut buf[..])
            } else {
                (&buf[..], &mut *v)
            };
            let mut s = 0;
            while s < n {
                let mid = (s + width).min(n);
                let end = (s + 2 * width).min(n);
                let (mut i, mut j, mut k) = (s, mid, s);
                while i < mid && j < end {
                    if cmp(src[j], src[i]) < 0.0 {
                        dst[k] = src[j];
                        j += 1;
                    } else {
                        dst[k] = src[i];
                        i += 1;
                    }
                    k += 1;
                }
                while i < mid {
                    dst[k] = src[i];
                    i += 1;
                    k += 1;
                }
                while j < end {
                    dst[k] = src[j];
                    j += 1;
                    k += 1;
                }
                s += 2 * width;
            }
        }
        in_v = !in_v;
        width *= 2;
    }
    if !in_v {
        v.copy_from_slice(&buf);
    }
}

// ---- Types (layered.ts) --------------------------------------------------

/// Ports `const enum Kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Node = 0,
    Dummy = 1,
    Label = 2,
    StartPort = 3,
    EndPort = 4,
}

const NODE: u8 = Kind::Node as u8;
const DUMMY: u8 = Kind::Dummy as u8;
const LABEL: u8 = Kind::Label as u8;
const START_PORT: u8 = Kind::StartPort as u8;
const END_PORT: u8 = Kind::EndPort as u8;

/// Ports `interface LNode`.
#[derive(Debug, Clone, PartialEq)]
pub struct LNode {
    pub w: f64,
    pub h: f64,
    pub kind: Kind,
    pub pin: f64,
    /// Port's cross-flow destination; NaN if unknown.
    pub hint: f64,
    /// Initial cross-flow position; NaN if unknown.
    pub key: f64,
    /// How much of the side that edges meet may be shared out between them.
    pub span: f64,
    pub x: f64,
    pub y: f64,
    pub rank: i32,
}

/// Ports `interface LEdge`.
#[derive(Debug, Clone, PartialEq)]
pub struct LEdge {
    pub tail: usize,
    pub head: usize,
    pub minlen: f64,
    pub label_w: f64,
    pub label_h: f64,
    pub tail_dx: f64,
    pub head_dx: f64,
    pub tail_fixed: bool,
    pub head_fixed: bool,
    pub reversed: bool,
    pub points: Vec<f64>,
    pub label_x: f64,
    pub label_y: f64,
}

/// `LayeredOptions.groups`: `of` maps nodes to groups, `parent` groups to
/// parents, -1 for none.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Groups {
    pub of: Vec<i32>,
    pub parent: Vec<i32>,
}

/// Ports `interface LayeredOptions`. `head_room` is `headRoom ?? 0`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayeredOptions {
    pub node_sep: f64,
    pub edge_sep: f64,
    pub rank_sep: f64,
    pub port_sep: f64,
    pub tracks: bool,
    pub straight: bool,
    pub apart: bool,
    pub head_room: f64,
    pub groups: Option<Groups>,
    pub order_only: bool,
}

/// Ports `interface LayeredResult`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LayeredResult {
    pub width: f64,
    pub height: f64,
    pub crossings: Option<usize>,
    pub declared: Option<usize>,
}

/// Ports `lnode`.
pub fn lnode(w: f64, h: f64, kind: Kind) -> LNode {
    LNode {
        w,
        h,
        kind,
        pin: 0.0,
        hint: f64::NAN,
        key: f64::NAN,
        span: 0.0,
        x: 0.0,
        y: 0.0,
        rank: 0,
    }
}

/// Ports `ledge` (the defaults are `minlen = 1, labelW = 0, labelH = 0`).
pub fn ledge(tail: usize, head: usize, minlen: f64, label_w: f64, label_h: f64) -> LEdge {
    LEdge {
        tail,
        head,
        minlen,
        label_w,
        label_h,
        tail_dx: 0.0,
        head_dx: 0.0,
        tail_fixed: false,
        head_fixed: false,
        reversed: false,
        points: Vec::new(),
        label_x: 0.0,
        label_y: 0.0,
    }
}

const MAX_SWEEPS: usize = 24;
const MAX_DUMMIES: usize = 20000;
const PORT_PITCH: f64 = 16.0;
const MIN_PITCH: f64 = 8.0;
const MIN_SLOPE: f64 = 0.125;
const MAX_GAP: f64 = 3.0;
const TRACK_MARGIN: f64 = 18.0;
const TRACK_PITCH: f64 = 10.0;
const TRACK_SPREAD: f64 = 14.0;
const LEVEL_WIDE: f64 = 2.0;
const LEVEL_MARGIN: f64 = 28.0;
const LEVEL_PITCH: f64 = 12.0;
const LEVEL_LEAD: f64 = 12.0;
const LEVEL_CLEAR: f64 = 8.0;
const LEVEL_TOUCH: f64 = 4.0;
const CROSSED_SLOPE: f64 = 0.5;
const IN_LINE: f64 = 0.5;
const TWIN: f64 = 1.0;
const SNAP: f64 = 12.0;
const BEND_SNAP: f64 = 24.0;
const LEAN_CLEAR: f64 = 40.0;
const MAX_WEIGHED: usize = 400;
const SAMPLES: usize = 16;

// `const enum End`.
const HELD: u8 = 0;
const LEAVES: u8 = 1;
const ARRIVES: u8 = 2;

/// Ports `isDummy`.
fn is_dummy(kind: u8) -> bool {
    kind == DUMMY || kind == LABEL
}

/// Ports `sortRange` (an insertion sort by `key`, ties by value).
fn sort_range(list: &mut [usize], from: usize, to: usize, key: &[i32]) {
    for i in from + 1..to {
        let item = list[i];
        let k = key[item];
        let mut j = i as isize - 1;
        while j >= from as isize
            && (key[list[j as usize]] > k
                || (key[list[j as usize]] == k && list[j as usize] > item))
        {
            list[(j + 1) as usize] = list[j as usize];
            j -= 1;
        }
        list[(j + 1) as usize] = item;
    }
}

/// Ports `sortByBary`: insertion-sorted runs of 8, then merged bottom up.
fn sort_by_bary(
    list: &mut [usize],
    count: usize,
    bary: &[f64],
    pos: &[i32],
    right: bool,
    spare: &mut [usize],
) {
    let before = |a: usize, b: usize| -> bool {
        bary[a] < bary[b]
            || (bary[a] == bary[b]
                && (if right {
                    pos[a] > pos[b]
                } else {
                    pos[a] < pos[b]
                }))
    };
    const RUN: usize = 8;
    let mut s = 0;
    while s < count {
        let end = count.min(s + RUN);
        for i in s + 1..end {
            let item = list[i];
            let mut j = i as isize - 1;
            while j >= s as isize && before(item, list[j as usize]) {
                list[(j + 1) as usize] = list[j as usize];
                j -= 1;
            }
            list[(j + 1) as usize] = item;
        }
        s += RUN;
    }
    let mut in_list = true;
    let mut width = RUN;
    while width < count {
        {
            let (from, to): (&[usize], &mut [usize]) = if in_list {
                (&*list, &mut *spare)
            } else {
                (&*spare, &mut *list)
            };
            let mut s = 0;
            while s < count {
                let mid = count.min(s + width);
                let end = count.min(s + 2 * width);
                let (mut i, mut j, mut k) = (s, mid, s);
                while i < mid && j < end {
                    if before(from[j], from[i]) {
                        to[k] = from[j];
                        j += 1;
                    } else {
                        to[k] = from[i];
                        i += 1;
                    }
                    k += 1;
                }
                while i < mid {
                    to[k] = from[i];
                    i += 1;
                    k += 1;
                }
                while j < end {
                    to[k] = from[j];
                    j += 1;
                    k += 1;
                }
                s += 2 * width;
            }
        }
        in_list = !in_list;
        width *= 2;
    }
    if !in_list {
        list[..count].copy_from_slice(&spare[..count]);
    }
}

/// Ports `interface Graph`.
struct Graph {
    n: usize,
    real: usize,
    max_rank: usize,
    w: Vec<f64>,
    kind: Vec<u8>,
    rank: Vec<usize>,
    ef: Vec<usize>,
    et: Vec<usize>,
    efd: Vec<f64>,
    etd: Vec<f64>,
    succ_start: Vec<usize>,
    succ_e: Vec<usize>,
    pred_start: Vec<usize>,
    pred_e: Vec<usize>,
    layer_start: Vec<usize>,
    order: Vec<usize>,
    pos: Vec<i32>,
    offsets: bool,
    fixed: Vec<u8>,
    grp: Option<Vec<i32>>,
    group_parent: Option<Vec<i32>>,
}

/// Ports `interface Pieces`.
struct Pieces {
    from: Vec<f64>,
    to: Vec<f64>,
    rank: Vec<usize>,
    lower: Vec<isize>,
    track: Vec<i32>,
    levels: Vec<i32>,
    wide: Vec<u8>,
}

/// Ports `sortRank` (inside `layered`).
fn sort_rank(
    order: &mut [usize],
    layer_start: &[usize],
    r: usize,
    key: impl Fn(usize) -> f64,
) -> bool {
    let s = layer_start[r];
    let e = layer_start[r + 1];
    if e - s < 2 {
        return false;
    }
    for &v in &order[s..e] {
        if key(v).is_nan() {
            return false;
        }
    }
    let mut layer: Vec<(usize, usize)> = order[s..e]
        .iter()
        .enumerate()
        .map(|(i, &v)| (v, i))
        .collect();
    sort_by(&mut layer, |a, b| {
        or(key(a.0) - key(b.0), a.1 as f64 - b.1 as f64)
    });
    for (i, &(v, _)) in layer.iter().enumerate() {
        order[s + i] = v;
    }
    true
}

/// Ports `layered`: lays out one flat graph, writing every node's `x`, `y`
/// and `rank` and every edge's `points` and label.
pub fn layered(nodes: &mut [LNode], edges: &mut [LEdge], opt: &LayeredOptions) -> LayeredResult {
    let real = nodes.len();
    if real == 0 {
        return LayeredResult::default();
    }
    let m = edges.len();
    if real == 1 && m == 0 && nodes[0].kind == Kind::Node {
        let node = &mut nodes[0];
        node.rank = 0;
        node.x = node.w / 2.0;
        node.y = node.h / 2.0;
        return LayeredResult {
            width: node.w,
            height: node.h,
            crossings: None,
            declared: None,
        };
    }

    // Every edge gets a middle rank of its own when labels or parallel edges need the room.
    let mut step: i32 = 1;
    if m > 0 {
        let mut seen = BTreeSet::new();
        for e in edges.iter() {
            let a = e.tail.min(e.head) as u64;
            let b = e.tail.max(e.head) as u64;
            let key = a * real as u64 + b;
            if e.label_w > 0.0 || seen.contains(&key) {
                step = 2;
                break;
            }
            seen.insert(key);
        }
    }

    orient(nodes, edges);
    rank(nodes, edges, step);

    let mut span = alloc::vec![0usize; m];
    let mut dummies = 0usize;
    for ei in 0..m {
        let e = &edges[ei];
        span[ei] = (nodes[e.head].rank as i64 - nodes[e.tail].rank as i64).unsigned_abs() as usize;
        if span[ei] > 1 {
            dummies += span[ei] - 1;
        }
    }
    let mut direct = alloc::vec![0u8; m];
    if dummies > MAX_DUMMIES {
        let mut longest: Vec<usize> = (0..m).filter(|&ei| span[ei] > 1).collect();
        sort_by(&mut longest, |a, b| {
            or(span[b] as f64 - span[a] as f64, a as f64 - b as f64)
        });
        for &ei in &longest {
            if dummies <= MAX_DUMMIES {
                break;
            }
            direct[ei] = 1;
            dummies -= span[ei] - 1;
        }
    }

    let n = real + dummies;
    let mut w = alloc::vec![0.0f64; n];
    let mut h = alloc::vec![0.0f64; n];
    let mut kind = alloc::vec![0u8; n];
    let mut rnk = alloc::vec![0usize; n];
    let mut max_rank = 0usize;
    for i in 0..real {
        let node = &nodes[i];
        w[i] = node.w;
        h[i] = node.h;
        kind[i] = node.kind as u8;
        rnk[i] = node.rank as usize;
        if rnk[i] > max_rank {
            max_rank = rnk[i];
        }
    }

    // The graph that gets ordered and positioned: every edge cut into rank-to-rank segments.
    // (An edge always spans at least one rank; `max(1)` only keeps a degenerate
    // zero-length edge from writing past the arrays.)
    let mut segments = 0usize;
    for ei in 0..m {
        if direct[ei] == 0 {
            segments += span[ei].max(1);
        }
    }
    let mut ef = alloc::vec![0usize; segments];
    let mut et = alloc::vec![0usize; segments];
    let mut efd = alloc::vec![0.0f64; segments];
    let mut etd = alloc::vec![0.0f64; segments];
    let mut first_dummy = alloc::vec![-1isize; m];
    // Which way the edges with a label in each rank head: 1 down, 2 up, 3 both.
    let mut heads = alloc::vec![0u8; max_rank + 1];
    let mut label_node = alloc::vec![-1isize; m];
    let mut spans = false;
    for node in nodes.iter() {
        if node.span > 0.0 && node.kind == Kind::Node {
            spans = true;
        }
    }
    let mut efk: Option<Vec<u8>> = if spans {
        Some(alloc::vec![0u8; segments])
    } else {
        None
    };
    let mut etk: Option<Vec<u8>> = if spans {
        Some(alloc::vec![0u8; segments])
    } else {
        None
    };
    let mut first_seg = alloc::vec![0usize; m];
    let mut last_seg = alloc::vec![0usize; m];
    let mut key = alloc::vec![f64::NAN; n];
    for i in 0..real {
        key[i] = nodes[i].key;
    }
    let mut offsets = false;
    {
        let mut next = real;
        let mut ce = 0usize;
        for ei in 0..m {
            if direct[ei] != 0 {
                continue;
            }
            let e = &edges[ei];
            let a = if e.reversed { e.head } else { e.tail };
            let b = if e.reversed { e.tail } else { e.head };
            let a_dx = if e.reversed { e.head_dx } else { e.tail_dx };
            let b_dx = if e.reversed { e.tail_dx } else { e.head_dx };
            if a_dx != 0.0 || b_dx != 0.0 {
                offsets = true;
            }
            let ra = rnk[a] as i64;
            let rb = rnk[b] as i64;
            let mut label_rank: i64 = -1;
            if e.label_w > 0.0 {
                label_rank = ra + ((rb - ra) >> 1);
                if step == 2 && rb - ra >= 4 {
                    if (label_rank & 1) == 1 {
                        label_rank += if e.reversed { 1 } else { -1 };
                    }
                } else if step == 2 && (label_rank & 1) == 0 {
                    label_rank += if e.reversed { 1 } else { -1 };
                }
            }
            let mut prev = a;
            if rb - ra > 1 {
                first_dummy[ei] = next as isize;
            }
            first_seg[ei] = ce;
            if let Some(k) = efk.as_mut() {
                if !(if e.reversed {
                    e.head_fixed
                } else {
                    e.tail_fixed
                }) {
                    k[ce] = if e.reversed { ARRIVES } else { LEAVES };
                }
            }
            for r in ra + 1..rb {
                let d = next;
                next += 1;
                rnk[d] = r as usize;
                key[d] = key[a] + ((key[b] - key[a]) * (r - ra) as f64) / (rb - ra) as f64;
                if r == label_rank {
                    w[d] = e.label_w;
                    h[d] = e.label_h;
                    kind[d] = LABEL;
                    label_node[ei] = d as isize;
                    heads[r as usize] |= if e.reversed { 2 } else { 1 };
                } else {
                    kind[d] = DUMMY;
                }
                ef[ce] = prev;
                et[ce] = d;
                efd[ce] = if prev == a { a_dx } else { 0.0 };
                ce += 1;
                prev = d;
            }
            ef[ce] = prev;
            et[ce] = b;
            efd[ce] = if prev == a { a_dx } else { 0.0 };
            etd[ce] = b_dx;
            if let Some(k) = etk.as_mut() {
                if !(if e.reversed {
                    e.tail_fixed
                } else {
                    e.head_fixed
                }) {
                    k[ce] = if e.reversed { LEAVES } else { ARRIVES };
                }
            }
            last_seg[ei] = ce;
            ce += 1;
        }
    }

    // Bends belong to the innermost group containing both edge endpoints.
    let mut grp: Option<Vec<i32>> = None;
    if let Some(groups) = &opt.groups {
        let of = &groups.of;
        let parent = &groups.parent;
        let mut gv = alloc::vec![0i32; n];
        gv[..real].copy_from_slice(&of[..real]);
        let depth = |mut gr: i32| -> i32 {
            let mut d = 0;
            while gr >= 0 {
                d += 1;
                gr = parent[gr as usize];
            }
            d
        };
        let lca = |mut x: i32, mut y: i32| -> i32 {
            let mut dx = depth(x);
            let mut dy = depth(y);
            while dx > dy {
                x = parent[x as usize];
                dx -= 1;
            }
            while dy > dx {
                y = parent[y as usize];
                dy -= 1;
            }
            while x != y {
                x = parent[x as usize];
                y = parent[y as usize];
            }
            x
        };
        for ei in 0..m {
            if first_dummy[ei] < 0 {
                continue;
            }
            let e = &edges[ei];
            let gr = lca(of[e.tail], of[e.head]);
            let d0 = first_dummy[ei] as usize;
            for slot in &mut gv[d0..d0 + span[ei] - 1] {
                *slot = gr;
            }
        }
        grp = Some(gv);
    }

    let mut succ_start = alloc::vec![0usize; n + 1];
    let mut pred_start = alloc::vec![0usize; n + 1];
    for e in 0..segments {
        succ_start[ef[e] + 1] += 1;
        pred_start[et[e] + 1] += 1;
    }
    for i in 0..n {
        succ_start[i + 1] += succ_start[i];
        pred_start[i + 1] += pred_start[i];
    }
    let mut succ_e = alloc::vec![0usize; segments];
    let mut pred_e = alloc::vec![0usize; segments];
    {
        let mut sf = succ_start[..n].to_vec();
        let mut pf = pred_start[..n].to_vec();
        for e in 0..segments {
            succ_e[sf[ef[e]]] = e;
            sf[ef[e]] += 1;
            pred_e[pf[et[e]]] = e;
            pf[et[e]] += 1;
        }
    }

    let mut layer_start = alloc::vec![0usize; max_rank + 2];
    for &r in &rnk {
        layer_start[r + 1] += 1;
    }
    for r in 0..=max_rank {
        layer_start[r + 1] += layer_start[r];
    }
    let mut order = alloc::vec![0usize; n];
    let mut pos = alloc::vec![0i32; n];

    // Initial order: depth-first from the top so that subtrees start out contiguous.
    {
        let mut by_rank = alloc::vec![0usize; n];
        let mut fill = layer_start[..max_rank + 1].to_vec();
        for i in 0..n {
            by_rank[fill[rnk[i]]] = i;
            fill[rnk[i]] += 1;
        }
        fill.copy_from_slice(&layer_start[..max_rank + 1]);
        let mut visited = alloc::vec![0u8; n];
        let mut stack: Vec<usize> = Vec::new();
        for &start in &by_rank {
            if visited[start] != 0 {
                continue;
            }
            stack.push(start);
            while let Some(v) = stack.pop() {
                if visited[v] != 0 {
                    continue;
                }
                visited[v] = 1;
                order[fill[rnk[v]]] = v;
                fill[rnk[v]] += 1;
                let mut j = succ_start[v + 1];
                while j > succ_start[v] {
                    j -= 1;
                    let wv = et[succ_e[j]];
                    if visited[wv] == 0 {
                        stack.push(wv);
                    }
                }
            }
        }
    }

    for r in 0..=max_rank {
        sort_rank(&mut order, &layer_start, r, |v| key[v]);
    }

    let mut pinned = false;
    for node in nodes.iter() {
        if node.pin != 0.0 {
            pinned = true;
        }
    }
    if pinned {
        for r in 0..=max_rank {
            let s = layer_start[r];
            let mut layer: Vec<(usize, usize)> = order[s..layer_start[r + 1]]
                .iter()
                .enumerate()
                .map(|(i, &v)| (v, i))
                .collect();
            let pin_of = |v: usize| if v < real { nodes[v].pin } else { 0.0 };
            sort_by(&mut layer, |a, b| {
                or(pin_of(a.0) - pin_of(b.0), a.1 as f64 - b.1 as f64)
            });
            for (i, &(v, _)) in layer.iter().enumerate() {
                order[s + i] = v;
            }
        }
    }
    // Preserve port order to avoid crossings outside the group.
    let mut fixed = alloc::vec![0u8; max_rank + 1];
    {
        let hint = |v: usize| {
            if v < real && (nodes[v].kind == Kind::StartPort || nodes[v].kind == Kind::EndPort) {
                nodes[v].hint
            } else {
                f64::NAN
            }
        };
        for r in [0, max_rank] {
            if sort_rank(&mut order, &layer_start, r, hint) {
                fixed[r] = 1;
            }
        }
    }
    let mut g = Graph {
        n,
        real,
        max_rank,
        w,
        kind,
        rank: rnk,
        ef,
        et,
        efd,
        etd,
        succ_start,
        succ_e,
        pred_start,
        pred_e,
        layer_start,
        order: Vec::new(),
        pos: Vec::new(),
        offsets,
        fixed,
        grp,
        group_parent: opt.groups.as_ref().map(|gr| gr.parent.clone()),
    };
    set_positions(&order, &g.layer_start, max_rank, &mut pos);
    g.order = order;
    g.pos = pos;

    let mut crossings = 0usize;
    let mut declared = 0usize;
    if segments > 0 && max_rank > 0 {
        let (c, d) = reduce_crossings(&mut g, nodes);
        crossings = c;
        declared = d;
        settle_positions(&mut g);
    }
    if opt.order_only {
        // Points are (rank, cross-flow position) pairs.
        let at = position(&mut g, opt);
        for i in 0..real {
            nodes[i].x = at[i];
        }
        for ei in 0..m {
            let e = &mut edges[ei];
            let a = if e.reversed { e.head } else { e.tail };
            let b = if e.reversed { e.tail } else { e.head };
            let mut pts = alloc::vec![g.rank[a] as f64, at[a]];
            if direct[ei] == 0 && first_dummy[ei] >= 0 {
                let d0 = first_dummy[ei] as usize;
                for d in d0..d0 + span[ei] - 1 {
                    pts.push(g.rank[d] as f64);
                    pts.push(at[d]);
                }
            }
            pts.push(g.rank[b] as f64);
            pts.push(at[b]);
            e.points = pts;
        }
        return LayeredResult {
            width: 0.0,
            height: 0.0,
            crossings: Some(crossings),
            declared: Some(declared),
        };
    }

    if let (Some(efk), Some(etk)) = (&efk, &etk) {
        spread(&mut g, nodes, efk, etk);
    }

    let mut xs = position(&mut g, opt);

    let empty = empty_ranks(&g);
    if let (Some(efk), Some(etk)) = (&efk, &etk) {
        let free = free_ends(&g, efk, etk);
        align_labels(&g, nodes, &mut xs, &free, opt, empty.as_deref());
        settle(&mut g, nodes, &xs, efk, etk, &free, empty.as_deref());
        snap(&g, &mut xs, empty.as_deref(), opt);
        for ei in 0..m {
            if direct[ei] != 0 {
                continue;
            }
            let e = &mut edges[ei];
            let a_dx = g.efd[first_seg[ei]];
            let b_dx = g.etd[last_seg[ei]];
            e.tail_dx = if e.reversed { b_dx } else { a_dx };
            e.head_dx = if e.reversed { a_dx } else { b_dx };
        }
    }

    align_ports(&g, &mut xs, empty.as_deref());

    // A rank that holds nothing but the bends of edges is only there because some other edge has a
    // label or a twin. Its bends move onto the straight line between the ends of their edges, and
    // one that gets there is left out of the route, so that the edge is drawn as a single curve.
    let mut on_line = alloc::vec![0u8; n];
    if let Some(empty) = empty.as_deref() {
        if !opt.tracks {
            straighten(
                &g,
                &mut xs,
                empty,
                &first_dummy,
                &span,
                if opt.apart { 0.0 } else { opt.edge_sep },
                &mut on_line,
            );
        }
    }
    if !opt.tracks {
        past_labels(&g, &mut xs, &first_dummy, &span, opt.edge_sep, &mut on_line);
    }
    // An edge is cut where it passes a rank that holds something, and each piece crosses one gap.
    let mut piece_start = alloc::vec![0usize; m + 1];
    let mut pieces: Option<Pieces> = if opt.tracks || (efk.is_some() && !opt.straight) {
        Some(Pieces {
            from: Vec::new(),
            to: Vec::new(),
            rank: Vec::new(),
            lower: Vec::new(),
            track: Vec::new(),
            levels: Vec::new(),
            wide: Vec::new(),
        })
    } else {
        None
    };
    let mut track_count = alloc::vec![0i32; max_rank + 1];
    if let Some(pieces) = pieces.as_mut() {
        for ei in 0..m {
            piece_start[ei] = pieces.from.len();
            if direct[ei] != 0 {
                continue;
            }
            let d0 = first_dummy[ei];
            let end = if d0 < 0 {
                d0
            } else {
                d0 + span[ei] as isize - 1
            };
            // An edge that keeps a bend to stay clear of its twin is drawn through the bend, as it is.
            let mut twin = false;
            let mut d = d0;
            while d < end {
                // So is one that has a bend left out in a rank of labels: its curve crosses that rank.
                let du = d as usize;
                let bare = empty.as_ref().is_some_and(|em| em[g.rank[du]] == 1);
                if !opt.tracks && bare != (on_line[du] == 1) {
                    twin = true;
                }
                d += 1;
            }
            if twin {
                continue;
            }
            let fs = first_seg[ei];
            let mut x = xs[g.ef[fs]] + g.efd[fs];
            let mut rank = g.rank[g.ef[fs]];
            let mut d = d0;
            while d < end {
                let du = d as usize;
                d += 1;
                if empty.as_ref().is_some_and(|em| em[g.rank[du]] != 0) {
                    continue;
                }
                pieces.from.push(x);
                pieces.to.push(xs[du]);
                pieces.rank.push(rank);
                pieces.lower.push(du as isize);
                x = xs[du];
                rank = g.rank[du];
            }
            let ls = last_seg[ei];
            pieces.from.push(x);
            pieces.to.push(xs[g.et[ls]] + g.etd[ls]);
            pieces.rank.push(rank);
            pieces.lower.push(-1);
        }
        piece_start[m] = pieces.from.len();
    }

    // Ranks that hold only border ports or their pass-through dummies need little room of their own.
    let port_only = |r: usize, k: u8| -> bool {
        let mut found = false;
        for i in g.layer_start[r]..g.layer_start[r + 1] {
            let kk = g.kind[g.order[i]];
            if kk == k {
                found = true;
            } else if kk != DUMMY {
                return false;
            }
        }
        found
    };
    let start_ranks: i64 = if port_only(0, START_PORT) {
        step as i64
    } else {
        0
    };
    let end_ranks: i64 = if port_only(max_rank, END_PORT) {
        step as i64
    } else {
        0
    };
    // The longest way across that any edge travels in the gap below each rank.
    let mut run = alloc::vec![0.0f64; max_rank + 1];
    for e in 0..segments {
        let across = (xs[g.ef[e]] + g.efd[e] - xs[g.et[e]] - g.etd[e]).abs();
        if across > run[g.rank[g.ef[e]]] {
            run[g.rank[g.ef[e]]] = across;
        }
    }
    let mut band_top = alloc::vec![0.0f64; max_rank + 1];
    let mut band_bottom = alloc::vec![0.0f64; max_rank + 1];
    let usual = |r: usize| -> f64 {
        (if (r as i64) < start_ranks || (r as i64) >= max_rank as i64 - end_ranks {
            opt.port_sep
        } else {
            opt.rank_sep
        }) / step as f64
    };
    // The room under each rank. Edges that are not cut into pieces need more where they run far across.
    let mut gap_under = alloc::vec![0.0f64; max_rank + 1];
    for r in 0..max_rank {
        gap_under[r] = if pieces.is_some() {
            usual(r)
        } else {
            jmax(
                usual(r),
                jmin((MAX_GAP * opt.rank_sep) / step as f64, run[r] * MIN_SLOPE),
            )
        };
    }
    // A rank between two ranks of nodes, which holds labels and bends only.
    let mut between = alloc::vec![1u8; max_rank + 1];
    for i in 0..real {
        between[g.rank[i]] = 0;
    }
    for r in 1..max_rank {
        if opt.tracks || between[r] == 0 || (heads[r] != 1 && heads[r] != 2) {
            continue;
        }
        let shift = jmin(
            opt.head_room / 2.0,
            gap_under[if heads[r] == 1 { r - 1 } else { r }] / 2.0,
        );
        gap_under[r - 1] += if heads[r] == 1 { -shift } else { shift };
        gap_under[r] += if heads[r] == 1 { shift } else { -shift };
    }
    let margin = if opt.tracks {
        TRACK_MARGIN
    } else {
        LEVEL_MARGIN
    };
    let least_pitch = if opt.tracks { TRACK_PITCH } else { LEVEL_PITCH };
    let most_pitch = if opt.tracks {
        TRACK_SPREAD
    } else {
        LEVEL_PITCH
    };
    let mut below = alloc::vec![0usize; max_rank + 1];
    if let Some(pieces) = pieces.as_mut() {
        let mut room = alloc::vec![0.0f64; max_rank + 1];
        for r in 0..max_rank {
            if empty.as_ref().is_some_and(|em| em[r] != 0) {
                continue;
            }
            let mut next = r + 1;
            room[r] = gap_under[r];
            while next < max_rank && empty.as_ref().is_some_and(|em| em[next] != 0) {
                room[r] += gap_under[next];
                next += 1;
            }
            below[r] = next;
        }
        for k in 0..pieces.from.len() {
            let wide = (pieces.from[k] - pieces.to[k]).abs() > LEVEL_WIDE * room[pieces.rank[k]];
            pieces.wide.push(if wide { 1 } else { 0 });
        }
        let mut tall = alloc::vec![0.0f64; max_rank + 1];
        assign_tracks(pieces, &mut track_count, !opt.tracks, &room, &mut tall);
        for r in 0..max_rank {
            if track_count[r] > 0 {
                gap_under[r] += jmax(
                    0.0,
                    2.0 * margin + (track_count[r] - 1) as f64 * least_pitch - room[r],
                );
            } else if tall[r] > 0.0 {
                gap_under[r] += jmax(0.0, jmin(MAX_GAP * opt.rank_sep, tall[r]) - room[r]);
            }
        }
    }
    let mut y = 0.0f64;
    for r in 0..=max_rank {
        let mut hh = 0.0f64;
        for i in g.layer_start[r]..g.layer_start[r + 1] {
            if h[g.order[i]] > hh {
                hh = h[g.order[i]];
            }
        }
        band_top[r] = y;
        band_bottom[r] = y + hh;
        y += hh + gap_under[r];
    }
    let height = y;
    // The part of its rank that an edge runs straight through at a bend or label.
    let inset = |d: usize| -> f64 {
        if g.kind[d] == DUMMY && between[g.rank[d]] != 0 && !opt.tracks {
            (band_bottom[g.rank[d]] - band_top[g.rank[d]]) / 4.0
        } else {
            0.0
        }
    };
    // Where a piece runs across its gap, or would on another track of its group.
    let track_y = |pieces: &Pieces, piece: usize, on_track: i32| -> f64 {
        let r = pieces.rank[piece];
        let top = band_bottom[r];
        let bottom = band_top[below[r]];
        let count = if opt.tracks {
            track_count[r]
        } else {
            pieces.levels[piece]
        };
        let pitch = if count > 1 {
            jmin(
                most_pitch,
                (bottom - top - 2.0 * margin) / (count - 1) as f64,
            )
        } else {
            0.0
        };
        (top + bottom) / 2.0 + (on_track as f64 - (count - 1) as f64 / 2.0) * pitch
    };

    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    for i in 0..n {
        let half = g.w[i] / 2.0;
        if xs[i] - half < min_x {
            min_x = xs[i] - half;
        }
        if xs[i] + half > max_x {
            max_x = xs[i] + half;
        }
    }
    // Where edges spread along their nodes, a rank whose nodes only send edges on stands on one
    // line, and a rank whose nodes only receive them hangs from one.
    let mut arrives = alloc::vec![0u8; max_rank + 1];
    let mut leaves = alloc::vec![0u8; max_rank + 1];
    for i in 0..real {
        if g.kind[i] != NODE || efk.is_none() {
            continue;
        }
        if g.pred_start[i + 1] > g.pred_start[i] {
            arrives[g.rank[i]] = 1;
        }
        if g.succ_start[i + 1] > g.succ_start[i] {
            leaves[g.rank[i]] = 1;
        }
    }
    for i in 0..real {
        let node = &mut nodes[i];
        let r = node.rank as usize;
        node.x = xs[i] - min_x;
        if g.kind[i] == NODE && leaves[r] != 0 && arrives[r] == 0 {
            node.y = band_bottom[r] - node.h / 2.0;
        } else if g.kind[i] == NODE && arrives[r] != 0 && leaves[r] == 0 {
            node.y = band_top[r] + node.h / 2.0;
        } else {
            node.y = (band_top[r] + band_bottom[r]) / 2.0;
        }
    }

    for ei in 0..m {
        let (fx, fy, fh, lx, ly, lh) = {
            let e = &edges[ei];
            let first = &nodes[if e.reversed { e.head } else { e.tail }];
            let last = &nodes[if e.reversed { e.tail } else { e.head }];
            (first.x, first.y, first.h, last.x, last.y, last.h)
        };
        let e = &mut edges[ei];
        let mut pts: Vec<f64> = alloc::vec![
            fx + (if e.reversed { e.head_dx } else { e.tail_dx }),
            fy + fh / 2.0
        ];
        let d0 = first_dummy[ei];
        match pieces.as_ref() {
            Some(pieces) if piece_start[ei + 1] > piece_start[ei] => {
                for k in piece_start[ei]..piece_start[ei + 1] {
                    if pieces.track[k] >= 0 && (opt.tracks || pieces.wide[k] == 1) {
                        let across = track_y(pieces, k, pieces.track[k]);
                        pts.push(pieces.from[k] - min_x);
                        pts.push(across);
                        pts.push(pieces.to[k] - min_x);
                        pts.push(across);
                    } else if pieces.track[k] > 0 {
                        // A short piece among levels runs straight until it is past the level before its
                        // own, and turns in one S from there, so that it crosses none of them.
                        pts.push(pieces.from[k] - min_x);
                        pts.push(track_y(pieces, k, pieces.track[k] - 1));
                    }
                    let d = pieces.lower[k];
                    if d < 0 {
                        continue;
                    }
                    let d = d as usize;
                    let rd = g.rank[d];
                    pts.push(xs[d] - min_x);
                    pts.push(band_top[rd] + inset(d));
                    if band_bottom[rd] > band_top[rd] {
                        pts.push(xs[d] - min_x);
                        pts.push(band_bottom[rd] - inset(d));
                    }
                }
            }
            _ => {
                if d0 >= 0 {
                    let d0 = d0 as usize;
                    for d in d0..d0 + span[ei] - 1 {
                        if on_line[d] != 0 {
                            continue;
                        }
                        let x = xs[d] - min_x;
                        let top = band_top[g.rank[d]];
                        let bottom = band_bottom[g.rank[d]];
                        pts.push(x);
                        pts.push(top + inset(d));
                        if bottom > top {
                            pts.push(x);
                            pts.push(bottom - inset(d));
                        }
                    }
                }
            }
        }
        pts.push(lx + (if e.reversed { e.tail_dx } else { e.head_dx }));
        pts.push(ly - lh / 2.0);
        if e.reversed {
            let mut i = 0usize;
            let mut j = pts.len() - 2;
            while i < j {
                pts.swap(i, j);
                pts.swap(i + 1, j + 1);
                i += 2;
                j -= 2;
            }
        }
        let ln = label_node[ei];
        if ln >= 0 {
            let ln = ln as usize;
            e.label_x = xs[ln] - min_x;
            e.label_y = (band_top[g.rank[ln]] + band_bottom[g.rank[ln]]) / 2.0;
        } else if e.label_w > 0.0 {
            e.label_x = (pts[0] + pts[pts.len() - 2]) / 2.0;
            e.label_y = (pts[1] + pts[pts.len() - 1]) / 2.0;
        }
        e.points = pts;
    }

    LayeredResult {
        width: max_x - min_x,
        height,
        crossings: None,
        declared: None,
    }
}

/// `setPositions` inside `layered`.
fn set_positions(order: &[usize], layer_start: &[usize], max_rank: usize, pos: &mut [i32]) {
    for r in 0..=max_rank {
        let s = layer_start[r];
        for i in s..layer_start[r + 1] {
            pos[order[i]] = (i - s) as i32;
        }
    }
}

/// `settlePositions` inside `reduceCrossings` (and `setPositions`).
fn settle_positions(g: &mut Graph) {
    let Graph {
        order,
        layer_start,
        pos,
        max_rank,
        ..
    } = g;
    set_positions(order, layer_start, *max_rank, pos);
}

/// Ports `orient`: reverses the edges that close a cycle, found by
/// depth-first search in declaration order.
pub fn orient(nodes: &[LNode], edges: &mut [LEdge]) {
    let n = nodes.len();
    let mut start = alloc::vec![0usize; n + 1];
    for e in edges.iter_mut() {
        e.reversed = nodes[e.head].kind == Kind::StartPort || nodes[e.tail].kind == Kind::EndPort;
        if !e.reversed {
            start[e.tail + 1] += 1;
        }
    }
    for i in 0..n {
        start[i + 1] += start[i];
    }
    let mut out = alloc::vec![0usize; start[n]];
    {
        let mut fill = start[..n].to_vec();
        for (i, e) in edges.iter().enumerate() {
            if !e.reversed {
                out[fill[e.tail]] = i;
                fill[e.tail] += 1;
            }
        }
    }
    let mut state = alloc::vec![0u8; n];
    let mut cursor = start[..n].to_vec();
    let mut stack: Vec<usize> = Vec::new();
    for root in 0..n {
        if state[root] != 0 {
            continue;
        }
        stack.push(root);
        state[root] = 1;
        while let Some(&v) = stack.last() {
            if cursor[v] < start[v + 1] {
                let ei = out[cursor[v]];
                cursor[v] += 1;
                let wv = edges[ei].head;
                if state[wv] == 1 {
                    edges[ei].reversed = true;
                } else if state[wv] == 0 {
                    state[wv] = 1;
                    stack.push(wv);
                }
            } else {
                state[v] = 2;
                stack.pop();
            }
        }
    }
}

/// Ports `rank`: longest-path ranks, then each node moved toward the side
/// with more edges; ports go above and below everything.
pub fn rank(nodes: &mut [LNode], edges: &[LEdge], step: i32) {
    let n = nodes.len();
    let m = edges.len();
    let mut from = alloc::vec![0usize; m];
    let mut to = alloc::vec![0usize; m];
    let mut len = alloc::vec![0i32; m];
    let mut out_start = alloc::vec![0usize; n + 1];
    let mut in_start = alloc::vec![0usize; n + 1];
    for (i, e) in edges.iter().enumerate() {
        let a = if e.reversed { e.head } else { e.tail };
        let b = if e.reversed { e.tail } else { e.head };
        from[i] = a;
        to[i] = b;
        len[i] = to_i32(jmax(1.0, e.minlen) * step as f64);
        out_start[a + 1] += 1;
        in_start[b + 1] += 1;
    }
    for i in 0..n {
        out_start[i + 1] += out_start[i];
        in_start[i + 1] += in_start[i];
    }
    let mut out_e = alloc::vec![0usize; m];
    let mut in_e = alloc::vec![0usize; m];
    {
        let mut of = out_start[..n].to_vec();
        let mut inf = in_start[..n].to_vec();
        for i in 0..m {
            out_e[of[from[i]]] = i;
            of[from[i]] += 1;
            in_e[inf[to[i]]] = i;
            inf[to[i]] += 1;
        }
    }

    let mut r = alloc::vec![0i32; n];
    // Start ports go above every node later, so they don't push their targets down a rank.
    for i in 0..n {
        if nodes[i].kind == Kind::StartPort {
            r[i] = -step;
        }
    }
    let mut indeg = alloc::vec![0i32; n];
    for i in 0..n {
        indeg[i] = (in_start[i + 1] - in_start[i]) as i32;
    }
    let mut topo = alloc::vec![0usize; n];
    let mut count = 0usize;
    for i in 0..n {
        if indeg[i] == 0 {
            topo[count] = i;
            count += 1;
        }
    }
    let mut q = 0;
    while q < count {
        let v = topo[q];
        for k in out_start[v]..out_start[v + 1] {
            let ei = out_e[k];
            let wv = to[ei];
            if r[v] + len[ei] > r[wv] {
                r[wv] = r[v] + len[ei];
            }
            indeg[wv] -= 1;
            if indeg[wv] == 0 {
                topo[count] = wv;
                count += 1;
            }
        }
        q += 1;
    }

    // Longest-path ranking leaves loosely attached nodes far from their neighbours.
    // Move each node toward the side that has more edges.
    for _pass in 0..8 {
        let mut changed = false;
        for q in (0..count).rev() {
            let v = topo[q];
            let ins = in_start[v + 1] - in_start[v];
            let outs = out_start[v + 1] - out_start[v];
            if ins == 0 && outs == 0 {
                continue;
            }
            let mut lo = f64::NEG_INFINITY;
            let mut hi = f64::INFINITY;
            for k in in_start[v]..in_start[v + 1] {
                let ei = in_e[k];
                lo = jmax(lo, (r[from[ei]] + len[ei]) as f64);
            }
            for k in out_start[v]..out_start[v + 1] {
                let ei = out_e[k];
                hi = jmin(hi, (r[to[ei]] - len[ei]) as f64);
            }
            let mut next = r[v] as f64;
            if outs > ins && hi != f64::INFINITY {
                next = hi;
            } else if ins > outs && lo != f64::NEG_INFINITY {
                next = lo;
            }
            if next != r[v] as f64 {
                r[v] = to_i32(next);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut start_ports = false;
    for i in 0..n {
        let kind = nodes[i].kind;
        if kind == Kind::StartPort {
            start_ports = true;
        }
        if kind == Kind::StartPort || kind == Kind::EndPort {
            continue;
        }
        if (r[i] as f64) < min {
            min = r[i] as f64;
        }
        if (r[i] as f64) > max {
            max = r[i] as f64;
        }
    }
    if min == f64::INFINITY {
        min = 0.0;
        max = 0.0;
    }
    for i in 0..n {
        if nodes[i].kind == Kind::StartPort {
            r[i] = to_i32(min - step as f64);
        } else if nodes[i].kind == Kind::EndPort {
            r[i] = to_i32(max + step as f64);
        }
    }
    if start_ports {
        min -= step as f64;
    }
    for i in 0..n {
        nodes[i].rank = (r[i] as f64 - min) as i32;
    }
}

/// Ports `sideEnds`: the ends of edges that may move along one side of a
/// node, ordered by their opposite nodes, and their minimum gaps. Returns
/// the sum of the gaps.
fn side_ends(
    g: &Graph,
    v: usize,
    side: usize,
    span: f64,
    kinds: &[u8],
    ids: &mut Vec<usize>,
    room: &mut Vec<f64>,
) -> f64 {
    let (start, list, other) = if side == 0 {
        (&g.pred_start, &g.pred_e, &g.ef)
    } else {
        (&g.succ_start, &g.succ_e, &g.et)
    };
    let pos = &g.pos;
    for k in start[v]..start[v + 1] {
        if kinds[list[k]] != HELD {
            ids.push(list[k]);
        }
    }
    if ids.len() < 2 {
        return 0.0;
    }
    sort_by(ids, |a, b| {
        or((pos[other[a]] - pos[other[b]]) as f64, a as f64 - b as f64)
    });
    let ids: &[usize] = ids;
    room.clear();
    room.resize(ids.len(), 0.0);
    // mode 0: `() => 1`; 1: `(leaves) => (leaves ? 0.5 : 1)`; 2: `(_leaves, same) => (same ? 0 : 1)`.
    let fill = |room: &mut Vec<f64>, mode: u8| -> f64 {
        let mut total = 0.0;
        room[0] = 0.0;
        for k in 1..ids.len() {
            let same = kinds[ids[k]] == kinds[ids[k - 1]];
            let leaves = same && kinds[ids[k]] == LEAVES;
            room[k] = match mode {
                0 => 1.0,
                1 => {
                    if leaves {
                        0.5
                    } else {
                        1.0
                    }
                }
                _ => {
                    if same {
                        0.0
                    } else {
                        1.0
                    }
                }
            };
            total += room[k];
        }
        total
    };
    let mut total = fill(room, 0);
    if span / total < MIN_PITCH {
        total = fill(room, 1);
    }
    if span / total < MIN_PITCH {
        total = fill(room, 2);
    }
    if total == 0.0 {
        return 0.0;
    }
    let pitch = jmin(PORT_PITCH, span / total);
    for r in room.iter_mut().skip(1) {
        *r *= pitch;
    }
    total * pitch
}

/// Ports `spread`: gives the edges that meet one side of a node places of
/// their own along it, around its middle.
fn spread(g: &mut Graph, nodes: &[LNode], efk: &[u8], etk: &[u8]) {
    for v in 0..g.real {
        let span = jmin(nodes[v].span, g.w[v]);
        if span <= 0.0 || nodes[v].kind != Kind::Node {
            continue;
        }
        for side in 0..2 {
            let mut ids: Vec<usize> = Vec::new();
            let mut room: Vec<f64> = Vec::new();
            let total = side_ends(
                g,
                v,
                side,
                span,
                if side == 0 { etk } else { efk },
                &mut ids,
                &mut room,
            );
            if total == 0.0 {
                continue;
            }
            let dx = if side == 0 { &mut g.etd } else { &mut g.efd };
            let mut at = -total / 2.0;
            for k in 0..ids.len() {
                at += room[k];
                dx[ids[k]] = at;
            }
        }
    }
}

/// Ports `follow`: the segment of an edge that reaches the next point held
/// in place, on the way up from segment `e` (side 0) or down.
fn follow(g: &Graph, empty: Option<&[u8]>, mut e: usize, side: usize) -> usize {
    let Some(empty) = empty else {
        return e;
    };
    for _hops in 0..100000 {
        let u = if side == 0 { g.ef[e] } else { g.et[e] };
        if g.kind[u] != DUMMY || empty[g.rank[u]] == 0 {
            break;
        }
        e = if side == 0 {
            g.pred_e[g.pred_start[u]]
        } else {
            g.succ_e[g.succ_start[u]]
        };
    }
    e
}

/// Ports `alignPorts`: aligns border ports with the ends on their nodes so
/// edges run straight across the gap.
fn align_ports(g: &Graph, xs: &mut [f64], empty: Option<&[u8]>) {
    for (r, kind) in [(0, START_PORT), (g.max_rank, END_PORT)] {
        let s = g.layer_start[r];
        let end = g.layer_start[r + 1];
        let mut to: Vec<f64> = Vec::new();
        for i in s..end {
            let v = g.order[i];
            let start = kind == START_PORT;
            let degree = if start {
                g.succ_start[v + 1] - g.succ_start[v]
            } else {
                g.pred_start[v + 1] - g.pred_start[v]
            };
            if g.kind[v] != kind || degree != 1 {
                break;
            }
            let e = follow(
                g,
                empty,
                if start {
                    g.succ_e[g.succ_start[v]]
                } else {
                    g.pred_e[g.pred_start[v]]
                },
                if start { 1 } else { 0 },
            );
            let at = if start {
                xs[g.et[e]] + g.etd[e]
            } else {
                xs[g.ef[e]] + g.efd[e]
            };
            if let Some(&last) = to.last() {
                if at < last + TWIN {
                    break;
                }
            }
            to.push(at);
        }
        if to.is_empty() || to.len() != end - s {
            continue;
        }
        for i in s..end {
            xs[g.order[i]] = to[i - s];
        }
    }
}

/// Ports `freeEnds`: how many ends that may move along it each node has on
/// the side its edges arrive at, and on the other.
fn free_ends(g: &Graph, efk: &[u8], etk: &[u8]) -> [Vec<i32>; 2] {
    let mut free = [alloc::vec![0i32; g.n], alloc::vec![0i32; g.n]];
    for e in 0..g.ef.len() {
        if etk[e] != HELD {
            free[0][g.et[e]] += 1;
        }
        if efk[e] != HELD {
            free[1][g.ef[e]] += 1;
        }
    }
    free
}

/// Ports `ease`: how far along the flow the handles of a plain S reach, as
/// a share of the way it travels along it.
pub fn ease(across: f64, along: f64) -> f64 {
    jmax(0.25, jmin(0.5, (4.0 * along.abs()) / or(across.abs(), 1.0)))
}

/// Ports `alignLabels`: brings the labels and bends of each edge into line
/// with an end of the edge.
fn align_labels(
    g: &Graph,
    nodes: &[LNode],
    xs: &mut [f64],
    free: &[Vec<i32>; 2],
    opt: &LayeredOptions,
    empty: Option<&[u8]>,
) {
    let sep = opt.edge_sep;
    let Graph {
        n,
        real,
        max_rank,
        kind,
        rank,
        w,
        ef,
        et,
        efd,
        etd,
        succ_start,
        succ_e,
        pred_start,
        pred_e,
        layer_start,
        order,
        pos,
        ..
    } = g;
    let (n, real, max_rank) = (*n, *real, *max_rank);
    let placed = |v: usize| {
        kind[v] == LABEL || (kind[v] == DUMMY && empty.is_none_or(|em| em[rank[v]] != 1))
    };
    let end = |xs: &[f64], e: usize, side: usize| -> (f64, i32) {
        let u = if side == 0 { ef[e] } else { et[e] };
        let x = if side == 0 {
            xs[u] + efd[e]
        } else {
            xs[u] + etd[e]
        };
        (
            x,
            if u < real && nodes[u].kind == Kind::Node {
                free[1 - side][u]
            } else {
                0
            },
        )
    };
    let room = |xs: &[f64], v: usize| -> (f64, f64) {
        let s = layer_start[rank[v]];
        let i = s + pos[v] as usize;
        let clear = |other: usize| -> f64 {
            w[other] / 2.0
                + w[v] / 2.0
                + (if kind[other] == NODE {
                    opt.node_sep / 2.0 + sep / 2.0
                } else if kind[other] == kind[v] {
                    sep
                } else {
                    sep / 2.0
                })
        };
        (
            if i > s {
                xs[order[i - 1]] + clear(order[i - 1])
            } else {
                f64::NEG_INFINITY
            },
            if i + 1 < layer_start[rank[v] + 1] {
                xs[order[i + 1]] - clear(order[i + 1])
            } else {
                f64::INFINITY
            },
        )
    };
    let choose = |x: f64, up: (f64, i32), down: (f64, i32), lo: f64, hi: f64| -> f64 {
        let up_first = if up.1 != down.1 {
            up.1 < down.1
        } else {
            (up.0 - x).abs() <= (down.0 - x).abs()
        };
        let choices = if up_first {
            [up.0, down.0]
        } else {
            [down.0, up.0]
        };
        match choices.iter().find(|&&c| c >= lo && c <= hi) {
            Some(&c) => c,
            None => jmax(lo, jmin(hi, choices[0])),
        }
    };
    let mut done = alloc::vec![0u8; n];
    let mut chain: Vec<usize> = Vec::new();
    for r in 1..max_rank {
        for i in layer_start[r]..layer_start[r + 1] {
            if !placed(order[i]) || done[order[i]] != 0 {
                continue;
            }
            // The edge's labels and bends from its first node to its last, and the room they share.
            let mut top = order[i];
            while is_dummy(kind[ef[pred_e[pred_start[top]]]]) {
                top = ef[pred_e[pred_start[top]]];
            }
            let mut bottom = top;
            let mut lo = f64::NEG_INFINITY;
            let mut hi = f64::INFINITY;
            chain.clear();
            let mut d = top;
            while is_dummy(kind[d]) {
                bottom = d;
                if placed(d) {
                    done[d] = 1;
                    chain.push(d);
                    let (least, most) = room(xs, d);
                    if least > lo {
                        lo = least;
                    }
                    if most < hi {
                        hi = most;
                    }
                }
                d = et[succ_e[succ_start[d]]];
            }
            if lo <= hi {
                let x = choose(
                    xs[chain[0]],
                    end(xs, pred_e[pred_start[top]], 0),
                    end(xs, succ_e[succ_start[bottom]], 1),
                    lo,
                    hi,
                );
                for &d in &chain {
                    xs[d] = x;
                }
                continue;
            }
            for &d in &chain {
                let (least, most) = room(xs, d);
                if least > most {
                    continue;
                }
                let up = end(xs, follow(g, empty, pred_e[pred_start[d]], 0), 0);
                let down = end(xs, follow(g, empty, succ_e[succ_start[d]], 1), 1);
                xs[d] = choose(xs[d], up, down, least, most);
            }
        }
    }
}

/// `wanted` inside `settle`: where the end of segment `e` on node `v`
/// should be, from the middle of its side.
#[allow(clippy::too_many_arguments)]
fn settle_wanted(
    g: &Graph,
    nodes: &[LNode],
    xs: &[f64],
    efk: &[u8],
    etk: &[u8],
    free: &[Vec<i32>; 2],
    empty: Option<&[u8]>,
    e: usize,
    side: usize,
    v: usize,
    span: f64,
    again: bool,
) -> f64 {
    if free[side][v] < 2 {
        return 0.0;
    }
    let e = follow(g, empty, e, side);
    let u = if side == 0 { g.ef[e] } else { g.et[e] };
    let point = if side == 0 {
        xs[u] + g.efd[e]
    } else {
        xs[u] + g.etd[e]
    };
    // Once the other end has its place, this one comes into line with it if it can.
    if again && (point - xs[v]).abs() <= span / 2.0 + IN_LINE {
        return jmax(-span / 2.0, jmin(span / 2.0, point - xs[v]));
    }
    // The part of the other side that its end may move along, if it may move.
    let mut middle = point;
    let mut width = 0.0;
    if u < g.real
        && nodes[u].kind == Kind::Node
        && (if side == 0 { efk } else { etk })[e] != HELD
        && free[1 - side][u] >= 2
    {
        width = jmax(0.0, jmin(nodes[u].span, g.w[u]));
        if width > 0.0 {
            middle = xs[u];
        }
    }
    let lo = jmax(xs[v] - span / 2.0, middle - width / 2.0);
    let hi = jmin(xs[v] + span / 2.0, middle + width / 2.0);
    // Two sides with no part in line: the end leans towards the other, if its side is long enough.
    if lo > hi + IN_LINE {
        let lean = jmax(0.0, span / 2.0 - LEAN_CLEAR);
        return jmax(-lean, jmin(lean, point - xs[v]));
    }
    // Both ends settle on one place in the part the two sides share: nearer the middle of the narrower.
    let place = (xs[v] * width + middle * span) / (width + span);
    jmax(lo, jmin(hi, place)) - xs[v]
}

/// One side of a node in `settle`.
struct SettleSide {
    v: usize,
    side: usize,
    span: f64,
    ids: Vec<usize>,
    room: Vec<f64>,
    all: Vec<usize>,
}

/// Ports `settle`: once the nodes have their places, moves an end along its
/// side when that lets its edge run straight.
fn settle(
    g: &mut Graph,
    nodes: &[LNode],
    xs: &[f64],
    efk: &[u8],
    etk: &[u8],
    free: &[Vec<i32>; 2],
    empty: Option<&[u8]>,
) {
    // End order and spacing stay fixed across passes; held ends divide free ends into runs.
    let mut sides: Vec<SettleSide> = Vec::new();
    for v in 0..g.real {
        let span = jmin(nodes[v].span, g.w[v]);
        if span <= 0.0 || nodes[v].kind != Kind::Node {
            continue;
        }
        for side in 0..2 {
            let mut ids: Vec<usize> = Vec::new();
            let mut room: Vec<f64> = Vec::new();
            side_ends(
                g,
                v,
                side,
                span,
                if side == 0 { etk } else { efk },
                &mut ids,
                &mut room,
            );
            if ids.is_empty() {
                continue;
            }
            if ids.len() == 1 {
                room.clear();
                room.push(0.0);
            }
            let (other, start, list) = if side == 0 {
                (&g.ef, &g.pred_start, &g.pred_e)
            } else {
                (&g.et, &g.succ_start, &g.succ_e)
            };
            let mut all: Vec<usize> = list[start[v]..start[v + 1]].to_vec();
            let pos = &g.pos;
            sort_by(&mut all, |a, b| {
                or((pos[other[a]] - pos[other[b]]) as f64, a as f64 - b as f64)
            });
            sides.push(SettleSide {
                v,
                side,
                span,
                ids,
                room,
                all,
            });
        }
    }
    // The ends of an edge follow each other, so the nodes are gone over until none moves.
    let mut pass = 0;
    let mut moved = true;
    while moved && pass < 6 {
        moved = false;
        for sd in &sides {
            let kinds = if sd.side == 0 { etk } else { efk };
            let get = |g: &Graph, id: usize| if sd.side == 0 { g.etd[id] } else { g.efd[id] };
            let mut k = 0usize;
            let mut lo = -sd.span / 2.0;
            for i in 0..=sd.all.len() {
                let held = i < sd.all.len() && kinds[sd.all[i]] == HELD;
                if i < sd.all.len() && !held {
                    continue;
                }
                // Free ends since the last held end occupy ids[k..k + size).
                let mut size = 0usize;
                let mut j = i as isize - 1;
                while j >= 0 && kinds[sd.all[j as usize]] != HELD {
                    size += 1;
                    j -= 1;
                }
                let hi = if held {
                    get(g, sd.all[i]) - PORT_PITCH
                } else {
                    sd.span / 2.0
                };
                if size > 0 {
                    // Isotonic fitting preserves order and spacing by pooling neighbours that are too close.
                    let mut sum: Vec<f64> = Vec::new();
                    let mut count: Vec<usize> = Vec::new();
                    let mut before = 0.0f64;
                    for j in 0..size {
                        if j > 0 {
                            before += sd.room[k + j];
                        }
                        let want = settle_wanted(
                            g,
                            nodes,
                            xs,
                            efk,
                            etk,
                            free,
                            empty,
                            sd.ids[k + j],
                            sd.side,
                            sd.v,
                            sd.span,
                            pass > 0,
                        );
                        sum.push(want - before);
                        count.push(1);
                        // Keep ends together when spacing forced them to share a place.
                        let mut together = j > 0 && sd.room[k + j] == 0.0;
                        loop {
                            let l = sum.len();
                            if l <= 1
                                || !(together
                                    || sum[l - 2] / count[l - 2] as f64
                                        > sum[l - 1] / count[l - 1] as f64)
                            {
                                break;
                            }
                            together = false;
                            let top = sum.pop().unwrap_or(0.0);
                            let pooled = count.pop().unwrap_or(0);
                            let l = sum.len();
                            sum[l - 1] += top;
                            count[l - 1] += pooled;
                        }
                    }
                    let total = before;
                    let mut j = 0usize;
                    before = 0.0;
                    for b in 0..sum.len() {
                        // Center the run if it cannot fit between held ends.
                        let level = if hi - total < lo {
                            (lo + hi - total) / 2.0
                        } else {
                            jmax(lo, jmin(hi - total, sum[b] / count[b] as f64))
                        };
                        for _c in 0..count[b] {
                            if j > 0 {
                                before += sd.room[k + j];
                            }
                            let id = sd.ids[k + j];
                            if (get(g, id) - level - before).abs() > 0.01 {
                                moved = true;
                            }
                            if sd.side == 0 {
                                g.etd[id] = level + before;
                            } else {
                                g.efd[id] = level + before;
                            }
                            j += 1;
                        }
                    }
                    k += size;
                }
                if held {
                    lo = get(g, sd.all[i]) + PORT_PITCH;
                }
            }
        }
        pass += 1;
    }
}

/// Ports `snap`: brings edges that are nearly straight into line, by moving
/// a node or bend a little across the flow along with everything it is in
/// line with already.
fn snap(g: &Graph, xs: &mut [f64], empty: Option<&[u8]>, opt: &LayeredOptions) {
    let n = g.n;
    let passed = |u: usize| empty.is_some_and(|em| g.kind[u] == DUMMY && em[g.rank[u]] == 1);
    // Each edge from one point that is held in place to the next.
    let mut a: Vec<usize> = Vec::new();
    let mut b: Vec<usize> = Vec::new();
    let mut da: Vec<f64> = Vec::new();
    let mut db: Vec<f64> = Vec::new();
    for e in 0..g.ef.len() {
        if passed(g.ef[e]) {
            continue;
        }
        let f = follow(g, empty, e, 1);
        a.push(g.ef[e]);
        b.push(g.et[f]);
        da.push(g.efd[e]);
        db.push(g.etd[f]);
    }
    let off = |xs: &[f64], k: usize| -> f64 { xs[b[k]] + db[k] - xs[a[k]] - da[k] };
    // The nodes in line with each other move as one.
    let mut block: Vec<usize> = (0..n).collect();
    let mut members: Vec<Vec<usize>> = (0..n).map(|i| alloc::vec![i]).collect();
    fn join(block: &mut [usize], members: &mut [Vec<usize>], x: usize, y: usize) {
        let mut keep = block[x];
        let mut drop = block[y];
        if keep == drop {
            return;
        }
        if members[keep].len() < members[drop].len() {
            core::mem::swap(&mut keep, &mut drop);
        }
        let moved = core::mem::take(&mut members[drop]);
        for &v in &moved {
            block[v] = keep;
        }
        members[keep].extend_from_slice(&moved);
    }
    let mut near: Vec<usize> = Vec::new();
    for k in 0..a.len() {
        let d = off(xs, k).abs();
        if d < 0.01 {
            join(&mut block, &mut members, a[k], b[k]);
        } else if d <= BEND_SNAP {
            near.push(k);
        }
    }
    if near.is_empty() {
        return;
    }
    {
        let xs_r: &[f64] = xs;
        sort_by(&mut near, |x, y| {
            or(off(xs_r, x).abs() - off(xs_r, y).abs(), x as f64 - y as f64)
        });
    }
    let gap = |v: usize| half_room(g, opt, v);
    // Whether every node of a block can move by `shift` and keep its room from those beside it.
    let free =
        |xs: &[f64], block: &[usize], members: &[Vec<usize>], id: usize, shift: f64| -> bool {
            for &v in &members[id] {
                let s = g.layer_start[g.rank[v]];
                let end = g.layer_start[g.rank[v] + 1];
                let i = s + g.pos[v] as usize;
                if i > s
                    && block[g.order[i - 1]] != id
                    && xs[v] + shift - xs[g.order[i - 1]] < gap(v) + gap(g.order[i - 1]) - 0.01
                {
                    return false;
                }
                if i + 1 < end
                    && block[g.order[i + 1]] != id
                    && xs[g.order[i + 1]] - xs[v] - shift < gap(v) + gap(g.order[i + 1]) - 0.01
                {
                    return false;
                }
            }
            true
        };
    for &k in &near {
        let d = off(xs, k);
        if d.abs() < 0.01 {
            join(&mut block, &mut members, a[k], b[k]);
            continue;
        }
        if d.abs() > BEND_SNAP || block[a[k]] == block[b[k]] {
            continue;
        }
        // The smaller of the two blocks moves, if it can; else the other.
        let upper = block[a[k]];
        let lower = block[b[k]];
        let first = if members[upper].len() <= members[lower].len() {
            upper
        } else {
            lower
        };
        for id in [first, if first == upper { lower } else { upper }] {
            let shift = if id == upper { d } else { -d };
            if d.abs() > SNAP && !members[id].iter().all(|&v| g.kind[v] == DUMMY) {
                continue;
            }
            if !free(xs, &block, &members, id, shift) {
                continue;
            }
            for &v in &members[id] {
                xs[v] += shift;
            }
            join(&mut block, &mut members, a[k], b[k]);
            break;
        }
    }
}

/// Ports `emptyRanks`: marks the ranks that hold nothing but the bends of
/// edges, if there are any.
fn empty_ranks(g: &Graph) -> Option<Vec<u8>> {
    let mut empty = alloc::vec![0u8; g.max_rank + 1];
    let mut any = false;
    for r in 1..g.max_rank {
        let mut only = g.layer_start[r + 1] > g.layer_start[r];
        let mut i = g.layer_start[r];
        while only && i < g.layer_start[r + 1] {
            if g.kind[g.order[i]] != DUMMY {
                only = false;
            }
            i += 1;
        }
        if only {
            empty[r] = 1;
            any = true;
        }
    }
    if any {
        Some(empty)
    } else {
        None
    }
}

/// What `assignTracks` works out once about the pieces: where each begins
/// and ends across the flow.
struct TrackCtx<'a> {
    from: &'a [f64],
    to: &'a [f64],
    lo: Vec<f64>,
    hi: Vec<f64>,
}

impl TrackCtx<'_> {
    fn within(&self, x: f64, k: usize) -> i32 {
        if x >= self.lo[k] - IN_LINE && x <= self.hi[k] + IN_LINE {
            1
        } else {
            0
        }
    }
    fn mates(&self, a: usize, b: usize) -> bool {
        (self.from[a] - self.from[b]).abs() < IN_LINE || (self.to[a] - self.to[b]).abs() < IN_LINE
    }
    fn overlap(&self, a: usize, b: usize) -> bool {
        self.lo[a] < self.hi[b] - IN_LINE && self.lo[b] < self.hi[a] - IN_LINE
    }
    // Whether a and b need tracks of their own.
    fn clash(&self, a: usize, b: usize) -> bool {
        self.overlap(a, b) && !self.mates(a, b)
    }
    // Whether one of a and b could cross the other, depending on which is above.
    fn meet(&self, a: usize, b: usize) -> bool {
        self.overlap(a, b)
            || self.within(self.to[a], b)
                + self.within(self.from[b], a)
                + self.within(self.to[b], a)
                + self.within(self.from[a], b)
                > 0
    }
    // How often a and b cross with a on the higher track.
    fn crossings(&self, a: usize, b: usize) -> i32 {
        self.within(self.to[a], b) + self.within(self.from[b], a)
    }
    // Returns pairs of list indices, smaller first. Omitted pairs cannot meet or clash.
    fn near_pairs(&self, list: &[usize]) -> Vec<usize> {
        let mut by_lo: Vec<usize> = (0..list.len()).collect();
        sort_by(&mut by_lo, |a, b| self.lo[list[a]] - self.lo[list[b]]);
        let mut pairs: Vec<usize> = Vec::new();
        for x in 0..by_lo.len() {
            let i = by_lo[x];
            let reach = self.hi[list[i]] + 4.0 * IN_LINE;
            let mut y = x + 1;
            while y < by_lo.len() && self.lo[list[by_lo[y]]] <= reach {
                let j = by_lo[y];
                pairs.push(i.min(j));
                pairs.push(i.max(j));
                y += 1;
            }
        }
        pairs
    }
}

/// `lay` inside `assignTracks`: gives the pieces of one gap in `list` their
/// tracks. Returns the number of tracks.
fn assign_lay(
    ctx: &TrackCtx,
    list: &mut [usize],
    track: &mut [i32],
    levels: &mut [i32],
    curved: bool,
) -> i32 {
    // A plain order to start from: the pieces that reach furthest go first.
    sort_by(list, |a, b| {
        or(
            or(
                (ctx.hi[b] - ctx.lo[b]) - (ctx.hi[a] - ctx.lo[a]),
                ctx.lo[a] - ctx.lo[b],
            ),
            a as f64 - b as f64,
        )
    });
    let list: &[usize] = list;
    let count = list.len();
    let mut above: Vec<Vec<usize>> = Vec::new();
    let mut group: Vec<usize> = (0..count).collect();
    fn find(group: &mut [usize], mut i: usize) -> usize {
        while group[i] != i {
            group[i] = group[group[i]];
            i = group[i];
        }
        i
    }
    let weighed = count <= MAX_WEIGHED;
    let pairs = if weighed {
        ctx.near_pairs(list)
    } else {
        Vec::new()
    };
    let mut clashes: Vec<Vec<usize>> = if weighed {
        (0..count).map(|_| Vec::new()).collect()
    } else {
        Vec::new()
    };
    let mut p = 0;
    while p < pairs.len() {
        let i = pairs[p];
        let j = pairs[p + 1];
        p += 2;
        if !ctx.clash(list[i], list[j]) {
            continue;
        }
        clashes[i].push(j);
        clashes[j].push(i);
    }
    let mut sequence: Vec<usize> = (0..count).collect();
    if weighed {
        let mut waiting = alloc::vec![0i32; count];
        let mut after: Vec<Vec<usize>> = Vec::new();
        for _ in 0..count {
            above.push(Vec::new());
            after.push(Vec::new());
        }
        let mut p = 0;
        while p < pairs.len() {
            let i = pairs[p];
            let j = pairs[p + 1];
            p += 2;
            let a = list[i];
            let b = list[j];
            if !ctx.meet(a, b) {
                continue;
            }
            let fi = find(&mut group, i);
            let fj = find(&mut group, j);
            group[fi] = fj;
            let ab = ctx.crossings(a, b);
            let ba = ctx.crossings(b, a);
            if ab == ba {
                continue;
            }
            let first = if ab < ba { i } else { j };
            let second = if ab < ba { j } else { i };
            above[second].push(first);
            after[first].push(second);
            waiting[second] += 1;
        }
        let mut placed = alloc::vec![0u8; count];
        sequence.clear();
        for _done in 0..count {
            let mut pick: isize = -1;
            for i in 0..count {
                if placed[i] != 0 {
                    continue;
                }
                if pick < 0 || waiting[i] < waiting[pick as usize] {
                    pick = i as isize;
                }
                if waiting[i] == 0 {
                    break;
                }
            }
            let pick = pick as usize;
            placed[pick] = 1;
            sequence.push(pick);
            for &j in &after[pick] {
                waiting[j] -= 1;
            }
        }
    }
    let mut taken: Vec<i32> = Vec::new();
    let mut on_track: Vec<Vec<usize>> = Vec::new();
    let mut rows = 0i32;
    for &at in &sequence {
        let k = list[at];
        let mut t = 0i32;
        if let Some(ab) = above.get(at) {
            for &i in ab {
                if track[list[i]] >= t {
                    t = track[list[i]] + 1;
                }
            }
        }
        if weighed {
            taken.clear();
            for &i in &clashes[at] {
                let tr = track[list[i]];
                if !taken.contains(&tr) {
                    taken.push(tr);
                }
            }
            while taken.contains(&t) {
                t += 1;
            }
        } else {
            while on_track
                .get(t as usize)
                .is_some_and(|row| row.iter().any(|&other| ctx.clash(k, other)))
            {
                t += 1;
            }
            while on_track.len() <= t as usize {
                on_track.push(Vec::new());
            }
            on_track[t as usize].push(k);
        }
        track[k] = t;
        if t >= rows {
            rows = t + 1;
        }
    }
    let mut first = alloc::vec![rows; count];
    let mut last = alloc::vec![-1i32; count];
    for i in 0..count {
        let root = find(&mut group, i);
        first[root] = first[root].min(track[list[i]]);
        last[root] = last[root].max(track[list[i]]);
    }
    for i in 0..count {
        let root = find(&mut group, i);
        levels[list[i]] = last[root] - first[root] + 1;
        if curved {
            track[list[i]] -= first[root];
        }
    }
    rows
}

/// `sample` inside `assignTracks`.
fn track_sample(ctx: &TrackCtx, k: usize, h: f64, out: &mut [f64; 2 * SAMPLES + 2]) {
    let (from, to) = (ctx.from, ctx.to);
    let c = ease(to[k] - from[k], h);
    for i in 0..=SAMPLES {
        let t = i as f64 / SAMPLES as f64;
        let u = 1.0 - t;
        out[2 * i] = from[k] + (to[k] - from[k]) * t * t * (3.0 - 2.0 * t);
        out[2 * i + 1] = h * (3.0 * c * t * u * u + 3.0 * (1.0 - c) * t * t * u + t * t * t);
    }
}

/// `reaches` inside `assignTracks`.
fn track_reaches(points: &[f64], line: &[f64], limit: f64) -> bool {
    // Allow rounding error in the cheap distance check before calling Math.hypot.
    let bound = limit * limit * (1.0 + 1e-9);
    for i in 0..=SAMPLES {
        let px = points[2 * i];
        let py = points[2 * i + 1];
        for j in 0..SAMPLES {
            let dx = line[2 * j + 2] - line[2 * j];
            let dy = line[2 * j + 3] - line[2 * j + 1];
            let t = jmax(
                0.0,
                jmin(
                    1.0,
                    ((px - line[2 * j]) * dx + (py - line[2 * j + 1]) * dy)
                        / or(dx * dx + dy * dy, 1.0),
                ),
            );
            let ex = px - line[2 * j] - t * dx;
            let ey = py - line[2 * j + 1] - t * dy;
            if ex * ex + ey * ey < bound && jhypot(ex, ey) < limit {
                return true;
            }
        }
    }
    false
}

/// `near` inside `assignTracks`.
fn track_near(ctx: &TrackCtx, a: usize, b: usize, height: f64, limit: f64) -> bool {
    let h = jmax(height / 2.0, height - 2.0 * LEVEL_LEAD);
    let mut line_a = [0.0f64; 2 * SAMPLES + 2];
    let mut line_b = [0.0f64; 2 * SAMPLES + 2];
    track_sample(ctx, a, h, &mut line_a);
    track_sample(ctx, b, h, &mut line_b);
    track_reaches(&line_a, &line_b, limit) || track_reaches(&line_b, &line_a, limit)
}

/// Ports `assignTracks`: gives every piece that does not run straight down
/// a track across its gap, and counts the tracks of each gap.
fn assign_tracks(
    pieces: &mut Pieces,
    track_count: &mut [i32],
    curved: bool,
    room: &[f64],
    tall: &mut [f64],
) {
    let Pieces {
        from,
        to,
        rank,
        track,
        levels,
        wide,
        ..
    } = pieces;
    let from: &[f64] = from;
    let to: &[f64] = to;
    let rank: &[usize] = rank;
    let wide: &[u8] = wide;
    let total = from.len();
    let mut by_rank: Vec<(usize, Vec<usize>)> = Vec::new();
    let mut slot = alloc::vec![usize::MAX; track_count.len()];
    for k in 0..total {
        track.push(-1);
        levels.push(0);
        if (from[k] - to[k]).abs() < IN_LINE {
            continue;
        }
        let r = rank[k];
        if slot[r] == usize::MAX {
            slot[r] = by_rank.len();
            by_rank.push((r, Vec::new()));
        }
        by_rank[slot[r]].1.push(k);
    }
    if by_rank.is_empty() {
        return;
    }
    let mut lo = alloc::vec![0.0f64; total];
    let mut hi = alloc::vec![0.0f64; total];
    for k in 0..total {
        lo[k] = jmin(from[k], to[k]);
        hi[k] = jmax(from[k], to[k]);
    }
    let ctx = TrackCtx { from, to, lo, hi };

    for (r, all) in by_rank.iter_mut() {
        let r = *r;
        if !curved {
            track_count[r] = assign_lay(&ctx, all, track, levels, curved);
            continue;
        }
        if all.len() > MAX_WEIGHED {
            continue;
        }
        let all: &[usize] = all;
        // The pieces that turn onto levels.
        let mut far: Vec<usize> = all.iter().copied().filter(|&k| wide[k] == 1).collect();
        assign_lay(&ctx, &mut far, track, levels, curved);
        let mut level = alloc::vec![0u8; total];
        let mut level_size = 0usize;
        for &k in &far {
            if levels[k] > 1 && level[k] == 0 {
                level[k] = 1;
                level_size += 1;
            }
        }
        let pairs = ctx.near_pairs(all);
        let mut p = 0;
        while p < pairs.len() {
            let a = all[pairs[p]];
            let b = all[pairs[p + 1]];
            p += 2;
            if (level[a] != 0 && level[b] != 0)
                || !ctx.clash(a, b)
                || (from[a] < to[a]) != (from[b] < to[b])
            {
                continue;
            }
            let limit = if wide[a] == 1 || wide[b] == 1 {
                LEVEL_CLEAR
            } else {
                LEVEL_TOUCH
            };
            if track_near(&ctx, a, b, room[r], limit) {
                for x in [a, b] {
                    if level[x] == 0 {
                        level[x] = 1;
                        level_size += 1;
                    }
                }
            }
        }
        for &k in &far {
            track[k] = -1;
            levels[k] = 0;
        }
        if level_size == 0 {
            continue;
        }
        let mut crossing = 0usize;
        let mut alongside = 0usize;
        let mut beside = alloc::vec![0u8; total];
        let mut p = 0;
        while p < pairs.len() {
            let a = all[pairs[p]];
            let b = all[pairs[p + 1]];
            p += 2;
            if level[a] != 0 && level[b] != 0 {
                if !ctx.overlap(a, b) {
                    continue;
                }
                if (from[a] < to[a]) != (from[b] < to[b]) {
                    crossing += 1;
                } else if !ctx.mates(a, b) {
                    alongside += 1;
                }
            } else if (level[a] != 0 || level[b] != 0) && ctx.meet(a, b) {
                beside[a] = 1;
                beside[b] = 1;
            }
        }
        if crossing > alongside {
            for &k in all {
                if (ctx.hi[k] - ctx.lo[k]) * CROSSED_SLOPE > tall[r] {
                    tall[r] = (ctx.hi[k] - ctx.lo[k]) * CROSSED_SLOPE;
                }
            }
            continue;
        }
        // With them go the short pieces that could cross one of them.
        let mut members: Vec<usize> = all
            .iter()
            .copied()
            .filter(|&k| level[k] != 0 || (wide[k] != 1 && beside[k] != 0))
            .collect();
        assign_lay(&ctx, &mut members, track, levels, curved);
        let mut most = 0i32;
        for &k in &members {
            if levels[k] > most {
                most = levels[k];
            }
        }
        track_count[r] = most;
    }
}

/// Ports `straighten`: moves the bends in ranks that hold only bends onto
/// the straight line between the nearest points of their edges that are
/// held in place, and marks the ones that can be left out of the route.
fn straighten(
    g: &Graph,
    xs: &mut [f64],
    empty: &[u8],
    first_dummy: &[isize],
    span: &[usize],
    sep: f64,
    on_line: &mut [u8],
) {
    let len = xs.len();
    let mut want = alloc::vec![0.0f64; len];
    let mut from = alloc::vec![0.0f64; len];
    let mut to = alloc::vec![0.0f64; len];
    for ei in 0..first_dummy.len() {
        let d0 = first_dummy[ei];
        if d0 < 0 {
            continue;
        }
        let d0 = d0 as usize;
        let end = d0 + span[ei] - 1;
        // A bend's rank is its place in the run, counted from the rank after the edge's first node.
        let base = g.rank[d0];
        let mut d = d0;
        while d < end {
            if empty[base + d - d0] == 0 {
                d += 1;
                continue;
            }
            let mut last = d;
            while last + 1 < end && empty[base + last + 1 - d0] != 0 {
                last += 1;
            }
            let above = g.pred_e[g.pred_start[d]];
            let below = g.succ_e[g.succ_start[last]];
            let x0 = xs[g.ef[above]] + g.efd[above];
            let x1 = xs[g.et[below]] + g.etd[below];
            let steps = (last - d + 2) as f64;
            for k in d..=last {
                want[k] = x0 + ((x1 - x0) * (k - d + 1) as f64) / steps;
                from[k] = x0;
                to[k] = x1;
            }
            d = last + 1;
        }
    }

    // Nothing in such a rank is in a bend's way, so each one goes onto its line and out of the
    // route. Only twins stay.
    let mut twins: Vec<usize> = Vec::new();
    for r in 1..g.max_rank {
        if empty[r] == 0 {
            continue;
        }
        let s = g.layer_start[r];
        let end = g.layer_start[r + 1];
        let mut row: Vec<usize> = g.order[s..end].to_vec();
        sort_by(&mut row, |a, b| {
            or(or(from[a] - from[b], to[a] - to[b]), a as f64 - b as f64)
        });
        let mut i = 0;
        while i < row.len() {
            twins.clear();
            let mut j = i;
            while j < row.len()
                && (from[row[j]] - from[row[i]]).abs() < TWIN
                && (to[row[j]] - to[row[i]]).abs() < TWIN
            {
                twins.push(row[j]);
                j += 1;
            }
            // The twins keep the order the rank gave them.
            sort_by(&mut twins, |a, b| (g.pos[a] - g.pos[b]) as f64);
            let tl = twins.len();
            for (k, &d) in twins.iter().enumerate() {
                xs[d] = want[d] + (k as f64 - (tl - 1) as f64 / 2.0) * sep;
                if tl == 1 || sep == 0.0 {
                    on_line[d] = 1;
                }
            }
            i = j;
        }
    }
}

/// Ports `pastLabels`: leaves out a bend in a rank of labels where the edge
/// would pass clear of every label anyway.
fn past_labels(
    g: &Graph,
    xs: &mut [f64],
    first_dummy: &[isize],
    span: &[usize],
    clear: f64,
    on_line: &mut [u8],
) {
    let max_rank = g.max_rank;
    let mut labelled = alloc::vec![0u8; max_rank + 1];
    let mut any = false;
    for r in 1..max_rank {
        let mut labels = false;
        let mut others = false;
        for i in g.layer_start[r]..g.layer_start[r + 1] {
            let kind = g.kind[g.order[i]];
            if kind == LABEL {
                labels = true;
            } else if kind != DUMMY {
                others = true;
            }
        }
        if labels && !others {
            labelled[r] = 1;
            any = true;
        }
    }
    if !any {
        return;
    }
    for ei in 0..first_dummy.len() {
        let d0 = first_dummy[ei];
        if d0 < 0 {
            continue;
        }
        let d0 = d0 as usize;
        let end = d0 + span[ei] - 1;
        for d in d0..end {
            if g.kind[d] != DUMMY || labelled[g.rank[d]] == 0 {
                continue;
            }
            // The points on either side that the edge is held at, past any bends already left out.
            let mut first = d;
            let mut last = d;
            while first > d0 && on_line[first - 1] != 0 {
                first -= 1;
            }
            while last + 1 < end && on_line[last + 1] != 0 {
                last += 1;
            }
            let above = g.pred_e[g.pred_start[first]];
            let below = g.succ_e[g.succ_start[last]];
            let from = xs[g.ef[above]] + g.efd[above];
            let to = xs[g.et[below]] + g.etd[below];
            let lo = jmin(from, to) - clear;
            let hi = jmax(from, to) + clear;
            let mut free = true;
            let r = g.rank[d];
            let mut i = g.layer_start[r];
            while free && i < g.layer_start[r + 1] {
                let label = g.order[i];
                if g.kind[label] == LABEL
                    && xs[label] + g.w[label] / 2.0 > lo
                    && xs[label] - g.w[label] / 2.0 < hi
                {
                    free = false;
                }
                i += 1;
            }
            if !free {
                continue;
            }
            on_line[d] = 1;
            // The bends left out between the two points lie on the line between them.
            let steps = (last - first + 2) as f64;
            for k in first..=last {
                xs[k] = from + ((to - from) * (k - first + 1) as f64) / steps;
            }
        }
    }
}

/// The crossing counter of `reduceCrossings` (`count` and the arrays it
/// keeps between calls).
struct Counter {
    place: Option<Vec<usize>>,
    place_in: Vec<usize>,
    places: Vec<usize>,
    leaving: Vec<usize>,
    mixed: Vec<u8>,
    tree: Vec<usize>,
    targets: Vec<usize>,
}

impl Counter {
    fn new(g: &Graph, widest: usize) -> Counter {
        let n = g.n;
        let segs = g.ef.len();
        let has = g.offsets;
        let place = if has {
            Some(alloc::vec![0usize; segs])
        } else {
            None
        };
        let mut place_in = alloc::vec![0usize; if has { segs } else { 0 }];
        let mut places = alloc::vec![0usize; if has { n } else { 0 }];
        let mut leaving = g.succ_e.clone();
        let mut mixed = alloc::vec![0u8; if has { n } else { 0 }];
        if has {
            let mut arriving = g.pred_e.clone();
            let by_offset = |list: &mut [usize], from: usize, to: usize, dx: &[f64]| -> bool {
                for k in from + 1..to {
                    if dx[list[k]] == dx[list[from]] {
                        continue;
                    }
                    sort_by(&mut list[from..to], |a, b| dx[a] - dx[b]);
                    return true;
                }
                false
            };
            for v in 0..n {
                places[v] = 1;
                if by_offset(&mut arriving, g.pred_start[v], g.pred_start[v + 1], &g.etd) {
                    for k in g.pred_start[v] + 1..g.pred_start[v + 1] {
                        if g.etd[arriving[k]] != g.etd[arriving[k - 1]] {
                            places[v] += 1;
                        }
                        place_in[arriving[k]] = places[v] - 1;
                    }
                }
                if by_offset(&mut leaving, g.succ_start[v], g.succ_start[v + 1], &g.efd) {
                    mixed[v] = 1;
                }
            }
        }
        let mut size = 1usize;
        while size < (if has { widest + segs } else { widest }) {
            size <<= 1;
        }
        Counter {
            place,
            place_in,
            places,
            leaving,
            mixed,
            tree: alloc::vec![0usize; 2 * size],
            targets: alloc::vec![0usize; segs],
        }
    }

    /// `count` inside `reduceCrossings`.
    fn count(&mut self, g: &Graph) -> usize {
        let ls = &g.layer_start;
        let mut total = 0usize;
        for r in 0..g.max_rank {
            let mut south_len = ls[r + 2] - ls[r + 1];
            if let Some(place) = self.place.as_mut() {
                let mut next = 0usize;
                for i in ls[r + 1]..ls[r + 2] {
                    let v = g.order[i];
                    for k in g.pred_start[v]..g.pred_start[v + 1] {
                        place[g.pred_e[k]] = next + self.place_in[g.pred_e[k]];
                    }
                    next += self.places[v];
                }
                south_len = next;
            }
            if south_len < 2 {
                continue;
            }
            let targets = &mut self.targets;
            let mut t = 0usize;
            for i in ls[r]..ls[r + 1] {
                let v = g.order[i];
                let from = g.succ_start[v];
                let to = g.succ_start[v + 1];
                let t0 = t;
                for k in from..to {
                    targets[t] = match &self.place {
                        Some(place) => place[self.leaving[k]],
                        None => g.pos[g.et[g.succ_e[k]]] as usize,
                    };
                    t += 1;
                }
                let d = t - t0;
                if d < 2 {
                    continue;
                }
                if self.place.is_some() && self.mixed[v] != 0 {
                    // Sort equal-offset edges by destination to avoid counting shared endpoints as crossings.
                    for a in t0 + 1..t {
                        let item = targets[a];
                        let dx = g.efd[self.leaving[from + a - t0]];
                        let mut b = a as isize - 1;
                        while b >= t0 as isize
                            && g.efd[self.leaving[from + b as usize - t0]] == dx
                            && targets[b as usize] > item
                        {
                            targets[(b + 1) as usize] = targets[b as usize];
                            b -= 1;
                        }
                        targets[(b + 1) as usize] = item;
                    }
                } else if d <= 12 {
                    for a in t0 + 1..t {
                        let item = targets[a];
                        let mut b = a as isize - 1;
                        while b >= t0 as isize && targets[b as usize] > item {
                            targets[(b + 1) as usize] = targets[b as usize];
                            b -= 1;
                        }
                        targets[(b + 1) as usize] = item;
                    }
                } else {
                    targets[t0..t].sort_unstable();
                }
            }
            if t < 2 {
                continue;
            }
            let mut leaves = 1usize;
            while leaves < south_len {
                leaves <<= 1;
            }
            let tree = &mut self.tree;
            tree[..2 * leaves].fill(0);
            for &target in &targets[..t] {
                let mut i = target + leaves;
                tree[i] += 1;
                while i > 1 {
                    if (i & 1) == 0 {
                        total += tree[i + 1];
                    }
                    i >>= 1;
                    tree[i] += 1;
                }
            }
        }
        total
    }
}

/// `arrange` inside `reduceCrossings`, with the arrays it keeps: keeps
/// groups contiguous, ordering each by its mean key.
struct Arranger {
    grp: Vec<i32>,
    gp: Vec<i32>,
    depth: Vec<i32>,
    sum: Vec<f64>,
    count: Vec<i32>,
    first: Vec<usize>,
}

impl Arranger {
    fn arrange(&mut self, g: &mut Graph, r: usize, key: &[f64]) {
        let n = g.n;
        let s = g.layer_start[r];
        let e = g.layer_start[r + 1];
        for i in s..e {
            let mut gr = self.grp[g.order[i]];
            while gr >= 0 {
                self.sum[gr as usize] = 0.0;
                self.count[gr as usize] = 0;
                gr = self.gp[gr as usize];
            }
        }
        for i in s..e {
            let v = g.order[i];
            let mut gr = self.grp[v];
            while gr >= 0 {
                let gu = gr as usize;
                if self.count[gu] == 0 {
                    self.first[gu] = i;
                }
                self.sum[gu] += key[v];
                self.count[gu] += 1;
                gr = self.gp[gu];
            }
        }
        // Compare siblings in their nearest common group; group items use n + group ID.
        let this = &*self;
        let pos = &g.pos;
        let value = |item: usize| -> f64 {
            if item >= n {
                this.sum[item - n] / this.count[item - n] as f64
            } else {
                key[item]
            }
        };
        let first = |item: usize| -> f64 {
            if item >= n {
                this.first[item - n] as f64
            } else {
                (s + pos[item] as usize) as f64
            }
        };
        let compare = |a: usize, b: usize| -> f64 {
            let mut ia = a;
            let mut ib = b;
            let mut ca = this.grp[a];
            let mut cb = this.grp[b];
            let mut da = if ca < 0 { 0 } else { this.depth[ca as usize] };
            let mut db = if cb < 0 { 0 } else { this.depth[cb as usize] };
            while da > db {
                ia = n + ca as usize;
                ca = this.gp[ca as usize];
                da -= 1;
            }
            while db > da {
                ib = n + cb as usize;
                cb = this.gp[cb as usize];
                db -= 1;
            }
            while ca != cb {
                ia = n + ca as usize;
                ca = this.gp[ca as usize];
                ib = n + cb as usize;
                cb = this.gp[cb as usize];
            }
            or(value(ia) - value(ib), first(ia) - first(ib))
        };
        let mut row: Vec<usize> = g.order[s..e].to_vec();
        sort_by(&mut row, compare);
        g.order[s..e].copy_from_slice(&row);
        for i in s..e {
            g.pos[g.order[i]] = (i - s) as i32;
        }
    }
}

/// `sweep` inside `reduceCrossings`.
#[allow(clippy::too_many_arguments)]
fn sweep(
    g: &mut Graph,
    nodes: &[LNode],
    down: bool,
    bias_right: bool,
    bary: &mut [f64],
    moving: &mut [usize],
    spare: &mut [usize],
    arranger: &mut Option<Arranger>,
) {
    let max_rank = g.max_rank;
    let real = g.real;
    for step in 1..=max_rank {
        let r = if down { step } else { max_rank - step };
        if g.fixed[r] != 0 {
            continue;
        }
        let s = g.layer_start[r];
        let e = g.layer_start[r + 1];
        let mut movable = 0usize;
        {
            let (start, list, other, other_dx) = if down {
                (&g.pred_start, &g.pred_e, &g.ef, &g.efd)
            } else {
                (&g.succ_start, &g.succ_e, &g.et, &g.etd)
            };
            for i in s..e {
                let v = g.order[i];
                let pin = if v < real { nodes[v].pin } else { 0.0 };
                if pin != 0.0 {
                    bary[v] = pin * 1e9;
                    moving[movable] = v;
                    movable += 1;
                    continue;
                }
                let from = start[v];
                let to = start[v + 1];
                if from == to {
                    if g.grp.is_some() {
                        // Preserve relative position when there are no neighbours in the adjacent rank.
                        let across = if down {
                            g.layer_start[r] - g.layer_start[r - 1]
                        } else {
                            g.layer_start[r + 2] - g.layer_start[r + 1]
                        };
                        bary[v] = ((i - s) as f64 * across as f64) / (e - s) as f64;
                        continue;
                    }
                    bary[v] = -1.0;
                    continue;
                }
                let mut sum = 0.0f64;
                for k in from..to {
                    let u = other[list[k]];
                    sum += g.pos[u] as f64;
                    if g.offsets && g.w[u] > 0.0 {
                        sum += other_dx[list[k]] / g.w[u];
                    }
                }
                bary[v] = sum / (to - from) as f64;
                moving[movable] = v;
                movable += 1;
            }
        }
        if let Some(arr) = arranger.as_mut() {
            arr.arrange(g, r, bary);
            continue;
        }
        if movable < 2 {
            continue;
        }
        sort_by_bary(moving, movable, bary, &g.pos, bias_right, spare);
        let mut k = 0;
        for i in s..e {
            if bary[g.order[i]] != -1.0 {
                g.order[i] = moving[k];
                k += 1;
            }
        }
        for i in s..e {
            g.pos[g.order[i]] = (i - s) as i32;
        }
    }
}

/// `outOfOrder` inside `reduceCrossings`: how many pairs of nodes in one
/// rank are not in the order they were given.
fn out_of_order(g: &Graph) -> usize {
    let mut total = 0usize;
    for r in 0..=g.max_rank {
        let end = g.layer_start[r + 1];
        for i in g.layer_start[r]..end {
            if g.order[i] >= g.real {
                continue;
            }
            for j in i + 1..end {
                if g.order[j] < g.order[i] {
                    total += 1;
                }
            }
        }
    }
    total
}

/// `at` inside `reduceCrossings`.
fn rc_at(g: &Graph, e: usize, up: bool) -> f64 {
    let u = if up { g.ef[e] } else { g.et[e] };
    if g.offsets && g.w[u] > 0.0 {
        g.pos[u] as f64 + (if up { g.efd[e] } else { g.etd[e] }) / g.w[u]
    } else {
        g.pos[u] as f64
    }
}

/// `between` inside `reduceCrossings`: crossings between a and b, assuming
/// a is left of b.
fn rc_between(g: &Graph, a: usize, b: usize) -> usize {
    let mut c = 0;
    for k in g.pred_start[a]..g.pred_start[a + 1] {
        let x = rc_at(g, g.pred_e[k], true);
        for j in g.pred_start[b]..g.pred_start[b + 1] {
            if x > rc_at(g, g.pred_e[j], true) {
                c += 1;
            }
        }
    }
    for k in g.succ_start[a]..g.succ_start[a + 1] {
        let x = rc_at(g, g.succ_e[k], false);
        for j in g.succ_start[b]..g.succ_start[b + 1] {
            if x > rc_at(g, g.succ_e[j], false) {
                c += 1;
            }
        }
    }
    c
}

/// `transpose` inside `reduceCrossings`: adjacent swaps can remove
/// crossings missed by barycenter sweeps.
fn transpose(g: &mut Graph, nodes: &[LNode]) {
    let real = g.real;
    let pin_of = |v: usize| if v < real { nodes[v].pin } else { 0.0 };
    for _pass in 0..8 {
        let mut improved = false;
        for r in 0..=g.max_rank {
            if g.fixed[r] != 0 {
                continue;
            }
            let s = g.layer_start[r];
            let mut i = s;
            while i + 1 < g.layer_start[r + 1] {
                let a = g.order[i];
                let b = g.order[i + 1];
                let same_group = match &g.grp {
                    Some(grp) => grp[a] == grp[b],
                    None => true,
                };
                if pin_of(a) != pin_of(b) || !same_group {
                    i += 1;
                    continue;
                }
                if rc_between(g, b, a) >= rc_between(g, a, b) {
                    i += 1;
                    continue;
                }
                g.order[i] = b;
                g.order[i + 1] = a;
                g.pos[b] = (i - s) as i32;
                g.pos[a] = (i + 1 - s) as i32;
                improved = true;
                i += 1;
            }
        }
        if !improved {
            break;
        }
    }
}

/// Ports `reduceCrossings`. Returns [final crossings, crossings before
/// grouping or reordering].
fn reduce_crossings(g: &mut Graph, nodes: &[LNode]) -> (usize, usize) {
    let n = g.n;
    let real = g.real;
    let max_rank = g.max_rank;
    let mut widest = 0usize;
    for r in 0..=max_rank {
        widest = widest.max(g.layer_start[r + 1] - g.layer_start[r]);
    }
    if widest < 2 {
        return (0, 0);
    }
    let mut counter = Counter::new(g, widest);

    let declared = counter.count(g);
    let mut bary = alloc::vec![0.0f64; n];
    let mut moving = alloc::vec![0usize; widest];
    let mut spare = alloc::vec![0usize; widest];

    // Keep groups contiguous, ordering each by its mean key.
    let mut arranger: Option<Arranger> = match (&g.grp, &g.group_parent) {
        (Some(grp), Some(gp)) => {
            let groups = gp.len();
            let mut depth = alloc::vec![0i32; groups];
            for (gr, d) in depth.iter_mut().enumerate() {
                let mut p = gr as i32;
                while p >= 0 {
                    *d += 1;
                    p = gp[p as usize];
                }
            }
            Some(Arranger {
                grp: grp.clone(),
                gp: gp.clone(),
                depth,
                sum: alloc::vec![0.0; groups],
                count: alloc::vec![0; groups],
                first: alloc::vec![0; groups],
            })
        }
        _ => None,
    };
    if let Some(arr) = arranger.as_mut() {
        let mut at = alloc::vec![0.0f64; n];
        for r in 0..=max_rank {
            for i in g.layer_start[r]..g.layer_start[r + 1] {
                at[g.order[i]] = (i - g.layer_start[r]) as f64;
            }
            arr.arrange(g, r, &at);
        }
    }
    let mut best = if arranger.is_some() {
        counter.count(g)
    } else {
        declared
    };
    if best == 0 {
        return (0, declared);
    }
    let mut best_order = g.order.clone();
    // Require improvements over both the grouped and ungrouped starting orders.
    let start = best.min(declared);

    // The sweeps are run twice from the same start, going down first and going up first.
    let initial = g.order.clone();
    let small = n <= 600;
    let mut chosen = best_order.clone();
    let mut chosen_out = usize::MAX;
    for attempt in 0..(if small { 2 } else { 1 }) {
        if attempt > 0 {
            g.order.copy_from_slice(&initial);
            settle_positions(g);
        }
        let mut found = start;
        let mut found_order = initial.clone();
        let mut i = 0usize;
        let mut stale = 0usize;
        while stale < 4 && i < MAX_SWEEPS {
            sweep(
                g,
                nodes,
                (i + attempt).is_multiple_of(2),
                i % 4 >= 2,
                &mut bary,
                &mut moving,
                &mut spare,
                &mut arranger,
            );
            let c = counter.count(g);
            if c < found {
                found = c;
                found_order.copy_from_slice(&g.order);
                stale = 0;
                if c == 0 {
                    break;
                }
            }
            i += 1;
            stale += 1;
        }
        g.order.copy_from_slice(&found_order);
        if small && found > 0 {
            settle_positions(g);
            transpose(g, nodes);
            let c = counter.count(g);
            if c < found {
                found = c;
                found_order.copy_from_slice(&g.order);
            }
        }
        let out = if small { out_of_order(g) } else { 0 };
        if attempt == 0 || found < best || (found == best && out < chosen_out) {
            best = found;
            chosen = found_order;
            chosen_out = out;
        }
    }
    g.order.copy_from_slice(&chosen);
    settle_positions(g);
    best_order.copy_from_slice(&chosen);
    // The retained grouped order may exceed the ungrouped threshold; recount it.
    if arranger.is_some() {
        best = counter.count(g);
    }

    // A drawing and its mirror image cross as often too. Of those two, likewise.
    if !small {
        return (best, declared);
    }
    for node in nodes.iter().take(real) {
        if node.pin != 0.0 {
            return (best, declared);
        }
    }
    if chosen_out == 0 || g.fixed[0] != 0 || g.fixed[max_rank] != 0 {
        return (best, declared);
    }
    for r in 0..=max_rank {
        let (s, e) = (g.layer_start[r], g.layer_start[r + 1]);
        g.order[s..e].reverse();
    }
    settle_positions(g);
    if counter.count(g) > best || out_of_order(g) >= chosen_out {
        g.order.copy_from_slice(&best_order);
    }
    (best, declared)
}

/// Ports `halfRoom`: half the room a node, bend or port takes across the
/// flow, with its share of the gap beside it.
fn half_room(g: &Graph, opt: &LayeredOptions, v: usize) -> f64 {
    g.w[v] / 2.0
        + (if g.kind[v] == NODE {
            opt.node_sep
        } else {
            opt.edge_sep
        }) / 2.0
}

/// Ports `position`: Brandes-Köpf horizontal coordinate assignment, four
/// aligned layouts, balanced.
fn position(g: &mut Graph, opt: &LayeredOptions) -> Vec<f64> {
    let n = g.n;
    let max_rank = g.max_rank;
    let segments = g.ef.len();
    let gap: Vec<f64> = (0..n).map(|i| half_room(g, opt, i)).collect();

    // Neighbours in left-to-right order, so the medians can be read off directly.
    let mut key = alloc::vec![0i32; segments];
    for e in 0..segments {
        key[e] = g.pos[g.et[e]];
    }
    for v in 0..n {
        if g.succ_start[v + 1] - g.succ_start[v] > 1 {
            sort_range(&mut g.succ_e, g.succ_start[v], g.succ_start[v + 1], &key);
        }
    }
    for e in 0..segments {
        key[e] = g.pos[g.ef[e]];
    }
    for v in 0..n {
        if g.pred_start[v + 1] - g.pred_start[v] > 1 {
            sort_range(&mut g.pred_e, g.pred_start[v], g.pred_start[v + 1], &key);
        }
    }
    let g: &Graph = g;
    let (kind, ef, et, efd, etd) = (&g.kind, &g.ef, &g.et, &g.efd, &g.etd);
    let (succ_start, succ_e, pred_start, pred_e) =
        (&g.succ_start, &g.succ_e, &g.pred_start, &g.pred_e);
    let (layer_start, order, pos) = (&g.layer_start, &g.order, &g.pos);

    // An edge between two real nodes may not cross a dummy-to-dummy segment; mark those that would.
    let mut conflict = alloc::vec![0u8; segments];
    for r in 1..=max_rank {
        let prev_len = (layer_start[r] - layer_start[r - 1]) as i32;
        let s = layer_start[r];
        let end = layer_start[r + 1];
        let mut k0 = 0i32;
        let mut scan = s;
        for i in s..end {
            let v = order[i];
            let mut inner: isize = -1;
            if is_dummy(kind[v]) {
                let u = ef[pred_e[pred_start[v]]];
                if is_dummy(kind[u]) {
                    inner = u as isize;
                }
            }
            let k1 = if inner >= 0 {
                pos[inner as usize]
            } else {
                prev_len
            };
            if inner >= 0 || i == end - 1 {
                for &wv in &order[scan..=i] {
                    let w_dummy = is_dummy(kind[wv]);
                    for &e in &pred_e[pred_start[wv]..pred_start[wv + 1]] {
                        let up = pos[ef[e]];
                        if (up < k0 || k1 < up) && !(w_dummy && is_dummy(kind[ef[e]])) {
                            conflict[e] = 1;
                        }
                    }
                }
                scan = i + 1;
                k0 = k1;
            }
        }
    }

    let mut results: Vec<Vec<f64>> = Vec::new();
    let mut root = alloc::vec![0usize; n];
    let mut align = alloc::vec![0usize; n];
    // Offset of each node from its block's root, so that aligned edges meet at their attachment points.
    let mut shift = alloc::vec![0.0f64; n];
    let mut e_from = alloc::vec![0usize; n];
    let mut e_to = alloc::vec![0usize; n];
    let mut e_w = alloc::vec![0.0f64; n];
    let mut indeg = alloc::vec![0i32; n];
    let mut out_start = alloc::vec![0usize; n + 1];
    let mut out_e = alloc::vec![0usize; n];
    let mut fill = alloc::vec![0usize; n];
    let mut topo = alloc::vec![0usize; n];
    let mut x = alloc::vec![0.0f64; n];

    for variant in 0..4 {
        let up = variant < 2;
        let right = (variant & 1) == 1;
        let (start, list, other) = if up {
            (pred_start, pred_e, ef)
        } else {
            (succ_start, succ_e, et)
        };
        let own_dx = if up { etd } else { efd };
        let other_dx = if up { efd } else { etd };
        for i in 0..n {
            root[i] = i;
            align[i] = i;
            shift[i] = 0.0;
            indeg[i] = 0;
            out_start[i + 1] = 0;
            x[i] = 0.0;
        }

        for step in 0..=max_rank {
            let r = if up { step } else { max_rank - step };
            let s = layer_start[r];
            let end = layer_start[r + 1];
            let adjacent: isize = if up { r as isize - 1 } else { r as isize + 1 };
            let adjacent_last: i32 = if adjacent >= 0 && adjacent <= max_rank as isize {
                let a = adjacent as usize;
                (layer_start[a + 1] - layer_start[a]) as i32 - 1
            } else {
                0
            };
            let mut prev_idx: i32 = -1;
            for c in 0..end - s {
                let i = if right { end - 1 - c } else { s + c };
                let v = order[i];
                let from = start[v];
                let d = start[v + 1] - from;
                if d == 0 {
                    continue;
                }
                let lo = from + ((d - 1) >> 1);
                let hi = from + (d >> 1);
                for pass in 0..2 {
                    if pass == 1 && lo == hi {
                        break;
                    }
                    let e = list[if (pass == 0) != right { lo } else { hi }];
                    let wv = other[e];
                    let idx = if right {
                        adjacent_last - pos[wv]
                    } else {
                        pos[wv]
                    };
                    if align[v] == v && prev_idx < idx && conflict[e] == 0 {
                        align[wv] = v;
                        root[v] = root[wv];
                        align[v] = root[v];
                        shift[v] = shift[wv] + other_dx[e] - own_dx[e];
                        prev_idx = idx;
                    }
                }
            }
        }

        let mut block_edges = 0usize;
        for r in 0..=max_rank {
            let s = layer_start[r];
            let end = layer_start[r + 1];
            for i in s + 1..end {
                let a = if right { order[i] } else { order[i - 1] };
                let b = if right { order[i - 1] } else { order[i] };
                let sep = gap[a] + gap[b];
                e_from[block_edges] = root[a];
                e_to[block_edges] = root[b];
                e_w[block_edges] = if right {
                    sep - shift[a] + shift[b]
                } else {
                    sep + shift[a] - shift[b]
                };
                indeg[root[b]] += 1;
                out_start[root[a] + 1] += 1;
                block_edges += 1;
            }
        }
        for i in 0..n {
            out_start[i + 1] += out_start[i];
        }
        fill[..n].copy_from_slice(&out_start[..n]);
        for e in 0..block_edges {
            out_e[fill[e_from[e]]] = e;
            fill[e_from[e]] += 1;
        }

        let mut count = 0usize;
        for i in 0..n {
            if root[i] == i && indeg[i] == 0 {
                topo[count] = i;
                count += 1;
            }
        }
        let mut q = 0;
        while q < count {
            let v = topo[q];
            for k in out_start[v]..out_start[v + 1] {
                let e = out_e[k];
                let wv = e_to[e];
                if x[v] + e_w[e] > x[wv] {
                    x[wv] = x[v] + e_w[e];
                }
                indeg[wv] -= 1;
                if indeg[wv] == 0 {
                    topo[count] = wv;
                    count += 1;
                }
            }
            q += 1;
        }
        for q in (0..count).rev() {
            let v = topo[q];
            let mut min = f64::INFINITY;
            for k in out_start[v]..out_start[v + 1] {
                let e = out_e[k];
                let cand = x[e_to[e]] - e_w[e];
                if cand < min {
                    min = cand;
                }
            }
            if min != f64::INFINITY && min > x[v] {
                x[v] = min;
            }
        }
        let mut xs = alloc::vec![0.0f64; n];
        for i in 0..n {
            xs[i] = (if right { -x[root[i]] } else { x[root[i]] }) + shift[i];
        }
        results.push(xs);
    }

    let mut narrowest = 0usize;
    let mut best_width = f64::INFINITY;
    let mut mins: Vec<f64> = Vec::new();
    let mut maxs: Vec<f64> = Vec::new();
    for (k, xs) in results.iter().enumerate() {
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for i in 0..n {
            let half = g.w[i] / 2.0;
            if xs[i] - half < lo {
                lo = xs[i] - half;
            }
            if xs[i] + half > hi {
                hi = xs[i] + half;
            }
        }
        mins.push(lo);
        maxs.push(hi);
        if hi - lo < best_width {
            best_width = hi - lo;
            narrowest = k;
        }
    }
    for (k, xs) in results.iter_mut().enumerate() {
        let delta = if (k & 1) == 0 {
            mins[narrowest] - mins[k]
        } else {
            maxs[narrowest] - maxs[k]
        };
        if delta != 0.0 {
            for v in xs.iter_mut() {
                *v += delta;
            }
        }
    }

    let mut out = alloc::vec![0.0f64; n];
    for i in 0..n {
        let mut a = results[0][i];
        let mut b = results[1][i];
        let mut c = results[2][i];
        let mut d = results[3][i];
        if a > b {
            core::mem::swap(&mut a, &mut b);
        }
        if c > d {
            core::mem::swap(&mut c, &mut d);
        }
        if a > c {
            core::mem::swap(&mut a, &mut c);
        }
        if b > d {
            core::mem::swap(&mut b, &mut d);
        }
        if b > c {
            core::mem::swap(&mut b, &mut c);
        }
        out[i] = (b + c) / 2.0;
    }
    out
}
