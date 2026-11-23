//! Ports diagrams/mindmap/render.ts.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use super::db::MindmapDb;
use crate::json::Value;
use crate::pele::common::fit_width::{title_room, turn_to_fit};
use crate::pele::common::icon::icon_svg;
use crate::pele::core::{RenderOptions, Rendered};
use crate::pele::errors::PeleError;
use crate::pele::js::{jmax, jmin, jround};
use crate::pele::preprocess::Config;
use crate::pele::svg::builder::{esc_text, label_svg, num};
use crate::pele::svg::palette::Palette;
use crate::pele::svg::root::{svg_document, Accessible};
use crate::pele::svg::shapes::{draw_shape, shape_inset, shape_size};
use crate::pele::svg::theme::{class_names, series_color};
use crate::pele::text::label::{icon_markup, label_line, layout_label, line_middle, Icons, Label};
use crate::pele::text::measurer::{style, Measure, METRICS};

/// Indexed by Mermaid's node type. A node without delimiters is text on a line, except the root.
const SHAPES: [&str; 7] = ["", "rounded", "rect", "circle", "cloud", "bang", "hex"];
const ROOT_SHAPE: &str = "stadium";

const GAP_X: f64 = 36.0;
const GAP_Y: f64 = 8.0;
/// Between two subtrees, of which at least one has children of its own.
const GROUP_GAP: f64 = 18.0;
const BRANCH_GAP_X: f64 = 52.0;
const BRANCH_GAP_Y: f64 = 28.0;
const TEXT_PAD: f64 = 6.0;
const PILL_PAD: f64 = 12.0;
const TINT: f64 = 0.22;
const ICON_GAP: f64 = 6.0;
/// Running down, each level is set in by this much, and links leave a node this far from its left edge.
const INDENT: f64 = 28.0;
const STEM: f64 = 12.0;
const BEND: f64 = 8.0;
const ROW_GAP: f64 = 10.0;

/// Ports `leftLabel`: a label with each line starting at `left`, for an outline.
fn left_label(label: &Label, left: f64, cy: f64, icons: Icons) -> String {
    if label.lines.len() < 2 {
        return label_svg(
            label,
            left + label.width / 2.0,
            cy,
            " class=\"pele-label\"",
            icons,
        );
    }
    let mut out = String::new();
    let top = cy - label.height / 2.0;
    for k in 0..label.lines.len() {
        out.push_str(&label_svg(
            &label_line(label, k),
            left + label.widths[k] / 2.0,
            top + line_middle(label, k),
            " class=\"pele-label\"",
            icons,
        ));
    }
    out
}

/// Ports `renderMindmap`.
pub fn render_mindmap(
    model: &MindmapDb,
    config: &Config,
    options: &RenderOptions,
    palette: &Palette,
) -> Result<Rendered, PeleError> {
    turn_to_fit(options, |down| -> Result<Rendered, PeleError> {
        Ok(draw(model, config, options, palette, down))
    })
}

/// Ports `draw`: `down` draws the map as an outline.
fn draw(
    model: &MindmapDb,
    config: &Config,
    options: &RenderOptions,
    palette: &Palette,
    down: bool,
) -> Rendered {
    let size = options.font_size.unwrap_or(16.0);
    let measurer: &dyn Measure = options.measurer.unwrap_or(&METRICS);
    let pad = options.padding.unwrap_or(8.0);
    let icons = options.icons;
    let wrap = match config.get("mindmap").and_then(|m| m.get("maxNodeWidth")) {
        Some(Value::Num(v)) if *v > 0.0 => jmin(*v, 2000.0),
        _ => 200.0,
    };
    let glyph = size * 1.25;

    let nodes = &model.nodes;
    let n = nodes.len();
    let mut parent = alloc::vec![0usize; n];
    let mut depth = alloc::vec![0.0f64; n];
    for i in 0..n {
        for &c in &nodes[i].children {
            parent[c] = i;
            depth[c] = depth[i] + 1.0;
        }
    }
    // An outline has the width the host gives it, less what its level is set in.
    let room = match options.max_width {
        Some(m) if down && m > 0.0 => m - 2.0 * pad,
        _ => f64::INFINITY,
    };
    let mut labels: Vec<Label> = Vec::with_capacity(n);
    let mut shapes: Vec<&'static str> = Vec::with_capacity(n);
    let mut w = alloc::vec![0.0f64; n];
    let mut h = alloc::vec![0.0f64; n];
    let mut dy = alloc::vec![0.0f64; n];
    // Where links meet the node, measured down from its top: its middle.
    let mut anchor = alloc::vec![0.0f64; n];
    // Whether the host has the node's icon. One it does not have takes no room.
    let mut drawn = alloc::vec![false; n];

    for i in 0..n {
        let node = &nodes[i];
        drawn[i] = node
            .icon
            .as_ref()
            .is_some_and(|icon| !icon_markup(icons, icon).is_empty());
        let fit = jmin(
            wrap,
            jmax(
                72.0,
                room - depth[i] * INDENT
                    - 2.0 * TEXT_PAD
                    - if drawn[i] { glyph + ICON_GAP } else { 0.0 },
            ),
        );
        let label = layout_label(Some(&node.descr), true, measurer, size, fit, 0, icons);
        let tw = label.width
            + if drawn[i] {
                glyph + if label.width > 0.0 { ICON_GAP } else { 0.0 }
            } else {
                0.0
            };
        let th = jmax(label.height, jround(size * 1.5));
        let named = SHAPES.get(node.kind as usize).copied().unwrap_or("");
        let mut shape = if !named.is_empty() {
            named
        } else if i == 0 {
            ROOT_SHAPE
        } else {
            ""
        };
        // In an outline the root heads a column, and the trunk drops from under its left end.
        if down && i == 0 && shape != "rect" && shape != "rounded" {
            shape = ROOT_SHAPE;
        }
        shapes.push(shape);
        if shape.is_empty() {
            // A node with no shape of its own is plain text when it ends a branch, and text in a
            // tinted pill when the branch goes on from it, which sets the levels apart.
            let pill = !node.children.is_empty();
            w[i] = tw + 2.0 * if pill { PILL_PAD } else { TEXT_PAD };
            h[i] = th + if pill { 6.0 } else { 0.0 };
            anchor[i] = h[i] / 2.0;
        } else {
            let s = shape_size(shape, tw, th);
            w[i] = s.w;
            h[i] = s.h;
            dy[i] = s.dy;
            anchor[i] = s.h / 2.0;
        }
        labels.push(label);
    }

    // Each subtree takes a band of height `extent`. `rel` is a subtree's offset among its siblings,
    // `own` the node's offset inside its band, and `kids` the offset of its children's bands.
    let mut extent = alloc::vec![0.0f64; n];
    let mut rel = alloc::vec![0.0f64; n];
    let mut own = alloc::vec![0.0f64; n];
    let mut kids = alloc::vec![0.0f64; n];
    let mut stacked = 0.0;

    // Stacks subtrees top to bottom and returns the height at which their parent's anchor belongs.
    let stack = |list: &[usize],
                 from: usize,
                 to: usize,
                 gap: f64,
                 rel: &mut [f64],
                 extent: &[f64],
                 own: &[f64],
                 stacked: &mut f64|
     -> f64 {
        let mut y = 0.0;
        let mut last = 0.0;
        for k in from..to {
            let c = list[k];
            rel[c] = y;
            let grouped = !nodes[c].children.is_empty()
                || (k + 1 < to && !nodes[list[k + 1]].children.is_empty());
            last = if grouped { jmax(gap, GROUP_GAP) } else { gap };
            y += extent[c] + last;
        }
        *stacked = y - last;
        let a = list[from];
        let b = list[to - 1];
        (rel[a] + own[a] + anchor[a] + rel[b] + own[b] + anchor[b]) / 2.0
    };

    // Nodes are in depth-first order, so going backwards visits children before their parent.
    for i in (1..n).rev() {
        let children = &nodes[i].children;
        if children.is_empty() {
            extent[i] = h[i];
            continue;
        }
        let top = stack(
            children,
            0,
            children.len(),
            GAP_Y,
            &mut rel,
            &extent,
            &own,
            &mut stacked,
        ) - anchor[i];
        let low = jmin(0.0, top);
        extent[i] = jmax(stacked, top + h[i]) - low;
        own[i] = top - low;
        kids[i] = -low;
    }

    let mut x = alloc::vec![0.0f64; n];
    let mut y = alloc::vec![0.0f64; n];
    let mut band = alloc::vec![0.0f64; n];
    let mut side = alloc::vec![0.0f64; n];
    // Running down: how far the trunk under each node has been drawn.
    let mut trunk = alloc::vec![0.0f64; n];

    if down {
        // Nodes are in depth-first order, which is the order of an outline's rows.
        let mut row = 0.0;
        for i in 0..n {
            if i > 0 && parent[i] == 0 {
                row += ROW_GAP;
            }
            x[i] = depth[i] * INDENT + w[i] / 2.0;
            y[i] = row;
            row += h[i] + ROW_GAP;
        }
    } else if n > 0 {
        // The first branches go right and the rest left, split where the two sides are closest in height.
        let branches = &nodes[0].children;
        let count = branches.len();
        let mut total = 0.0;
        for &b in branches {
            total += extent[b];
        }
        let mut split = count;
        let mut best = f64::INFINITY;
        let mut right = 0.0;
        for k in 1..count {
            right += extent[branches[k - 1]];
            let diff = (2.0 * right - total + (2.0 * k as f64 - count as f64) * BRANCH_GAP_Y).abs();
            if diff < best {
                best = diff;
                split = k;
            }
        }
        let (mut from, mut to, mut dir) = (0usize, split, 1.0);
        while from < count {
            let middle = stack(
                branches,
                from,
                to,
                BRANCH_GAP_Y,
                &mut rel,
                &extent,
                &own,
                &mut stacked,
            );
            for &c in &branches[from..to] {
                band[c] = rel[c] - middle;
                side[c] = dir;
            }
            from = to;
            to = count;
            dir = -1.0;
        }
        y[0] = -h[0] / 2.0;
        for i in 0..n {
            if i > 0 {
                y[i] = band[i] + own[i];
            }
            for &c in &nodes[i].children {
                if i > 0 {
                    side[c] = side[i];
                    band[c] = band[i] + kids[i] + rel[c];
                }
                x[c] = x[i]
                    + side[c]
                        * (w[i] / 2.0 + if i > 0 { GAP_X } else { BRANCH_GAP_X } + w[c] / 2.0);
            }
        }
    }

    let mut min_x: f64 = 0.0;
    let mut max_x: f64 = 0.0;
    let mut min_y: f64 = 0.0;
    let mut max_y: f64 = 0.0;
    let mut out = String::new();
    let mut edges = String::new();
    let mut body = String::new();
    let mut color = String::new();
    let flush = |section: Option<usize>,
                 out: &mut String,
                 edges: &mut String,
                 body: &mut String,
                 color: &str| {
        if body.is_empty() {
            return;
        }
        out.push_str(&format!(
            "<g class=\"pele-branch pele-section-{}\"><g class=\"pele-edges\" fill=\"none\" stroke=\"{}\" stroke-width=\"1.5\">{}</g>{}</g>",
            section.unwrap_or(0),
            color,
            edges,
            body
        ));
        edges.clear();
        body.clear();
    };

    // The root is drawn last, on top of the links that leave it.
    for k in 1..=n {
        let i = k % n;
        let node = &nodes[i];
        let shape = shapes[i];
        let label = &labels[i];
        let cx = x[i];
        let top = y[i];
        min_x = jmin(min_x, cx - w[i] / 2.0);
        max_x = jmax(max_x, cx + w[i] / 2.0);
        min_y = jmin(min_y, top);
        max_y = jmax(max_y, top + h[i]);

        if i == 0 || parent[i] == 0 {
            flush(
                nodes[k - 1].section,
                &mut out,
                &mut edges,
                &mut body,
                &color,
            );
        }
        if i > 0 && down {
            if parent[i] == 0 {
                color = series_color(node.section.unwrap_or(0) as i64);
            }
            let p = parent[i];
            // The link drops from under its parent and turns in to the node's left.
            let stem = if p == 0 {
                jmin(jmax(STEM, h[0] / 2.0), INDENT - BEND)
            } else {
                STEM
            };
            let x1 = x[p] - w[p] / 2.0 + jmin(stem, w[p] / 2.0);
            let y1 = if trunk[p] != 0.0 && !trunk[p].is_nan() {
                trunk[p]
            } else {
                y[p] + h[p]
            };
            let left = cx - w[i] / 2.0;
            let x2 = left
                + if !shape.is_empty() {
                    shape_inset(shape, w[i], h[i], 3, 0.0)
                } else {
                    0.0
                };
            let y2 = top + anchor[i];
            let bend = jmax(0.0, jmin(jmin(BEND, x2 - x1), y2 - y1));
            // The next link carries on from where this one leaves the trunk, so the trunk has no gap.
            trunk[p] = y2 - bend;
            edges.push_str(&format!(
                "<path class=\"pele-edge\" d=\"M{},{}V{}Q{},{} {},{}H{}\"/>",
                num(x1),
                num(y1),
                num(y2 - bend),
                num(x1),
                num(y2),
                num(x1 + bend),
                num(y2),
                num(x2)
            ));
        } else if i > 0 {
            if parent[i] == 0 {
                color = series_color(node.section.unwrap_or(0) as i64);
            }
            let p = parent[i];
            let dir = side[i];
            let from: u8 = if dir > 0.0 { 1 } else { 3 };
            let x1 = x[p]
                + dir
                    * (w[p] / 2.0
                        - if !shapes[p].is_empty() {
                            shape_inset(shapes[p], w[p], h[p], from, 0.0)
                        } else {
                            0.0
                        });
            let y1 = y[p] + anchor[p];
            let near = cx - (dir * w[i]) / 2.0;
            let x2 = near
                + if !shape.is_empty() {
                    dir * shape_inset(shape, w[i], h[i], 4 - from, 0.0)
                } else {
                    0.0
                };
            let y2 = top + anchor[i];
            let mid = num((x1 + x2) / 2.0);
            edges.push_str(&format!(
                "<path class=\"pele-edge\" d=\"M{},{}C{},{} {},{} {},{}\"/>",
                num(x1),
                num(y1),
                mid,
                num(y1),
                mid,
                num(y2),
                num(x2),
                num(y2)
            ));
        }

        let pill = shape.is_empty() && !node.children.is_empty();
        // Every node off the root is filled with a tint of its branch's color and outlined in it.
        let tint = format!(
            " fill=\"{}\" fill-opacity=\"{}\" stroke=\"{}\"",
            color,
            crate::pele::js::num_str(TINT),
            color
        );
        let mut inner = String::new();
        if !shape.is_empty() {
            inner = if i > 0 {
                draw_shape(shape, w[i], h[i], &tint, &format!(" stroke=\"{}\"", color))
            } else {
                draw_shape(
                    shape,
                    w[i],
                    h[i],
                    " fill=\"var(--_s)\" stroke=\"var(--_b)\"",
                    " stroke=\"var(--_b)\"",
                )
            };
        } else if pill {
            inner = format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{}\"{}/>",
                num(-w[i] / 2.0),
                num(-h[i] / 2.0),
                num(w[i]),
                num(h[i]),
                num(h[i] / 2.0),
                tint
            );
        }
        let mut shift = 0.0;
        if let Some(icon) = node.icon.as_ref().filter(|i| !i.is_empty()) {
            if drawn[i] {
                shift = (glyph + if label.width > 0.0 { ICON_GAP } else { 0.0 }) / 2.0;
                inner.push_str(&icon_svg(
                    icon,
                    -shift - label.width / 2.0,
                    dy[i] - glyph / 2.0,
                    glyph,
                    icons,
                ));
            }
        }
        let kind = if !shape.is_empty() {
            shape
        } else if pill {
            "branch"
        } else {
            "text"
        };
        let group = format!(
            "<g class=\"pele-node pele-shape-{}{}{}\" data-id=\"{}\" transform=\"translate({},{})\">{}{}</g>",
            kind,
            if i > 0 { "" } else { " pele-root" },
            class_names(node.class.as_deref().unwrap_or("")),
            esc_text(&node.node_id),
            num(cx),
            num(top + h[i] / 2.0),
            inner,
            if down && shape.is_empty() {
                left_label(label, shift - label.width / 2.0, dy[i], icons)
            } else {
                label_svg(label, shift, dy[i], " class=\"pele-label\"", icons)
            }
        );
        if i > 0 {
            body.push_str(&group);
        } else {
            out.push_str(&group);
        }
    }

    // Ports `titleLabel`, with the host's icons as Pele's layoutLabel has them.
    let title = layout_label(
        model.title.as_deref(),
        false,
        measurer,
        size,
        title_room(options),
        style::BOLD,
        icons,
    );
    let title_height = if title.height > 0.0 {
        title.height + 16.0
    } else {
        0.0
    };
    let width = jmax(max_x - min_x, title.width);
    let total_width = (width + 2.0 * pad).ceil();
    let total_height = (max_y - min_y + title_height + 2.0 * pad).ceil();
    let content = label_svg(
        &title,
        total_width / 2.0,
        pad + title.height / 2.0,
        " class=\"pele-title\" font-weight=\"var(--_tw)\"",
        None,
    ) + &format!(
        "<g transform=\"translate({},{})\">{}</g>",
        num(pad - min_x + (width - max_x + min_x) / 2.0),
        num(pad + title_height - min_y),
        out
    );

    Rendered {
        svg: svg_document(
            "mindmap",
            total_width,
            total_height,
            size,
            options,
            &palette.style,
            &Accessible::default(),
            &content,
        ),
        width: total_width,
        height: total_height,
        links: Vec::new(),
    }
}
