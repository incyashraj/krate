//! Ports diagrams/er/render.ts: lays out and draws a parsed ER diagram.
//!
//! Math and the `monospace` render option are not ported: no span is math
//! and attributes are never set in monospace.

#![allow(clippy::needless_range_loop)]

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use super::db::ErDb;
use super::graph::{build_er_graph, ErGraphEdge, ErGraphNode};
use super::markers::{er_marker, MARKER_LENGTH};
use crate::json::Value;
use crate::pele::common::fit_width::{runs_across, turn_to_fit};
use crate::pele::common::generics::parse_generic_types;
use crate::pele::common::ordered::OrderedMap;
use crate::pele::core::{LinkInfo, RenderOptions, Rendered};
use crate::pele::errors::PeleError;
use crate::pele::js::{self, jhypot, jmax, jmin, jround, sign};
use crate::pele::layout::{
    cnode, compound_layout, direction, shift_layout, CEdge, CNode, Dir, LayeredOptions,
};
use crate::pele::svg::builder::{
    esc, esc_text, label_svg, num, shared_style, span_style, text_style,
};
use crate::pele::svg::cluster::{cluster_title_x, mark_crossings, Crossings};
use crate::pele::svg::edges::{
    casing, casings_svg, default_reach, edge_ink, edge_label_svg, fill_under, hollow_on,
    route_path, Fill, Under,
};
use crate::pele::svg::ink::{ink, ink_fill, paints, ON_DARK, ON_LIGHT};
use crate::pele::svg::root::{svg_document, Accessible};
use crate::pele::svg::theme::{
    class_names, resolve_style, rounded, sets_fill, ResolvedStyle, ROUNDED,
};
use crate::pele::svg::title::title_label;
use crate::pele::text::entities::decode_entities;
use crate::pele::text::label::{layout_label, line_middle, Icons, Label};
use crate::pele::text::measurer::{style, Measure, METRICS};
use crate::pele::util::url::{internal_link, rel_attr};

const NODE_SEP: f64 = 48.0;
const EDGE_SEP: f64 = 20.0;
const RANK_SEP: f64 = 64.0;
const GROUP_PAD: f64 = 20.0;
const LOOP: f64 = 28.0;
const LOOP_BAND: f64 = 38.0;
const MIN_WIDTH: f64 = 96.0;
const BOX_PAD_X: f64 = 18.0;
const BOX_PAD_Y: f64 = 12.0;
const HEAD_PAD_Y: f64 = 9.0;
const CELL_PAD_X: f64 = 12.0;
const CELL_GAP: f64 = 16.0;
const ROW_PAD: f64 = 9.0;
const COMMENT_WRAP: f64 = 260.0;
const NO_WRAP: f64 = 1e6;
const PORT_GAP: f64 = 18.0;
const PORT_MARGIN: f64 = 14.0;
const BOX_ATTRS: &str = " fill=\"var(--_s)\" stroke=\"var(--_b)\"";

/// Ports `numberOption`.
fn number_option(config: &Value, key: &str, fallback: f64) -> f64 {
    match config.get("er").and_then(|s| s.get(key)) {
        Some(Value::Num(v)) if *v > 0.0 => jmin(*v, 2000.0),
        _ => fallback,
    }
}

/// Ports `interface Row`: the type, name, keys and comment cells.
struct Row {
    cells: [Label; 4],
    h: f64,
}

/// Ports `interface NodeView`.
struct NodeView {
    style: ResolvedStyle,
    label: Label,
    w: f64,
    h: f64,
    head_h: f64,
    rows: Vec<Row>,
    /// Left edge of each column, measured from the left edge of the box.
    cols: Vec<f64>,
    row_size: f64,
    key_size: f64,
    mono: u32,
    /// Whether self-relationships loop out of the bottom side rather than the right.
    below: bool,
}

/// Ports `interface Loop`: a relationship from an entity to itself, with
/// the stretch of the side it loops out of.
struct Loop {
    edge: usize,
    band: f64,
    at: f64,
}

/// Ports `textLeft`: writes a measured label starting at x, with the
/// baseline of its first line at y.
fn text_left(label: &Label, x: f64, y: f64, attrs: &str, icons: Icons, mono: u32) -> String {
    let lines = &label.lines;
    if lines.is_empty() {
        return String::new();
    }
    let only = if lines.len() == 1 && lines[0].len() == 1 {
        Some(&lines[0][0])
    } else {
        None
    };
    // The short form collapses repeated spaces in monospace text.
    if let Some(only) = only {
        if only.icon.is_none() && only.style == mono && !(mono != 0 && only.text.contains("  ")) {
            return format!(
                "<text{} x=\"{}\" y=\"{}\">{}</text>",
                text_style(attrs, mono),
                num(x),
                num(y),
                esc(&only.text)
            );
        }
    }
    let shared = shared_style(label);
    let shift = label.size * 0.35;
    let top = y - shift - label.line_height / 2.0;
    let mut body = String::new();
    let mut extra = String::new();
    for (i, line) in lines.iter().enumerate() {
        let middle = top + line_middle(label, i);
        let y = middle + shift;
        let has_icon = line.iter().any(|s| s.icon.is_some());
        let mut sx = x;
        let mut placed = false;
        for span in line {
            if let Some(icon) = &span.icon {
                let side = label.size * 1.1;
                extra.push_str(&format!(
                    "<svg class=\"pele-icon\" data-icon=\"{}\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\">{}</svg>",
                    esc_text(icon),
                    num(sx),
                    num(y - label.size * 0.35 - side / 2.0),
                    num(side),
                    num(side),
                    crate::pele::text::label::icon_markup(icons, icon)
                ));
            } else if !span.text.is_empty() {
                let pos = if has_icon || !placed {
                    format!(" x=\"{}\" y=\"{}\"", num(sx), num(y))
                } else {
                    String::new()
                };
                body.push_str(&format!(
                    "<tspan{}{}>{}</tspan>",
                    pos,
                    span_style(span.style & !shared),
                    esc(&span.text)
                ));
                placed = true;
            }
            sx += span.width;
        }
    }
    format!(
        "<text{} xml:space=\"preserve\">{}</text>{}",
        text_style(attrs, shared),
        body,
        extra
    )
}

/// Ports `cellText`: Mermaid reads `~` pairs as angle brackets. They are
/// written as entities so the label parser does not take `<int>` for a tag.
fn cell_text(raw: &str) -> String {
    let converted = parse_generic_types(raw);
    if converted == raw {
        converted
    } else {
        converted.replace('<', "&lt;").replace('>', "&gt;")
    }
}

/// Ports `renderEr`.
pub fn render_er(
    db: &ErDb,
    config: &Value,
    options: &RenderOptions,
    palette_style: &str,
) -> Result<Rendered, PeleError> {
    if !runs_across(Some(&db.direction)) {
        return Ok(draw(db, config, options, palette_style, false));
    }
    turn_to_fit(options, |down| {
        Ok::<Rendered, PeleError>(draw(db, config, options, palette_style, down))
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
    db: &ErDb,
    config: &Value,
    options: &RenderOptions,
    palette_style: &str,
    turned: bool,
) -> Rendered {
    let graph = build_er_graph(db);
    let size = options.font_size.unwrap_or(16.0);
    let small_size = jround(size * 0.875);
    let measurer: &dyn Measure = options.measurer.unwrap_or(&METRICS);
    let pad = options.padding.unwrap_or(8.0);
    let icons = options.icons;

    let mut index: OrderedMap<usize> = OrderedMap::new();
    let mut views: Vec<NodeView> = Vec::new();
    let mut cnodes: Vec<CNode> = Vec::new();
    for (i, node) in graph.nodes.iter().enumerate() {
        index.insert(&node.id, i);
    }

    let mut cedges: Vec<CEdge> = Vec::new();
    let mut edge_labels: Vec<Label> = Vec::new();
    let mut drawn: Vec<&ErGraphEdge> = Vec::new();
    // The loops of each node, in the order their nodes first loop.
    let mut loops: OrderedMap<Vec<Loop>> = OrderedMap::new();
    let mut loop_of: Vec<Option<(usize, usize)>> = Vec::new();
    for edge in &graph.edges {
        let (Some(&src), Some(&dst)) = (index.get(&edge.start), index.get(&edge.end)) else {
            continue;
        };
        let label = layout_label(
            Some(&edge.label),
            true,
            measurer,
            small_size,
            200.0,
            0,
            icons,
        );
        let mut lp = None;
        if src == dst {
            let key = js::num_str(src as f64);
            let k = cedges.len();
            match loops.get_mut(&key) {
                Some(list) => {
                    lp = Some((src, list.len()));
                    list.push(Loop {
                        edge: k,
                        band: 0.0,
                        at: 0.0,
                    });
                }
                None => {
                    lp = Some((src, 0));
                    loops.insert(
                        &key,
                        alloc::vec![Loop {
                            edge: k,
                            band: 0.0,
                            at: 0.0
                        }],
                    );
                }
            }
        }
        loop_of.push(lp);
        drawn.push(edge);
        cedges.push(CEdge {
            src,
            dst,
            minlen: 1.0,
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

    // The direction of the level a node is laid out in: its nearest subgraph that sets one, or the diagram's.
    let horizontal = |i: usize| -> bool {
        let mut at = i;
        // Subgraphs can name each other as members, so the walk up is bounded.
        for _ in 0..graph.nodes.len() {
            let up = graph.nodes[at]
                .parent_id
                .as_ref()
                .and_then(|p| index.get(p))
                .copied();
            let Some(up) = up else {
                break;
            };
            if let Some(dir) = graph.nodes[up].dir.as_deref().filter(|d| !d.is_empty()) {
                return dir == "LR" || dir == "RL";
            }
            at = up;
        }
        db.direction == "LR" || db.direction == "RL"
    };

    for i in 0..graph.nodes.len() {
        let node: &ErGraphNode = &graph.nodes[i];
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
        let base =
            (if st.bold { style::BOLD } else { 0 }) | (if st.italic { style::ITALIC } else { 0 });
        // The `monospace` option is not ported.
        let mono = 0;
        let name_size = st.font_size.unwrap_or(size);
        let row_size = st.font_size.unwrap_or(small_size);
        let key_size = st.font_size.unwrap_or(jround(size * 0.75));
        let label = if node.is_group {
            layout_label(
                Some(&node.label),
                true,
                measurer,
                name_size,
                4000.0,
                base,
                icons,
            )
        } else {
            let shown = if node.alias.is_empty() {
                &node.label
            } else {
                &node.alias
            };
            layout_label(Some(shown), true, measurer, name_size, 240.0, base, icons)
        };
        let key = js::num_str(i as f64);
        let own_count = loops.get(&key).map_or(0, |l| l.len());
        // Loops leave by a side the layout keeps free of other edges: the right when ranks run down,
        // the bottom when they run across.
        let below = own_count > 0 && !node.is_group && horizontal(i);
        let mut view = NodeView {
            style: st,
            label,
            w: 0.0,
            h: 0.0,
            head_h: 0.0,
            rows: Vec::new(),
            cols: Vec::new(),
            row_size,
            key_size,
            mono,
            below,
        };
        let mut reach = 0.0;
        let mut bands = 0.0;
        if let Some(own) = loops.get_mut(&key) {
            for lp in own.iter_mut() {
                let l = &edge_labels[lp.edge];
                lp.band = jmax(
                    LOOP_BAND,
                    if below {
                        l.width + 12.0
                    } else {
                        l.height + 6.0
                    },
                );
                bands += lp.band;
                reach = jmax(reach, if below { l.height } else { l.width });
            }
        }
        let min_w = jmax(MIN_WIDTH, if below { bands + 8.0 } else { 0.0 });
        let mut c: CNode;
        if node.is_group {
            c = cnode(0.0, 0.0, -1);
            c.is_group = true;
            c.dir = node
                .dir
                .as_deref()
                .filter(|d| !d.is_empty())
                .map(|d| direction(Some(d)));
            c.pad_x = GROUP_PAD;
            c.pad_top = if view.label.height > 0.0 {
                view.label.height + 16.0
            } else {
                GROUP_PAD
            };
            c.pad_bottom = GROUP_PAD;
            c.min_w = jmax(view.label.width + 2.0 * GROUP_PAD - 12.0, MIN_WIDTH);
        } else {
            let name_h = jmax(view.label.height, jround(name_size * 1.5));
            if node.attributes.is_empty() {
                view.w = jmax(view.label.width + 2.0 * BOX_PAD_X, min_w);
                view.h = jmax(
                    name_h + 2.0 * BOX_PAD_Y,
                    if below { 0.0 } else { bands + 8.0 },
                );
            } else {
                let row_line = jround(row_size * 1.5);
                let mut widths = [0.0f64; 4];
                for attribute in &node.attributes {
                    let cells: [Label; 4] = [
                        layout_label(
                            Some(&cell_text(&attribute.kind)),
                            true,
                            measurer,
                            row_size,
                            NO_WRAP,
                            base | mono,
                            icons,
                        ),
                        layout_label(
                            Some(&cell_text(&attribute.name)),
                            true,
                            measurer,
                            row_size,
                            NO_WRAP,
                            base | mono,
                            icons,
                        ),
                        layout_label(
                            Some(&attribute.keys.join(", ")),
                            false,
                            measurer,
                            key_size,
                            NO_WRAP,
                            base | mono,
                            icons,
                        ),
                        layout_label(
                            Some(&cell_text(&attribute.comment)),
                            true,
                            measurer,
                            row_size,
                            COMMENT_WRAP,
                            base,
                            icons,
                        ),
                    ];
                    let mut h = row_line;
                    for k in 0..4 {
                        if cells[k].width > widths[k] {
                            widths[k] = cells[k].width;
                        }
                        if cells[k].height > h {
                            h = cells[k].height;
                        }
                    }
                    view.rows.push(Row {
                        cells,
                        h: h + ROW_PAD,
                    });
                    view.h += h + ROW_PAD;
                }
                let mut columns = 0.0;
                let mut inner = 0.0;
                for &w in &widths {
                    if w == 0.0 {
                        continue;
                    }
                    inner += w + if columns > 0.0 { CELL_GAP } else { 0.0 };
                    columns += 1.0;
                }
                view.w = jmax(
                    jmax(inner + 2.0 * CELL_PAD_X, view.label.width + 2.0 * BOX_PAD_X),
                    min_w,
                );
                // A box wider than its table shares the spare width between the columns.
                let spare = if columns > 0.0 {
                    (view.w - inner - 2.0 * CELL_PAD_X) / columns
                } else {
                    0.0
                };
                let mut x = CELL_PAD_X;
                for &w in &widths {
                    view.cols.push(x);
                    if w > 0.0 {
                        x += w + spare + CELL_GAP;
                    }
                }
                view.head_h = name_h + 2.0 * HEAD_PAD_Y;
                view.h += view.head_h;
            }
            // The layout keeps a node centered in its box, so room for loops is added on both sides.
            let room = if own_count > 0 {
                2.0 * (MARKER_LENGTH + LOOP * 0.75 + 10.0 + reach)
            } else {
                0.0
            };
            c = if below {
                cnode(view.w, view.h + room, -1)
            } else {
                cnode(view.w + room, view.h, -1)
            };
        }
        c.seq = views.len() as f64;
        views.push(view);
        cnodes.push(c);
    }
    for (i, node) in graph.nodes.iter().enumerate() {
        cnodes[i].parent = match &node.parent_id {
            Some(p) => index.get(p).map_or(-1, |&k| k as i32),
            None => -1,
        };
    }

    let layout = compound_layout(
        &mut cnodes,
        &mut cedges,
        if turned {
            Dir::Tb
        } else {
            direction(Some(&db.direction))
        },
        &LayeredOptions {
            node_sep: number_option(config, "nodeSpacing", NODE_SEP),
            edge_sep: EDGE_SEP,
            rank_sep: number_option(config, "rankSpacing", RANK_SEP),
            port_sep: 24.0,
            ..LayeredOptions::default()
        },
    );

    spread_ends(&mut cedges, &cnodes, &views);

    // A subgraph gets its size from the layout, so its loops hang outside it and may widen the diagram.
    let mut layout_width = layout.width;
    for (key, own) in loops.iter() {
        let i = js::number(key) as usize;
        let c = &cnodes[i];
        if !c.is_group {
            continue;
        }
        let mut reach = 0.0;
        for lp in own {
            reach = jmax(reach, edge_labels[lp.edge].width);
        }
        layout_width = jmax(
            layout_width,
            c.x + c.w / 2.0 + MARKER_LENGTH + LOOP * 0.75 + 10.0 + reach,
        );
    }

    let title = title_label(db.title.as_deref(), measurer, size, 4000.0);
    let title_height = if title.height > 0.0 {
        title.height + 12.0
    } else {
        0.0
    };
    let inner = jmax(layout_width, title.width);
    let ox = pad + (inner - layout_width) / 2.0;
    let oy = pad + title_height;
    shift_layout(&mut cnodes, &mut cedges, ox, oy);
    let width = (inner + 2.0 * pad).ceil();
    let height = (layout.height + title_height + 2.0 * pad).ceil();
    let mut links: Vec<LinkInfo> = Vec::new();

    let mut crossings: Crossings = Crossings::new();
    if cnodes.iter().any(|c| c.is_group) {
        for e in &cedges {
            mark_crossings(&mut crossings, &e.route);
        }
    }

    let mut clusters = String::new();
    let mut fills: Vec<Fill> = Vec::new();
    let mut nodes_out = String::new();
    for i in 0..views.len() {
        let c = &cnodes[i];
        let node = &graph.nodes[i];
        let classes = class_names(&without_default(&node.css_classes));
        if c.is_group {
            views[i].w = c.w;
            views[i].h = c.h;
            let view = &views[i];
            let x = c.x - c.w / 2.0;
            let y = c.y - c.h / 2.0;
            let title_x = cluster_title_x(c, view.label.width, &crossings, GROUP_PAD);
            if paints(&view.style.shape) {
                fills.push(Fill {
                    bx: [x, y, x + c.w, y + c.h],
                    shape: view.style.shape.clone(),
                });
            }
            clusters.push_str(&format!(
                "<g class=\"pele-cluster{}\" data-id=\"{}\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"var(--_a)\"{} stroke=\"var(--_b)\"{}/>{}</g>",
                classes,
                esc_text(&node.id),
                num(x),
                num(y),
                num(c.w),
                num(c.h),
                if sets_fill(&view.style.shape) {
                    ""
                } else {
                    " fill-opacity=\"0.5\""
                },
                rounded(&view.style.shape),
                label_svg(
                    &view.label,
                    title_x,
                    y + 8.0 + view.label.height / 2.0,
                    &format!(
                        " class=\"pele-cluster-label\"{}{}",
                        ink_fill(&view.style.shape, &view.style.text, Some("var(--_m)")),
                        view.style.text
                    ),
                    icons
                )
            ));
            continue;
        }

        let view = &views[i];
        let w = view.w;
        let h = view.h;
        let x0 = -w / 2.0;
        let y0 = -h / 2.0;
        let text = &view.style.text;
        let color = ink_fill(&view.style.shape, &view.style.text, None);
        let muted = if color.is_empty() {
            String::from(" fill=\"var(--_m)\"")
        } else {
            color.clone()
        };
        let shape = format!(
            "x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"",
            num(x0),
            num(y0),
            num(w),
            num(h)
        );
        let mut name = label_svg(
            &view.label,
            0.0,
            if !view.rows.is_empty() {
                y0 + view.head_h / 2.0
            } else {
                0.0
            },
            &format!(" class=\"pele-label\"{}{}", color, text),
            icons,
        );
        let display = decode_entities(if node.alias.is_empty() {
            &node.label
        } else {
            &node.alias
        });
        let spaced = format!("{} ", classes);
        if (spaced.contains(" internal-link ")) && !name.is_empty() && options.links != Some(false)
        {
            // The name ends up in an href, so it gets the same check as a URL.
            let safe = internal_link(&display, options);
            let target = esc(&display);
            name = format!(
                "<a class=\"internal-link\"{} data-href=\"{}\">{}</a>",
                if safe {
                    format!(" href=\"{}\"{}", target, rel_attr(options))
                } else {
                    String::new()
                },
                target,
                name
            );
            links.push(LinkInfo {
                id: node.label.clone(),
                href: display.clone(),
                internal: true,
            });
        }

        let body = if view.rows.is_empty() {
            format!(
                "<rect {}{}/>{}",
                shape,
                rounded(&format!("{}{}", BOX_ATTRS, view.style.shape)),
                name
            )
        } else {
            let row_line = jround(view.row_size * 1.5);
            let line_color = match &view.style.stroke {
                Some(s) if !s.is_empty() => esc(s),
                _ => String::from("var(--_b)"),
            };
            let mut stripes = String::new();
            let mut cells = String::new();
            let mut y = y0 + view.head_h;
            for (r, row) in view.rows.iter().enumerate() {
                if r % 2 == 0 {
                    let span = format!("x=\"{}\" width=\"{}\"", num(x0), num(w));
                    if r == view.rows.len() - 1 {
                        // The last row follows the box's rounded bottom corners: a rounded rect, squared off on top.
                        stripes.push_str(&format!(
                            "<rect {} y=\"{}\" height=\"{}\"{}/><rect {} y=\"{}\" height=\"{}\"/>",
                            span,
                            num(y),
                            num(row.h),
                            ROUNDED,
                            span,
                            num(y),
                            num(row.h / 2.0)
                        ));
                    } else {
                        stripes.push_str(&format!(
                            "<rect {} y=\"{}\" height=\"{}\"/>",
                            span,
                            num(y),
                            num(row.h)
                        ));
                    }
                }
                let baseline = y + ROW_PAD / 2.0 + row_line / 2.0 + view.row_size * 0.35;
                let [kind, attribute, keys, comment] = &row.cells;
                cells.push_str(&text_left(
                    kind,
                    x0 + view.cols[0],
                    baseline,
                    &format!(" class=\"pele-er-type\"{}{}", muted, text),
                    icons,
                    view.mono,
                ));
                cells.push_str(&text_left(
                    attribute,
                    x0 + view.cols[1],
                    baseline,
                    &format!(" class=\"pele-er-name\"{}{}", color, text),
                    icons,
                    view.mono,
                ));
                cells.push_str(&text_left(
                    keys,
                    x0 + view.cols[2],
                    baseline,
                    &format!(
                        " class=\"pele-er-keys\" font-size=\"{}\"{}{}",
                        js::num_str(view.key_size),
                        muted,
                        text
                    ),
                    icons,
                    view.mono,
                ));
                cells.push_str(&text_left(
                    comment,
                    x0 + view.cols[3],
                    baseline,
                    &format!(" class=\"pele-er-comment\"{}{}", muted, text),
                    icons,
                    0,
                ));
                y += row.h;
            }
            let divider_y = num(y0 + view.head_h);
            // Keep row shading light on light fills and dark on dark fills to preserve text contrast.
            let tone = ink(&view.style.shape, "");
            let shade = if tone == Some(ON_LIGHT) {
                ON_DARK
            } else if tone == Some(ON_DARK) {
                ON_LIGHT
            } else {
                "var(--_bg)"
            };
            format!(
                "<rect {}{}/><g class=\"pele-er-stripes\" fill=\"{}\"{}>{}</g><path class=\"pele-er-divider\" d=\"M{},{}H{}\" stroke=\"{}\"/><rect {} fill=\"none\" stroke=\"var(--_b)\"{}/>{}<g class=\"pele-er-attributes\" font-size=\"{}\">{}</g>",
                shape,
                rounded(&format!("{}{}", BOX_ATTRS, view.style.shape)),
                shade,
                if view.style.shape.contains("fill:") {
                    " opacity=\"0.6\""
                } else {
                    ""
                },
                stripes,
                num(x0),
                divider_y,
                num(x0 + w),
                line_color,
                shape,
                rounded(&view.style.line),
                name,
                js::num_str(view.row_size),
                cells
            )
        };
        nodes_out.push_str(&format!(
            "<g class=\"pele-node pele-entity{}\" data-id=\"{}\" transform=\"translate({},{})\">{}</g>",
            classes,
            esc_text(&node.label),
            num(c.x),
            num(c.y),
            body
        ));
    }

    // Self-relationships share the side they loop out of, squeezed together if it is too short for them.
    let keys: Vec<String> = loops.keys().map(String::from).collect();
    for key in &keys {
        let i = js::number(key) as usize;
        let view = &views[i];
        let own = loops.get_mut(key).unwrap();
        let mut total = 0.0;
        for lp in own.iter() {
            total += lp.band;
        }
        let scale = jmin(
            1.0,
            jmax((if view.below { view.w } else { view.h }) - 8.0, 0.0) / total,
        );
        let mut at = (-total * scale) / 2.0;
        for lp in own.iter_mut() {
            lp.band *= scale;
            lp.at = at + lp.band / 2.0;
            at += lp.band;
        }
    }

    let mut edges_out = String::new();
    let mut casings = String::new();
    let mut labels_out = String::new();
    for i in 0..cedges.len() {
        let edge = drawn[i];
        let label = &edge_labels[i];
        let d: String;
        let mut start: String;
        let mut end: String;
        if let Some((node, k)) = loop_of[i] {
            let key = js::num_str(node as f64);
            let lp = &loops.get(&key).unwrap()[k];
            let c = &cnodes[cedges[i].src];
            let view = &views[cedges[i].src];
            let spread = jmax(jmin(lp.band / 2.0 - 6.0, 13.0), 1.0);
            let far = MARKER_LENGTH + LOOP * 0.75 + 6.0;
            if view.below {
                let x = c.x + lp.at;
                let y = c.y + view.h / 2.0;
                let turn = num(y + MARKER_LENGTH);
                let out = num(y + MARKER_LENGTH + LOOP);
                d = format!(
                    "M{},{}V{}C{},{} {},{} {},{}V{}",
                    num(x - spread),
                    num(y),
                    turn,
                    num(x - spread),
                    out,
                    num(x + spread),
                    out,
                    num(x + spread),
                    turn,
                    num(y)
                );
                start = er_marker(&edge.arrow_type_start, x - spread, y, 0.0, -1.0);
                end = er_marker(&edge.arrow_type_end, x + spread, y, 0.0, -1.0);
                cedges[i].label_x = x;
                cedges[i].label_y = y + far + label.height / 2.0;
            } else {
                let x = c.x + view.w / 2.0;
                let y = c.y + lp.at;
                let turn = num(x + MARKER_LENGTH);
                let out = num(x + MARKER_LENGTH + LOOP);
                d = format!(
                    "M{},{}H{}C{},{} {},{} {},{}H{}",
                    num(x),
                    num(y - spread),
                    turn,
                    out,
                    num(y - spread),
                    out,
                    num(y + spread),
                    turn,
                    num(y + spread),
                    num(x)
                );
                start = er_marker(&edge.arrow_type_start, x, y - spread, -1.0, 0.0);
                end = er_marker(&edge.arrow_type_end, x, y + spread, -1.0, 0.0);
                cedges[i].label_x = x + far + label.width / 2.0;
                cedges[i].label_y = y;
            }
        } else {
            let mut route = straighten(&cedges[i].route);
            if route.len() < 6 {
                continue;
            }
            let (src, dst) = (cedges[i].src, cedges[i].dst);
            inset(&mut route, 0, &views[src], &cnodes[src]);
            let last = route.len() - 3;
            inset(&mut route, last, &views[dst], &cnodes[dst]);
            // A curve stops short of each box so that the marker sits on a straight run of line.
            let dx = (route[route.len() - 3] - route[0]).abs();
            let dy = (route[route.len() - 2] - route[1]).abs();
            let direct = route.len() == 6
                && (dx < 0.01 || dy < 0.01 || jhypot(dx, dy) <= 3.0 * MARKER_LENGTH);
            let stub = if direct { 0.0 } else { MARKER_LENGTH };
            let path = route_path(
                &route,
                None,
                stub,
                stub,
                default_reach(stub),
                default_reach(stub),
            );
            d = if stub > 0.0 {
                format!(
                    "M{},{}L{}L{},{}",
                    num(path.sx),
                    num(path.sy),
                    &path.d[1..],
                    num(path.ex),
                    num(path.ey)
                )
            } else {
                path.d.clone()
            };
            start = er_marker(&edge.arrow_type_start, path.sx, path.sy, path.sdx, path.sdy);
            end = er_marker(&edge.arrow_type_end, path.ex, path.ey, path.edx, path.edy);
        }
        let under = fill_under(&d, &[], &fills, None);
        let over: Option<String> = match &under {
            Under::On(f) => Some(edge_ink(f, "")),
            Under::Crosses => Some(String::new()),
            Under::Off => None,
        };
        if over.as_deref() == Some("") {
            casings.push_str(&casing(&d, &(start.clone() + &end), "", false));
        }
        if let (Under::On(f), Some(o)) = (&under, &over) {
            if !o.is_empty() {
                start = hollow_on(&start, f);
                end = hollow_on(&end, f);
            }
        }
        let over_s = over.as_deref().unwrap_or("");
        edges_out.push_str(&format!(
            "<g class=\"pele-edge pele-er-{}\" data-id=\"{}\"{}><path d=\"{}\"{}/>{}{}</g>",
            if edge.dashed {
                "non-identifying"
            } else {
                "identifying"
            },
            esc_text(&edge.id),
            if over_s.is_empty() {
                String::new()
            } else {
                format!(" stroke=\"{}\"", over_s)
            },
            d,
            if edge.dashed {
                " stroke-dasharray=\"4 3\""
            } else {
                ""
            },
            start,
            end
        ));
        if label.width > 0.0 {
            labels_out.push_str(&edge_label_svg(
                &edge.id,
                label,
                cedges[i].label_x,
                cedges[i].label_y,
                "",
                icons,
            ));
        }
    }

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
    if !labels_out.is_empty() {
        content.push_str(&format!(
            "<g class=\"pele-edge-labels\" font-size=\"{}\">{}</g>",
            js::num_str(small_size),
            labels_out
        ));
    }
    content.push_str(&format!("<g class=\"pele-nodes\">{}</g>", nodes_out));
    let svg = svg_document(
        "er",
        width,
        height,
        size,
        options,
        palette_style,
        &Accessible {
            acc_title: db.acc_title.clone(),
            acc_descr: db.acc_descr.clone(),
        },
        &content,
    );
    Rendered {
        svg,
        width,
        height,
        links,
    }
}

/// Ports `straighten`.
fn straighten(route: &[f64]) -> Vec<f64> {
    let mut out: Vec<f64> = route.iter().take(3).copied().collect();
    let mut k = 3;
    while k < route.len() {
        let prev = out.len() - 3;
        let flat = |axis: usize| -> bool {
            k + 3 + axis < route.len()
                && (out[prev + axis] - route[k + axis]).abs() < 0.01
                && (route[k + axis] - route[k + 3 + axis]).abs() < 0.01
        };
        if k + 3 < route.len() && (flat(0) || flat(1)) {
            k += 3;
            continue;
        }
        out.push(route[k]);
        out.push(route[k + 1]);
        out.push(route[k + 2]);
        k += 3;
    }
    out
}

/// Ports `interface End`: `edge` is the edge whose route it is.
struct End {
    edge: usize,
    at: usize,
    toward: f64,
}

/// Ports `spreadEnds`: the layout brings every edge that ends on one side
/// of a box to the middle of that side, where their markers would cover
/// each other. This spaces them along the side, in the order they fan out.
fn spread_ends(cedges: &mut [CEdge], cnodes: &[CNode], views: &[NodeView]) {
    let mut sides: OrderedMap<Vec<End>> = OrderedMap::new();
    let add = |sides: &mut OrderedMap<Vec<End>>,
               node: usize,
               route: &[f64],
               edge: usize,
               at: usize,
               next: usize| {
        let c = &cnodes[node];
        let dx = route[at] - c.x;
        let dy = route[at + 1] - c.y;
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
            return;
        };
        let key = js::num_str((node * 4 + side) as f64);
        let end = End {
            edge,
            at,
            toward: route[next + if side & 1 == 1 { 1 } else { 0 }],
        };
        match sides.get_mut(&key) {
            Some(list) => list.push(end),
            None => sides.insert(&key, alloc::vec![end]),
        }
    };
    for (k, e) in cedges.iter().enumerate() {
        if e.route.len() < 6 || e.src == e.dst {
            continue;
        }
        let last = e.route.len() - 3;
        add(&mut sides, e.src, &e.route, k, 0, 3);
        add(&mut sides, e.dst, &e.route, k, last, last - 3);
    }
    let keys: Vec<String> = sides.keys().map(String::from).collect();
    for key in keys {
        let ends = sides.get_mut(&key).unwrap();
        if ends.len() < 2 {
            continue;
        }
        let k = js::number(&key) as usize;
        let node = k >> 2;
        let vertical = (k & 1) == 1;
        let (bw, bh) = if cnodes[node].is_group {
            (cnodes[node].w, cnodes[node].h)
        } else {
            (views[node].w, views[node].h)
        };
        let length = if vertical { bh } else { bw };
        let step = jmax(
            jmin(
                PORT_GAP,
                (length - 2.0 * PORT_MARGIN) / (ends.len() as f64 - 1.0),
            ),
            0.0,
        );
        let center = if vertical {
            cnodes[node].y
        } else {
            cnodes[node].x
        };
        ends.sort_by(|a, b| {
            let d = a.toward - b.toward;
            if d < 0.0 {
                core::cmp::Ordering::Less
            } else if d > 0.0 {
                core::cmp::Ordering::Greater
            } else {
                core::cmp::Ordering::Equal
            }
        });
        let count = ends.len() as f64;
        for (i, end) in ends.iter().enumerate() {
            cedges[end.edge].route[end.at + if vertical { 1 } else { 0 }] =
                center + (i as f64 - (count - 1.0) / 2.0) * step;
        }
    }
}

/// Ports `inset`: an entity with a self-relationship is laid out larger
/// than it is drawn; this moves a route end from the side of that larger
/// box onto the entity's own side.
fn inset(route: &mut [f64], at: usize, view: &NodeView, c: &CNode) {
    if c.is_group {
        return;
    }
    let dx = route[at] - c.x;
    let dy = route[at + 1] - c.y;
    if c.w != view.w && (dx.abs() - c.w / 2.0).abs() < 0.5 {
        route[at] -= (sign(dx) * (c.w - view.w)) / 2.0;
    }
    if c.h != view.h && (dy.abs() - c.h / 2.0).abs() < 0.5 {
        route[at + 1] -= (sign(dy) * (c.h - view.h)) / 2.0;
    }
}
