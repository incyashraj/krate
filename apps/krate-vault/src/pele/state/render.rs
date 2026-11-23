//! Ports diagrams/state/render.ts: lays out and draws a parsed state
//! diagram.

#![allow(clippy::needless_range_loop)]

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use super::db::StateDb;
use super::graph::{StateEdge, StateNode};
use crate::json::Value;
use crate::pele::common::fit_width::{runs_across, turn_to_fit};
use crate::pele::core::{LinkInfo, RenderOptions, Rendered};
use crate::pele::errors::PeleError;
use crate::pele::js::{self, jhypot, jmax, jmin, jround, or, sign};
use crate::pele::layout::{cnode, compound_layout, direction, CEdge, CNode, Dir, LayeredOptions};
use crate::pele::svg::builder::{esc, esc_text, label_svg, num};
use crate::pele::svg::edges::{
    casings_svg, edge_casing, edge_label_svg, marker, marker_trim, over_fills, route_path,
    EdgePath, Fill, SideReach,
};
use crate::pele::svg::ink::{ink_fill, paints};
use crate::pele::svg::root::{svg_document, Accessible};
use crate::pele::svg::shapes::end_side;
use crate::pele::svg::theme::{class_names, resolve_style, sets_fill, ResolvedStyle};
use crate::pele::svg::title::with_title;
use crate::pele::text::entities::decode_entities;
use crate::pele::text::label::{empty_label, layout_label, Label};
use crate::pele::text::measurer::{style, Measure, METRICS};
use crate::pele::util::url::{linked, safe_url};

const NODE_SEP: f64 = 40.0;
const EDGE_SEP: f64 = 16.0;
const RANK_SEP: f64 = 44.0;
const GROUP_PAD: f64 = 16.0;
const REGION_PAD: f64 = 10.0;
const PAD_X: f64 = 16.0;
const PAD_Y: f64 = 10.0;
const LOOP: f64 = 26.0;
const FOLD: f64 = 8.0;
const NOTE_WRAP: f64 = 320.0;
const CORNER: f64 = 10.0;
const BAR: f64 = 8.0;
const SURFACE: &str = " fill=\"var(--_s)\" stroke=\"var(--_b)\"";
const RULE: &str = " fill=\"none\" stroke=\"var(--_b)\"";

/// Ports `const enum K`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum K {
    State,
    Described,
    Start,
    End,
    Choice,
    Fork,
    Join,
    Note,
    Composite,
    Region,
    Hidden,
}

/// Ports `KINDS`.
fn kind_of(shape: Option<&str>) -> K {
    match shape {
        Some("rectWithTitle") => K::Described,
        Some("stateStart") => K::Start,
        Some("stateEnd") => K::End,
        Some("choice") => K::Choice,
        Some("fork") => K::Fork,
        Some("join") => K::Join,
        Some("note") => K::Note,
        Some("divider") => K::Hidden,
        _ => K::State,
    }
}

/// Ports `CLASSES`.
const CLASSES: [&str; 9] = [
    "pele-node pele-state",
    "pele-node pele-state",
    "pele-node pele-state-start",
    "pele-node pele-state-end",
    "pele-node pele-state-choice",
    "pele-node pele-state-fork",
    "pele-node pele-state-join",
    "pele-note",
    "pele-cluster pele-state-composite",
];

/// Ports `interface View`. `node` is absent for the unseen helpers the
/// layout needs; it is the node's place in the graph.
struct View {
    node: Option<usize>,
    kind: K,
    style: Option<ResolvedStyle>,
    label: Label,
    lines: Vec<Label>,
    w: f64,
    h: f64,
    title_h: f64,
}

/// Ports `vertical`.
fn vertical(dir: Option<Dir>) -> bool {
    matches!(dir, Some(Dir::Tb) | Some(Dir::Bt))
}

/// Ports `numberOption`.
fn number_option(config: &Value, key: &str, fallback: f64) -> f64 {
    match config.get("state").and_then(|s| s.get(key)) {
        Some(Value::Num(v)) if *v > 0.0 => jmin(*v, 2000.0),
        _ => fallback,
    }
}

/// Ports `unquote`: Mermaid keeps the quotes of a click statement's strings
/// and strips them when it draws.
fn unquote(text: &str) -> &str {
    text.trim_matches('"')
}

/// Ports `clamp`.
fn clamp(value: f64, min: f64, max: f64) -> f64 {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

/// Ports `rect`.
fn rect(x: f64, y: f64, w: f64, h: f64, rest: &str) -> String {
    format!(
        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"{}/>",
        num(x),
        num(y),
        num(w),
        num(h),
        rest
    )
}

/// Ports `group`.
fn group(dir: Dir, pad_x: f64, pad_top: f64, pad_bottom: f64) -> CNode {
    let mut c = cnode(0.0, 0.0, -1);
    c.is_group = true;
    c.dir = Some(dir);
    c.pad_x = pad_x;
    c.pad_top = pad_top;
    c.pad_bottom = pad_bottom;
    c
}

/// `cssClasses.replace(MERMAID_CLASSES, '')`: Mermaid's own class names out.
fn without_mermaid_classes(classes: &str) -> String {
    classes
        .split(' ')
        .filter(|c| {
            !matches!(
                *c,
                "statediagram-state"
                    | "statediagram-cluster-alt"
                    | "statediagram-cluster"
                    | "statediagram-note"
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Ports `renderState`.
pub fn render_state(
    db: &StateDb,
    config: &Value,
    options: &RenderOptions,
    palette_style: &str,
) -> Result<Rendered, PeleError> {
    if !runs_across(Some(db.direction())) {
        return Ok(draw(db, config, options, palette_style, false));
    }
    turn_to_fit(options, |down| {
        Ok::<Rendered, PeleError>(draw(db, config, options, palette_style, down))
    })
}

/// Ports `draw`: `turned` draws a diagram that runs across as one that
/// runs down.
fn draw(
    db: &StateDb,
    config: &Value,
    options: &RenderOptions,
    palette_style: &str,
    turned: bool,
) -> Rendered {
    let graph = &db.graph;
    // State diagrams follow the flowchart curve.
    let curve: Option<&str> = config
        .get("flowchart")
        .and_then(|f| f.get("curve"))
        .and_then(|c| c.str());
    let size = options.font_size.unwrap_or(16.0);
    let small = jround(size * 0.875);
    let measurer: &dyn Measure = options.measurer.unwrap_or(&METRICS);
    let wrap_width = number_option(config, "wrappingWidth", 200.0);
    let pad = options.padding.unwrap_or(8.0);
    let icons = options.icons;
    let root_dir = if turned {
        Dir::Tb
    } else {
        direction(Some(db.direction()))
    };
    let text = |raw: Option<&str>, font_size: f64, wrap: f64, st: u32| -> Label {
        layout_label(raw, true, measurer, font_size, wrap, st, icons)
    };
    fn node_at<'g>(graph: &'g super::graph::StateGraph, views: &[View], i: usize) -> &'g StateNode {
        &graph.nodes[views[i].node.unwrap()]
    }
    let node_of = |views: &[View], i: usize| -> &StateNode { node_at(graph, views, i) };

    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    let mut views: Vec<View> = Vec::new();
    let mut cnodes: Vec<CNode> = Vec::new();
    let add = |views: &mut Vec<View>,
               cnodes: &mut Vec<CNode>,
               node: Option<usize>,
               kind: K,
               c: Option<CNode>|
     -> usize {
        views.push(View {
            node,
            kind,
            style: None,
            label: empty_label(),
            lines: Vec::new(),
            w: 0.0,
            h: 0.0,
            title_h: 0.0,
        });
        if let Some(c) = c {
            cnodes.push(c);
        }
        views.len() - 1
    };
    for (k, node) in graph.nodes.iter().enumerate() {
        if node.shape.as_deref() == Some("noteGroup") {
            continue;
        }
        let kind = if node.is_group {
            if node.shape.as_deref() == Some("divider") {
                K::Region
            } else {
                K::Composite
            }
        } else {
            kind_of(node.shape.as_deref())
        };
        let at = add(&mut views, &mut cnodes, Some(k), kind, None);
        index.insert(node.id.clone(), at);
    }
    let states = views.len();

    let mut logical: Vec<i32> = Vec::new();
    for i in 0..states {
        let parent = node_of(&views, i)
            .parent_id
            .as_ref()
            .and_then(|p| index.get(p))
            .copied();
        logical.push(match parent {
            Some(p) if p != i && node_of(&views, p).is_group => p as i32,
            _ => -1,
        });
    }
    // Composites that are redeclared inside each other form a ring with no way in from the root.
    // One member of each ring moves to the root, which leaves a tree.
    {
        let mut kids: Vec<Vec<usize>> = (0..states).map(|_| Vec::new()).collect();
        let mut queue: Vec<usize> = Vec::new();
        let mut placed = alloc::vec![false; states];
        for i in 0..states {
            if logical[i] == -1 {
                placed[i] = true;
                queue.push(i);
            } else {
                kids[logical[i] as usize].push(i);
            }
        }
        let mut done = 0;
        let settle = |queue: &mut Vec<usize>, placed: &mut Vec<bool>, done: &mut usize| {
            while *done < queue.len() {
                for &kid in &kids[queue[*done]] {
                    if placed[kid] {
                        continue;
                    }
                    placed[kid] = true;
                    queue.push(kid);
                }
                *done += 1;
            }
        };
        settle(&mut queue, &mut placed, &mut done);
        let mut seen = alloc::vec![-1i64; states];
        for i in 0..states {
            if placed[i] {
                continue;
            }
            let mut ring = i;
            while seen[ring] != i as i64 {
                seen[ring] = i as i64;
                ring = logical[ring] as usize;
            }
            logical[ring] = -1;
            placed[ring] = true;
            queue.push(ring);
            settle(&mut queue, &mut placed, &mut done);
        }
    }
    let mut owners = alloc::vec![-1i32; states];
    for i in 0..states {
        let owner = node_of(&views, i)
            .note_for
            .as_ref()
            .and_then(|n| index.get(n))
            .copied();
        let Some(owner) = owner else {
            continue;
        };
        owners[i] = owner as i32;
        logical[i] = logical[owner];
    }

    // As in Mermaid, a composite that sets no direction runs top to bottom whatever surrounds it.
    let inner_dir = |views: &[View], composite: usize| -> Dir {
        direction(node_of(views, composite).dir.as_deref())
    };
    // Whether the flow around a state runs top to bottom or bottom to top.
    let across = |views: &[View], logical: &[i32], i: usize| -> bool {
        vertical(Some(if logical[i] == -1 {
            root_dir
        } else {
            inner_dir(views, logical[i] as usize)
        }))
    };

    let mut degree = alloc::vec![0i32; states];
    let mut noted = alloc::vec![false; states];
    // When each state first takes part in a transition.
    let mut appear: Vec<f64> = alloc::vec![f64::INFINITY; states];
    let mut appeared = 0.0;
    let mut counts = alloc::vec![0i32; 2 * states];
    for edge in &graph.edges {
        let (Some(&src), Some(&dst)) = (index.get(&edge.start), index.get(&edge.end)) else {
            continue;
        };
        if edge.dashed {
            noted[if views[src].kind == K::Note { dst } else { src }] = true;
            continue;
        }
        counts[2 * src] += 1;
        degree[src] = degree[src].max(counts[2 * src]);
        counts[2 * dst + 1] += 1;
        degree[dst] = degree[dst].max(counts[2 * dst + 1]);
        if appear[src] == f64::INFINITY {
            appear[src] = appeared;
            appeared += 1.0;
        }
        if appear[dst] == f64::INFINITY {
            appear[dst] = appeared;
            appeared += 1.0;
        }
    }

    for i in 0..states {
        let node = node_of(&views, i);
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
        let font_size = st.font_size.unwrap_or(size);
        let kind = views[i].kind;
        let mut c: Option<CNode> = None;
        match kind {
            K::Composite => {
                let label = text(node.label.as_deref(), font_size, 4000.0, text_style);
                let title_h = if label.height > 0.0 {
                    label.height + 12.0
                } else {
                    0.0
                };
                let mut g = group(
                    inner_dir(&views, i),
                    GROUP_PAD,
                    title_h + GROUP_PAD,
                    GROUP_PAD,
                );
                g.min_w = label.width + 2.0 * GROUP_PAD;
                views[i].label = label;
                views[i].title_h = title_h;
                c = Some(g);
            }
            K::Region => {
                c = Some(group(
                    inner_dir(&views, i),
                    REGION_PAD,
                    REGION_PAD,
                    REGION_PAD,
                ));
            }
            K::Start => {
                views[i].w = 14.0;
                views[i].h = 14.0;
            }
            K::End => {
                views[i].w = 18.0;
                views[i].h = 18.0;
            }
            K::Choice => {
                views[i].w = 30.0;
                views[i].h = 30.0;
            }
            K::Fork | K::Join => {
                let length = clamp(40.0 * degree[i] as f64 + 16.0, 72.0, 320.0);
                let a = across(&views, &logical, i);
                views[i].w = if a { length } else { BAR };
                views[i].h = if a { BAR } else { length };
            }
            K::Hidden => {}
            K::Note => {
                let label = text(node.label.as_deref(), small, jmax(wrap_width, NOTE_WRAP), 0);
                views[i].w = label.width + 24.0;
                views[i].h = label.height + 16.0;
                views[i].label = label;
            }
            _ => {
                let label = text(node.label.as_deref(), font_size, wrap_width, text_style);
                let mut width = label.width;
                let mut height = label.height + 2.0 * PAD_Y;
                if kind == K::Described {
                    height += 14.0;
                    let mut lines = Vec::new();
                    for line in &node.description {
                        let l = text(
                            Some(if line.is_empty() { " " } else { line }),
                            small,
                            wrap_width,
                            text_style,
                        );
                        if l.width > width {
                            width = l.width;
                        }
                        height += l.height;
                        lines.push(l);
                    }
                    views[i].lines = lines;
                }
                views[i].w = jmax(width + 2.0 * PAD_X, 56.0);
                views[i].h = height;
                views[i].label = label;
            }
        }
        views[i].style = Some(st);
        let mut c = c.unwrap_or_else(|| cnode(views[i].w, views[i].h, -1));
        // The transitions on one side of a state's box spread along it, clear of its corners.
        if kind == K::State || kind == K::Described {
            c.span = (if across(&views, &logical, i) {
                views[i].w
            } else {
                views[i].h
            }) - 24.0;
        }
        cnodes.push(c);
    }

    let mut cedges: Vec<CEdge> = Vec::new();
    let mut edge_labels: Vec<Label> = Vec::new();
    let mut drawn: Vec<&StateEdge> = Vec::new();
    // The transitions each state makes to itself: how many, their widest label, and their labels' height.
    let mut loop_count = alloc::vec![0i32; states];
    let mut loop_width = alloc::vec![0.0f64; states];
    let mut loop_height = alloc::vec![0.0f64; states];
    // A transition between a composite and a state inside it runs between that state and a point
    // just inside the composite, and is then drawn on to the composite's border.
    let mut anchors: BTreeMap<usize, usize> = BTreeMap::new();
    let encloses = |cnodes: &[CNode], logical: &[i32], composite: usize, item: usize| -> bool {
        if !cnodes[composite].is_group {
            return false;
        }
        let mut p = logical[item];
        while p != -1 {
            if p as usize == composite {
                return true;
            }
            p = logical[p as usize];
        }
        false
    };
    for edge in &graph.edges {
        let (Some(&s0), Some(&d0)) = (index.get(&edge.start), index.get(&edge.end)) else {
            continue;
        };
        let (mut src, mut dst) = (s0, d0);
        let label = text(Some(&edge.label), small, wrap_width, 0);
        let label_w = if label.width > 0.0 {
            label.width + 12.0
        } else {
            0.0
        };
        if src == dst {
            loop_count[src] += 1;
            loop_width[src] = jmax(loop_width[src], label_w);
            loop_height[src] += label.height;
        } else if encloses(&cnodes, &logical, src, dst) {
            anchors.insert(cedges.len(), src);
            appear.push(-1.0);
            src = add(
                &mut views,
                &mut cnodes,
                None,
                K::Hidden,
                Some(cnode(0.0, 0.0, src as i32)),
            );
        } else if encloses(&cnodes, &logical, dst, src) {
            anchors.insert(cedges.len(), dst);
            appear.push(f64::INFINITY);
            dst = add(
                &mut views,
                &mut cnodes,
                None,
                K::Hidden,
                Some(cnode(0.0, 0.0, dst as i32)),
            );
        }
        drawn.push(edge);
        cedges.push(CEdge {
            src,
            dst,
            minlen: 1.0,
            label_w,
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

    // A state with notes, or a composite with self-transitions, sits in an unseen group of its
    // own: the notes go beside the state, across the flow, without disturbing its transitions.
    for i in 0..states {
        if owners[i] != -1 {
            continue;
        }
        let a = across(&views, &logical, i);
        // Loops hang off the right, or below when the flow is horizontal, with their labels beyond them.
        let mut room = 0.0;
        if loop_count[i] > 0 {
            let reach = LOOP + (loop_count[i] - 1) as f64 * 8.0;
            room = if a {
                reach + loop_width[i]
            } else {
                reach * 0.75 + 4.0 + loop_height[i]
            };
            let item = &mut cnodes[i];
            if !item.is_group {
                if a {
                    item.w += 2.0 * room;
                    item.h = jmax(item.h, loop_height[i]);
                } else {
                    item.h += 2.0 * room;
                    item.w = jmax(item.w, loop_width[i]);
                }
                room = 0.0;
            }
        }
        if !noted[i] && room == 0.0 {
            cnodes[i].parent = logical[i];
            continue;
        }
        let mut c = if a {
            group(Dir::Lr, room, 0.0, 0.0)
        } else {
            group(Dir::Tb, 0.0, room, room)
        };
        c.parent = logical[i];
        appear.push(appear[i]);
        let g = add(&mut views, &mut cnodes, None, K::Hidden, Some(c));
        cnodes[i].parent = g as i32;
    }
    for i in 0..states {
        if owners[i] != -1 {
            cnodes[i].parent = cnodes[owners[i] as usize].parent;
        }
    }

    {
        // Numbers the tree so that, at every level, initial states come first and the rest follow
        // in the order the transitions introduce them.
        let mut members: Vec<Vec<usize>> = (0..cnodes.len()).map(|_| Vec::new()).collect();
        let mut top: Vec<usize> = Vec::new();
        for i in 0..cnodes.len() {
            let p = cnodes[i].parent;
            if p >= 0 {
                members[p as usize].push(i);
            } else {
                top.push(i);
            }
        }
        let initial = |i: usize| -> i32 {
            if views[i].kind == K::Start {
                0
            } else {
                1
            }
        };
        let by_flow = |a: &usize, b: &usize| -> core::cmp::Ordering {
            let (a, b) = (*a, *b);
            initial(a)
                .cmp(&initial(b))
                .then_with(|| {
                    if appear[a] == appear[b] {
                        core::cmp::Ordering::Equal
                    } else if appear[a] < appear[b] {
                        core::cmp::Ordering::Less
                    } else {
                        core::cmp::Ordering::Greater
                    }
                })
                .then(a.cmp(&b))
        };
        let mut seq = 0.0;
        top.sort_by(by_flow);
        top.reverse();
        let mut pending = top;
        while let Some(i) = pending.pop() {
            cnodes[i].seq = seq;
            seq += 1.0;
            let mut inside = core::mem::take(&mut members[i]);
            inside.sort_by(by_flow);
            for k in (0..inside.len()).rev() {
                pending.push(inside[k]);
            }
        }
    }

    let layout = compound_layout(
        &mut cnodes,
        &mut cedges,
        root_dir,
        &LayeredOptions {
            node_sep: number_option(config, "nodeSpacing", NODE_SEP),
            edge_sep: EDGE_SEP,
            rank_sep: number_option(config, "rankSpacing", RANK_SEP),
            port_sep: 20.0,
            tracks: matches!(curve, Some("step") | Some("stepBefore") | Some("stepAfter")),
            straight: curve == Some("linear"),
            head_room: marker_trim("arrow_point"),
            ..LayeredOptions::default()
        },
    );

    let width = (layout.width + 2.0 * pad).ceil();
    let height = (layout.height + 2.0 * pad).ceil();
    let mut links: Vec<LinkInfo> = Vec::new();

    // Route points by rounded y, to find the edges that cross a composite's title bar.
    let mut crossings: BTreeMap<i64, Vec<f64>> = BTreeMap::new();
    for e in &cedges {
        let route = &e.route;
        let mut k = 3;
        while k + 3 < route.len() {
            crossings
                .entry(jround(route[k + 1]) as i64)
                .or_default()
                .push(route[k]);
            k += 3;
        }
    }

    let mut children: Vec<Vec<usize>> = (0..cnodes.len()).map(|_| Vec::new()).collect();
    let mut order: Vec<usize> = Vec::new();
    for i in 0..cnodes.len() {
        let p = cnodes[i].parent;
        if p >= 0 {
            children[p as usize].push(i);
        } else {
            order.push(i);
        }
    }
    let roots = order.clone();
    let mut q = 0;
    while q < order.len() {
        let kids = children[order[q]].clone();
        order.extend(kids);
        q += 1;
    }

    let mut clusters = String::new();
    let mut fills: Vec<Fill> = Vec::new();
    let mut dividers = String::new();
    let mut nodes_out = String::new();
    let mut notes_out = String::new();
    // Draws the dashed lines between the regions among a composite's members.
    let divide =
        |dividers: &mut String, items: &[usize], left: f64, top: f64, right: f64, bottom: f64| {
            let regions: Vec<usize> = items
                .iter()
                .copied()
                .filter(|&i| views[i].kind == K::Region)
                .collect();
            if regions.len() < 2 {
                return;
            }
            let lo = |i: usize, x: bool| -> f64 {
                if x {
                    cnodes[i].x - cnodes[i].w / 2.0
                } else {
                    cnodes[i].y - cnodes[i].h / 2.0
                }
            };
            let hi = |i: usize, x: bool| -> f64 {
                if x {
                    cnodes[i].x + cnodes[i].w / 2.0
                } else {
                    cnodes[i].y + cnodes[i].h / 2.0
                }
            };
            for x in [true, false] {
                let mut sorted = regions.clone();
                sorted.sort_by(|&a, &b| {
                    let d = lo(a, x) - lo(b, x);
                    if d != 0.0 && !d.is_nan() {
                        if d < 0.0 {
                            core::cmp::Ordering::Less
                        } else {
                            core::cmp::Ordering::Greater
                        }
                    } else {
                        a.cmp(&b)
                    }
                });
                let mut apart = true;
                for k in 1..sorted.len() {
                    if hi(sorted[k - 1], x) > lo(sorted[k], x) + 0.5 {
                        apart = false;
                    }
                }
                if !apart {
                    continue;
                }
                for k in 1..sorted.len() {
                    let at = num((hi(sorted[k - 1], x) + lo(sorted[k], x)) / 2.0 + pad);
                    let d = if x {
                        format!("{},{}V{}", at, num(top), num(bottom))
                    } else {
                        format!("{},{}H{}", num(left), at, num(right))
                    };
                    dividers.push_str(&format!("<path class=\"pele-divider\" d=\"M{}\"/>", d));
                }
                return;
            }
            // Regions that sit neither side by side nor in a stack each get an outline.
            for &i in &regions {
                let c = &cnodes[i];
                dividers.push_str(&rect(
                    c.x - c.w / 2.0 + pad,
                    c.y - c.h / 2.0 + pad,
                    c.w,
                    c.h,
                    &format!(" class=\"pele-divider\" rx=\"{}\"", js::num_str(CORNER)),
                ));
            }
        };
    divide(
        &mut dividers,
        &roots,
        pad / 2.0,
        pad / 2.0,
        width - pad / 2.0,
        height - pad / 2.0,
    );

    for &i in &order {
        let view = &views[i];
        let (Some(k), Some(st)) = (view.node, view.style.as_ref()) else {
            continue;
        };
        if view.kind >= K::Region {
            continue;
        }
        let node = &graph.nodes[k];
        let c = &cnodes[i];
        let x = c.x + pad;
        let y = c.y + pad;
        let w = view.w;
        let h = view.h;
        let ink = ink_fill(&st.shape, &st.text, None);
        let label_attrs = format!(" class=\"pele-label\"{}{}", ink, st.text);
        let mut body: String;

        match view.kind {
            K::Composite => {
                let left = x - c.w / 2.0;
                let top = y - c.h / 2.0;
                if paints(&st.shape) {
                    fills.push(Fill {
                        bx: [left, top, left + c.w, top + c.h],
                        shape: st.shape.clone(),
                    });
                }
                body = rect(
                    left,
                    top,
                    c.w,
                    c.h,
                    &format!(
                        " rx=\"{}\" fill=\"var(--_a)\"{} stroke=\"var(--_b)\"{}",
                        js::num_str(CORNER),
                        if sets_fill(&st.shape) {
                            ""
                        } else {
                            " fill-opacity=\"0.5\""
                        },
                        st.shape
                    ),
                );
                if view.title_h > 0.0 {
                    body.push_str(&format!(
                        "<path d=\"M{},{}H{}\"{}{}/>",
                        num(left),
                        num(top + view.title_h),
                        num(left + c.w),
                        RULE,
                        st.line
                    ));
                    body.push_str(&label_svg(
                        &view.label,
                        title_x(c, view.label.width, &crossings) + pad,
                        top + view.title_h / 2.0,
                        &format!(" class=\"pele-cluster-label\"{}{}", ink, st.text),
                        icons,
                    ));
                }
                divide(
                    &mut dividers,
                    &children[i],
                    left,
                    top + view.title_h,
                    left + c.w,
                    top + c.h,
                );
            }
            K::Start => {
                body = format!("<circle r=\"7\" fill=\"var(--_l)\"{}/>", st.shape);
            }
            K::End => {
                body = format!(
                    "<circle r=\"8.5\" fill=\"var(--_bg)\" stroke=\"var(--_l)\"{}/><circle r=\"5\" fill=\"var(--_l)\"{}/>",
                    st.line, st.shape
                );
            }
            K::Choice => {
                body = format!(
                    "<polygon points=\"0,{} {},0 0,{} {},0\"{}{}/>",
                    num(-h / 2.0),
                    num(w / 2.0),
                    num(h / 2.0),
                    num(-w / 2.0),
                    SURFACE,
                    st.shape
                );
            }
            K::Fork | K::Join => {
                body = rect(
                    -w / 2.0,
                    -h / 2.0,
                    w,
                    h,
                    &format!(" rx=\"2\" fill=\"var(--_l)\"{}", st.shape),
                );
            }
            K::Note => {
                let r = num(w / 2.0);
                let b = num(h / 2.0);
                let fold = num(w / 2.0 - FOLD);
                let corner = num(FOLD - h / 2.0);
                body = format!(
                    "<path d=\"M-{r},-{b}H{fold}L{r},{corner}V{b}H-{r}Z\" fill=\"var(--_bg)\" stroke=\"var(--_b)\"/><path d=\"M{fold},-{b}V{corner}H{r}\"{RULE}/>{}",
                    label_svg(
                        &view.label,
                        0.0,
                        0.0,
                        &format!(
                            " class=\"pele-label\" font-size=\"{}\" fill=\"var(--_m)\"",
                            js::num_str(small)
                        ),
                        icons
                    )
                );
            }
            _ => {
                body = rect(
                    -w / 2.0,
                    -h / 2.0,
                    w,
                    h,
                    &format!(
                        " rx=\"{}\"{}{}",
                        num(jmin(CORNER, h / 2.0)),
                        SURFACE,
                        st.shape
                    ),
                );
                if view.kind == K::State {
                    body.push_str(&label_svg(&view.label, 0.0, 0.0, &label_attrs, icons));
                } else {
                    let mut at = PAD_Y - h / 2.0 + view.label.height;
                    body.push_str(&format!(
                        "<path d=\"M{},{}H{}\"{}{}/>",
                        num(-w / 2.0),
                        num(at + 6.0),
                        num(w / 2.0),
                        RULE,
                        st.line
                    ));
                    body.push_str(&label_svg(
                        &view.label,
                        0.0,
                        at - view.label.height / 2.0,
                        &label_attrs,
                        icons,
                    ));
                    at += 14.0;
                    for line in &view.lines {
                        body.push_str(&label_svg(
                            line,
                            0.0,
                            at + line.height / 2.0,
                            &format!(
                                " class=\"pele-state-description\" font-size=\"{}\"{}{}",
                                js::num_str(small),
                                ink,
                                st.text
                            ),
                            icons,
                        ));
                        at += line.height;
                    }
                }
            }
        }

        if let Some(link) = db.links.get(&node.id) {
            // Entity codes are resolved first, so the URL is checked in the form a browser would follow.
            let tooltip = decode_entities(unquote(&link.tooltip));
            if !tooltip.is_empty() {
                body = format!("<title>{}</title>{}", esc(&tooltip), body);
            }
            body = linked(
                &body,
                &safe_url(unquote(&link.url)),
                &node.id,
                &mut links,
                options,
                " target=\"_blank\"",
            );
        }
        let open = format!(
            "<g class=\"{}{}\" data-id=\"{}\"",
            CLASSES[view.kind as usize],
            class_names(&without_mermaid_classes(&node.css_classes)),
            esc_text(&node.id)
        );
        if view.kind == K::Composite {
            clusters.push_str(&format!("{}>{}</g>", open, body));
        } else if view.kind == K::Note {
            notes_out.push_str(&format!(
                "{} transform=\"translate({},{})\">{}</g>",
                open,
                num(x),
                num(y),
                body
            ));
        } else {
            nodes_out.push_str(&format!(
                "{} transform=\"translate({},{})\">{}</g>",
                open,
                num(x),
                num(y),
                body
            ));
        }
    }

    // Moves a route end from the layout box onto the drawn shape, and spreads the ends along a bar.
    let attach = |route: &mut Vec<f64>, at: usize, i: usize| {
        let c = &cnodes[i];
        if c.is_group {
            return;
        }
        let view = &views[i];
        let bar = view.kind == K::Fork || view.kind == K::Join;
        let other = if at == 0 { 3 } else { at - 3 };
        // 0 is the x axis and 1 the y axis; the end sits on a side that faces along `axis`.
        for axis in [1usize, 0] {
            let extent = if axis == 1 { c.h } else { c.w };
            let drawn_extent = if axis == 1 { view.h } else { view.w };
            let breadth = if axis == 1 { view.w } else { view.h };
            let center = (if axis == 1 { c.y } else { c.x }) + pad;
            let cross = (if axis == 1 { c.x } else { c.y }) + pad;
            let delta = route[at + axis] - center;
            if (delta.abs() - extent / 2.0).abs() >= 0.5 {
                continue;
            }
            route[at + axis] -= (sign(delta) * (extent - drawn_extent)) / 2.0;
            if bar && breadth > drawn_extent {
                route[at + 1 - axis] = clamp(
                    route[other + 1 - axis],
                    cross - breadth / 2.0 + 6.0,
                    cross + breadth / 2.0 - 6.0,
                );
            }
            return;
        }
    };

    let mut reach = SideReach::new();
    for i in 0..cedges.len() {
        let ce = &cedges[i];
        if ce.src == ce.dst || ce.route.len() < 6 || drawn[i].dashed {
            continue;
        }
        reach.add(
            ce.dst,
            end_side(&ce.route, ce.route.len() - 3, &cnodes[ce.dst]),
            "arrow_point",
        );
    }

    let mut casings = String::new();
    let mut edges_out = String::new();
    let mut labels_out = String::new();
    let mut loop_seen = alloc::vec![0i32; states];
    let mut loop_text = alloc::vec![0.0f64; states];
    for i in 0..cedges.len() {
        let edge = drawn[i];
        let label = &edge_labels[i];
        let note = edge.dashed;
        let end_type = if note { "none" } else { "arrow_point" };
        let (src, dst) = (cedges[i].src, cedges[i].dst);
        let path: EdgePath = if src == dst {
            let a = &cnodes[src];
            let view = &views[src];
            let k = loop_seen[src];
            loop_seen[src] += 1;
            let k = k as f64;
            // The loop hangs off a side the flow does not use: the right, or below when the flow is horizontal.
            let side = across(&views, &logical, src);
            let w = if a.is_group { a.w } else { view.w };
            let h = if a.is_group { a.h } else { view.h };
            let out = (if side { w } else { h }) / 2.0;
            let spread = jmax(
                jmin((if side { h } else { w }) / 2.0 - 4.0, 8.0 + k * 6.0),
                2.0,
            );
            let reach = LOOP + k * 8.0;
            let trim = marker_trim(end_type) / or(jhypot(reach, spread), 1.0);
            // Offsets are given for a loop on the right and turned a quarter for one below.
            let (ax, ay) = (a.x, a.y);
            let px = |dx: f64, dy: f64| -> f64 { ax + pad + if side { out + dx } else { -dy } };
            let py = |dx: f64, dy: f64| -> f64 { ay + pad + if side { dy } else { out + dx } };
            let at =
                |dx: f64, dy: f64| -> String { format!("{},{}", num(px(dx, dy)), num(py(dx, dy))) };
            let mut path = EdgePath {
                d: format!(
                    "M{}C{} {} {}",
                    at(0.0, -spread),
                    at(reach, -2.0 * spread),
                    at(reach, 2.0 * spread),
                    at(reach * trim, spread + spread * trim)
                ),
                tokens: None,
                sx: 0.0,
                sy: 0.0,
                sdx: 0.0,
                sdy: 0.0,
                ex: px(0.0, spread),
                ey: py(0.0, spread),
                edx: px(-reach, -spread) - px(0.0, 0.0),
                edy: py(-reach, -spread) - py(0.0, 0.0),
            };
            let len = or(jhypot(path.edx, path.edy), 1.0);
            path.edx /= len;
            path.edy /= len;
            // Labels stack beyond the outermost loop, in the order of their transitions.
            let furthest = LOOP + (loop_count[src] - 1) as f64 * 8.0;
            let below = loop_text[src] + label.height / 2.0;
            loop_text[src] += label.height;
            let ce = &mut cedges[i];
            ce.label_x = if side {
                ax + out + furthest + 4.0 + ce.label_w / 2.0
            } else {
                ax
            };
            ce.label_y = if side {
                ay - loop_height[src] / 2.0 + below
            } else {
                ay + out + furthest * 0.75 + 4.0 + below
            };
            path
        } else {
            let mut route = cedges[i].route.clone();
            let leaves = reach.of(src, end_side(&route, 0, &cnodes[src]));
            let arrives = reach.of(dst, end_side(&route, route.len() - 3, &cnodes[dst]));
            let mut k = 0;
            while k < route.len() {
                route[k] += pad;
                route[k + 1] += pad;
                k += 3;
            }
            attach(&mut route, 0, src);
            let last = route.len() - 3;
            attach(&mut route, last, dst);
            if let Some(&composite) = anchors.get(&i) {
                if route.len() >= 6 {
                    let g = &cnodes[composite];
                    let inward = views[src].kind == K::Hidden;
                    let at = if inward { 0 } else { route.len() - 3 };
                    let near = matches!(g.dir, Some(Dir::Tb) | Some(Dir::Lr)) == inward;
                    let point = if vertical(g.dir) {
                        [
                            route[at],
                            g.y + pad
                                + if near {
                                    views[composite].title_h - g.h / 2.0
                                } else {
                                    g.h / 2.0
                                },
                            0.0,
                        ]
                    } else {
                        [
                            g.x + pad + if near { -g.w / 2.0 } else { g.w / 2.0 },
                            route[at + 1],
                            1.0,
                        ]
                    };
                    if inward {
                        route.splice(0..0, point);
                    } else {
                        route.extend(point);
                    }
                }
            }
            route_path(&route, curve, 0.0, marker_trim(end_type), leaves, arrives)
        };
        let over = over_fills(
            &path.d,
            &[path.ex, path.ey],
            &fills,
            "",
            path.tokens.as_deref(),
        );
        if over.as_deref() == Some("") {
            casings.push_str(&edge_casing(&path, "none", end_type, "", false));
        }
        let over_s = over.as_deref().unwrap_or("");
        let kind = if note {
            format!(
                "pele-note-edge\" stroke=\"{}\" stroke-dasharray=\"3 4",
                if over_s.is_empty() {
                    "var(--_b)"
                } else {
                    over_s
                }
            )
        } else if !over_s.is_empty() {
            format!("pele-transition\" stroke=\"{}", over_s)
        } else {
            String::from("pele-transition")
        };
        edges_out.push_str(&format!(
            "<g class=\"pele-edge {}\" data-id=\"{}\"><path d=\"{}\"/>{}</g>",
            kind,
            esc_text(&edge.id),
            path.d,
            marker(
                end_type,
                path.ex,
                path.ey,
                path.edx,
                path.edy,
                if over_s.is_empty() {
                    "var(--_l)"
                } else {
                    over_s
                },
                "none"
            )
        ));
        if label.width > 0.0 {
            let x = cedges[i].label_x + pad;
            let y = cedges[i].label_y + pad;
            labels_out.push_str(&edge_label_svg(&edge.id, label, x, y, "", icons));
        }
    }

    let mut content = String::new();
    if !clusters.is_empty() {
        content.push_str(&format!("<g class=\"pele-clusters\">{}</g>", clusters));
    }
    if !dividers.is_empty() {
        content.push_str(&format!(
            "<g class=\"pele-dividers\" stroke-dasharray=\"4 4\"{}>{}</g>",
            RULE, dividers
        ));
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
            js::num_str(small),
            labels_out
        ));
    }
    content.push_str(&format!(
        "<g class=\"pele-nodes\">{}{}</g>",
        nodes_out, notes_out
    ));
    let titled = with_title(
        db.title.as_deref(),
        measurer,
        size,
        pad,
        width,
        height,
        content,
    );
    let svg = svg_document(
        "state",
        titled.width,
        titled.height,
        size,
        options,
        palette_style,
        &Accessible {
            acc_title: db.acc_title.clone(),
            acc_descr: db.acc_descr.clone(),
        },
        &titled.content,
    );
    Rendered {
        svg,
        width: titled.width,
        height: titled.height,
        links,
    }
}

/// Ports `titleX`: picks where a composite's title sits in its title bar
/// so that edges entering from above do not cross it.
fn title_x(c: &CNode, width: f64, crossings: &BTreeMap<i64, Vec<f64>>) -> f64 {
    let Some(xs) = crossings.get(&(jround(c.y - c.h / 2.0) as i64)) else {
        return c.x;
    };
    if width == 0.0 {
        return c.x;
    }
    let left = c.x - c.w / 2.0;
    let half = width / 2.0;
    let mut best = c.x;
    let mut best_gap = -1.0;
    for x in [c.x, left + GROUP_PAD + half, left + c.w - GROUP_PAD - half] {
        let mut gap = f64::INFINITY;
        for &cx in xs {
            if cx > left && cx < left + c.w {
                gap = jmin(gap, (cx - x).abs() - half);
            }
        }
        if gap >= 6.0 {
            return x;
        }
        if gap > best_gap + 0.5 {
            best_gap = gap;
            best = x;
        }
    }
    best
}
