//! Ports diagrams/class/render.ts: lays out and draws a parsed class
//! diagram.
//!
//! Math and the `monospace` render option are not ported: without them
//! `mathText` finds no math and members are never set in monospace.

#![allow(clippy::needless_range_loop)]

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use super::db::ClassDb;
use super::graph::{build_class_graph, GraphEdge, GraphNode, NodeShape};
use super::markers::{class_marker, class_marker_trim};
use super::members::ClassMember;
use super::types::ClassNode;
use crate::json::Value;
use crate::pele::common::fit_width::{runs_across, tighten, turn_to_fit};
use crate::pele::common::generics::parse_generic_types;
use crate::pele::common::ordered::OrderedMap;
use crate::pele::core::{LinkInfo, RenderOptions, Rendered};
use crate::pele::errors::PeleError;
use crate::pele::js::{self, jmax, jmin, jround};
use crate::pele::layout::{
    cnode, compound_layout, direction, wrap_components, CEdge, CNode, Dir, LayeredOptions,
};
use crate::pele::svg::builder::{esc, esc_text, label_svg, num};
use crate::pele::svg::cluster::{
    cluster_title_x, mark_crossings, struck_title, title_crossed, Crossings,
};
use crate::pele::svg::edges::{default_reach, edge_label_svg, route_path, EdgePath};
use crate::pele::svg::ink::ink_fill;
use crate::pele::svg::root::{svg_document, Accessible};
use crate::pele::svg::shapes::end_side;
use crate::pele::svg::theme::{class_names, resolve_style, rounded, ResolvedStyle, ROUNDED};
use crate::pele::svg::title::with_title;
use crate::pele::text::entities::decode_entities;
use crate::pele::text::label::{layout_label, Label};
use crate::pele::text::measurer::{style, Measure, METRICS};
use crate::pele::util::url::linked;

const NODE_SEP: f64 = 40.0;
const EDGE_SEP: f64 = 16.0;
const RANK_SEP: f64 = 56.0;
const GROUP_PAD: f64 = 20.0;
const LOOP: f64 = 26.0;
const PAD_X: f64 = 12.0;
const TITLE_PAD: f64 = 8.0;
const SECTION_PAD: f64 = 6.0;
const EMPTY_SECTION: f64 = 10.0;
const MIN_WIDTH: f64 = 64.0;
const FOLD: f64 = 8.0;
const LINE: &str = "var(--_l)";
const TARGETS: [&str; 4] = ["_self", "_blank", "_parent", "_top"];

/// Ports `numberOption`.
fn number_option(config: &Value, key: &str, fallback: f64) -> f64 {
    match config.get(key) {
        Some(Value::Num(v)) if *v > 0.0 => jmin(*v, 2000.0),
        _ => fallback,
    }
}

/// Ports `interface Line`.
#[derive(Debug, Clone)]
struct Line {
    text: String,
    width: f64,
    classifier: String,
}

/// Ports `interface View`. Annotations are always text, as math is not
/// ported.
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct View {
    style: ResolvedStyle,
    label: Label,
    w: f64,
    h: f64,
    loops: usize,
    annotations: Vec<String>,
    attributes: Vec<Line>,
    methods: Vec<Line>,
    title_h: f64,
    attributes_h: f64,
    font_size: f64,
}

/// Ports `interface End`. `edge` is the edge whose route it is.
#[derive(Debug, Clone, Copy)]
struct End {
    edge: usize,
    at: usize,
    toward: f64,
    order: usize,
    room: f64,
    side: i32,
    /// Whether the text of this end goes on the right of, or below, its line.
    after: bool,
}

/// A cardinality's text and place.
struct Card {
    text: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    parent: i32,
}

/// Ports `memberLine`.
fn member_line(member: &ClassMember, measurer: &dyn Measure, size: f64, mono: bool) -> Line {
    let decoded = decode_entities(&member.display_details().0);
    let text: String = js::trim(&decoded).into();
    let st = (if member.classifier == "*" {
        style::ITALIC
    } else {
        0
    }) | (if mono { style::MONO } else { 0 });
    Line {
        width: measurer.width(&text, size, st),
        text,
        classifier: member.classifier.clone(),
    }
}

/// Ports `renderClass`.
pub fn render_class(
    db: &ClassDb,
    config: &Value,
    options: &RenderOptions,
    palette_style: &str,
) -> Result<Rendered, PeleError> {
    let fitted = |down: bool| {
        tighten(options, |tight| {
            Ok::<Rendered, PeleError>(draw(db, config, options, palette_style, down, tight))
        })
    };
    if runs_across(Some(&db.direction)) {
        turn_to_fit(options, fitted)
    } else {
        fitted(false)
    }
}

/// `decl.slice(0, decl.indexOf(':')).trim().toLowerCase()`.
fn property(decl: &str) -> String {
    let end = match decl.find(':') {
        Some(k) => k,
        // slice(0, -1)
        None => decl.char_indices().next_back().map_or(0, |(i, _)| i),
    };
    js::lower(js::trim(&decl[..end]))
}

/// Ports `draw`: `turned` draws a diagram that runs across as one that
/// runs down; `tight` gives up the room that keeps a namespace's name
/// clear of an edge coming in beside it.
fn draw(
    db: &ClassDb,
    config: &Value,
    options: &RenderOptions,
    palette_style: &str,
    turned: bool,
    tight: bool,
) -> Rendered {
    let empty = Value::obj();
    let conf: &Value = match config.get("class") {
        Some(c @ Value::Obj(_)) => c,
        _ => &empty,
    };
    let graph = build_class_graph(
        db,
        conf.get("hierarchicalNamespaces") != Some(&Value::Bool(false)),
    );
    let hide_empty = conf.get("hideEmptyMembersBox") == Some(&Value::Bool(true));
    // The `monospace` option is not ported, so members are never monospaced.
    let mono = |_style: &ResolvedStyle| false;
    let size = options.font_size.unwrap_or(16.0);
    let small = jround(size * 0.875);
    let tiny = jround(size * 0.8);
    let tiny_height = jround(tiny * 1.4);
    let measurer: &dyn Measure = options.measurer.unwrap_or(&METRICS);
    let pad = options.padding.unwrap_or(8.0);
    let icons = options.icons;
    let card_width = |raw: &str| -> f64 {
        if raw.is_empty() {
            0.0
        } else {
            measurer.width(&decode_entities(raw), tiny, 0)
        }
    };
    let root_dir = if turned {
        Dir::Tb
    } else {
        direction(Some(&db.direction))
    };

    // A class may carry the declarations of many classDefs many times over. Each distinct declaration
    // is checked once, and only the last value of each property that has an effect is resolved.
    let mut effect: BTreeMap<String, String> = BTreeMap::new();
    let mut from_defs: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut class_style = |cls: &ClassNode| -> ResolvedStyle {
        let mut kept: OrderedMap<String> = OrderedMap::new();
        let mut keep = |decl: &str, kept: &mut OrderedMap<String>| {
            let prop = match effect.get(decl) {
                Some(p) => p.clone(),
                None => {
                    let one = resolve_style(&[decl]);
                    let p = if one.shape.is_empty() && one.text.is_empty() {
                        String::new()
                    } else {
                        property(decl)
                    };
                    effect.insert(decl.into(), p.clone());
                    p
                }
            };
            if !prop.is_empty() {
                kept.insert(&prop, decl.into());
            }
        };
        match from_defs.get(&cls.css_classes) {
            None => {
                for name in cls.css_classes.split(' ') {
                    if let Some(def) = db.style_classes.get(name) {
                        for s in &def.styles {
                            for part in s.split(',') {
                                keep(part, &mut kept);
                            }
                        }
                    }
                }
                let base: Vec<(String, String)> = kept
                    .iter()
                    .map(|(k, v)| (String::from(k), v.clone()))
                    .collect();
                from_defs.insert(cls.css_classes.clone(), base);
            }
            Some(base) => {
                for (prop, decl) in base {
                    kept.insert(prop, decl.clone());
                }
            }
        }
        for s in &cls.styles {
            keep(s, &mut kept);
        }
        let values: Vec<String> = kept.values().cloned().collect();
        resolve_style(&values)
    };

    let n = graph.nodes.len();
    let mut children = alloc::vec![0i32; n];
    for node in &graph.nodes {
        if node.parent >= 0 {
            children[node.parent as usize] += 1;
        }
    }

    // Namespaces that an edge may cross into are made wide enough for the title to sit beside it.
    let mut crossed = alloc::vec![false; n];
    for edge in &graph.edges {
        if edge.source < 0 || edge.target < 0 {
            continue;
        }
        let a = graph.nodes[edge.source as usize].parent;
        let b = graph.nodes[edge.target as usize].parent;
        if a == b {
            continue;
        }
        let mut g = a;
        while g >= 0 && !crossed[g as usize] {
            crossed[g as usize] = true;
            g = graph.nodes[g as usize].parent;
        }
        let mut g = b;
        while g >= 0 && !crossed[g as usize] {
            crossed[g as usize] = true;
            g = graph.nodes[g as usize].parent;
        }
    }

    let base = |label: Label, w: f64, h: f64, font_size: f64| -> View {
        View {
            style: ResolvedStyle::default(),
            label,
            w,
            h,
            loops: 0,
            annotations: Vec::new(),
            attributes: Vec::new(),
            methods: Vec::new(),
            title_h: 0.0,
            attributes_h: 0.0,
            font_size,
        }
    };

    let mut views: Vec<View> = Vec::new();
    let mut cnodes: Vec<CNode> = Vec::new();
    for (i, node) in graph.nodes.iter().enumerate() {
        let mut view: View;
        let mut c: CNode;
        if node.is_group {
            let label = layout_label(Some(&node.label), false, measurer, size, 4000.0, 0, icons);
            view = base(label.clone(), 0.0, 0.0, size);
            if children[i] > 0 {
                c = cnode(0.0, 0.0, -1);
                c.is_group = true;
                c.pad_x = GROUP_PAD;
                c.pad_top = if label.height > 0.0 {
                    label.height + 16.0
                } else {
                    GROUP_PAD
                };
                c.pad_bottom = GROUP_PAD;
                c.min_w = label.width + 2.0 * GROUP_PAD - 12.0
                    + if crossed[i] && !tight {
                        jmin(label.width + 24.0, 240.0)
                    } else {
                        0.0
                    };
            } else {
                view.w = jmax(label.width + 2.0 * GROUP_PAD, 80.0);
                view.h = label.height + 2.0 * GROUP_PAD;
                c = cnode(view.w, view.h, -1);
            }
        } else if let Some(k) = node.class_node {
            let cls = db.classes.at(k).1;
            view = class_view(
                cls,
                class_style(cls),
                measurer,
                size,
                tiny,
                tiny_height,
                hide_empty,
                icons,
                &mono,
            );
            c = cnode(view.w, view.h, -1);
        } else if node.shape == NodeShape::Note {
            let label = layout_label(Some(&node.label), true, measurer, small, 260.0, 0, icons);
            let (w, h) = (
                jmax(label.width + 2.0 * PAD_X, 40.0),
                label.height + 2.0 * TITLE_PAD,
            );
            view = base(label, w, h, small);
            c = cnode(view.w, view.h, -1);
        } else {
            let label = layout_label(Some(&node.label), false, measurer, small, 200.0, 0, icons);
            let (w, h) = (jmax(label.width, 12.0) + 4.0, jmax(label.height, 12.0));
            view = base(label, w, h, small);
            c = cnode(view.w, view.h, -1);
        }
        c.parent = node.parent;
        c.seq = i as f64;
        views.push(view);
        cnodes.push(c);
    }

    let mut cedges: Vec<CEdge> = Vec::new();
    let mut drawn: Vec<&GraphEdge> = Vec::new();
    let mut edge_labels: Vec<Label> = Vec::new();
    let mut start_room: Vec<f64> = Vec::new();
    let mut end_room: Vec<f64> = Vec::new();
    // Edges leave one side of a class and arrive at the other, so a class with many of them
    // is widened until they, and the text at their ends, fit side by side.
    let mut crowd = alloc::vec![0.0f64; views.len() * 4];
    let meet = |crowd: &mut Vec<f64>, item: usize, slot: usize, room: f64| {
        crowd[item * 4 + slot] += 1.0;
        if room > crowd[item * 4 + slot + 1] {
            crowd[item * 4 + slot + 1] = room;
        }
    };
    for edge in &graph.edges {
        if edge.source < 0 || edge.target < 0 {
            continue;
        }
        let (src, dst) = (edge.source as usize, edge.target as usize);
        let label = layout_label(
            edge.label.as_deref(),
            true,
            measurer,
            small,
            200.0,
            0,
            icons,
        );
        let start_w = card_width(&edge.start_label_right);
        let end_w = card_width(&edge.end_label_left);
        if src == dst {
            views[src].loops += 1;
            cnodes[src].w += 2.0
                * (LOOP
                    + if label.width > 0.0 {
                        label.width + 12.0
                    } else {
                        0.0
                    });
        } else {
            meet(&mut crowd, src, 0, start_w);
            meet(&mut crowd, dst, 2, end_w);
        }
        start_room.push(start_w);
        end_room.push(end_w);
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

    let vertical = matches!(root_dir, Dir::Tb | Dir::Bt);
    if vertical {
        for i in 0..views.len() {
            if graph.nodes[i].class_node.is_none() {
                continue;
            }
            let mut need = 0.0;
            for slot in [0usize, 2] {
                let count = crowd[i * 4 + slot];
                let room = crowd[i * 4 + slot + 1];
                if count > 1.0 {
                    need = jmax(
                        need,
                        (count - 1.0) * (if room > 0.0 { room + 24.0 } else { 14.0 }) + 24.0,
                    );
                }
            }
            need = jmin(need, 480.0);
            if need > views[i].w {
                cnodes[i].w += need - views[i].w;
                views[i].w = need;
            }
        }
    }

    let lettered = start_room
        .iter()
        .zip(end_room.iter())
        .any(|(s, e)| *s > 0.0 || *e > 0.0);
    // Cardinalities at both ends of a vertical edge need room for their height between the ranks.
    let mut tall_cards = 0.0;
    if vertical {
        let card_height = |raw: &str| -> f64 {
            if raw.is_empty() {
                0.0
            } else {
                jmax(tiny_height, 0.0)
            }
        };
        for edge in &drawn {
            tall_cards = jmax(
                tall_cards,
                card_height(&edge.start_label_right) + card_height(&edge.end_label_left)
                    - 2.0 * tiny_height,
            );
        }
    }
    let layout = compound_layout(
        &mut cnodes,
        &mut cedges,
        root_dir,
        &LayeredOptions {
            node_sep: number_option(conf, "nodeSpacing", NODE_SEP),
            edge_sep: EDGE_SEP,
            rank_sep: number_option(
                conf,
                "rankSpacing",
                if lettered { RANK_SEP + 16.0 } else { RANK_SEP },
            ) + tall_cards,
            port_sep: 20.0,
            apart: true,
            ..LayeredOptions::default()
        },
    );

    // Every edge reaches the middle of a side. Where several share a side they are spread along it,
    // in the order of where they come from. An end with text runs straight for the length of the text,
    // which sits beside it on the side the line then bends toward, away from the neighbors that bend
    // the other way. Neighbors are set apart far enough for the text between them.
    let mut sides: Vec<(usize, Vec<End>)> = Vec::new();
    let mut side_index: BTreeMap<usize, usize> = BTreeMap::new();
    let mut cards: Vec<Card> = Vec::new();
    let mut attach = |cedges: &mut Vec<CEdge>,
                      e: usize,
                      at: usize,
                      item: usize,
                      room: f64,
                      order: usize|
     -> Option<End> {
        let c = &cnodes[item];
        let side = end_side(&cedges[e].route, at, c);
        if side < 0 {
            return None;
        }
        let slack = (c.w - views[item].w) / 2.0;
        let route = &mut cedges[e].route;
        if side == 1 {
            route[at] -= slack;
        } else if side == 3 {
            route[at] += slack;
        }
        let across = (side & 1) as usize;
        let toward = route[(if at == 0 { 3 } else { at - 3 }) + across];
        let bend = toward - route[at + across];
        let end = End {
            edge: e,
            at,
            toward,
            order,
            room,
            side,
            after: if across == 1 { bend > 0.0 } else { bend >= 0.0 },
        };
        let key = item * 4 + side as usize;
        match side_index.get(&key) {
            Some(&k) => sides[k].1.push(end),
            None => {
                side_index.insert(key, sides.len());
                sides.push((key, alloc::vec![end]));
            }
        }
        Some(end)
    };
    let mut starts: Vec<Option<End>> = Vec::new();
    let mut ends: Vec<Option<End>> = Vec::new();
    for i in 0..cedges.len() {
        let routed = cedges[i].src != cedges[i].dst && cedges[i].route.len() >= 6;
        let (src, dst) = (cedges[i].src, cedges[i].dst);
        starts.push(if routed {
            attach(&mut cedges, i, 0, src, start_room[i], i)
        } else {
            None
        });
        let last = cedges[i].route.len().wrapping_sub(3);
        ends.push(if routed {
            attach(&mut cedges, i, last, dst, end_room[i], i)
        } else {
            None
        });
    }
    // Whether a point lies on the border of one of the namespaces around a class.
    let on_border = |cnodes: &[CNode], item: usize, x: f64, y: f64| -> bool {
        let mut g = cnodes[item].parent;
        let mut hops = 0;
        while g >= 0 && hops < 10000 {
            let c = &cnodes[g as usize];
            let dx = (x - c.x).abs();
            let dy = (y - c.y).abs();
            if ((dy - c.h / 2.0).abs() < 0.5 && dx <= c.w / 2.0 + 0.5)
                || ((dx - c.w / 2.0).abs() < 0.5 && dy <= c.h / 2.0 + 0.5)
            {
                return true;
            }
            g = c.parent;
            hops += 1;
        }
        false
    };
    let mut gaps: Vec<f64> = Vec::new();
    for (key, list) in sides.iter_mut() {
        if list.len() < 2 {
            continue;
        }
        let item = *key >> 2;
        let across = *key & 1;
        let length = if across == 1 {
            views[item].h
        } else {
            views[item].w
        };
        list.sort_by(|a, b| {
            let d = a.toward - b.toward;
            if d != 0.0 && !d.is_nan() {
                if d < 0.0 {
                    core::cmp::Ordering::Less
                } else {
                    core::cmp::Ordering::Greater
                }
            } else {
                a.order.cmp(&b.order)
            }
        });
        let mut total = 0.0;
        for j in 0..list.len() - 1 {
            let a = &list[j];
            let b = &list[j + 1];
            let mut gap = if across == 1 { 20.0 } else { 24.0 };
            if a.room > 0.0 && a.after {
                gap += if across == 1 {
                    tiny_height + 6.0
                } else {
                    a.room
                };
            }
            if b.room > 0.0 && !b.after {
                gap += if across == 1 {
                    tiny_height + 6.0
                } else {
                    b.room
                };
            }
            if gaps.len() <= j {
                gaps.resize(j + 1, f64::NAN);
            }
            gaps[j] = gap;
            total += gap;
        }
        let scale = jmin(1.0, (length - 24.0) / total);
        if scale <= 0.0 {
            continue;
        }
        let mut at = (if across == 1 {
            cnodes[item].y
        } else {
            cnodes[item].x
        }) - (total * scale) / 2.0;
        for j in 0..list.len() {
            let End { edge, at: end, .. } = list[j];
            let route_len = cedges[edge].route.len() as isize;
            // An edge that crosses the border of the class's namespace close by runs straight to it:
            // the point where it crosses moves into line with its end.
            let step: isize = if end == 0 { 3 } else { -3 };
            let first = end as isize + step;
            let mut border: isize = -1;
            {
                let route = &cedges[edge].route;
                let fa = route[first as usize + across];
                let mut k = first;
                while k >= 0 && k < route_len && (route[k as usize + across] - fa).abs() < 0.5 {
                    if on_border(&cnodes, item, route[k as usize], route[k as usize + 1]) {
                        border = k;
                        break;
                    }
                    k += step;
                }
            }
            let route = &mut cedges[edge].route;
            if border >= 0 && (route[first as usize + across] - at).abs() <= 24.0 {
                let mut k = first;
                while k != border + step {
                    route[k as usize + across] = at;
                    k += step;
                }
            }
            route[end + across] = at;
            at += gaps.get(j).copied().unwrap_or(f64::NAN) * scale;
        }
    }

    let straighten = |route: &mut Vec<f64>, end: Option<End>, room: f64, first: bool| {
        let Some(end) = end else {
            return;
        };
        if room <= 0.0 {
            return;
        }
        let across = (end.side & 1) as usize;
        let at = if first { 0 } else { route.len() - 3 };
        let along = at + 1 - across;
        let reach = (route[(if first { 3 } else { at - 3 }) + 1 - across] - route[along]).abs();
        let length = jmin(
            if across == 1 {
                room + 10.0
            } else {
                tiny_height + 6.0
            },
            reach * 0.35,
        );
        if length < 4.0 {
            return;
        }
        let mut point = [route[at], route[at + 1], route[at + 2]];
        point[1 - across] += (if end.side == 0 || end.side == 3 {
            -1.0
        } else {
            1.0
        }) * length;
        let place = if first { 3 } else { at };
        route.splice(place..place, point);
    };
    for i in 0..cedges.len() {
        straighten(&mut cedges[i].route, starts[i], start_room[i], true);
        straighten(&mut cedges[i].route, ends[i], end_room[i], false);
    }

    let mut min_x = 0.0;
    let mut min_y = 0.0;
    let mut max_x = layout.width;
    let mut max_y = layout.height;
    let mut card = |cards: &mut Vec<Card>,
                    parent: usize,
                    raw: &str,
                    width: f64,
                    x: f64,
                    y: f64,
                    side: i32,
                    after: bool| {
        if raw.is_empty() {
            return;
        }
        let h = tiny_height;
        let mut cx = x;
        let mut cy = y;
        if side & 1 == 1 {
            cx += (if side == 3 { -1.0 } else { 1.0 }) * (5.0 + width / 2.0);
            cy += (if after { 1.0 } else { -1.0 }) * (7.0 + h / 2.0);
        } else {
            cx += (if after { 1.0 } else { -1.0 }) * (9.0 + width / 2.0);
            cy += (if side == 0 { -1.0 } else { 1.0 }) * (3.0 + h / 2.0);
        }
        cards.push(Card {
            text: decode_entities(raw),
            x: cx,
            y: cy,
            w: width,
            h,
            parent: parent as i32,
        });
        min_x = jmin(min_x, cx - width / 2.0);
        max_x = jmax(max_x, cx + width / 2.0);
        min_y = jmin(min_y, cy - h / 2.0);
        max_y = jmax(max_y, cy + h / 2.0);
    };
    let mut loop_seen: BTreeMap<usize, usize> = BTreeMap::new();
    let mut loop_at = alloc::vec![0usize; cedges.len()];
    for i in 0..cedges.len() {
        let edge = drawn[i];
        let (src, dst) = (cedges[i].src, cedges[i].dst);
        if src == dst {
            let k = loop_seen.get(&src).copied().unwrap_or(0);
            loop_seen.insert(src, k + 1);
            loop_at[i] = k;
            let c = &cnodes[src];
            let view = &views[src];
            let x = c.x + view.w / 2.0;
            let spread = jmin(view.h / 2.0 - 4.0, 8.0 + k as f64 * 6.0);
            let (cx, cy) = (c.x, c.y);
            card(
                &mut cards,
                src,
                &edge.start_label_right,
                start_room[i],
                x,
                cy - spread,
                1,
                false,
            );
            card(
                &mut cards,
                dst,
                &edge.end_label_left,
                end_room[i],
                x,
                cy + spread,
                1,
                true,
            );
            let ce = &mut cedges[i];
            ce.label_x = cx + view.w / 2.0 + LOOP + k as f64 * 8.0 + 4.0 + ce.label_w / 2.0;
            ce.label_y = cy;
        } else if cedges[i].route.len() >= 6 {
            let route = &cedges[i].route;
            let last = route.len() - 3;
            let (r0, r1, rl, rl1) = (route[0], route[1], route[last], route[last + 1]);
            let from = starts[i];
            let to = ends[i];
            card(
                &mut cards,
                src,
                &edge.start_label_right,
                start_room[i],
                r0,
                r1,
                from.map_or(2, |f| f.side),
                from.is_none_or(|f| f.after),
            );
            card(
                &mut cards,
                dst,
                &edge.end_label_left,
                end_room[i],
                rl,
                rl1,
                to.map_or(0, |t| t.side),
                to.is_none_or(|t| t.after),
            );
        }
    }

    if let Some(available) = options.max_width.filter(|a| *a > 0.0) {
        if max_x - min_x + 2.0 * pad > available {
            let mut all: Vec<CNode> = cnodes.clone();
            for item in &cards {
                let mut c = cnode(item.w, item.h, item.parent);
                c.x = item.x;
                c.y = item.y;
                all.push(c);
            }
            let wrapped = wrap_components(
                &mut all,
                &mut cedges,
                jmax(1.0, available - 2.0 * pad),
                number_option(conf, "nodeSpacing", NODE_SEP),
            );
            let count = cnodes.len();
            cnodes.clone_from_slice(&all[..count]);
            if let Some(size) = wrapped {
                for (i, item) in cards.iter_mut().enumerate() {
                    item.x = all[count + i].x;
                    item.y = all[count + i].y;
                }
                min_x = 0.0;
                min_y = 0.0;
                max_x = size.width;
                max_y = size.height;
            }
        }
    }

    let ox = pad - min_x;
    let oy = pad - min_y;
    let width = (max_x - min_x + 2.0 * pad).ceil();
    let height = (max_y - min_y + 2.0 * pad).ceil();
    let mut links: Vec<LinkInfo> = Vec::new();

    let mut crossings: Crossings = Crossings::new();
    if graph.nodes.first().is_some_and(|n| n.is_group) {
        for e in &cedges {
            mark_crossings(&mut crossings, &e.route);
        }
    }
    let parents: Vec<i32> = graph.nodes.iter().map(|n| n.parent).collect();

    let mut clusters = String::new();
    let mut struck_titles = String::new();
    let mut nodes_out = String::new();
    for i in 0..views.len() {
        let view = &views[i];
        let c = &cnodes[i];
        let node: &GraphNode = &graph.nodes[i];
        let x = c.x + ox;
        let y = c.y + oy;
        let id = esc_text(&node.id);
        let w = if c.is_group { c.w } else { view.w };
        let h = if c.is_group { c.h } else { view.h };
        if node.is_group {
            let title_x = if c.is_group {
                cluster_title_x(c, view.label.width, &crossings, GROUP_PAD)
            } else {
                c.x
            };
            let title_y = y - h / 2.0 + 8.0 + view.label.height / 2.0;
            let title_attrs = " class=\"pele-cluster-label\" fill=\"var(--_m)\"";
            let struck = c.is_group && title_crossed(c, view.label.width, title_x, &crossings);
            if struck {
                struck_titles.push_str(&struck_title(
                    &view.label,
                    title_x + ox,
                    title_y,
                    title_attrs,
                    &id,
                    &parents,
                    i,
                    None,
                ));
            }
            clusters.push_str(&format!(
                "<g class=\"pele-cluster pele-namespace\" data-id=\"{}\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"var(--_a)\" fill-opacity=\"0.5\" stroke=\"var(--_b)\"{}/>{}</g>",
                id,
                num(x - w / 2.0),
                num(y - h / 2.0),
                num(w),
                num(h),
                ROUNDED,
                if struck {
                    String::new()
                } else {
                    label_svg(&view.label, title_x + ox, title_y, title_attrs, None)
                }
            ));
            continue;
        }
        let at = format!(" transform=\"translate({},{})\"", num(x), num(y));
        if let Some(k) = node.class_node {
            let cls = db.classes.at(k).1;
            nodes_out.push_str(&class_svg(
                view,
                cls,
                &id,
                &at,
                size,
                tiny,
                tiny_height,
                options,
                &mut links,
                &mono,
            ));
        } else if node.shape == NodeShape::Note {
            let r = w / 2.0;
            let t = -h / 2.0;
            nodes_out.push_str(&format!(
                "<g class=\"pele-node pele-note\" data-id=\"{}\"{}><path d=\"M{},{}H{}L{},{}V{}H{}Z\" fill=\"var(--_a)\" stroke=\"var(--_b)\" stroke-linejoin=\"round\"/><path d=\"M{},{}V{}H{}\" fill=\"none\" stroke=\"var(--_b)\" stroke-linejoin=\"round\"/>{}</g>",
                id,
                at,
                num(-r),
                num(t),
                num(r - FOLD),
                num(r),
                num(t + FOLD),
                num(-t),
                num(-r),
                num(r - FOLD),
                num(t),
                num(t + FOLD),
                num(r),
                label_svg(
                    &view.label,
                    0.0,
                    0.0,
                    &format!(" class=\"pele-label\" font-size=\"{}\"", js::num_str(small)),
                    icons
                )
            ));
        } else {
            nodes_out.push_str(&format!(
                "<g class=\"pele-node pele-interface\" data-id=\"{}\"{}>{}</g>",
                id,
                at,
                label_svg(
                    &view.label,
                    0.0,
                    0.0,
                    &format!(" class=\"pele-label\" font-size=\"{}\"", js::num_str(small)),
                    icons
                )
            ));
        }
    }

    let mut edges_out = String::new();
    let mut labels_out = String::new();
    for i in 0..cedges.len() {
        let edge = drawn[i];
        let label = &edge_labels[i];
        let start_type = edge.arrow_type_start;
        let end_type = edge.arrow_type_end;
        let start_trim = class_marker_trim(start_type);
        let end_trim = class_marker_trim(end_type);
        let (src, dst) = (cedges[i].src, cedges[i].dst);
        let path: EdgePath = if src == dst {
            let c = &cnodes[src];
            let view = &views[src];
            let k = loop_at[i] as f64;
            let x = c.x + ox + view.w / 2.0;
            let y = c.y + oy;
            let spread = jmin(view.h / 2.0 - 4.0, 8.0 + k * 6.0);
            let reach = LOOP + k * 8.0;
            EdgePath {
                d: format!(
                    "M{},{}C{},{} {},{} {},{}",
                    num(x + start_trim),
                    num(y - spread),
                    num(x + reach + start_trim),
                    num(y - spread),
                    num(x + reach + end_trim),
                    num(y + spread),
                    num(x + end_trim),
                    num(y + spread)
                ),
                tokens: None,
                sx: x,
                sy: y - spread,
                sdx: -1.0,
                sdy: 0.0,
                ex: x,
                ey: y + spread,
                edx: -1.0,
                edy: 0.0,
            }
        } else {
            let route = &mut cedges[i].route;
            let mut k = 0;
            while k < route.len() {
                route[k] += ox;
                route[k + 1] += oy;
                k += 3;
            }
            route_path(
                route,
                None,
                start_trim,
                end_trim,
                default_reach(start_trim),
                default_reach(end_trim),
            )
        };
        let attrs = match edge.pattern {
            "dashed" => " stroke-dasharray=\"6 4\"",
            "dotted" => " stroke-dasharray=\"2 4\"",
            _ => "",
        };
        edges_out.push_str(&format!(
            "<g class=\"pele-edge{}\" data-id=\"{}\"><path d=\"{}\"{}/>{}{}</g>",
            if edge.pattern == "dotted" {
                " pele-note-edge"
            } else {
                " pele-relation"
            },
            esc_text(&edge.id),
            path.d,
            attrs,
            class_marker(start_type, path.sx, path.sy, path.sdx, path.sdy, LINE),
            class_marker(end_type, path.ex, path.ey, path.edx, path.edy, LINE)
        ));
        if label.width > 0.0 {
            let x = cedges[i].label_x + ox;
            let y = cedges[i].label_y + oy;
            labels_out.push_str(&edge_label_svg(&edge.id, label, x, y, "", icons));
        }
    }

    let mut cards_out = String::new();
    for item in &cards {
        cards_out.push_str(&format!(
            "<text class=\"pele-cardinality\" x=\"{}\" y=\"{}\" text-anchor=\"middle\">{}</text>",
            num(item.x + ox),
            num(item.y + oy + tiny * 0.35),
            esc(&item.text)
        ));
    }

    let mut content = String::new();
    if !clusters.is_empty() {
        content.push_str(&format!("<g class=\"pele-clusters\">{}</g>", clusters));
    }
    if !edges_out.is_empty() {
        content.push_str(&format!(
            "<g class=\"pele-edges\" fill=\"none\" stroke=\"{}\" stroke-linecap=\"round\">{}</g>",
            LINE, edges_out
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
            js::num_str(small),
            labels_out
        ));
    }
    if !cards_out.is_empty() {
        content.push_str(&format!(
            "<g class=\"pele-cardinalities\" font-size=\"{}\" fill=\"var(--_m)\">{}</g>",
            js::num_str(tiny),
            cards_out
        ));
    }
    content.push_str(&format!("<g class=\"pele-nodes\">{}</g>", nodes_out));
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
        "class",
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

/// Ports `classView`.
#[allow(clippy::too_many_arguments)]
fn class_view(
    cls: &ClassNode,
    st: ResolvedStyle,
    measurer: &dyn Measure,
    size: f64,
    tiny: f64,
    tiny_height: f64,
    hide_empty: bool,
    icons: crate::pele::text::label::Icons,
    mono: &dyn Fn(&ResolvedStyle) -> bool,
) -> View {
    let font_size = st.font_size.unwrap_or(size);
    let member_size = jround(font_size * 0.875);
    let line_height = jround(member_size * 1.5);
    // The generic type is escaped so that `<T>` is not read as a tag.
    let generic = if cls.kind.is_empty() {
        String::new()
    } else {
        format!(
            "&lt;{}&gt;",
            parse_generic_types(&cls.kind)
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        )
    };
    let label = layout_label(
        Some(&format!("{}{}", cls.label, generic)),
        false,
        measurer,
        font_size,
        4000.0,
        style::BOLD | if st.italic { style::ITALIC } else { 0 },
        icons,
    );
    let annotations: Vec<String> = cls
        .annotations
        .iter()
        .map(|a| format!("\u{ab}{}\u{bb}", js::trim(&decode_entities(a))))
        .collect();
    let attributes: Vec<Line> = cls
        .members
        .iter()
        .map(|m| member_line(m, measurer, member_size, mono(&st)))
        .collect();
    let methods: Vec<Line> = cls
        .methods
        .iter()
        .map(|m| member_line(m, measurer, member_size, mono(&st)))
        .collect();
    let mut inner = label.width;
    let mut annotations_h = 0.0;
    for a in &annotations {
        inner = jmax(inner, measurer.width(a, tiny, 0));
        annotations_h += tiny_height;
    }
    for line in &attributes {
        inner = jmax(inner, line.width);
    }
    for line in &methods {
        inner = jmax(inner, line.width);
    }
    let sections = !(hide_empty && attributes.is_empty() && methods.is_empty());
    let section = |count: usize| -> f64 {
        if !sections {
            0.0
        } else if count > 0 {
            2.0 * SECTION_PAD + count as f64 * line_height
        } else {
            EMPTY_SECTION
        }
    };
    let title_h = 2.0 * TITLE_PAD + annotations_h + jmax(label.height, jround(font_size * 1.5));
    let attributes_h = section(attributes.len());
    let h = title_h + attributes_h + section(methods.len());
    View {
        style: st,
        w: jmax(inner + 2.0 * PAD_X, MIN_WIDTH),
        h,
        label,
        loops: 0,
        annotations,
        attributes,
        methods,
        title_h,
        attributes_h,
        font_size,
    }
}

/// Ports `classSvg`.
#[allow(clippy::too_many_arguments)]
fn class_svg(
    view: &View,
    cls: &ClassNode,
    id: &str,
    at: &str,
    size: f64,
    tiny: f64,
    tiny_height: f64,
    options: &RenderOptions,
    links: &mut Vec<LinkInfo>,
    mono: &dyn Fn(&ResolvedStyle) -> bool,
) -> String {
    let (w, h, st) = (view.w, view.h, &view.style);
    let left = -w / 2.0;
    let top = -h / 2.0;
    let member_size = jround(view.font_size * 0.875);
    let line_height = jround(member_size * 1.5);
    let mut body = format!(
        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"var(--_s)\" stroke=\"var(--_b)\"{}/>",
        num(left),
        num(top),
        num(w),
        num(h),
        rounded(&st.shape)
    );
    let ink = ink_fill(&st.shape, &st.text, None);
    let first = top + view.title_h;
    let second = first + view.attributes_h;
    if h > view.title_h {
        let stroke = match &st.stroke {
            Some(s) if !s.is_empty() => format!(" style=\"stroke:{}\"", esc(s)),
            _ => String::new(),
        };
        body.push_str(&format!(
            "<path class=\"pele-divider\" d=\"M{},{}H{}M{},{}H{}\" fill=\"none\" stroke=\"var(--_b)\"{}/>",
            num(left),
            num(first),
            num(-left),
            num(left),
            num(second),
            num(-left),
            stroke
        ));
    }
    let mut y = top + TITLE_PAD;
    let ink_or_muted = if ink.is_empty() {
        String::from(" fill=\"var(--_m)\"")
    } else {
        ink.clone()
    };
    for annotation in &view.annotations {
        body.push_str(&format!(
            "<text class=\"pele-annotation\" x=\"0\" y=\"{}\" text-anchor=\"middle\" font-size=\"{}\"{}>{}</text>",
            num(y + tiny_height / 2.0 + tiny * 0.35),
            js::num_str(tiny),
            ink_or_muted,
            esc(annotation)
        ));
        y += tiny_height;
    }
    let title_size = if view.font_size == size {
        String::new()
    } else {
        format!(" font-size=\"{}\"", num(view.font_size))
    };
    body.push_str(&label_svg(
        &view.label,
        0.0,
        (y + first - TITLE_PAD) / 2.0,
        &format!(
            " class=\"pele-label\"{} font-weight=\"var(--_hw)\"{}{}",
            title_size, ink, st.text
        ),
        options.icons,
    ));
    let lines = |list: &[Line], kind: &str, from: f64| -> String {
        let mut out = String::new();
        let mut baseline = from + SECTION_PAD + line_height / 2.0 + member_size * 0.35;
        for line in list {
            let mark = if line.classifier == "$" {
                format!(
                    " pele-static\"{}",
                    if !st.text.is_empty() {
                        format!(
                            "{}text-decoration:underline\"",
                            &st.text[..st.text.len() - 1]
                        )
                    } else {
                        String::from(" style=\"text-decoration:underline\"")
                    }
                )
            } else if line.classifier == "*" {
                format!(" pele-abstract\" font-style=\"italic\"{}", st.text)
            } else {
                format!("\"{}", st.text)
            };
            out.push_str(&format!(
                "<text class=\"pele-member pele-{}{} x=\"{}\" y=\"{}\">{}</text>",
                kind,
                mark,
                num(left + PAD_X),
                num(baseline),
                esc(&line.text)
            ));
            baseline += line_height;
        }
        out
    };
    if !view.attributes.is_empty() || !view.methods.is_empty() {
        body.push_str(&format!(
            "<g class=\"pele-members\" font-size=\"{}\"{}{}>{}{}</g>",
            js::num_str(member_size),
            ink,
            if mono(st) {
                format!(" font-family=\"{}\"", crate::pele::svg::theme::FONT_MONO)
            } else {
                String::new()
            },
            lines(&view.attributes, "attribute", first),
            lines(&view.methods, "method", second)
        ));
    }
    if let Some(tip) = cls.tooltip.as_deref().filter(|t| !t.is_empty()) {
        body = format!("<title>{}</title>{}", esc_text(tip), body);
    }
    if let Some(link) = cls.link.as_deref().filter(|l| !l.is_empty()) {
        let target = match cls.link_target.as_deref() {
            Some(t) if TARGETS.contains(&t) => format!(" target=\"{}\"", t),
            _ => String::new(),
        };
        body = linked(&body, link, &cls.id, links, options, &target);
    }
    // cssClasses.replace(/^default\s?/, '')
    let css = match cls.css_classes.strip_prefix("default") {
        Some(rest) => {
            let mut chars = rest.chars();
            match chars.next() {
                Some(c) if js::is_space(c) => chars.as_str().to_string(),
                _ => rest.to_string(),
            }
        }
        None => cls.css_classes.clone(),
    };
    format!(
        "<g class=\"pele-node pele-class{}\" data-id=\"{}\"{}>{}</g>",
        class_names(&css),
        id,
        at,
        body
    )
}
