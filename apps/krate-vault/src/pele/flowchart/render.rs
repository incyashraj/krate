//! Ports diagrams/flowchart/render.ts: lays out and draws a parsed
//! flowchart.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use super::db::FlowDb;
use super::graph::{build_flow_graph, FlowGraph, GraphEdge, GraphNode};
use super::shapes::canonical_shape;
use super::types::LabelType;
use crate::json::Value;
use crate::pele::common::fit_width::{runs_across, tighten, title_room, turn_to_fit};
use crate::pele::core::{LinkInfo, RenderOptions, Rendered};
use crate::pele::errors::PeleError;
use crate::pele::js::{jmax, jmin, jround};
use crate::pele::layout::{
    cnode, compound_layout, direction, inherit_crossed_directions, shift_layout, CEdge, CNode, Dir,
    LayeredOptions, Size,
};
use crate::pele::svg::builder::{esc, esc_text, label_svg, num};
use crate::pele::svg::cluster::{
    cluster_title_x, mark_crossings, struck_title, title_crossed, Crossings,
};
use crate::pele::svg::edges::{
    casings_svg, edge_casing, edge_label_svg, loop_path, marker, marker_trim, over_fills,
    route_path, EdgePath, Fill, LoopLabel, SideReach,
};
use crate::pele::svg::ink::{ink_fill, paints};
use crate::pele::svg::root::{svg_document, Accessible};
use crate::pele::svg::shapes::{
    draw_shape, end_side, inset_route, shape_filled, shape_has_label, shape_size, shape_span,
};
use crate::pele::svg::theme::{class_names, resolve_style, rounded, sets_fill, ResolvedStyle};
use crate::pele::svg::title::title_label;
use crate::pele::text::entities::decode_entities;
use crate::pele::text::label::{layout_label, Icons, Label};
use crate::pele::text::measurer::{style, Measure, METRICS};
use crate::pele::util::url::{image_url, internal_link, linked, rel_attr, safe_url};

const NODE_SEP: f64 = 40.0;
const EDGE_SEP: f64 = 16.0;
const RANK_SEP: f64 = 48.0;
const GROUP_PAD: f64 = 20.0;
const LOOP: f64 = 26.0;
const SHAPE_ATTRS: &str = " fill=\"var(--_s)\" stroke=\"var(--_b)\"";
const LINE_ATTRS: &str = " stroke=\"var(--_b)\"";

/// Ports `numberOption`.
fn number_option(section: &Value, key: &str, fallback: f64) -> f64 {
    match section.get(key) {
        Some(Value::Num(v)) if *v > 0.0 => jmin(*v, 2000.0),
        _ => fallback,
    }
}

/// Ports `interface FlowVariant`: a diagram type that is a flowchart with a
/// different layout, which may draw its top-level groups itself.
pub trait FlowVariant {
    fn kind(&self) -> &str;
    fn prepare(&self, graph: &mut FlowGraph);
    fn layout(
        &self,
        nodes: &mut [CNode],
        edges: &mut [CEdge],
        dir: Dir,
        opt: &LayeredOptions,
        tight: bool,
    ) -> Size;
    /// Whether the variant has `drawGroup`.
    fn has_draw_group(&self) -> bool {
        false
    }
    #[allow(clippy::too_many_arguments)]
    fn draw_group(
        &self,
        _c: &CNode,
        _label: &Label,
        _style: &ResolvedStyle,
        _classes: &str,
        _id: &str,
        _dir: Dir,
        _icons: Icons,
    ) -> String {
        String::new()
    }
    /// The part of a drawGroup group that takes its fill, as [left, top, right, bottom].
    fn filled_box(&self, _c: &CNode, _dir: Dir) -> Option<[f64; 4]> {
        None
    }
}

/// Ports `assetSize`.
fn asset_size(value: Option<f64>, fallback: f64) -> f64 {
    match value {
        Some(v) if v > 0.0 && v <= 4000.0 => v,
        _ => fallback,
    }
}

/// Ports `interface NodeView`.
struct NodeView {
    style: ResolvedStyle,
    label: Label,
    shape: String,
    w: f64,
    h: f64,
    dy: f64,
    loops: usize,
}

/// Ports `renderFlowchart`.
pub fn render_flowchart(
    db: &FlowDb,
    config: &Value,
    options: &RenderOptions,
    palette_style: &str,
    variant: Option<&dyn FlowVariant>,
) -> Result<Rendered, PeleError> {
    let fitted = |down: bool| {
        tighten(options, |tight| {
            draw(db, config, options, palette_style, variant, down, tight)
        })
    };
    if runs_across(db.direction.as_deref()) {
        turn_to_fit(options, fitted)
    } else {
        fitted(false)
    }
}

/// Ports `draw`: `turned` draws a flowchart that runs across as one that
/// runs down; `tight` sets nodes closer together.
#[allow(clippy::too_many_arguments)]
fn draw(
    db: &FlowDb,
    config: &Value,
    options: &RenderOptions,
    palette_style: &str,
    variant: Option<&dyn FlowVariant>,
    turned: bool,
    tight: bool,
) -> Result<Rendered, PeleError> {
    let group_pad = if tight { 12.0 } else { GROUP_PAD };
    let own = variant.and_then(|v| config.get(v.kind()));
    let empty = Value::obj();
    let flow: &Value = match own {
        Some(o @ Value::Obj(_)) => o,
        _ => match config.get("flowchart") {
            Some(f @ Value::Obj(_)) => f,
            _ => &empty,
        },
    };
    let curve = match flow.get("curve") {
        Some(Value::Str(s)) => Some(s.as_str()),
        _ => None,
    };
    let mut graph = build_flow_graph(db, curve);
    if let Some(v) = variant {
        v.prepare(&mut graph);
    }
    let size = options.font_size.unwrap_or(16.0);
    let edge_size = jround(size * 0.875);
    let measurer: &dyn Measure = options.measurer.unwrap_or(&METRICS);
    let wrap_width = jmin(
        number_option(flow, "wrappingWidth", 200.0),
        if variant.is_some() && tight {
            100.0
        } else {
            f64::INFINITY
        },
    );
    let pad = options.padding.unwrap_or(8.0);
    let icons = options.icons;

    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    let mut views: Vec<NodeView> = Vec::new();
    let mut cnodes: Vec<CNode> = Vec::new();
    for (i, node) in graph.nodes.iter().enumerate() {
        index.insert(node.id.clone(), i);
    }

    for node in &graph.nodes {
        let declarations: Vec<String> = if !node.css_styles.is_empty() {
            node.css_compiled_styles
                .iter()
                .chain(node.css_styles.iter())
                .cloned()
                .collect()
        } else {
            node.css_compiled_styles.clone()
        };
        let st = resolve_style(&declarations);
        let text_style =
            (if st.bold { style::BOLD } else { 0 }) | (if st.italic { style::ITALIC } else { 0 });
        let markdown = node.label_type == LabelType::Markdown;
        let font_size = st.font_size.unwrap_or(size);
        let view;
        let mut c;
        if node.is_group {
            let max = if variant.is_some() && tight {
                jmax(80.0, title_room(options) / 2.0)
            } else {
                4000.0
            };
            let label = layout_label(
                node.label.as_deref(),
                markdown,
                measurer,
                font_size,
                max,
                text_style,
                icons,
            );
            c = cnode(0.0, 0.0, -1);
            c.is_group = true;
            c.dir = node
                .dir
                .as_deref()
                .filter(|d| !d.is_empty())
                .map(|d| direction(Some(d)));
            c.pad_x = group_pad;
            c.pad_top = if label.height > 0.0 {
                label.height + 16.0
            } else {
                GROUP_PAD
            };
            c.pad_bottom = group_pad;
            c.min_w = label.width + 2.0 * group_pad - 12.0;
            view = NodeView {
                style: st,
                label,
                shape: "group".to_string(),
                w: 0.0,
                h: 0.0,
                dy: 0.0,
                loops: 0,
            };
        } else {
            let shape = canonical_shape(Some(&node.shape));
            let has_img = node.img.as_deref().is_some_and(|s| !s.is_empty());
            let has_icon = node.icon.as_deref().is_some_and(|s| !s.is_empty());
            let text = if has_img || has_icon || shape_has_label(shape) {
                node.label.as_deref()
            } else {
                None
            };
            let label = layout_label(
                text, markdown, measurer, font_size, wrap_width, text_style, icons,
            );
            let (w, h, mut dy);
            if has_img || has_icon {
                let bx = if has_img { 80.0 } else { 48.0 };
                let aw = asset_size(node.asset_width, bx);
                let ah = asset_size(node.asset_height, bx);
                w = jmax(aw, label.width + 8.0);
                h = ah
                    + if label.height > 0.0 {
                        label.height + 6.0
                    } else {
                        0.0
                    };
                dy = if node.pos.as_deref() == Some("t") {
                    -ah / 2.0
                } else {
                    ah / 2.0
                };
                if label.height == 0.0 {
                    dy = 0.0;
                }
            } else {
                let s = shape_size(shape, label.width, label.height);
                w = s.w;
                h = s.h;
                dy = s.dy;
            }
            c = cnode(w, h, -1);
            view = NodeView {
                style: st,
                label,
                shape: shape.to_string(),
                w,
                h,
                dy,
                loops: 0,
            };
        }
        c.seq = views.len() as f64;
        views.push(view);
        cnodes.push(c);
    }
    for (i, node) in graph.nodes.iter().enumerate() {
        cnodes[i].parent = match &node.parent_id {
            Some(p) => index.get(p).map(|&k| k as i32).unwrap_or(-1),
            None => -1,
        };
    }
    let ends: Vec<(i32, i32)> = graph
        .edges
        .iter()
        .map(|e| {
            (
                index.get(&e.start).map(|&k| k as i32).unwrap_or(-1),
                index.get(&e.end).map(|&k| k as i32).unwrap_or(-1),
            )
        })
        .collect();
    inherit_crossed_directions(&mut cnodes, &ends);

    let root_dir = if turned {
        Dir::Tb
    } else {
        direction(db.direction.as_deref())
    };
    let flows_sideways = |cnodes: &[CNode], i: usize| -> bool {
        let mut dir: Option<Dir> = None;
        let mut p = cnodes[i].parent;
        let mut hops = 0;
        while p >= 0 && dir.is_none() && hops < 10000 {
            dir = cnodes[p as usize].dir;
            p = cnodes[p as usize].parent;
            hops += 1;
        }
        let dir = dir.unwrap_or(root_dir);
        dir == Dir::Lr || dir == Dir::Rl
    };
    // The edges on one side of a node spread along it; a picture or an icon keeps them at its middle.
    for i in 0..views.len() {
        let node = &graph.nodes[i];
        if node.is_group {
            cnodes[i].span = f64::INFINITY;
        } else if node.img.as_deref().is_none_or(|s| s.is_empty())
            && node.icon.as_deref().is_none_or(|s| s.is_empty())
        {
            let sideways = flows_sideways(&cnodes, i);
            cnodes[i].span = shape_span(&views[i].shape, views[i].w, views[i].h, sideways);
        }
    }

    let mut cedges: Vec<CEdge> = Vec::new();
    let mut edge_labels: Vec<Label> = Vec::new();
    let mut drawn: Vec<&GraphEdge> = Vec::new();
    for edge in &graph.edges {
        let (Some(&src), Some(&dst)) = (index.get(&edge.start), index.get(&edge.end)) else {
            continue;
        };
        let label = layout_label(
            Some(&edge.label),
            edge.label_type == LabelType::Markdown,
            measurer,
            edge_size,
            wrap_width,
            0,
            icons,
        );
        if src == dst {
            // A self-loop sits beside the node, across the flow, and the node's box grows to hold it.
            views[src].loops += 1;
            if flows_sideways(&cnodes, src) {
                cnodes[src].h += 2.0
                    * (LOOP
                        + if label.height > 0.0 {
                            label.height + 8.0
                        } else {
                            0.0
                        });
            } else {
                cnodes[src].w += 2.0
                    * (LOOP
                        + if label.width > 0.0 {
                            label.width + 12.0
                        } else {
                            0.0
                        });
            }
        }
        drawn.push(edge);
        cedges.push(CEdge {
            src,
            dst,
            minlen: edge.minlen.unwrap_or(1.0),
            label_w: if label.width > 0.0 {
                label.width + 12.0
            } else {
                0.0
            },
            label_h: if label.height > 0.0 {
                label.height + 4.0
            } else {
                0.0
            },
            route: Vec::new(),
            label_x: 0.0,
            label_y: 0.0,
        });
        edge_labels.push(label);
    }

    let layout_options = LayeredOptions {
        node_sep: number_option(flow, "nodeSpacing", NODE_SEP) * if tight { 0.5 } else { 1.0 },
        edge_sep: EDGE_SEP,
        rank_sep: number_option(flow, "rankSpacing", RANK_SEP) * if tight { 0.75 } else { 1.0 },
        port_sep: 20.0,
        tracks: curve.is_some_and(|c| c.starts_with("step")),
        straight: curve == Some("linear"),
        apart: false,
        head_room: marker_trim("arrow_point"),
        groups: None,
        order_only: false,
    };
    let layout = match variant {
        Some(v) => v.layout(&mut cnodes, &mut cedges, root_dir, &layout_options, tight),
        None => compound_layout(&mut cnodes, &mut cedges, root_dir, &layout_options),
    };

    let title = title_label(
        db.title.as_deref(),
        measurer,
        size,
        if tight { title_room(options) } else { 4000.0 },
    );
    let title_height = if title.height > 0.0 {
        title.height + 12.0
    } else {
        0.0
    };
    let inner = jmax(layout.width, title.width);
    let ox = pad + (inner - layout.width) / 2.0;
    let oy = pad + title_height;
    shift_layout(&mut cnodes, &mut cedges, ox, oy);
    let width = (inner + 2.0 * pad).ceil();
    let height = (layout.height + title_height + 2.0 * pad).ceil();
    let mut links: Vec<LinkInfo> = Vec::new();

    let mut crossings: Crossings = Crossings::new();
    let any_group = cnodes.iter().any(|c| c.is_group);
    if any_group {
        for e in &cedges {
            mark_crossings(&mut crossings, &e.route);
        }
    }
    let parents: Vec<i32> = cnodes.iter().map(|c| c.parent).collect();

    let mut clusters = String::new();
    let mut struck_titles = String::new();
    let mut nodes_out = String::new();
    let mut fills: Vec<Fill> = Vec::new();
    for i in 0..views.len() {
        let view = &views[i];
        let c = &cnodes[i];
        let node: &GraphNode = &graph.nodes[i];
        let x = c.x;
        let y = c.y;
        let classes = class_names(strip_default(&node.css_classes));
        let id = esc_text(&node.id);
        if c.is_group {
            if let Some(v) = variant.filter(|v| v.has_draw_group() && c.parent < 0) {
                if paints(&view.style.shape) {
                    if let Some(bx) = v.filled_box(c, root_dir) {
                        fills.push(Fill {
                            bx,
                            shape: view.style.shape.clone(),
                        });
                    }
                }
                clusters.push_str(&v.draw_group(
                    c,
                    &view.label,
                    &view.style,
                    &classes,
                    &id,
                    root_dir,
                    icons,
                ));
                continue;
            }
        }
        if c.is_group {
            if paints(&view.style.shape) {
                fills.push(Fill {
                    bx: [x - c.w / 2.0, y - c.h / 2.0, x + c.w / 2.0, y + c.h / 2.0],
                    shape: view.style.shape.clone(),
                });
            }
            let title_x = cluster_title_x(c, view.label.width, &crossings, group_pad);
            let title_y = y - c.h / 2.0 + 8.0 + view.label.height / 2.0;
            let struck = title_crossed(c, view.label.width, title_x, &crossings);
            // A title drawn over the edges sits on a halo in the theme's colours.
            let title_attrs = format!(
                " class=\"pele-cluster-label\"{}{}",
                if struck {
                    " fill=\"var(--_m)\"".to_string()
                } else {
                    ink_fill(&view.style.shape, &view.style.text, Some("var(--_m)"))
                },
                view.style.text
            );
            if struck {
                struck_titles.push_str(&struck_title(
                    &view.label,
                    title_x,
                    title_y,
                    &title_attrs,
                    &id,
                    &parents,
                    i,
                    icons,
                ));
            }
            clusters.push_str(&format!(
                "<g class=\"pele-cluster{}\" data-id=\"{}\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"var(--_a)\"{} stroke=\"var(--_b)\"{}/>{}</g>",
                classes,
                id,
                num(x - c.w / 2.0),
                num(y - c.h / 2.0),
                num(c.w),
                num(c.h),
                if sets_fill(&view.style.shape) { "" } else { " fill-opacity=\"0.5\"" },
                rounded(&view.style.shape),
                if struck {
                    String::new()
                } else {
                    label_svg(&view.label, title_x, title_y, &title_attrs, icons)
                }
            ));
            continue;
        }

        let w = view.w;
        let h = view.h;
        let inner;
        let mut ink = String::new();
        let img = node.img.as_deref().filter(|s| !s.is_empty());
        let icon = node.icon.as_deref().filter(|s| !s.is_empty());
        if let Some(img) = img {
            let aw = asset_size(node.asset_width, 80.0);
            let ah = asset_size(node.asset_height, 80.0);
            let top = if view.label.height > 0.0 {
                if node.pos.as_deref() == Some("t") {
                    h / 2.0 - ah
                } else {
                    -h / 2.0
                }
            } else {
                -ah / 2.0
            };
            inner = match image_url(&safe_url(img), options) {
                None => String::new(),
                Some(src) => format!(
                    "<image href=\"{}\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" preserveAspectRatio=\"{}\"/>",
                    esc(&src),
                    num(-aw / 2.0),
                    num(top),
                    num(aw),
                    num(ah),
                    if node.constraint.as_deref() == Some("on") { "xMidYMid meet" } else { "none" }
                ),
            };
        } else if let Some(icon) = icon {
            let bx = asset_size(node.asset_height, 48.0);
            let top = if view.label.height > 0.0 {
                if node.pos.as_deref() == Some("t") {
                    h / 2.0 - bx
                } else {
                    -h / 2.0
                }
            } else {
                -bx / 2.0
            };
            let frame = match node.shape.as_str() {
                "icon" => String::new(),
                "iconCircle" => format!(
                    "<circle cy=\"{}\" r=\"{}\"{}{}/>",
                    num(top + bx / 2.0),
                    num(bx / 2.0),
                    SHAPE_ATTRS,
                    view.style.shape
                ),
                _ => format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{}\"{}{}/>",
                    num(-bx / 2.0),
                    num(top),
                    num(bx),
                    num(bx),
                    if node.shape == "iconRounded" { 8 } else { 0 },
                    SHAPE_ATTRS,
                    view.style.shape
                ),
            };
            let glyph = bx * 0.6;
            let markup = icons.and_then(|f| f(icon)).unwrap_or_default();
            inner = format!(
                "{}<svg class=\"pele-icon\" data-icon=\"{}\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\">{}</svg>",
                frame,
                esc_text(icon),
                num(-glyph / 2.0),
                num(top + (bx - glyph) / 2.0),
                num(glyph),
                num(glyph),
                markup
            );
        } else {
            inner = draw_shape(
                &view.shape,
                w,
                h,
                &format!("{}{}", SHAPE_ATTRS, view.style.shape),
                &format!("{}{}", LINE_ATTRS, view.style.line),
            );
            if shape_filled(&view.shape) {
                ink = ink_fill(&view.style.shape, &view.style.text, None);
            }
        }

        let mut text = label_svg(
            &view.label,
            0.0,
            view.dy,
            &format!(" class=\"pele-label\"{}{}", ink, view.style.text),
            icons,
        );
        let internal = format!("{} ", classes).contains(" internal-link ");
        if let Some(label) = node
            .label
            .as_deref()
            .filter(|l| internal && !l.is_empty() && options.links != Some(false))
        {
            // The label is a note name, but it still ends up in an href, so it gets the same check as a URL.
            let name = decode_entities(label);
            let safe = internal_link(&name, options);
            let target = esc(&name);
            text = format!(
                "<a class=\"internal-link\"{} data-href=\"{}\">{}</a>",
                if safe {
                    format!(" href=\"{}\"{}", target, rel_attr(options))
                } else {
                    String::new()
                },
                target,
                text
            );
            links.push(LinkInfo {
                id: node.id.clone(),
                href: name,
                internal: true,
            });
        }
        let mut body = inner + &text;
        if let Some(tt) = node.tooltip.as_deref().filter(|t| !t.is_empty()) {
            body = format!("<title>{}</title>{}", esc_text(tt), body);
        }
        if let Some(link) = node.link.as_deref().filter(|l| !l.is_empty()) {
            let attrs = match node.link_target.as_deref().filter(|t| !t.is_empty()) {
                Some(t) => format!(" target=\"{}\"", esc(t)),
                None => String::new(),
            };
            body = linked(&body, link, &node.id, &mut links, options, &attrs);
        }
        nodes_out.push_str(&format!(
            "<g class=\"pele-node pele-shape-{}{}\" data-id=\"{}\" transform=\"translate({},{})\">{}</g>",
            view.shape,
            classes,
            id,
            num(x),
            num(y),
            body
        ));
    }

    let mut reach = SideReach::new();
    for (i, ce) in cedges.iter().enumerate() {
        if ce.src == ce.dst
            || drawn[i].thickness.as_deref() == Some("invisible")
            || ce.route.len() < 6
        {
            continue;
        }
        reach.add(
            ce.src,
            end_side(&ce.route, 0, &cnodes[ce.src]),
            &drawn[i].arrow_type_start,
        );
        reach.add(
            ce.dst,
            end_side(&ce.route, ce.route.len() - 3, &cnodes[ce.dst]),
            &drawn[i].arrow_type_end,
        );
    }

    let mut casings = String::new();
    let mut edges_out = String::new();
    let mut labels_out = String::new();
    let mut loop_seen: BTreeMap<usize, usize> = BTreeMap::new();
    for i in 0..cedges.len() {
        let edge = drawn[i];
        let st = if !edge.style.is_empty() || !edge.css_compiled_styles.is_empty() {
            let all: Vec<String> = edge
                .css_compiled_styles
                .iter()
                .chain(edge.style.iter())
                .cloned()
                .collect();
            Some(resolve_style(&all))
        } else {
            None
        };
        let line = st.as_ref().map(|s| s.line.as_str()).unwrap_or("");
        let invisible = edge.thickness.as_deref() == Some("invisible");
        if !invisible {
            let start_type = edge.arrow_type_start.as_str();
            let end_type = edge.arrow_type_end.as_str();
            let (src, dst) = (cedges[i].src, cedges[i].dst);
            let path: EdgePath = if src == dst {
                let view = &views[src];
                let k = loop_seen.get(&src).copied().unwrap_or(0);
                loop_seen.insert(src, k + 1);
                let sideways = flows_sideways(&cnodes, src);
                let (cx, cy) = (cnodes[src].x, cnodes[src].y);
                let ce = &mut cedges[i];
                loop_path(
                    cx,
                    cy,
                    view.w,
                    view.h,
                    k as f64,
                    sideways,
                    LOOP,
                    marker_trim(end_type),
                    LoopLabel {
                        label_w: ce.label_w,
                        label_h: ce.label_h,
                        label_x: &mut ce.label_x,
                        label_y: &mut ce.label_y,
                    },
                )
            } else {
                let mut route = cedges[i].route.clone();
                let leaves = reach.of(src, end_side(&route, 0, &cnodes[src]));
                let arrives = reach.of(
                    dst,
                    end_side(&route, route.len().saturating_sub(3), &cnodes[dst]),
                );
                inset_route(
                    &mut route,
                    0,
                    &views[src].shape,
                    views[src].w,
                    views[src].h,
                    &cnodes[src],
                );
                let last = route.len().saturating_sub(3);
                inset_route(
                    &mut route,
                    last,
                    &views[dst].shape,
                    views[dst].w,
                    views[dst].h,
                    &cnodes[dst],
                );
                route_path(
                    &route,
                    edge.curve.as_deref(),
                    marker_trim(start_type),
                    marker_trim(end_type),
                    leaves,
                    arrives,
                )
            };
            let over = over_fills(
                &path.d,
                &[path.sx, path.sy, path.ex, path.ey],
                &fills,
                line,
                path.tokens.as_deref(),
            );
            let color = match over.as_deref() {
                Some(o) if !o.is_empty() => o.to_string(),
                _ => match st
                    .as_ref()
                    .and_then(|s| s.stroke.as_deref())
                    .filter(|s| !s.is_empty())
                {
                    Some(stroke) => esc(stroke),
                    None => "var(--_l)".to_string(),
                },
            };
            let mut attrs = "";
            if edge.thickness.as_deref() == Some("thick") {
                attrs = " stroke-width=\"2.5\"";
            } else if edge.thickness.as_deref() == Some("dotted") {
                attrs = " stroke-dasharray=\"3 4\"";
            }
            let classes = class_names(
                &edge
                    .classes
                    .replace("edge-thickness-normal", "")
                    .replace("edge-pattern-solid", "")
                    .replace("flowchart-link", ""),
            );
            if over.as_deref() == Some("") {
                casings.push_str(&edge_casing(
                    &path,
                    start_type,
                    end_type,
                    line,
                    edge.thickness.as_deref() == Some("thick"),
                ));
            }
            edges_out.push_str(&format!(
                "<g class=\"pele-edge{}\" data-id=\"{}\"{}{}><path d=\"{}\"{}/>{}{}</g>",
                classes,
                esc_text(&edge.id),
                match over.as_deref() {
                    Some(o) if !o.is_empty() => format!(" stroke=\"{}\"", o),
                    _ => String::new(),
                },
                line,
                path.d,
                attrs,
                marker(start_type, path.sx, path.sy, path.sdx, path.sdy, &color, "none"),
                marker(end_type, path.ex, path.ey, path.edx, path.edy, &color, "none")
            ));
        }
        let label = &edge_labels[i];
        if label.width > 0.0 {
            let ce = &cedges[i];
            labels_out.push_str(&edge_label_svg(
                &edge.id,
                label,
                ce.label_x,
                ce.label_y,
                st.as_ref().map(|s| s.text.as_str()).unwrap_or(""),
                icons,
            ));
        }
    }

    let acc = Accessible {
        acc_title: db.acc_title.clone(),
        acc_descr: db.acc_descr.clone(),
    };
    let mut content = label_svg(
        &title,
        width / 2.0,
        pad + title.height / 2.0,
        " class=\"pele-title\" font-weight=\"var(--_tw)\"",
        None,
    );
    if !clusters.is_empty() {
        content.push_str(&format!("<g class=\"pele-clusters\">{}</g>", clusters));
    }
    content.push_str(&casings_svg(&casings));
    if !edges_out.is_empty() {
        content.push_str(&format!(
            "<g class=\"pele-edges\" fill=\"none\" stroke=\"var(--_l)\" stroke-linecap=\"round\">{}</g>",
            edges_out
        ));
    }
    if !struck_titles.is_empty() {
        content.push_str(&format!(
            "<g class=\"pele-cluster-titles\">{}</g>",
            struck_titles
        ));
    }
    if !labels_out.is_empty() {
        content.push_str(&format!(
            "<g class=\"pele-edge-labels\" font-size=\"{}\">{}</g>",
            crate::pele::js::num_str(edge_size),
            labels_out
        ));
    }
    content.push_str(&format!("<g class=\"pele-nodes\">{}</g>", nodes_out));
    let kind = variant
        .map(|v| v.kind().to_string())
        .unwrap_or_else(|| "flowchart".to_string());
    let svg = svg_document(
        &kind,
        width,
        height,
        size,
        options,
        palette_style,
        &acc,
        &content,
    );
    Ok(Rendered {
        svg,
        width,
        height,
        links,
    })
}

/// `cssClasses.replace(/^default\s?/, '')`.
fn strip_default(classes: &str) -> &str {
    match classes.strip_prefix("default") {
        Some(rest) => match rest.chars().next() {
            Some(c) if crate::pele::js::is_space(c) => &rest[c.len_utf8()..],
            _ => rest,
        },
        None => classes,
    }
}
