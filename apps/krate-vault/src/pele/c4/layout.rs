//! Ports diagrams/c4/layout.ts: Mermaid's grid. In each boundary, its
//! shapes in rows of c4ShapeInRow, then the boundaries inside it in rows
//! of c4BoundaryInRow, each sized around its own content.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use super::db::{C4Db, ElRef};
use super::route::{Bx, HEAD, LABEL_PAD, LANE, NARROW};
use super::types::C4Element;
use crate::pele::js::{jmax, jmin};
use crate::pele::svg::edges::ARROW;
use crate::pele::text::label::{layout_label, Icons, Label};
use crate::pele::text::measurer::{style, Measure};

const SHAPE_W: f64 = 216.0;
const SHAPE_MIN_H: f64 = 64.0;
const PAD_X: f64 = 14.0;
const PAD_Y: f64 = 12.0;
/// Room between two columns for the label of a line across them.
const GAP_X: f64 = NARROW + LABEL_PAD + 3.0 * ARROW;
const GAP_Y: f64 = 88.0;
/// Room for a relation to run along the inside of a boundary.
pub const GROUP_PAD: f64 = LANE + 8.0;
const GROUP_GAP: f64 = 48.0;
/// Where a boundary's heading starts, from its top-left corner.
pub const HEAD_LEFT: f64 = 16.0;
pub const HEAD_TOP: f64 = 10.0;

/// Ports `FORMS`: shape keywords Mermaid accepts in $shape, $sprite or
/// $tags, as names of the shared shapes.
fn form_named(name: &str) -> Option<&'static str> {
    Some(match name {
        "person" => "person",
        "box" | "rounded" | "component" => "rect",
        "cylinder" | "database" | "db" => "cyl",
        "queue" | "pipe" => "h-cyl",
        _ => return None,
    })
}

/// Ports `interface Node`: a shape's box and labels (type, name,
/// technology, description). `shape` is its place in the db's shapes.
#[derive(Debug, Clone)]
pub struct Node {
    pub bx: Bx,
    pub shape: usize,
    pub labels: Vec<Label>,
    pub text_h: f64,
}

/// Ports `interface Group`: a boundary's box, what it holds, and its
/// heading (name, type, description). `boundary` is its place in the db's
/// boundaries; `kids` are groups, by the same places.
#[derive(Debug, Clone)]
pub struct Group {
    pub bx: Bx,
    pub boundary: usize,
    pub kids: Vec<usize>,
    pub nodes: Vec<Node>,
    pub labels: Vec<Label>,
    pub head: f64,
    pub head_w: f64,
    pub head_x: f64,
}

/// Ports `interface Layout`: the groups, one per boundary; `order` lists
/// those drawn, parents before children, the first the unnamed top level.
#[derive(Debug, Clone)]
pub struct Layout {
    pub groups: Vec<Group>,
    pub order: Vec<usize>,
    pub boxes: BTreeMap<ElRef, Bx>,
}

/// Ports `formOf`.
fn form_of(shape: &C4Element) -> &'static str {
    let lower = |s: &Option<String>| crate::pele::js::lower(s.as_deref().unwrap_or(""));
    if let Some(f) = form_named(&lower(&shape.shape)).or_else(|| form_named(&lower(&shape.sprite)))
    {
        return f;
    }
    if let Some(tags) = shape.tags.as_deref().filter(|t| !t.is_empty()) {
        for tag in tags.split(',') {
            if let Some(f) = form_named(&crate::pele::js::lower(crate::pele::js::trim(tag))) {
                return f;
            }
        }
    }
    let kind = &shape.kind;
    if kind.ends_with("person") {
        "person"
    } else if kind.ends_with("_db") {
        "cyl"
    } else if kind.ends_with("_queue") {
        "h-cyl"
    } else {
        "rect"
    }
}

/// Ports `measure`.
#[allow(clippy::too_many_arguments)]
fn measure(
    shape: &C4Element,
    index: usize,
    id: usize,
    measurer: &dyn Measure,
    size: f64,
    small: f64,
    width: f64,
    icons: Icons,
) -> Node {
    let form = form_of(shape);
    let cap_w = if form == "h-cyl" { 36.0 } else { 0.0 };
    let cap_h = if form == "person" {
        HEAD
    } else if form == "cyl" {
        24.0
    } else {
        0.0
    };
    let inner = width - 2.0 * PAD_X - cap_w;
    let techn = shape
        .techn
        .as_deref()
        .filter(|t| !t.is_empty())
        .map(|t| format!("[{}]", t));
    let labels = alloc::vec![
        layout_label(
            Some(&format!("\u{ab}{}\u{bb}", shape.kind.replace('_', " "))),
            false,
            measurer,
            small - 1.0,
            inner,
            0,
            icons
        ),
        layout_label(
            Some(&shape.label),
            false,
            measurer,
            size,
            inner,
            style::BOLD,
            icons
        ),
        layout_label(techn.as_deref(), false, measurer, small, inner, 0, icons),
        layout_label(
            shape.descr.as_deref(),
            false,
            measurer,
            small,
            inner,
            0,
            icons
        ),
    ];
    let mut tw = 0.0;
    let mut text_h = if labels[3].height > 0.0 { 4.0 } else { 0.0 };
    for label in &labels {
        tw = jmax(tw, label.width);
        text_h += label.height;
    }
    Node {
        bx: Bx {
            id,
            x: 0.0,
            y: 0.0,
            w: jmax(width, tw + 2.0 * PAD_X + cap_w),
            h: jmax(SHAPE_MIN_H, text_h + 2.0 * PAD_Y + cap_h),
            form,
        },
        shape: index,
        labels,
        text_h,
    }
}

/// Ports `rows`: places boxes left to right in rows of `per_row`, from
/// (0, 0), and returns the width and height the rows take. Shapes are
/// stretched to the height of their row; boundaries keep their own.
fn rows(boxes: &mut [Bx], per_row: usize, gap_x: f64, gap_y: f64, stretch: bool) -> (f64, f64) {
    let mut width = 0.0;
    let mut y = 0.0;
    let mut from = 0;
    while from < boxes.len() {
        let to = boxes.len().min(from.saturating_add(per_row));
        let mut x = 0.0;
        let mut row_h = 0.0;
        for b in &boxes[from..to] {
            row_h = jmax(row_h, b.h);
        }
        for b in &mut boxes[from..to] {
            b.x = x;
            b.y = y;
            if stretch {
                b.h = row_h;
            }
            x += b.w + gap_x;
        }
        width = jmax(width, x - gap_x);
        y += row_h + gap_y;
        from = from.saturating_add(per_row);
    }
    (width, if boxes.is_empty() { 0.0 } else { y - gap_y })
}

/// Ports `layoutC4`.
pub fn layout_c4(
    db: &C4Db,
    measurer: &dyn Measure,
    size: f64,
    small: f64,
    room: Option<f64>,
    gap: f64,
    icons: Icons,
) -> Layout {
    let group_pad = if room.is_none() { GROUP_PAD } else { 12.0 };
    let shapes_base = db.boundaries.len();
    let mut groups: Vec<Group> = Vec::new();
    for (k, boundary) in db.boundaries.iter().enumerate() {
        let label_room = match room {
            None => 4000.0,
            Some(r) => jmax(80.0, r - 2.0 * HEAD_LEFT - 2.0 * GROUP_PAD * 2.0),
        };
        let labels = if boundary.parent.is_some() {
            let kind = boundary
                .kind_type
                .as_deref()
                .filter(|t| !t.is_empty())
                .map(|t| format!("[{}]", t));
            alloc::vec![
                layout_label(
                    Some(&boundary.label),
                    false,
                    measurer,
                    size,
                    label_room,
                    style::BOLD,
                    icons
                ),
                layout_label(
                    kind.as_deref(),
                    false,
                    measurer,
                    small,
                    label_room,
                    0,
                    icons
                ),
                layout_label(
                    boundary.descr.as_deref(),
                    false,
                    measurer,
                    small,
                    label_room,
                    0,
                    icons
                ),
            ]
        } else {
            Vec::new()
        };
        let mut head = 0.0;
        let mut head_w = 0.0;
        for label in &labels {
            head += label.height;
            head_w = jmax(head_w, label.width);
        }
        groups.push(Group {
            bx: Bx {
                id: k,
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
                form: "rect",
            },
            boundary: k,
            kids: Vec::new(),
            nodes: Vec::new(),
            labels,
            head: if head > 0.0 {
                head + HEAD_TOP + 6.0 + LANE
            } else {
                group_pad
            },
            head_w,
            head_x: HEAD_LEFT,
        });
    }
    let root = 0usize;
    for k in 0..groups.len() {
        if let Some(parent) = db.boundaries[k].parent {
            groups[parent].kids.push(k);
        }
    }
    // A boundary that ends up inside itself is not reachable from the top, as in Mermaid.
    let mut order: Vec<usize> = alloc::vec![root];
    let mut depths: BTreeMap<usize, f64> = BTreeMap::new();
    depths.insert(root, 0.0);
    let mut i = 0;
    while i < order.len() {
        let kids = groups[order[i]].kids.clone();
        let d = depths.get(&order[i]).copied().unwrap_or(0.0);
        for kid in kids {
            depths.insert(kid, d + 1.0);
            order.push(kid);
        }
        i += 1;
    }
    for (k, shape) in db.shapes.iter().enumerate() {
        if let Some(parent) = shape.parent {
            let depth = depths.get(&parent).copied().unwrap_or(0.0);
            let node_width = match room {
                None => SHAPE_W,
                Some(r) => jmax(100.0, jmin(SHAPE_W, r - 2.0 * depth * group_pad - 32.0)),
            };
            let node = measure(
                shape,
                k,
                shapes_base + k,
                measurer,
                size,
                small,
                node_width,
                icons,
            );
            groups[parent].nodes.push(node);
        }
    }

    let per_row = jmax(1.0, db.shape_in_row);
    let groups_per_row = jmax(1.0, db.boundary_in_row);
    let mut kids_top: BTreeMap<usize, f64> = BTreeMap::new();
    for &g in order.iter().rev() {
        let gap_x = if room.is_none() { GAP_X } else { gap };
        let count = |boxes: &[Bx], limit: f64, gap: f64| -> usize {
            let Some(r) = room else {
                return limit as usize;
            };
            let mut widest: f64 = 1.0;
            for b in boxes {
                widest = jmax(widest, b.w);
            }
            jmax(
                1.0,
                jmin(
                    limit,
                    ((r + gap - 2.0 * group_pad) / (widest + gap)).floor(),
                ),
            ) as usize
        };
        let mut node_boxes: Vec<Bx> = groups[g].nodes.iter().map(|n| n.bx).collect();
        let n_count = count(&node_boxes, per_row, gap_x);
        let (grid_w, grid_h) = rows(&mut node_boxes, n_count, gap_x, GAP_Y, true);
        for (node, b) in groups[g].nodes.iter_mut().zip(node_boxes) {
            node.bx = b;
        }
        let mut kid_boxes: Vec<Bx> = groups[g].kids.iter().map(|&k| groups[k].bx).collect();
        let k_count = count(&kid_boxes, groups_per_row, GROUP_GAP);
        let (kids_w, kids_h) = rows(&mut kid_boxes, k_count, GROUP_GAP, GROUP_GAP, false);
        let kids = groups[g].kids.clone();
        for (k, b) in kids.into_iter().zip(kid_boxes) {
            groups[k].bx = b;
        }
        let between = if grid_h > 0.0 && kids_h > 0.0 {
            GROUP_GAP
        } else {
            0.0
        };
        let frame = if g == root { 0.0 } else { group_pad };
        kids_top.insert(g, grid_h + between);
        let group = &mut groups[g];
        group.bx.w = jmax(
            jmax(grid_w, kids_w),
            group.head_w + 2.0 * HEAD_LEFT - 2.0 * frame,
        ) + 2.0 * frame;
        group.bx.h =
            (if frame != 0.0 { group.head } else { frame }) + grid_h + between + kids_h + frame;
    }

    let mut boxes: BTreeMap<ElRef, Bx> = BTreeMap::new();
    for &g in &order {
        let left = groups[g].bx.x + if g == root { 0.0 } else { group_pad };
        let top = groups[g].bx.y + if g == root { 0.0 } else { groups[g].head };
        for node in groups[g].nodes.iter_mut() {
            node.bx.x += left;
            node.bx.y += top;
            boxes.insert(ElRef::Shape(node.shape), node.bx);
        }
        let kt = kids_top.get(&g).copied().unwrap_or(0.0);
        let kids = groups[g].kids.clone();
        for kid in kids {
            groups[kid].bx.x += left;
            groups[kid].bx.y += top + kt;
        }
        if g != root {
            boxes.insert(ElRef::Boundary(g), groups[g].bx);
        }
    }
    Layout {
        groups,
        order,
        boxes,
    }
}
