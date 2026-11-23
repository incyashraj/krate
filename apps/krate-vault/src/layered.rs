//! A layered graph layout, the way dagre lays out Mermaid's flowcharts,
//! class, state and ER diagrams (Sugiyama's method):
//!
//! 1. edges that close a cycle are turned round;
//! 2. every node gets a rank: the longest path from the top, then nodes
//!    with nothing above them are pulled down next to what they point at;
//! 3. ranks are doubled, as dagre doubles them, so every edge passes a
//!    point half way where its label sits; longer edges get a chain of
//!    such points;
//! 4. the order within each rank is improved by barycentre sweeps, down
//!    and up, keeping the order with the fewest crossings;
//! 5. x positions: each node moved toward the mean of its neighbours,
//!    sweeping down and up, the rank kept in order and apart (the least
//!    squares placement of each rank, block by block);
//! 6. each edge runs from border to border through its chain, drawn as
//!    d3's basis curve as Mermaid draws it.

use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

pub type Pt = (f32, f32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    TopDown,
    BottomUp,
    LeftRight,
    RightLeft,
}

impl Dir {
    pub fn of(s: &str) -> Dir {
        match s {
            "LR" => Dir::LeftRight,
            "RL" => Dir::RightLeft,
            "BT" => Dir::BottomUp,
            _ => Dir::TopDown,
        }
    }

    fn across(self) -> bool {
        matches!(self, Dir::LeftRight | Dir::RightLeft)
    }
}

/// What to lay out: node sizes, edges (from, to) and each edge's label
/// size (0 by 0 for none).
pub struct Input<'a> {
    pub sizes: &'a [(f32, f32)],
    pub edges: &'a [(usize, usize)],
    pub labels: &'a [(f32, f32)],
    /// Each node's outline, where edges meet it: 0 a box, 1 a diamond, 2
    /// a circle or ellipse. Missing means a box.
    pub shapes: &'a [u8],
    pub dir: Dir,
    /// Between nodes side by side.
    pub node_sep: f32,
    /// Between ranks.
    pub rank_sep: f32,
    /// The height every label rank keeps even with no label in it (Mermaid
    /// sizes an empty edge label one line high).
    pub label_gap: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// Each node's top-left corner.
    pub pos: Vec<Pt>,
    /// Each edge's curve, from the border of its first node to the border
    /// of its second.
    pub paths: Vec<Vec<Pt>>,
    /// Where each edge's label is centred.
    pub label_at: Vec<Pt>,
    pub w: f32,
    pub h: f32,
}

/// One node of the layered graph: a real node, or a point an edge passes.
#[derive(Clone)]
struct V {
    /// Width across the ranks and height along them (after turning LR).
    w: f32,
    h: f32,
    rank: usize,
    /// The input node, if real.
    real: Option<usize>,
    x: f32,
}

pub fn layout(inp: &Input<'_>) -> Layout {
    let n = inp.sizes.len();
    let across = inp.dir.across();
    // Work top down; LR is TB with width and height swapped.
    let size = |i: usize| -> (f32, f32) {
        let (w, h) = inp.sizes.get(i).copied().unwrap_or((0.0, 0.0));
        if across {
            (h, w)
        } else {
            (w, h)
        }
    };
    // 1. Cycles: a walk in input order marks the edges that go back.
    let mut adj: Vec<Vec<(usize, usize)>> = alloc::vec![Vec::new(); n];
    for (ei, &(a, b)) in inp.edges.iter().enumerate() {
        if a < n && b < n && a != b {
            adj[a].push((b, ei));
        }
    }
    let mut back = alloc::vec![false; inp.edges.len()];
    let mut state = alloc::vec![0u8; n];
    for root in 0..n {
        if state[root] != 0 {
            continue;
        }
        let mut stack: Vec<(usize, usize)> = alloc::vec![(root, 0)];
        state[root] = 1;
        while let Some(&(v, k)) = stack.last() {
            if let Some(&(w, ei)) = adj[v].get(k) {
                if let Some(top) = stack.last_mut() {
                    top.1 += 1;
                }
                match state[w] {
                    0 => {
                        state[w] = 1;
                        stack.push((w, 0));
                    }
                    1 => back[ei] = true,
                    _ => {}
                }
            } else {
                state[v] = 2;
                stack.pop();
            }
        }
    }
    // The acyclic edges, each (upper, lower, input edge).
    let dag: Vec<(usize, usize, usize)> = inp
        .edges
        .iter()
        .enumerate()
        .filter(|(_, (a, b))| *a < n && *b < n && a != b)
        .map(|(ei, &(a, b))| if back[ei] { (b, a, ei) } else { (a, b, ei) })
        .collect();
    // 2. Ranks: longest path, then sources pulled down to their targets.
    let mut rank = alloc::vec![0usize; n];
    for _ in 0..n.max(1) {
        let mut changed = false;
        for &(a, b, _) in &dag {
            if rank[b] < rank[a] + 1 {
                rank[b] = rank[a] + 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for _ in 0..n.max(1) {
        let mut changed = false;
        for v in 0..n {
            if dag.iter().any(|e| e.1 == v) {
                continue;
            }
            let lowest = dag.iter().filter(|e| e.0 == v).map(|e| rank[e.1]).min();
            if let Some(m) = lowest {
                if m > rank[v] + 1 {
                    rank[v] = m - 1;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    // 3. Doubled ranks, with a chain of points for every edge.
    let mut vs: Vec<V> = (0..n)
        .map(|i| {
            let (w, h) = size(i);
            V {
                w,
                h,
                rank: rank[i] * 2,
                real: Some(i),
                x: 0.0,
            }
        })
        .collect();
    // Each input edge's chain of layered nodes, top to bottom.
    let mut chains: Vec<Vec<usize>> = alloc::vec![Vec::new(); inp.edges.len()];
    let mut label_v: Vec<Option<usize>> = alloc::vec![None; inp.edges.len()];
    let mut links: Vec<(usize, usize)> = Vec::new();
    for &(a, b, ei) in &dag {
        let (ra, rb) = (vs[a].rank, vs[b].rank);
        let mut chain = alloc::vec![a];
        let mid = (ra + rb) / 2;
        for r in ra + 1..rb {
            let (lw, lh) = inp.labels.get(ei).copied().unwrap_or((0.0, 0.0));
            let (lw, lh) = if across { (lh, lw) } else { (lw, lh) };
            let has = r == mid && lw > 0.0;
            vs.push(V {
                w: if has { lw } else { 0.0 },
                h: if has { lh } else { 0.0 },
                rank: r,
                real: None,
                x: 0.0,
            });
            let id = vs.len() - 1;
            if r == mid {
                label_v[ei] = Some(id);
            }
            chain.push(id);
        }
        chain.push(b);
        for w in chain.windows(2) {
            links.push((w[0], w[1]));
        }
        chains[ei] = chain;
    }
    let nranks = vs.iter().map(|v| v.rank).max().unwrap_or(0) + 1;
    // 4. Order within ranks: first as a walk from the top finds them.
    let mut layers: Vec<Vec<usize>> = alloc::vec![Vec::new(); nranks];
    {
        let mut seen = alloc::vec![false; vs.len()];
        let mut down: Vec<Vec<usize>> = alloc::vec![Vec::new(); vs.len()];
        for &(a, b) in &links {
            down[a].push(b);
        }
        let mut roots: Vec<usize> = (0..vs.len()).collect();
        roots.sort_by_key(|v| vs[*v].rank);
        for r in roots {
            if seen[r] {
                continue;
            }
            let mut stack = alloc::vec![r];
            while let Some(v) = stack.pop() {
                if seen[v] {
                    continue;
                }
                seen[v] = true;
                layers[vs[v].rank].push(v);
                for &w in down[v].iter().rev() {
                    if !seen[w] {
                        stack.push(w);
                    }
                }
            }
        }
    }
    let mut ups: Vec<Vec<usize>> = alloc::vec![Vec::new(); vs.len()];
    let mut downs: Vec<Vec<usize>> = alloc::vec![Vec::new(); vs.len()];
    for &(a, b) in &links {
        downs[a].push(b);
        ups[b].push(a);
    }
    let crossings = |layers: &Vec<Vec<usize>>| -> usize {
        let mut pos = alloc::vec![0usize; vs.len()];
        for l in layers {
            for (i, v) in l.iter().enumerate() {
                pos[*v] = i;
            }
        }
        let mut total = 0;
        for l in layers {
            let mut segs: Vec<(usize, usize)> = Vec::new();
            for &v in l {
                for &w in &downs[v] {
                    segs.push((pos[v], pos[w]));
                }
            }
            for i in 0..segs.len() {
                for j in i + 1..segs.len() {
                    let (a, b) = (segs[i], segs[j]);
                    if (a.0 < b.0 && a.1 > b.1) || (a.0 > b.0 && a.1 < b.1) {
                        total += 1;
                    }
                }
            }
        }
        total
    };
    let mut best = layers.clone();
    let mut best_c = crossings(&layers);
    for it in 0..24 {
        let downward = it % 2 == 0;
        let mut pos = alloc::vec![0.0f32; vs.len()];
        for l in &layers {
            for (i, v) in l.iter().enumerate() {
                pos[*v] = i as f32;
            }
        }
        let order: Vec<usize> = if downward {
            (1..nranks).collect()
        } else {
            (0..nranks.saturating_sub(1)).rev().collect()
        };
        for r in order {
            let nb = if downward { &ups } else { &downs };
            let mut keyed: Vec<(f32, usize, usize)> = layers[r]
                .iter()
                .enumerate()
                .map(|(i, &v)| {
                    let ns = &nb[v];
                    let k = if ns.is_empty() {
                        i as f32
                    } else {
                        ns.iter().map(|w| pos[*w]).sum::<f32>() / ns.len() as f32
                    };
                    (k, i, v)
                })
                .collect();
            keyed.sort_by(|a, b| {
                a.0.partial_cmp(&b.0)
                    .unwrap_or(core::cmp::Ordering::Equal)
                    .then(a.1.cmp(&b.1))
            });
            layers[r] = keyed.iter().map(|k| k.2).collect();
            for (i, v) in layers[r].iter().enumerate() {
                pos[*v] = i as f32;
            }
        }
        // Swap neighbours where that crosses less.
        let c = crossings(&layers);
        if c < best_c {
            best_c = c;
            best = layers.clone();
        }
        if best_c == 0 {
            break;
        }
    }
    let layers = best;
    // 5. x: packed, then pulled toward neighbours, ranks kept in order.
    for l in &layers {
        let mut x = 0.0;
        for (i, &v) in l.iter().enumerate() {
            if i > 0 {
                x += gap(&vs, l[i - 1], v, inp.node_sep);
            }
            vs[v].x = x;
        }
        let mid = x * 0.5;
        for &v in l {
            vs[v].x -= mid;
        }
    }
    for it in 0..16 {
        let downward = it % 2 == 0;
        let order: Vec<usize> = if downward {
            (0..nranks).collect()
        } else {
            (0..nranks).rev().collect()
        };
        for r in order {
            let l = &layers[r];
            if l.is_empty() {
                continue;
            }
            let want: Vec<f32> = l
                .iter()
                .map(|&v| {
                    // A point of an edge lines up over the node the edge
                    // reaches, so edges arrive straight, as dagre's
                    // alignment draws them; real nodes go to the mean of
                    // their neighbours.
                    let ns: Vec<usize> = if vs[v].real.is_none() {
                        downs[v].clone()
                    } else if downward {
                        ups[v].clone()
                    } else {
                        downs[v].clone()
                    };
                    if ns.is_empty() {
                        vs[v].x
                    } else {
                        ns.iter().map(|w| vs[*w].x).sum::<f32>() / ns.len() as f32
                    }
                })
                .collect();
            let gaps: Vec<f32> = l
                .windows(2)
                .map(|p| gap(&vs, p[0], p[1], inp.node_sep))
                .collect();
            let xs = place_in_order(&want, &gaps);
            for (k, &v) in l.iter().enumerate() {
                vs[v].x = xs[k];
            }
        }
    }
    // Rank positions: each rank as tall as its tallest, half the rank gap
    // between doubled ranks.
    let mut rank_h = alloc::vec![0.0f32; nranks];
    for v in &vs {
        rank_h[v.rank] = rank_h[v.rank].max(v.h);
    }
    for (r, h) in rank_h.iter_mut().enumerate() {
        if r % 2 == 1 {
            *h = h.max(inp.label_gap);
        }
    }
    let mut rank_y = alloc::vec![0.0f32; nranks];
    let mut y = 0.0;
    for r in 0..nranks {
        rank_y[r] = y + rank_h[r] * 0.5;
        y += rank_h[r] + inp.rank_sep * 0.5;
    }
    let total_h = (y - inp.rank_sep * 0.5).max(0.0);
    let min_x = vs.iter().map(|v| v.x - v.w * 0.5).fold(f32::MAX, f32::min);
    let max_x = vs.iter().map(|v| v.x + v.w * 0.5).fold(f32::MIN, f32::max);
    let (min_x, max_x) = if vs.is_empty() {
        (0.0, 0.0)
    } else {
        (min_x, max_x)
    };
    let total_w = max_x - min_x;
    // Centres in TB space.
    let centre = |v: usize| -> Pt { (vs[v].x - min_x, rank_y[vs[v].rank]) };
    // Into the asked direction.
    let turn = |p: Pt| -> Pt {
        match inp.dir {
            Dir::TopDown => p,
            Dir::BottomUp => (p.0, total_h - p.1),
            Dir::LeftRight => (p.1, p.0),
            Dir::RightLeft => (total_h - p.1, p.0),
        }
    };
    let (w, h) = if across {
        (total_h, total_w)
    } else {
        (total_w, total_h)
    };
    let pos: Vec<Pt> = (0..n)
        .map(|i| {
            let c = turn(centre(i));
            let (sw, sh) = inp.sizes[i];
            (c.0 - sw * 0.5, c.1 - sh * 0.5)
        })
        .collect();
    // 6. Edges: through their chains, cut at the borders, curved.
    let mut paths = alloc::vec![Vec::new(); inp.edges.len()];
    let mut label_at = alloc::vec![(0.0, 0.0); inp.edges.len()];
    for (ei, &(a, b)) in inp.edges.iter().enumerate() {
        if a >= n || b >= n {
            continue;
        }
        if a == b {
            // A loop beside the node.
            let (x, y) = pos[a];
            let (sw, sh) = inp.sizes[a];
            let (rx, cy) = (x + sw, y + sh * 0.5);
            let d = sh * 0.35;
            paths[ei] = basis(&[
                (rx, cy - d),
                (rx + d * 1.6, cy - d * 1.2),
                (rx + d * 1.6, cy + d * 1.2),
                (rx, cy + d),
            ]);
            label_at[ei] = (rx + d * 1.8, cy);
            continue;
        }
        let mut pts: Vec<Pt> = chains[ei].iter().map(|v| turn(centre(*v))).collect();
        if back[ei] {
            pts.reverse();
        }
        if pts.len() < 2 {
            continue;
        }
        let first = pts[0];
        let last = pts[pts.len() - 1];
        let (sa, sb) = (inp.sizes[a], inp.sizes[b]);
        let shape = |i: usize| inp.shapes.get(i).copied().unwrap_or(0);
        pts[0] = border(first, sa, pts[1], shape(a));
        let k = pts.len();
        pts[k - 1] = border(last, sb, pts[k - 2], shape(b));
        label_at[ei] = label_v[ei].map_or(
            (
                (pts[0].0 + pts[k - 1].0) * 0.5,
                (pts[0].1 + pts[k - 1].1) * 0.5,
            ),
            |v| turn(centre(v)),
        );
        paths[ei] = basis(&pts);
    }
    Layout {
        pos,
        paths,
        label_at,
        w,
        h,
    }
}

/// The space between two neighbours in a rank, centre to centre.
fn gap(vs: &[V], a: usize, b: usize, sep: f32) -> f32 {
    let dummy = vs[a].real.is_none() && vs[a].w == 0.0 || vs[b].real.is_none() && vs[b].w == 0.0;
    vs[a].w * 0.5 + vs[b].w * 0.5 + if dummy { sep * 0.5 } else { sep }
}

/// Positions as near `want` as can be, in order, at least `gaps` apart:
/// least squares by merging blocks that would overlap.
fn place_in_order(want: &[f32], gaps: &[f32]) -> Vec<f32> {
    // offset of each item from its block's first.
    let mut off = alloc::vec![0.0f32; want.len()];
    for i in 1..want.len() {
        off[i] = off[i - 1] + gaps[i - 1];
    }
    // Blocks: (first, last, start position).
    let mut blocks: Vec<(usize, usize, f32)> = Vec::new();
    for i in 0..want.len() {
        let mut b = (i, i, want[i] - off[i]);
        // With offsets counted from the rank's first item, block starts
        // compare directly: a later block may not start before an earlier.
        while let Some(&p) = blocks.last() {
            if p.2 >= b.2 {
                // Overlap: merge, the start the mean of every member's wish.
                blocks.pop();
                let first = p.0;
                let last = b.1;
                let sum: f32 = (first..=last).map(|k| want[k] - off[k]).sum();
                b = (first, last, sum / (last - first + 1) as f32);
            } else {
                break;
            }
        }
        blocks.push(b);
    }
    let mut out = alloc::vec![0.0f32; want.len()];
    for (first, last, start) in blocks {
        for k in first..=last {
            out[k] = start + off[k];
        }
    }
    out
}

/// Where the line from `c` (the centre of a `size` box) toward `to`
/// leaves the box.
fn border(c: Pt, size: (f32, f32), to: Pt, shape: u8) -> Pt {
    let (dx, dy) = (to.0 - c.0, to.1 - c.1);
    let (hw, hh) = ((size.0 * 0.5).max(1e-3), (size.1 * 0.5).max(1e-3));
    let t = match shape {
        // |x|/hw + |y|/hh = 1
        1 => 1.0 / (dx.abs() / hw + dy.abs() / hh).max(1e-6),
        2 => {
            1.0 / ((dx / hw) * (dx / hw) + (dy / hh) * (dy / hh))
                .sqrt()
                .max(1e-6)
        }
        _ => {
            let tx = if dx.abs() > 1e-3 {
                hw / dx.abs()
            } else {
                f32::MAX
            };
            let ty = if dy.abs() > 1e-3 {
                hh / dy.abs()
            } else {
                f32::MAX
            };
            tx.min(ty)
        }
    }
    .min(1.0);
    (c.0 + dx * t, c.1 + dy * t)
}

/// d3's curveBasis through `pts`, flattened: it starts and ends on the
/// first and last points and is pulled toward the ones between.
pub fn basis(pts: &[Pt]) -> Vec<Pt> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let mut out = alloc::vec![pts[0]];
    let cubic = |out: &mut Vec<Pt>, p0: Pt, c1: Pt, c2: Pt, p3: Pt| {
        for k in 1..=8 {
            let t = k as f32 / 8.0;
            let u = 1.0 - t;
            out.push((
                u * u * u * p0.0
                    + 3.0 * u * u * t * c1.0
                    + 3.0 * u * t * t * c2.0
                    + t * t * t * p3.0,
                u * u * u * p0.1
                    + 3.0 * u * u * t * c1.1
                    + 3.0 * u * t * t * c2.1
                    + t * t * t * p3.1,
            ));
        }
    };
    let (mut x0, mut y0) = pts[0];
    let (mut x1, mut y1) = pts[1];
    let start = ((5.0 * x0 + x1) / 6.0, (5.0 * y0 + y1) / 6.0);
    out.push(start);
    let mut cur = start;
    for &(x, y) in &pts[2..] {
        let c1 = ((2.0 * x0 + x1) / 3.0, (2.0 * y0 + y1) / 3.0);
        let c2 = ((x0 + 2.0 * x1) / 3.0, (y0 + 2.0 * y1) / 3.0);
        let end = ((x0 + 4.0 * x1 + x) / 6.0, (y0 + 4.0 * y1 + y) / 6.0);
        cubic(&mut out, cur, c1, c2, end);
        cur = end;
        x0 = x1;
        y0 = y1;
        x1 = x;
        y1 = y;
    }
    // The end: one more piece toward the last point, then onto it.
    let c1 = ((2.0 * x0 + x1) / 3.0, (2.0 * y0 + y1) / 3.0);
    let c2 = ((x0 + 2.0 * x1) / 3.0, (y0 + 2.0 * y1) / 3.0);
    let end = ((x0 + 5.0 * x1) / 6.0, (y0 + 5.0 * y1) / 6.0);
    cubic(&mut out, cur, c1, c2, end);
    out.push((x1, y1));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(sizes: &[(f32, f32)], edges: &[(usize, usize)], dir: Dir) -> Layout {
        let labels = alloc::vec![(0.0, 0.0); edges.len()];
        layout(&Input {
            sizes,
            edges,
            labels: &labels,
            shapes: &[],
            dir,
            node_sep: 50.0,
            rank_sep: 50.0,
            label_gap: 0.0,
        })
    }

    #[test]
    fn ranks_order_and_place_like_dagre() {
        // A diamond: A to B and C, both to D. B and C side by side, A and D
        // centred over and under them.
        let s = [(60.0, 40.0); 4];
        let l = run(&s, &[(0, 1), (0, 2), (1, 3), (2, 3)], Dir::TopDown);
        let c = |i: usize| (l.pos[i].0 + 30.0, l.pos[i].1 + 20.0);
        assert!((c(1).1 - c(2).1).abs() < 0.1, "B and C share a rank");
        assert!(c(1).1 > c(0).1 && c(3).1 > c(1).1);
        assert!((c(0).0 - (c(1).0 + c(2).0) * 0.5).abs() < 1.0, "A centred");
        assert!((c(3).0 - c(0).0).abs() < 1.0, "D under A");
        assert!(
            (c(2).0 - c(1).0).abs() >= 110.0 - 0.1,
            "apart by width and gap"
        );
        // Rank gap: 40 high boxes, 50 between.
        assert!((c(1).1 - c(0).1 - 90.0).abs() < 0.1);
        // Paths start and end on the borders.
        let p = &l.paths[0];
        assert!((p[0].1 - (c(0).1 + 20.0)).abs() < 0.5);
        assert!((p[p.len() - 1].1 - (c(1).1 - 20.0)).abs() < 0.5);
    }

    #[test]
    fn crossings_are_removed_and_cycles_kept() {
        // Two sources whose targets are given crossed: the sweep uncrosses.
        let s = [(40.0, 30.0); 4];
        let l = run(&s, &[(0, 3), (1, 2)], Dir::TopDown);
        let x = |i: usize| l.pos[i].0;
        assert_eq!(x(0) < x(1), x(3) < x(2), "no crossing");
        // A cycle still lays out, every node placed once.
        let c = run(&s[..3], &[(0, 1), (1, 2), (2, 0)], Dir::LeftRight);
        assert!(
            c.pos[1].0 > c.pos[0].0 && c.pos[2].0 > c.pos[1].0,
            "left to right"
        );
        assert!(c.paths[2].len() >= 2);
        // A source pulled down next to its target: X only points at D.
        let p = run(
            &[(40.0, 30.0); 5],
            &[(0, 1), (1, 2), (2, 3), (4, 3)],
            Dir::TopDown,
        );
        assert!(
            p.pos[4].1 > p.pos[1].1,
            "X sits just above D, not at the top"
        );
    }

    #[test]
    fn basis_curves_end_where_they_start_and_stop() {
        let c = basis(&[(0.0, 0.0), (10.0, 10.0), (20.0, 0.0)]);
        assert_eq!(c[0], (0.0, 0.0));
        assert_eq!(*c.last().unwrap(), (20.0, 0.0));
        let top = c.iter().map(|p| p.1).fold(0.0f32, f32::max);
        assert!(
            top > 3.0 && top < 10.0,
            "pulled toward the middle point: {top}"
        );
        assert_eq!(
            place_in_order(&[0.0, 0.0, 0.0], &[10.0, 10.0]),
            [-10.0, 0.0, 10.0]
        );
    }
}
