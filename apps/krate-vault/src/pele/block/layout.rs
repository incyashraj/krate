//! Ports diagrams/block/layout.ts: the grid a block diagram is laid out
//! on, each container's children in rows of its columns.

use alloc::format;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use crate::pele::errors::{PeleError, PeleErrorCode};
use crate::pele::js;

/// Ports `interface Cell`. `children` are places in the arena the layout
/// is given.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Cell {
    pub children: Vec<usize>,
    /// Grid width of this container: a positive count, or -1 to put every child on one row.
    pub columns: f64,
    pub span: f64,
    pub space: bool,
    /// Height reserved above a container's grid for its title.
    pub head: f64,
    /// Size the content needs.
    pub nw: f64,
    pub nh: f64,
    /// Placement: grid position, then center and size in pixels.
    pub col: f64,
    pub row: f64,
    pub cols: f64,
    pub heights: Vec<f64>,
    /// Extra space after each row or column (0 where none was set).
    pub row_gaps: Vec<f64>,
    pub col_gaps: Vec<f64>,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// What the layout lays out: anything that holds a [`Cell`].
pub trait HasCell {
    fn cell(&self) -> &Cell;
    fn cell_mut(&mut self) -> &mut Cell;
}

const MAX_SPAN: f64 = 1000.0;

/// Ports `calculateBlockPosition`.
pub fn calculate_block_position(columns: f64, position: f64) -> Result<(f64, f64), PeleError> {
    if columns == 0.0 || columns.fract() != 0.0 || !columns.is_finite() {
        return Err(PeleError::of(
            "Columns must be an integer !== 0.",
            PeleErrorCode::Semantic,
            "block",
        ));
    }
    if position < 0.0 || position.fract() != 0.0 || !position.is_finite() {
        return Err(PeleError::of(
            format!(
                "Position must be a non-negative integer.{}",
                js::num_str(position)
            ),
            PeleErrorCode::Semantic,
            "block",
        ));
    }
    if columns < 0.0 {
        return Ok((position, 0.0));
    }
    Ok((position % columns, (position / columns).floor()))
}

/// Ports `total`.
fn total(gaps: &[f64]) -> f64 {
    let mut sum = 0.0;
    for &gap in gaps {
        sum += gap;
    }
    sum
}

/// Ports `measure`: puts each child in its grid cell and sizes the
/// container. Every column is as wide as the widest block needs. Every row
/// that holds a plain block is as tall as the tallest block in the
/// container, as in Mermaid; a row of nothing but containers is only as
/// tall as they need.
fn measure<V: HasCell>(cells: &mut [V], at: usize, gap: f64, inset: f64) -> Result<(), PeleError> {
    let columns = cells[at].cell().columns;
    let children = cells[at].cell().children.clone();
    let mut heights: Vec<f64> = Vec::new();
    let mut position = 0.0;
    let mut cols = 0.0;
    let mut cw = 0.0;
    let mut tallest = 0.0;
    for &k in &children {
        let (px, py) = calculate_block_position(columns, position)?;
        let child = cells[k].cell_mut();
        // Mermaid lets a block wider than the rest of its row stick out of the grid. Here it stops at the last column.
        let mut span = if child.span >= 1.0 {
            js::jmin(child.span, MAX_SPAN)
        } else {
            1.0
        };
        if columns > 0.0 && span > columns - px {
            span = columns - px;
        }
        child.span = span;
        child.col = px;
        child.row = py;
        position += span;
        if px + span > cols {
            cols = px + span;
        }
        if py as usize == heights.len() {
            heights.push(0.0);
        }
        let row = py as usize;
        if row >= heights.len() {
            heights.resize(row + 1, 0.0);
        }
        if child.children.is_empty() {
            heights[row] = -1.0;
        } else if heights[row] >= 0.0 && child.nh > heights[row] {
            heights[row] = child.nh;
        }
        if child.space {
            continue;
        }
        let width = (child.nw - gap * (span - 1.0)) / span;
        if width > cw {
            cw = width;
        }
        if child.nh > tallest {
            tallest = child.nh;
        }
    }
    let b = cells[at].cell_mut();
    let mut height = (heights.len() as f64 - 1.0) * gap + total(&b.row_gaps);
    for h in heights.iter_mut() {
        if *h < 0.0 {
            *h = tallest;
        }
        height += *h;
    }
    b.cols = cols;
    b.heights = heights;
    b.nw = js::jmax(
        b.nw,
        cols * cw + (cols - 1.0) * gap + total(&b.col_gaps) + 2.0 * inset,
    );
    b.nh = height + 2.0 * inset + b.head;
    Ok(())
}

/// Ports `arrange`: gives each child its cell, sharing out any room the
/// container has beyond what it needs.
fn arrange<V: HasCell>(cells: &mut [V], at: usize, gap: f64, inset: f64) {
    let b = cells[at].cell().clone();
    let (cols, heights, row_gaps, col_gaps) = (b.cols, &b.heights, &b.row_gaps, &b.col_gaps);
    let rows = heights.len() as f64;
    let cw = (b.w - 2.0 * inset - (cols - 1.0) * gap - total(col_gaps)) / cols;
    let room = b.h - 2.0 * inset - b.head - (rows - 1.0) * gap - total(row_gaps);
    let mut offsets: Vec<f64> = Vec::new();
    if !col_gaps.is_empty() {
        let mut sum = 0.0;
        let mut col = 0.0;
        while col < cols {
            offsets.push(sum);
            sum += col_gaps.get(col as usize).copied().unwrap_or(0.0);
            col += 1.0;
        }
    }
    let mut needed = 0.0;
    for &h in heights {
        needed += h;
    }
    let scale = if needed > 0.0 { room / needed } else { 0.0 };
    let share = if needed > 0.0 { 0.0 } else { room / rows };
    let left = b.x - b.w / 2.0 + inset;
    let mut top = b.y - b.h / 2.0 + inset + b.head;
    let mut row = 0usize;
    for &k in &b.children {
        let child = cells[k].cell_mut();
        while (row as f64) < child.row {
            top += heights[row] * scale + share + gap + row_gaps.get(row).copied().unwrap_or(0.0);
            row += 1;
        }
        let before = offsets.get(child.col as usize).copied().unwrap_or(0.0);
        let last = child.col + child.span - 1.0;
        let after = if last >= 0.0 {
            offsets.get(last as usize).copied().unwrap_or(0.0)
        } else {
            0.0
        };
        child.w = cw * child.span + gap * (child.span - 1.0) + after - before;
        child.h = heights.get(row).copied().unwrap_or(f64::NAN) * scale + share;
        child.x = left + child.col * (cw + gap) + before + child.w / 2.0;
        child.y = top + child.h / 2.0;
    }
}

/// Ports `layout`: lays out a tree of cells in place. The root (place 0)
/// has no border, so its grid starts at the origin.
pub fn layout<V: HasCell>(cells: &mut [V], root: usize, gap: f64) -> Result<(f64, f64), PeleError> {
    let mut order: Vec<usize> = alloc::vec![root];
    let mut i = 0;
    while i < order.len() {
        let kids = cells[order[i]].cell().children.clone();
        order.extend(kids);
        i += 1;
    }
    for i in (0..order.len()).rev() {
        let k = order[i];
        if !cells[k].cell().children.is_empty() {
            measure(cells, k, gap, if i == 0 { 0.0 } else { gap })?;
        }
    }
    {
        let r = cells[root].cell_mut();
        r.w = r.nw;
        r.h = r.nh;
        r.x = r.w / 2.0;
        r.y = r.h / 2.0;
    }
    for (i, &k) in order.iter().enumerate() {
        if !cells[k].cell().children.is_empty() {
            arrange(cells, k, gap, if i == 0 { 0.0 } else { gap });
        }
    }
    let r = cells[root].cell();
    Ok((r.w, r.h))
}
