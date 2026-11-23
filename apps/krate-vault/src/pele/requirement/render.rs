//! Ports diagrams/requirement/render.ts: lays out and draws a parsed
//! requirement diagram.

#![allow(clippy::needless_range_loop)]

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use super::db::RequirementDb;
use super::graph::{build_requirement_graph, GraphEdge, GraphNode};
use crate::json::Value;
use crate::pele::common::fit_width::{runs_across, turn_to_fit};
use crate::pele::common::ordered::OrderedMap;
use crate::pele::core::{RenderOptions, Rendered};
use crate::pele::errors::PeleError;
use crate::pele::js::{self, jmax, jmin, jround};
use crate::pele::layout::{cnode, compound_layout, direction, CEdge, CNode, Dir, LayeredOptions};
use crate::pele::svg::builder::{
    esc, esc_text, label_svg, num, shared_style, span_style, text_style,
};
use crate::pele::svg::edges::{default_reach, edge_label_svg, route_path, EdgePath};
use crate::pele::svg::ink::ink_fill;
use crate::pele::svg::root::{svg_document, Accessible};
use crate::pele::svg::theme::{class_names, resolve_style, rounded, ResolvedStyle};
use crate::pele::svg::title::title_label;
use crate::pele::text::label::{layout_label, line_middle, Label, Span};
use crate::pele::text::measurer::{style, Measure, METRICS};

const NODE_SEP: f64 = 40.0;
const EDGE_SEP: f64 = 16.0;
const RANK_SEP: f64 = 56.0;
const PAD_X: f64 = 14.0;
const PAD_Y: f64 = 10.0;
const ROW_GAP: f64 = 2.0;
const COLUMN_GAP: f64 = 12.0;
const MIN_WIDTH: f64 = 120.0;
const NAME_WRAP: f64 = 280.0;
const VALUE_WRAP: f64 = 240.0;
const LOOP: f64 = 26.0;
const CONTAINS_RADIUS: f64 = 6.0;
const ARROW_LENGTH: f64 = 9.0;
const ARROW_HALF: f64 = 4.5;
const END_STEP: f64 = 28.0;
const END_MARGIN: f64 = 14.0;

/// Ports `interface Row`.
struct Row {
    key: Label,
    value: Label,
    height: f64,
}

/// Ports `interface NodeView`.
struct NodeView {
    node: usize,
    style: ResolvedStyle,
    kind: Label,
    name: Label,
    rows: Vec<Row>,
    key_width: f64,
    head_height: f64,
    w: f64,
    h: f64,
}

/// Ports `restyle`: adds bold or italic to a whole label after its
/// markdown is read, since emphasis markers only toggle styles the label
/// does not start with.
fn restyle(label: Label, bits: u32, measurer: &dyn Measure) -> Label {
    if bits == 0 || label.lines.is_empty() {
        return label;
    }
    let mut width = 0.0;
    let mut widths: Vec<f64> = Vec::new();
    let lines: Vec<Vec<Span>> = label
        .lines
        .iter()
        .map(|line| {
            let mut total = 0.0;
            let spans: Vec<Span> = line
                .iter()
                .map(|span| {
                    let st = span.style | bits;
                    let w = if span.icon.is_some() {
                        span.width
                    } else {
                        measurer.width(&span.text, label.size, st)
                    };
                    total += w;
                    Span {
                        style: st,
                        width: w,
                        ..span.clone()
                    }
                })
                .collect();
            widths.push(total);
            if total > width {
                width = total;
            }
            spans
        })
        .collect();
    Label {
        lines,
        widths,
        width,
        ..label
    }
}

/// Ports `startText`: writes a measured label with its lines starting at
/// x, the first line's box starting at top.
fn start_text(label: &Label, x: f64, top: f64, attrs: &str) -> String {
    let shared = shared_style(label);
    let mut body = String::new();
    for (i, line) in label.lines.iter().enumerate() {
        let middle = top + line_middle(label, i);
        let y = middle + label.size * 0.35;
        let mut first = true;
        for span in line {
            if span.icon.is_some() || span.text.is_empty() {
                continue;
            }
            body.push_str(&format!(
                "<tspan{}{}>{}</tspan>",
                if first {
                    format!(" x=\"{}\" y=\"{}\"", num(x), num(y))
                } else {
                    String::new()
                },
                span_style(span.style & !shared),
                esc(&span.text)
            ));
            first = false;
        }
    }
    if body.is_empty() {
        String::new()
    } else {
        format!(
            "<text{} text-anchor=\"start\" xml:space=\"preserve\">{}</text>",
            text_style(attrs, shared),
            body
        )
    }
}

/// Ports `containsMarker`: a circled plus whose rim touches (x, y), where
/// (dx, dy) points at the node.
fn contains_marker(x: f64, y: f64, dx: f64, dy: f64) -> String {
    let r = CONTAINS_RADIUS;
    let cx = x - dx * r;
    let cy = y - dy * r;
    format!(
        "<circle class=\"pele-marker\" cx=\"{}\" cy=\"{}\" r=\"{}\"/><path class=\"pele-marker\" d=\"M{},{}L{},{}M{},{}L{},{}\"/>",
        num(cx),
        num(cy),
        js::num_str(r),
        num(cx - dx * r),
        num(cy - dy * r),
        num(cx + dx * r),
        num(cy + dy * r),
        num(cx + dy * r),
        num(cy - dx * r),
        num(cx - dy * r),
        num(cy + dx * r)
    )
}

/// Ports `arrowMarker`: an open arrowhead with its tip at (x, y), pointing
/// along (dx, dy).
fn arrow_marker(x: f64, y: f64, dx: f64, dy: f64) -> String {
    let bx = x - dx * ARROW_LENGTH;
    let by = y - dy * ARROW_LENGTH;
    format!(
        "<path class=\"pele-marker\" d=\"M{},{}L{},{}L{},{}\" stroke-linejoin=\"round\"/>",
        num(bx - dy * ARROW_HALF),
        num(by + dx * ARROW_HALF),
        num(x),
        num(y),
        num(bx + dy * ARROW_HALF),
        num(by - dx * ARROW_HALF)
    )
}

/// An end in `spreadEnds`: the edge whose route it is.
struct End {
    edge: usize,
    at: usize,
    toward: f64,
}

/// Ports `spreadEnds`: the layout attaches every edge to the middle of a
/// node's side. Ends that share a side are spread along it, in the order
/// of where they head, so their markers do not sit on each other.
fn spread_ends(cnodes: &[CNode], cedges: &mut [CEdge], views: &[NodeView]) {
    let mut sides: OrderedMap<Vec<End>> = OrderedMap::new();
    for (k, edge) in cedges.iter().enumerate() {
        let route = &edge.route;
        if edge.src == edge.dst || route.len() < 6 {
            continue;
        }
        for at in [0, route.len() - 3] {
            let node = if at == 0 { edge.src } else { edge.dst };
            let c = &cnodes[node];
            let dx = route[at] - c.x;
            let dy = route[at + 1] - c.y;
            let next = if at == 0 { 3 } else { at - 3 };
            let side: usize = if (dy.abs() - c.h / 2.0).abs() < 0.5 {
                if dy < 0.0 {
                    0
                } else {
                    2
                }
            } else if (dx.abs() - c.w / 2.0).abs() < 0.5 {
                if dx < 0.0 {
                    3
                } else {
                    1
                }
            } else {
                continue;
            };
            let key = js::num_str((node * 4 + side) as f64);
            let end = End {
                edge: k,
                at,
                toward: route[if side.is_multiple_of(2) {
                    next
                } else {
                    next + 1
                }],
            };
            match sides.get_mut(&key) {
                Some(list) => list.push(end),
                None => sides.insert(&key, alloc::vec![end]),
            }
        }
    }
    let keys: Vec<String> = sides.keys().map(String::from).collect();
    for key in keys {
        let list = sides.get_mut(&key).unwrap();
        if list.len() < 2 {
            continue;
        }
        let k = js::number(&key) as usize;
        let node = k >> 2;
        let vertical = k % 2 == 1;
        let span = (if vertical {
            views[node].h
        } else {
            views[node].w
        }) - 2.0 * END_MARGIN;
        let step = jmax(0.0, jmin(END_STEP, span / (list.len() as f64 - 1.0)));
        list.sort_by(|a, b| {
            let d = a.toward - b.toward;
            if d < 0.0 {
                core::cmp::Ordering::Less
            } else if d > 0.0 {
                core::cmp::Ordering::Greater
            } else {
                core::cmp::Ordering::Equal
            }
        });
        let count = list.len() as f64;
        for (j, end) in list.iter().enumerate() {
            let offset = (j as f64 - (count - 1.0) / 2.0) * step;
            if vertical {
                cedges[end.edge].route[end.at + 1] = cnodes[node].y + offset;
            } else {
                cedges[end.edge].route[end.at] = cnodes[node].x + offset;
            }
        }
    }
}

/// Ports `renderRequirement`.
pub fn render_requirement(
    model: &RequirementDb,
    config: &Value,
    options: &RenderOptions,
    palette_style: &str,
) -> Result<Rendered, PeleError> {
    if !runs_across(Some(&model.direction)) {
        return Ok(draw(model, config, options, palette_style, false));
    }
    turn_to_fit(options, |down| {
        Ok::<Rendered, PeleError>(draw(model, config, options, palette_style, down))
    })
}

/// `cssClasses.replace(/^default\s?/, '')`.
fn without_default(classes: &str) -> String {
    match classes.strip_prefix("default") {
        Some(rest) => {
            let mut chars = rest.chars();
            match chars.next() {
                Some(c) if js::is_space(c) => chars.as_str().into(),
                _ => rest.into(),
            }
        }
        None => classes.into(),
    }
}

/// Ports `draw`: `turned` draws a diagram that runs across as one that
/// runs down.
fn draw(
    model: &RequirementDb,
    _config: &Value,
    options: &RenderOptions,
    palette_style: &str,
    turned: bool,
) -> Rendered {
    let graph = build_requirement_graph(model);
    let size = options.font_size.unwrap_or(16.0);
    let small = jround(size * 0.875);
    let measurer: &dyn Measure = options.measurer.unwrap_or(&METRICS);
    let pad = options.padding.unwrap_or(8.0);
    let icons = options.icons;

    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    let mut views: Vec<NodeView> = Vec::new();
    let mut cnodes: Vec<CNode> = Vec::new();
    for (k, node) in graph.nodes.iter().enumerate() {
        let node: &GraphNode = node;
        let st = resolve_style(&node.css_styles);
        let base =
            (if st.bold { style::BOLD } else { 0 }) | (if st.italic { style::ITALIC } else { 0 });
        let name_size = st.font_size.unwrap_or(size);
        let body_size = st.font_size.unwrap_or(small);
        let requirement = node.requirement.as_ref();
        let element = node.element.as_ref();
        let kind = restyle(
            layout_label(
                Some(&format!(
                    "\u{ab}{}\u{bb}",
                    requirement.map_or("Element", |r| r.kind.as_str())
                )),
                false,
                measurer,
                body_size,
                4000.0,
                0,
                icons,
            ),
            base,
            measurer,
        );
        let name = restyle(
            layout_label(
                Some(&node.id),
                true,
                measurer,
                name_size,
                NAME_WRAP,
                0,
                icons,
            ),
            base | style::BOLD,
            measurer,
        );
        let fields: Vec<(&str, &str)> = match requirement {
            Some(r) => alloc::vec![
                ("ID", r.requirement_id.as_str()),
                ("Text", r.text.as_str()),
                ("Risk", r.risk.as_str()),
                ("Verification", r.verify_method.as_str()),
            ],
            None => {
                let e = element.unwrap();
                alloc::vec![("Type", e.kind.as_str()), ("Doc Ref", e.doc_ref.as_str())]
            }
        };
        let mut rows: Vec<Row> = Vec::new();
        let mut key_width = 0.0;
        let mut value_width = 0.0;
        let mut body_height = 0.0;
        for (label, text) in fields {
            if text.is_empty() {
                continue;
            }
            let key = restyle(
                layout_label(Some(label), false, measurer, body_size, 4000.0, 0, icons),
                base,
                measurer,
            );
            let value = restyle(
                layout_label(Some(text), true, measurer, body_size, VALUE_WRAP, 0, icons),
                base,
                measurer,
            );
            let height = jmax(key.height, value.height);
            key_width = jmax(key_width, key.width);
            value_width = jmax(value_width, value.width);
            body_height += height + ROW_GAP;
            rows.push(Row { key, value, height });
        }
        if !rows.is_empty() {
            body_height += 2.0 * PAD_Y - ROW_GAP;
        }
        let head_height = 2.0 * PAD_Y + kind.height + name.height;
        let w = jmax(
            jmax(MIN_WIDTH, jmax(kind.width, name.width) + 2.0 * PAD_X),
            if !rows.is_empty() {
                key_width + COLUMN_GAP + value_width + 2.0 * PAD_X
            } else {
                0.0
            },
        );
        // A requirement and an element may share a name; relationships then go to the requirement.
        if !index.contains_key(&node.id) {
            index.insert(node.id.clone(), views.len());
        }
        let mut c = cnode(w, head_height + body_height, -1);
        c.seq = views.len() as f64;
        views.push(NodeView {
            node: k,
            style: st,
            kind,
            name,
            rows,
            key_width,
            head_height,
            w,
            h: head_height + body_height,
        });
        cnodes.push(c);
    }

    let root_dir = if turned {
        Dir::Tb
    } else {
        direction(Some(&graph.direction))
    };
    let sideways = matches!(root_dir, Dir::Lr | Dir::Rl);
    let mut cedges: Vec<CEdge> = Vec::new();
    let mut edge_labels: Vec<Label> = Vec::new();
    let mut drawn: Vec<&GraphEdge> = Vec::new();
    for edge in &graph.edges {
        let src = edge.start.as_ref().and_then(|s| index.get(s)).copied();
        let dst = edge.end.as_ref().and_then(|s| index.get(s)).copied();
        let (Some(src), Some(dst)) = (src, dst) else {
            continue;
        };
        let label = layout_label(
            Some(&format!("\u{ab}{}\u{bb}", edge.kind)),
            false,
            measurer,
            small,
            4000.0,
            0,
            icons,
        );
        if src == dst {
            // A self-loop sits beside the node, across the flow, and the node's box grows to hold it.
            if sideways {
                cnodes[src].h += 2.0 * (LOOP + label.height + 8.0);
            } else {
                cnodes[src].w += 2.0 * (LOOP + label.width + 12.0);
            }
        }
        drawn.push(edge);
        cedges.push(CEdge {
            src,
            dst,
            minlen: 1.0,
            label_w: label.width + 12.0,
            label_h: label.height + 4.0,
            route: Vec::new(),
            label_x: 0.0,
            label_y: 0.0,
        });
        edge_labels.push(label);
    }

    let layout = compound_layout(
        &mut cnodes,
        &mut cedges,
        root_dir,
        &LayeredOptions {
            node_sep: NODE_SEP,
            edge_sep: EDGE_SEP,
            rank_sep: RANK_SEP,
            port_sep: 20.0,
            ..LayeredOptions::default()
        },
    );

    spread_ends(&cnodes, &mut cedges, &views);

    let title = title_label(model.title.as_deref(), measurer, size, 4000.0);
    let title_height = if title.height > 0.0 {
        title.height + 12.0
    } else {
        0.0
    };
    let inner = jmax(layout.width, title.width);
    let ox = pad + (inner - layout.width) / 2.0;
    let oy = pad + title_height;
    let width = (inner + 2.0 * pad).ceil();
    let height = (layout.height + title_height + 2.0 * pad).ceil();

    let mut nodes_out = String::new();
    for i in 0..views.len() {
        let view = &views[i];
        let node = &graph.nodes[view.node];
        let (w, h, st) = (view.w, view.h, &view.style);
        let color = ink_fill(&st.shape, &st.text, None);
        let muted = if color.is_empty() {
            String::from(" fill=\"var(--_m)\"")
        } else {
            color.clone()
        };
        let left = -w / 2.0;
        let top = -h / 2.0;
        let mut body = format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"var(--_s)\" stroke=\"var(--_b)\"{}/>",
            num(left),
            num(top),
            num(w),
            num(h),
            rounded(&st.shape)
        );
        body.push_str(&label_svg(
            &view.kind,
            0.0,
            top + PAD_Y + view.kind.height / 2.0,
            &format!(
                " class=\"pele-stereotype\" font-size=\"{}\"{}{}",
                js::num_str(view.kind.size),
                muted,
                st.text
            ),
            None,
        ));
        body.push_str(&label_svg(
            &view.name,
            0.0,
            top + PAD_Y + view.kind.height + view.name.height / 2.0,
            &format!(
                " class=\"pele-label\" font-size=\"{}\" font-weight=\"var(--_hw)\"{}{}",
                js::num_str(view.name.size),
                color,
                st.text
            ),
            None,
        ));
        if !view.rows.is_empty() {
            let divider = top + view.head_height;
            body.push_str(&format!(
                "<path class=\"pele-divider\" d=\"M{},{}H{}\" stroke=\"var(--_b)\"{}/>",
                num(left),
                num(divider),
                num(-left),
                st.line
            ));
            let mut y = divider + PAD_Y;
            let mut rows = String::new();
            for row in &view.rows {
                rows.push_str(&start_text(
                    &row.key,
                    left + PAD_X,
                    y,
                    &format!(" class=\"pele-field\"{}{}", muted, st.text),
                ));
                rows.push_str(&start_text(
                    &row.value,
                    left + PAD_X + view.key_width + COLUMN_GAP,
                    y,
                    &format!(" class=\"pele-value\"{}{}", color, st.text),
                ));
                y += row.height + ROW_GAP;
            }
            body.push_str(&format!(
                "<g class=\"pele-fields\" font-size=\"{}\">{}</g>",
                js::num_str(view.rows[0].key.size),
                rows
            ));
        }
        nodes_out.push_str(&format!(
            "<g class=\"pele-node {}{}\" data-id=\"{}\" transform=\"translate({},{})\">{}</g>",
            if node.requirement.is_some() {
                "pele-requirement"
            } else {
                "pele-element"
            },
            class_names(&without_default(&node.css_classes)),
            esc_text(&node.id),
            num(cnodes[i].x + ox),
            num(cnodes[i].y + oy),
            body
        ));
    }

    let mut edges_out = String::new();
    let mut labels_out = String::new();
    let mut loop_seen: BTreeMap<usize, usize> = BTreeMap::new();
    for i in 0..cedges.len() {
        let ce = &cedges[i];
        let edge = drawn[i];
        let label = &edge_labels[i];
        let start_trim = if edge.contains {
            2.0 * CONTAINS_RADIUS
        } else {
            0.0
        };
        let mut label_x = ce.label_x + ox;
        let mut label_y = ce.label_y + oy;
        let path: EdgePath = if ce.src == ce.dst {
            let view = &views[ce.src];
            let cx = cnodes[ce.src].x + ox;
            let cy = cnodes[ce.src].y + oy;
            let k = loop_seen.get(&ce.src).copied().unwrap_or(0);
            loop_seen.insert(ce.src, k + 1);
            let k = k as f64;
            let half = (if sideways { view.w } else { view.h }) / 2.0;
            let spread = jmin(half - 4.0, 12.0 + k * 6.0);
            let reach = LOOP + k * 8.0;
            // Local frame: `out` points away from the node, `along` runs along its side.
            let at = |out: f64, along: f64| -> String {
                if sideways {
                    format!("{},{}", num(cx + along), num(cy + view.h / 2.0 + out))
                } else {
                    format!("{},{}", num(cx + view.w / 2.0 + out), num(cy + along))
                }
            };
            // A cubic whose control points are this far out peaks at `reach`.
            let pull = reach / 0.75;
            let path = EdgePath {
                d: format!(
                    "M{}C{} {} {}",
                    at(start_trim, -spread),
                    at(pull, -spread),
                    at(pull, spread),
                    at(0.0, spread)
                ),
                tokens: None,
                sx: if sideways {
                    cx - spread
                } else {
                    cx + view.w / 2.0
                },
                sy: if sideways {
                    cy + view.h / 2.0
                } else {
                    cy - spread
                },
                sdx: if sideways { 0.0 } else { -1.0 },
                sdy: if sideways { -1.0 } else { 0.0 },
                ex: if sideways {
                    cx + spread
                } else {
                    cx + view.w / 2.0
                },
                ey: if sideways {
                    cy + view.h / 2.0
                } else {
                    cy + spread
                },
                edx: if sideways { 0.0 } else { -1.0 },
                edy: if sideways { -1.0 } else { 0.0 },
            };
            if sideways {
                label_x = cx;
                label_y = cy + view.h / 2.0 + reach + 4.0 + ce.label_h / 2.0;
            } else {
                label_x = cx + view.w / 2.0 + reach + 4.0 + ce.label_w / 2.0;
                label_y = cy;
            }
            path
        } else {
            let mut route = ce.route.clone();
            let mut k = 0;
            while k < route.len() {
                route[k] += ox;
                route[k + 1] += oy;
                k += 3;
            }
            route_path(
                &route,
                None,
                start_trim,
                0.0,
                default_reach(start_trim),
                0.0,
            )
        };
        edges_out.push_str(&format!(
            "<g class=\"pele-edge pele-relationship-{}\" data-id=\"{}\"><path d=\"{}\"{}/>{}</g>",
            if edge.contains { "contains" } else { "arrow" },
            esc_text(&edge.id),
            path.d,
            if edge.contains {
                ""
            } else {
                " stroke-dasharray=\"5 4\""
            },
            if edge.contains {
                contains_marker(path.sx, path.sy, path.sdx, path.sdy)
            } else {
                arrow_marker(path.ex, path.ey, path.edx, path.edy)
            }
        ));
        labels_out.push_str(&edge_label_svg(&edge.id, label, label_x, label_y, "", None));
    }

    let mut content = label_svg(
        &title,
        width / 2.0,
        pad + title.height / 2.0,
        " class=\"pele-title\" font-weight=\"var(--_tw)\"",
        None,
    );
    if !edges_out.is_empty() {
        content.push_str(&format!(
            "<g class=\"pele-edges\" fill=\"none\" stroke=\"var(--_l)\" stroke-linecap=\"round\">{}</g>",
            edges_out
        ));
    }
    if !labels_out.is_empty() {
        content.push_str(&format!(
            "<g class=\"pele-edge-labels\" font-size=\"{}\" fill=\"var(--_m)\">{}</g>",
            js::num_str(small),
            labels_out
        ));
    }
    content.push_str(&format!("<g class=\"pele-nodes\">{}</g>", nodes_out));
    let svg = svg_document(
        "requirement",
        width,
        height,
        size,
        options,
        palette_style,
        &Accessible {
            acc_title: model.acc_title.clone(),
            acc_descr: model.acc_descr.clone(),
        },
        &content,
    );
    Rendered {
        svg,
        width,
        height,
        links: Vec::new(),
    }
}
