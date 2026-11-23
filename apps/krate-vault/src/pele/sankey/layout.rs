//! Ports diagrams/sankey/layout.ts: nodes in columns by their distance
//! from a source, sized by their flow, then settled next to their
//! neighbours so that the bands cross less.

use alloc::format;
use alloc::vec::Vec;
use core::cmp::Ordering;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use super::db::SankeyDb;
use crate::pele::errors::{PeleError, PeleErrorCode};
use crate::pele::js::{jmax, jmin};

/// Ports `type Alignment`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alignment {
    Justify,
    Left,
    Right,
    Center,
}

/// Ports `interface LayoutOptions`.
#[derive(Debug, Clone, Copy)]
pub struct LayoutOptions {
    pub width: f64,
    pub height: f64,
    pub node_width: f64,
    pub node_padding: f64,
    /// Height of a label line; thin nodes are kept at least this far apart where there is room.
    pub label_height: f64,
    pub alignment: Alignment,
}

/// Ports `interface PlacedNode`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedNode {
    pub layer: usize,
    /// Total flow through the node, in the diagram's units.
    pub value: f64,
    pub x: f64,
    pub y: f64,
    pub height: f64,
}

/// Ports `interface PlacedLink`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedLink {
    pub source: usize,
    pub target: usize,
    pub value: f64,
    pub thickness: f64,
    /// Vertical centers of the band where it leaves its source and where it enters its target.
    pub source_y: f64,
    pub target_y: f64,
}

/// Ports `interface SankeyLayout`.
#[derive(Debug, Clone)]
pub struct SankeyLayout {
    pub nodes: Vec<PlacedNode>,
    pub links: Vec<PlacedLink>,
}

const PASSES: i32 = 6;

/// A comparator's `d || a - b`.
fn by(d: f64, a: usize, b: usize) -> f64 {
    if d != 0.0 && !d.is_nan() {
        d
    } else {
        a as f64 - b as f64
    }
}

/// Ports `sort`: an insertion sort for short lists, the built-in (stable) sort otherwise.
fn sort(list: &mut [usize], compare: &dyn Fn(usize, usize) -> f64) {
    if list.len() > 8 {
        list.sort_by(|&a, &b| {
            let d = compare(a, b);
            if d < 0.0 {
                Ordering::Less
            } else if d > 0.0 {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        });
        return;
    }
    for i in 1..list.len() {
        let item = list[i];
        let mut j = i as isize - 1;
        while j >= 0 && compare(list[j as usize], item) > 0.0 {
            list[(j + 1) as usize] = list[j as usize];
            j -= 1;
        }
        list[(j + 1) as usize] = item;
    }
}

/// Ports `layoutSankey`.
pub fn layout_sankey(model: &SankeyDb, options: &LayoutOptions) -> Result<SankeyLayout, PeleError> {
    let n = model.nodes.len();
    let mut links: Vec<PlacedLink> = model
        .links
        .iter()
        .map(|link| PlacedLink {
            source: link.source,
            target: link.target,
            value: if link.value.is_finite() && link.value > 0.0 {
                link.value
            } else {
                0.0
            },
            thickness: 0.0,
            source_y: 0.0,
            target_y: 0.0,
        })
        .collect();
    let mut outgoing: Vec<Vec<usize>> = (0..n).map(|_| Vec::new()).collect();
    let mut incoming: Vec<Vec<usize>> = (0..n).map(|_| Vec::new()).collect();
    for (k, link) in links.iter().enumerate() {
        outgoing[link.source].push(k);
        incoming[link.target].push(k);
    }

    // Longest distance from a source, in topological order; what is left over lies on a cycle.
    let mut depth = alloc::vec![0i32; n];
    let mut pending = alloc::vec![0i32; n];
    let mut order: Vec<usize> = Vec::new();
    for i in 0..n {
        pending[i] = incoming[i].len() as i32;
        if pending[i] == 0 {
            order.push(i);
        }
    }
    let mut q = 0;
    while q < order.len() {
        let i = order[q];
        for &k in &outgoing[i] {
            let t = links[k].target;
            if depth[t] < depth[i] + 1 {
                depth[t] = depth[i] + 1;
            }
            pending[t] -= 1;
            if pending[t] == 0 {
                order.push(t);
            }
        }
        q += 1;
    }
    if order.len() < n {
        // Every node left over has a predecessor that is left over too; walking back finds the loop.
        let mut stuck = 0;
        while pending[stuck] == 0 {
            stuck += 1;
        }
        let mut seen = alloc::vec![false; n];
        while !seen[stuck] {
            seen[stuck] = true;
            for &k in &incoming[stuck] {
                if pending[links[k].source] > 0 {
                    stuck = links[k].source;
                    break;
                }
            }
        }
        return Err(PeleError::of(
            format!(
                "Sankey diagrams cannot have circular links, and \"{}\" is part of one.",
                model.nodes[stuck].id
            ),
            PeleErrorCode::Semantic,
            "sankey",
        ));
    }
    let mut layers: i32 = 1;
    for &d in &depth {
        if d + 1 > layers {
            layers = d + 1;
        }
    }
    let mut reach = alloc::vec![0i32; n];
    for q in (0..n).rev() {
        let i = order[q];
        for &k in &outgoing[i] {
            if reach[i] < reach[links[k].target] + 1 {
                reach[i] = reach[links[k].target] + 1;
            }
        }
    }

    // Flows are scaled to the largest one, so that sums of huge values stay finite.
    let mut largest = 0.0;
    for link in &links {
        if link.value > largest {
            largest = link.value;
        }
    }
    let unit = if largest > 0.0 { 1.0 / largest } else { 0.0 };

    let mut nodes: Vec<PlacedNode> = Vec::with_capacity(n);
    let mut flow = alloc::vec![0.0f64; n];
    let mut columns: Vec<Vec<usize>> = (0..layers).map(|_| Vec::new()).collect();
    let step = if layers > 1 {
        (options.width - options.node_width) / (layers - 1) as f64
    } else {
        0.0
    };
    for i in 0..n {
        let layer: i32 = match options.alignment {
            Alignment::Left => depth[i],
            Alignment::Right => layers - 1 - reach[i],
            Alignment::Center => {
                let mut layer = depth[i];
                if incoming[i].is_empty() && !outgoing[i].is_empty() {
                    layer = layers;
                    for &k in &outgoing[i] {
                        if depth[links[k].target] - 1 < layer {
                            layer = depth[links[k].target] - 1;
                        }
                    }
                }
                layer
            }
            Alignment::Justify => {
                if !outgoing[i].is_empty() {
                    depth[i]
                } else {
                    layers - 1
                }
            }
        };
        let mut into = 0.0;
        let mut out = 0.0;
        let mut value = 0.0;
        let mut value_out = 0.0;
        for &k in &incoming[i] {
            into += links[k].value * unit;
            value += links[k].value;
        }
        for &k in &outgoing[i] {
            out += links[k].value * unit;
            value_out += links[k].value;
        }
        flow[i] = jmax(into, out);
        nodes.push(PlacedNode {
            layer: layer as usize,
            value: jmax(value, value_out),
            x: layer as f64 * step,
            y: 0.0,
            height: 0.0,
        });
        columns[layer as usize].push(i);
    }

    let mut tallest = 1;
    for column in &columns {
        if column.len() > tallest {
            tallest = column.len();
        }
    }
    let gap = if tallest > 1 {
        jmin(options.node_padding, options.height / (tallest - 1) as f64)
    } else {
        options.node_padding
    };
    let mut scale = f64::INFINITY;
    for column in &columns {
        let mut sum = 0.0;
        for &i in column {
            sum += flow[i];
        }
        if sum > 0.0 {
            scale = jmin(
                scale,
                (options.height - (column.len() as f64 - 1.0) * gap) / sum,
            );
        }
    }
    if !scale.is_finite() || scale < 0.0 {
        scale = 0.0;
    }

    // A thin node still needs room for its label. Where every column can afford it, each node
    // claims at least a label's height, and the flows are scaled down to make that fit.
    let mut claim = jmax(0.0, options.label_height - gap);
    let fits = |s: f64, claim: f64| -> bool {
        for column in &columns {
            let mut extent = (column.len() as f64 - 1.0) * gap;
            for &i in column {
                extent += jmax(flow[i] * s, claim);
            }
            if extent > options.height + 1e-6 {
                return false;
            }
        }
        true
    };
    if claim > 0.0 && !fits(scale, claim) {
        if fits(0.0, claim) {
            let mut low = 0.0;
            for _ in 0..40 {
                let mid = (low + scale) / 2.0;
                if fits(mid, claim) {
                    low = mid;
                } else {
                    scale = mid;
                }
            }
            scale = low;
        } else {
            claim = 0.0;
        }
    }

    // The room a node keeps clear above and below itself.
    let mut margin = alloc::vec![0.0f64; n];
    for link in links.iter_mut() {
        link.thickness = link.value * unit * scale;
    }
    for column in &columns {
        let mut y = 0.0;
        for &i in column {
            nodes[i].height = flow[i] * scale;
            margin[i] = jmax(0.0, claim - nodes[i].height) / 2.0;
            nodes[i].y = y + margin[i];
            y += nodes[i].height + 2.0 * margin[i] + gap;
        }
        // Spare room is shared out between the nodes of a column.
        let spare = (options.height - y + gap) / (column.len() as f64 + 1.0);
        for (k, &i) in column.iter().enumerate() {
            nodes[i].y += spare * (k as f64 + 1.0);
        }
    }

    // Bands leave and enter a node in the order of where they go, so they do not cross at it.
    // These are the offsets of each band's middle from the top of the node at either end.
    let mut source_offset = alloc::vec![0.0f64; links.len()];
    let mut target_offset = alloc::vec![0.0f64; links.len()];

    fn center(nodes: &[PlacedNode], i: usize) -> f64 {
        nodes[i].y + nodes[i].height / 2.0
    }
    fn stack_out(
        i: usize,
        nodes: &[PlacedNode],
        links: &[PlacedLink],
        outgoing: &mut [Vec<usize>],
        source_offset: &mut [f64],
    ) {
        let mut y = 0.0;
        let by_target = |a: usize, b: usize| -> f64 {
            by(
                center(nodes, links[a].target) - center(nodes, links[b].target),
                a,
                b,
            )
        };
        sort(&mut outgoing[i], &by_target);
        for &k in &outgoing[i] {
            source_offset[k] = y + links[k].thickness / 2.0;
            y += links[k].thickness;
        }
    }
    fn stack_in(
        i: usize,
        nodes: &[PlacedNode],
        links: &[PlacedLink],
        incoming: &mut [Vec<usize>],
        target_offset: &mut [f64],
    ) {
        let mut y = 0.0;
        let by_source = |a: usize, b: usize| -> f64 {
            by(
                center(nodes, links[a].source) - center(nodes, links[b].source),
                a,
                b,
            )
        };
        sort(&mut incoming[i], &by_source);
        for &k in &incoming[i] {
            target_offset[k] = y + links[k].thickness / 2.0;
            y += links[k].thickness;
        }
    }

    macro_rules! stack_all {
        () => {
            for i in 0..n {
                stack_out(i, &nodes, &links, &mut outgoing, &mut source_offset);
                stack_in(i, &nodes, &links, &mut incoming, &mut target_offset);
            }
        };
    }

    // Moves each node of a column towards where its bands would run level, then restores the
    // order and the gaps. `forward` looks at the bands coming in, otherwise at those going out.
    #[allow(clippy::too_many_arguments)]
    fn settle(
        column: &mut [usize],
        pull: f64,
        forward: bool,
        nodes: &mut [PlacedNode],
        links: &[PlacedLink],
        incoming: &mut [Vec<usize>],
        outgoing: &mut [Vec<usize>],
        source_offset: &mut [f64],
        target_offset: &mut [f64],
        margin: &[f64],
        gap: f64,
        height: f64,
    ) {
        for &i in column.iter() {
            let empty = if forward {
                incoming[i].is_empty()
            } else {
                outgoing[i].is_empty()
            };
            if empty {
                continue;
            }
            if forward {
                stack_in(i, nodes, links, incoming, target_offset);
            } else {
                stack_out(i, nodes, links, outgoing, source_offset);
            }
            let list = if forward { &incoming[i] } else { &outgoing[i] };
            let mut weighted = 0.0;
            let mut weight = 0.0;
            let mut plain = 0.0;
            for &k in list {
                let level = if forward {
                    nodes[links[k].source].y + source_offset[k] - target_offset[k]
                } else {
                    nodes[links[k].target].y + target_offset[k] - source_offset[k]
                };
                weighted += level * links[k].thickness;
                weight += links[k].thickness;
                plain += center(
                    nodes,
                    if forward {
                        links[k].source
                    } else {
                        links[k].target
                    },
                ) - nodes[i].height / 2.0;
            }
            let wanted = if weight > 0.0 {
                weighted / weight
            } else {
                plain / list.len() as f64
            };
            nodes[i].y += (wanted - nodes[i].y) * pull;
        }
        {
            let by_center =
                |a: usize, b: usize| -> f64 { by(center(nodes, a) - center(nodes, b), a, b) };
            sort(column, &by_center);
        }
        let mut y = 0.0;
        for &i in column.iter() {
            if nodes[i].y - margin[i] < y {
                nodes[i].y = y + margin[i];
            }
            y = nodes[i].y + nodes[i].height + margin[i] + gap;
        }
        y = height;
        for k in (0..column.len()).rev() {
            let i = column[k];
            if nodes[i].y + nodes[i].height + margin[i] > y {
                nodes[i].y = y - nodes[i].height - margin[i];
            }
            y = nodes[i].y - margin[i] - gap;
        }
    }

    stack_all!();
    for pass in 0..PASSES {
        let pull = 0.99f64.powf(pass as f64);
        for l in (0..(layers - 1).max(0) as usize).rev() {
            settle(
                &mut columns[l],
                pull,
                false,
                &mut nodes,
                &links,
                &mut incoming,
                &mut outgoing,
                &mut source_offset,
                &mut target_offset,
                &margin,
                gap,
                options.height,
            );
        }
        stack_all!();
        for column in columns.iter_mut().skip(1) {
            settle(
                column,
                pull,
                true,
                &mut nodes,
                &links,
                &mut incoming,
                &mut outgoing,
                &mut source_offset,
                &mut target_offset,
                &margin,
                gap,
                options.height,
            );
        }
        stack_all!();
    }
    for k in 0..links.len() {
        links[k].source_y = nodes[links[k].source].y + source_offset[k];
        links[k].target_y = nodes[links[k].target].y + target_offset[k];
    }

    Ok(SankeyLayout { nodes, links })
}
