//! Every Mermaid diagram kind Obsidian draws, laid out by the app into a
//! list of [`Item`]s the guest paints: flowcharts (`mermaid::flowchart`),
//! sequence, class, state, ER, Gantt, pie, journey, mindmap and timeline.
//! The layouts are Mermaid's in shape -- lifelines and arrows, boxes with
//! compartments, bars on a time axis, wedges with a legend -- not its
//! exact placement.

use crate::mermaid::{self, Shape};
use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

/// What colour a part takes; the guest maps each to the theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    None,
    Node,
    /// The page's own background (behind a label that sits on a line).
    Paper,
    Cluster,
    Note,
    Line,
    Text,
    Muted,
    /// One of the categorical colours (pie wedges, Gantt sections).
    Series(u8),
    Done,
    Active,
    Critical,
    /// Dark text on a coloured label (gitGraph's branch names).
    Ink,
    /// A chart's faint grid lines.
    Grid,
    /// A see-through band in a series colour (sankey's flows).
    Band(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Head {
    None,
    Arrow,
    Open,
    Hollow,
    Diamond,
    FilledDiamond,
    Cross,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Box {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        r: f32,
        fill: Role,
        stroke: Role,
        dashed: bool,
    },
    Diamond {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        fill: Role,
        stroke: Role,
    },
    Circle {
        cx: f32,
        cy: f32,
        r: f32,
        fill: Role,
        stroke: Role,
    },
    /// Text turned by quarters (1 reads top to bottom, 3 bottom to top),
    /// centred on (x, y).
    Turned {
        x: f32,
        y: f32,
        s: String,
        size: f32,
        turns: u8,
        role: Role,
    },
    /// A band filled with a gradient from one colour at x0 to another at x1
    /// (sankey's flows).
    Band {
        pts: Vec<(f32, f32)>,
        from: Role,
        to: Role,
        x0: f32,
        x1: f32,
    },
    /// A closed outline, filled.
    Poly {
        pts: Vec<(f32, f32)>,
        fill: Role,
        stroke: Role,
    },
    Wedge {
        cx: f32,
        cy: f32,
        r: f32,
        start: f32,
        sweep: f32,
        fill: Role,
    },
    Line {
        pts: Vec<(f32, f32)>,
        role: Role,
        width: f32,
        dashed: bool,
        head: Head,
        tail: Head,
    },
    Text {
        x: f32,
        y: f32,
        s: String,
        size: f32,
        /// 0 start, 1 middle, 2 end.
        anchor: u8,
        bold: bool,
        role: Role,
    },
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Chart {
    pub w: f32,
    pub h: f32,
    pub items: Vec<Item>,
}

pub(crate) fn text(x: f32, y: f32, s: &str, size: f32, anchor: u8, bold: bool, role: Role) -> Item {
    Item::Text {
        x,
        y,
        s: String::from(s),
        size,
        anchor,
        bold,
        role,
    }
}

pub(crate) fn line(pts: Vec<(f32, f32)>, dashed: bool, head: Head) -> Item {
    Item::Line {
        pts,
        role: Role::Line,
        width: 1.4,
        dashed,
        head,
        tail: Head::None,
    }
}

pub(crate) fn bx(x: f32, y: f32, w: f32, h: f32, r: f32, fill: Role) -> Item {
    Item::Box {
        x,
        y,
        w,
        h,
        r,
        fill,
        stroke: Role::Line,
        dashed: false,
    }
}

pub(crate) fn lines_of(src: &str) -> Vec<&str> {
    src.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("%%"))
        .collect()
}

pub(crate) fn unquote(s: &str) -> &str {
    s.trim().trim_matches('"')
}

/// The kind a Mermaid block is, by its first word.
pub fn kind(src: &str) -> Option<&'static str> {
    let first = lines_of(src).into_iter().next()?;
    let w = first.split_whitespace().next()?;
    Some(match w {
        "graph" | "flowchart" => "flowchart",
        "sequenceDiagram" => "sequence",
        "classDiagram" | "classDiagram-v2" => "class",
        "stateDiagram" | "stateDiagram-v2" => "state",
        "erDiagram" => "er",
        "gantt" => "gantt",
        "pie" => "pie",
        "journey" => "journey",
        "mindmap" => "mindmap",
        "timeline" => "timeline",
        "gitGraph" | "gitGraph:" => "git",
        "quadrantChart" => "quadrant",
        "xychart-beta" | "xychart" => "xy",
        "sankey-beta" | "sankey" => "sankey",
        "block-beta" | "block" => "block",
        "requirementDiagram" => "requirement",
        "C4Context" | "C4Container" | "C4Component" | "C4Dynamic" | "C4Deployment" => "c4",
        _ => return None,
    })
}

/// Lay out any supported Mermaid block. `width` measures a string at a
/// size; `size` is the label size.
pub fn chart(src: &str, width: &dyn Fn(&str, f32) -> f32, size: f32, max_w: f32) -> Option<Chart> {
    match kind(src)? {
        "flowchart" => {
            let m = |s: &str| width(s, size);
            mermaid::flowchart(src, &m, size).map(|d| from_flowchart(&d, size))
        }
        "sequence" => sequence(src, width, size),
        "class" => class(src, width, size),
        "state" => state(src, width, size),
        "er" => er(src, width, size),
        "gantt" => gantt(src, width, size, max_w),
        "pie" => pie(src, width, size),
        "journey" => journey(src, width, size),
        "mindmap" => mindmap(src, width, size),
        "timeline" => timeline(src, width, size),
        "git" => crate::charts_more::git(src, width, size),
        "quadrant" => crate::charts_more::quadrant(src, width, size),
        "xy" => crate::charts_more::xy(src, width, size),
        "sankey" => crate::charts_more::sankey(src, width, size),
        "block" => crate::charts_more::block(src, width, size),
        "requirement" => crate::charts_more::requirement(src, width, size),
        "c4" => crate::charts_more::c4(src, width, size),
        _ => None,
    }
}

fn from_flowchart(d: &mermaid::Diagram, size: f32) -> Chart {
    let mut items = Vec::new();
    // Subgraph boxes under everything, their titles at the top left.
    for (title, _, (x, y, w, h)) in &d.clusters {
        items.push(Item::Box {
            x: *x,
            y: *y,
            w: *w,
            h: *h,
            r: 4.0,
            fill: Role::Cluster,
            stroke: Role::Line,
            dashed: false,
        });
        items.push(text(
            x + size * 0.75,
            y + size * 1.5,
            title,
            size,
            0,
            false,
            Role::Muted,
        ));
    }
    for e in &d.edges {
        items.push(Item::Line {
            pts: if e.pts.len() >= 2 {
                e.pts.clone()
            } else {
                alloc::vec![(e.x0, e.y0), (e.x1, e.y1)]
            },
            role: Role::Line,
            width: if e.thick { 2.6 } else { 1.4 },
            dashed: e.dotted,
            head: if e.arrow { Head::Arrow } else { Head::None },
            tail: Head::None,
        });
        if !e.label.is_empty() {
            let (mx, my) = e.label_at;
            // Mermaid's label sits on a patch of the page over the line.
            let lw = e.label_w;
            items.push(Item::Box {
                x: mx - lw * 0.5,
                y: my - size * 0.7,
                w: lw,
                h: size * 1.4,
                r: 0.0,
                fill: Role::Paper,
                stroke: Role::None,
                dashed: false,
            });
            items.push(text(
                mx,
                my + size * 0.3,
                &e.label,
                size * 0.85,
                1,
                false,
                Role::Text,
            ));
        }
    }
    for n in &d.nodes {
        let (cx, cy) = (n.x + n.w * 0.5, n.y + n.h * 0.5);
        let mut text_y = cy + size * 0.36;
        match n.shape {
            Shape::Circle => items.push(Item::Circle {
                cx,
                cy,
                r: n.w * 0.5,
                fill: Role::Node,
                stroke: Role::Line,
            }),
            Shape::Diamond => items.push(Item::Diamond {
                x: n.x,
                y: n.y,
                w: n.w,
                h: n.h,
                fill: Role::Node,
                stroke: Role::Line,
            }),
            Shape::Stadium => items.push(bx(n.x, n.y, n.w, n.h, n.h * 0.5, Role::Node)),
            Shape::Round => items.push(bx(n.x, n.y, n.w, n.h, 8.0, Role::Node)),
            Shape::Rect => items.push(bx(n.x, n.y, n.w, n.h, 3.0, Role::Node)),
            Shape::Subroutine => {
                items.push(bx(n.x, n.y, n.w, n.h, 0.0, Role::Node));
                let inset = size * 0.5;
                for x in [n.x + inset, n.x + n.w - inset] {
                    items.push(line(
                        alloc::vec![(x, n.y), (x, n.y + n.h)],
                        false,
                        Head::None,
                    ));
                }
            }
            Shape::Cylinder => {
                // A drum: the body with its rounded foot, and the top's
                // whole ellipse.
                let ry = size * 0.45;
                let ell = |ey: f32, from: f32, to: f32| -> Vec<(f32, f32)> {
                    (0..=24)
                        .map(|k| {
                            let a = from + (to - from) * k as f32 / 24.0;
                            (cx + n.w * 0.5 * a.cos(), ey + ry * a.sin())
                        })
                        .collect()
                };
                let pi = core::f32::consts::PI;
                let mut body = ell(n.y + ry, pi, 2.0 * pi);
                body.extend(ell(n.y + n.h - ry, 0.0, pi));
                if let Some(f) = body.first().copied() {
                    body.push(f);
                }
                items.push(Item::Poly {
                    pts: body,
                    fill: Role::Node,
                    stroke: Role::Line,
                });
                items.push(line(ell(n.y + ry, 0.0, pi), false, Head::None));
                text_y += ry * 0.5;
            }
        }
        items.push(text(cx, text_y, &n.label, size, 1, false, Role::Text));
    }
    Chart {
        w: d.w,
        h: d.h,
        items,
    }
}

// ---- sequence ------------------------------------------------------------------------------

fn sequence(src: &str, width: &dyn Fn(&str, f32) -> f32, size: f32) -> Option<Chart> {
    let mut parts: Vec<(String, String, bool)> = Vec::new(); // id, label, actor
    let add =
        |parts: &mut Vec<(String, String, bool)>, id: &str, label: Option<&str>, actor: bool| {
            if let Some(p) = parts.iter_mut().find(|p| p.0 == id) {
                if let Some(l) = label {
                    p.1 = String::from(l);
                }
                return;
            }
            parts.push((String::from(id), String::from(label.unwrap_or(id)), actor));
        };
    enum Step {
        Msg(String, String, String, bool, Head),
        Note(Vec<String>, String),
        Open(String),
        Else(String),
        Close,
    }
    let mut steps = Vec::new();
    for l in lines_of(src).into_iter().skip(1) {
        let w0 = l.split_whitespace().next().unwrap_or("");
        match w0 {
            "participant" | "actor" => {
                let rest = l[w0.len()..].trim();
                let (id, label) = match rest.split_once(" as ") {
                    Some((a, b)) => (a.trim(), Some(b.trim())),
                    None => (rest, None),
                };
                add(&mut parts, id, label, w0 == "actor");
            }
            "loop" | "alt" | "opt" | "par" | "critical" | "rect" | "break" => {
                let mut s = String::from(w0);
                let rest = l[w0.len()..].trim();
                if !rest.is_empty() && w0 != "rect" {
                    s.push_str(" [");
                    s.push_str(rest);
                    s.push(']');
                }
                steps.push(Step::Open(s));
            }
            "else" | "and" => steps.push(Step::Else(String::from(l[w0.len()..].trim()))),
            "end" => steps.push(Step::Close),
            "activate" | "deactivate" | "autonumber" | "title" => {}
            _ if l.starts_with("Note ") || l.starts_with("note ") => {
                let rest = &l[5..];
                if let Some((place, txt)) = rest.split_once(':') {
                    let ids: Vec<String> = place
                        .trim()
                        .trim_start_matches("right of")
                        .trim_start_matches("left of")
                        .trim_start_matches("over")
                        .split(',')
                        .map(|s| String::from(s.trim()))
                        .collect();
                    for id in &ids {
                        add(&mut parts, id, None, false);
                    }
                    steps.push(Step::Note(ids, String::from(txt.trim())));
                }
            }
            _ => {
                // A -> B : text, with the arrow kinds Mermaid has.
                let arrows = ["-->>", "->>", "--x", "-x", "--)", "-)", "-->", "->"];
                let Some((pos, arrow)) = arrows
                    .iter()
                    .filter_map(|a| l.find(a).map(|p| (p, *a)))
                    .min_by_key(|(p, a)| (*p, usize::MAX - a.len()))
                else {
                    continue;
                };
                let from = l[..pos].trim();
                let after = &l[pos + arrow.len()..];
                let (to, msg) = match after.split_once(':') {
                    Some((t, m)) => (t.trim(), m.trim()),
                    None => (after.trim(), ""),
                };
                let to = to.trim_start_matches(['+', '-']);
                if from.is_empty() || to.is_empty() {
                    continue;
                }
                add(&mut parts, from, None, false);
                add(&mut parts, to, None, false);
                let head = if arrow.contains('x') {
                    Head::Cross
                } else if arrow.contains(">>") {
                    Head::Arrow
                } else {
                    Head::Open
                };
                steps.push(Step::Msg(
                    String::from(from),
                    String::from(to),
                    String::from(msg),
                    arrow.starts_with("--"),
                    head,
                ));
            }
        }
    }
    if parts.is_empty() {
        return None;
    }
    let gap = size * 3.0;
    let box_h = size * 2.6;
    let mut xs = Vec::new();
    let mut x = 0.0f32;
    let mut widths = Vec::new();
    for (_, label, _) in &parts {
        let w = width(label, size) + size * 2.0;
        widths.push(w.max(size * 6.0));
    }
    for (i, w) in widths.iter().enumerate() {
        // Room between two lifelines for the longest message between them.
        if i > 0 {
            x += gap;
        }
        xs.push(x + w * 0.5);
        x += w;
    }
    // Widen gaps for long messages between neighbours.
    for s in &steps {
        if let Step::Msg(a, b, m, ..) = s {
            let ia = parts.iter().position(|p| &p.0 == a).unwrap_or(0);
            let ib = parts.iter().position(|p| &p.0 == b).unwrap_or(0);
            if ia == ib {
                continue;
            }
            let (lo, hi) = (ia.min(ib), ia.max(ib));
            let need = width(m, size * 0.9) + size * 2.0;
            let have = xs[hi] - xs[lo];
            if need > have {
                let extra = need - have;
                for x in xs.iter_mut().skip(hi) {
                    *x += extra;
                }
            }
        }
    }
    let col = |id: &str| parts.iter().position(|p| p.0 == id).map_or(0.0, |i| xs[i]);
    let mut items = Vec::new();
    let mut y = box_h + size * 1.5;
    // Open loop/alt frames: where each began, its label, its else lines.
    type Frame = (f32, String, Vec<(f32, String)>);
    let mut frames: Vec<Frame> = Vec::new();
    let mut frame_items = Vec::new();
    let left = 0.0f32;
    let right = xs.last().copied().unwrap_or(0.0) + widths.last().copied().unwrap_or(0.0) * 0.5;
    for s in &steps {
        match s {
            Step::Msg(a, b, m, dashed, head) => {
                let (xa, xb) = (col(a), col(b));
                if (xa - xb).abs() < 1.0 {
                    // To itself: a loop out to the right.
                    let w = size * 2.5;
                    items.push(text(
                        xa + 6.0,
                        y + size * 0.2,
                        m,
                        size * 0.9,
                        0,
                        false,
                        Role::Text,
                    ));
                    items.push(Item::Line {
                        pts: alloc::vec![
                            (xa, y + size * 0.6),
                            (xa + w, y + size * 0.6),
                            (xa + w, y + size * 1.8),
                            (xa, y + size * 1.8)
                        ],
                        role: Role::Line,
                        width: 1.4,
                        dashed: *dashed,
                        head: *head,
                        tail: Head::None,
                    });
                    y += size * 2.8;
                } else {
                    items.push(text(
                        (xa + xb) * 0.5,
                        y,
                        m,
                        size * 0.9,
                        1,
                        false,
                        Role::Text,
                    ));
                    items.push(Item::Line {
                        pts: alloc::vec![(xa, y + size * 0.6), (xb, y + size * 0.6)],
                        role: Role::Line,
                        width: 1.4,
                        dashed: *dashed,
                        head: *head,
                        tail: Head::None,
                    });
                    y += size * 2.4;
                }
            }
            Step::Note(ids, t) => {
                let xs2: Vec<f32> = ids.iter().map(|i| col(i)).collect();
                let lo = xs2.iter().copied().fold(f32::MAX, f32::min);
                let hi = xs2.iter().copied().fold(f32::MIN, f32::max);
                let w = (width(t, size * 0.9) + size * 1.5).max(hi - lo + size * 4.0);
                let cx = (lo + hi) * 0.5 + if ids.len() == 1 { w * 0.5 + size } else { 0.0 };
                items.push(bx(
                    cx - w * 0.5,
                    y - size * 0.4,
                    w,
                    size * 2.0,
                    2.0,
                    Role::Note,
                ));
                items.push(text(
                    cx,
                    y + size * 0.95,
                    t,
                    size * 0.9,
                    1,
                    false,
                    Role::Text,
                ));
                y += size * 2.8;
            }
            Step::Open(label) => {
                frames.push((y, label.clone(), Vec::new()));
                y += size * 1.8;
            }
            Step::Else(label) => {
                if let Some(f) = frames.last_mut() {
                    f.2.push((y, label.clone()));
                }
                y += size * 1.6;
            }
            Step::Close => {
                if let Some((top, label, elses)) = frames.pop() {
                    let depth = frames.len() as f32 * size * 0.6;
                    frame_items.push(Item::Box {
                        x: left - size + depth,
                        y: top - size * 0.4,
                        w: right - left + size * 2.0 - depth * 2.0,
                        h: y - top,
                        r: 0.0,
                        fill: Role::None,
                        stroke: Role::Muted,
                        dashed: false,
                    });
                    frame_items.push(text(
                        left - size * 0.5 + depth,
                        top + size * 0.8,
                        &label,
                        size * 0.8,
                        0,
                        true,
                        Role::Text,
                    ));
                    for (ey, el) in elses {
                        frame_items.push(Item::Line {
                            pts: alloc::vec![(left - size + depth, ey), (right + size - depth, ey)],
                            role: Role::Muted,
                            width: 1.0,
                            dashed: true,
                            head: Head::None,
                            tail: Head::None,
                        });
                        if !el.is_empty() {
                            let mut s = String::from("[");
                            s.push_str(&el);
                            s.push(']');
                            frame_items.push(text(
                                (left + right) * 0.5,
                                ey + size * 1.1,
                                &s,
                                size * 0.8,
                                1,
                                false,
                                Role::Muted,
                            ));
                        }
                    }
                    y += size * 0.6;
                }
            }
        }
    }
    let bottom = y + size * 0.6;
    let mut out = Vec::new();
    for (i, (_, label, actor)) in parts.iter().enumerate() {
        let (cx, w) = (xs[i], widths[i]);
        out.push(Item::Line {
            pts: alloc::vec![(cx, box_h), (cx, bottom)],
            role: Role::Muted,
            width: 1.0,
            dashed: true,
            head: Head::None,
            tail: Head::None,
        });
        for top in [0.0, bottom] {
            if *actor {
                // A stick figure, as Mermaid draws an actor.
                let cy = top + size * 0.5;
                out.push(Item::Circle {
                    cx,
                    cy,
                    r: size * 0.35,
                    fill: Role::None,
                    stroke: Role::Line,
                });
                out.push(line(
                    alloc::vec![(cx, cy + size * 0.35), (cx, cy + size * 1.2)],
                    false,
                    Head::None,
                ));
                out.push(line(
                    alloc::vec![
                        (cx - size * 0.6, cy + size * 0.7),
                        (cx + size * 0.6, cy + size * 0.7)
                    ],
                    false,
                    Head::None,
                ));
                out.push(line(
                    alloc::vec![(cx, cy + size * 1.2), (cx - size * 0.5, cy + size * 1.8)],
                    false,
                    Head::None,
                ));
                out.push(line(
                    alloc::vec![(cx, cy + size * 1.2), (cx + size * 0.5, cy + size * 1.8)],
                    false,
                    Head::None,
                ));
                out.push(text(
                    cx,
                    top + box_h + size * 0.2,
                    label,
                    size,
                    1,
                    false,
                    Role::Text,
                ));
            } else {
                out.push(bx(cx - w * 0.5, top, w, box_h, 3.0, Role::Node));
                out.push(text(
                    cx,
                    top + box_h * 0.5 + size * 0.36,
                    label,
                    size,
                    1,
                    false,
                    Role::Text,
                ));
            }
        }
    }
    out.extend(frame_items);
    out.extend(items);
    Some(Chart {
        w: right + size,
        h: bottom + box_h + size,
        items: out,
    })
}

// ---- boxes joined by edges: class, state, ER ----------------------------------------------------

pub(crate) struct GNode {
    pub id: String,
    pub title: String,
    pub rows: Vec<String>,
    pub kind: u8, // 0 box, 1 start dot, 2 end dot
    /// A «stereotype» line over the title (requirement diagrams).
    pub stereo: String,
}

pub(crate) struct GEdge {
    pub a: usize,
    pub b: usize,
    pub label: String,
    pub dashed: bool,
    pub head: Head,
    pub tail: Head,
    pub a_card: String,
    pub b_card: String,
}

pub(crate) fn find_or_add(nodes: &mut Vec<GNode>, id: &str) -> usize {
    if let Some(i) = nodes.iter().position(|n| n.id == id) {
        return i;
    }
    let kind = match id {
        "[*]start" => 1,
        "[*]end" => 2,
        _ => 0,
    };
    nodes.push(GNode {
        id: String::from(id),
        title: String::from(if kind > 0 { "" } else { id }),
        rows: Vec::new(),
        kind,
        stereo: String::new(),
    });
    nodes.len() - 1
}

/// Boxes in ranks top to bottom (or left to right), edges between them.
pub(crate) fn graph_chart(
    nodes: &[GNode],
    edges: &[GEdge],
    width: &dyn Fn(&str, f32) -> f32,
    size: f32,
    across: bool,
) -> Chart {
    let sizes: Vec<(f32, f32)> = nodes
        .iter()
        .map(|n| match n.kind {
            0 => {
                let w = n
                    .rows
                    .iter()
                    .map(|r| width(r, size * 0.9))
                    .fold(width(&n.title, size), f32::max)
                    .max(width(&n.stereo, size * 0.85))
                    + size * 2.0;
                let h = size * 2.2
                    + if n.stereo.is_empty() { 0.0 } else { size * 1.3 }
                    + if n.rows.is_empty() {
                        0.0
                    } else {
                        n.rows.len() as f32 * size * 1.4 + size * 0.6
                    };
                (w.max(size * 5.0), h)
            }
            _ => (size * 1.2, size * 1.2),
        })
        .collect();
    // Laid out as dagre lays out Mermaid's (src/layered.rs).
    let pairs: Vec<(usize, usize)> = edges.iter().map(|e| (e.a, e.b)).collect();
    let labels: Vec<(f32, f32)> = edges
        .iter()
        .map(|e| {
            if e.label.is_empty() {
                (0.0, 0.0)
            } else {
                (width(&e.label, size * 0.85) + size * 0.6, size * 1.5)
            }
        })
        .collect();
    let lay = crate::layered::layout(&crate::layered::Input {
        sizes: &sizes,
        edges: &pairs,
        labels: &labels,
        shapes: &nodes
            .iter()
            .map(|n| if n.kind == 0 { 0 } else { 2 })
            .collect::<Vec<u8>>(),
        dir: if across {
            crate::layered::Dir::LeftRight
        } else {
            crate::layered::Dir::TopDown
        },
        node_sep: size * 3.125,
        rank_sep: size * 3.125,
        label_gap: size * 1.25,
        groups: &[],
        group_gap: 0.0,
    });
    struct Placed {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    }
    let d: Vec<Placed> = lay
        .pos
        .iter()
        .zip(sizes.iter())
        .map(|(p, s)| Placed {
            x: p.0,
            y: p.1,
            w: s.0,
            h: s.1,
        })
        .collect();
    let (cw, ch) = (lay.w, lay.h);
    let mut items = Vec::new();
    for (ei, e) in edges.iter().enumerate() {
        let pts = lay.paths.get(ei).cloned().unwrap_or_default();
        let (Some(&p0), Some(&p1)) = (pts.first(), pts.last()) else {
            continue;
        };
        items.push(Item::Line {
            pts: pts.clone(),
            role: Role::Line,
            width: 1.4,
            dashed: e.dashed,
            head: e.head,
            tail: e.tail,
        });
        if !e.label.is_empty() {
            let (mx, my) = lay.label_at.get(ei).copied().unwrap_or(p0);
            let w = width(&e.label, size * 0.85) + size * 0.6;
            items.push(Item::Box {
                x: mx - w * 0.5,
                y: my - size * 0.7,
                w,
                h: size * 1.3,
                r: 2.0,
                fill: Role::Paper,
                stroke: Role::None,
                dashed: false,
            });
            items.push(text(
                mx,
                my + size * 0.3,
                &e.label,
                size * 0.85,
                1,
                false,
                Role::Text,
            ));
        }
        // Cardinalities beside each end, along the curve's first and last
        // pieces.
        let near0 = pts.get(1).copied().unwrap_or(p1);
        let near1 = pts.get(pts.len().saturating_sub(2)).copied().unwrap_or(p0);
        for (card, p, q) in [(&e.a_card, p0, near0), (&e.b_card, p1, near1)] {
            if !card.is_empty() {
                let (dx, dy) = (q.0 - p.0, q.1 - p.1);
                let len = (dx * dx + dy * dy).sqrt().max(1.0);
                let (tx, ty) = (
                    p.0 + dx / len * size * 1.4 + 6.0,
                    p.1 + dy / len * size * 1.4,
                );
                items.push(text(tx, ty, card, size * 0.8, 0, false, Role::Muted));
            }
        }
    }
    for (i, n) in d.iter().enumerate() {
        let g = &nodes[i];
        match g.kind {
            1 => items.push(Item::Circle {
                cx: n.x + n.w * 0.5,
                cy: n.y + n.h * 0.5,
                r: n.w * 0.45,
                fill: Role::Line,
                stroke: Role::Line,
            }),
            2 => {
                items.push(Item::Circle {
                    cx: n.x + n.w * 0.5,
                    cy: n.y + n.h * 0.5,
                    r: n.w * 0.5,
                    fill: Role::None,
                    stroke: Role::Line,
                });
                items.push(Item::Circle {
                    cx: n.x + n.w * 0.5,
                    cy: n.y + n.h * 0.5,
                    r: n.w * 0.3,
                    fill: Role::Line,
                    stroke: Role::Line,
                });
            }
            _ => {
                items.push(bx(
                    n.x,
                    n.y,
                    n.w,
                    n.h,
                    if g.rows.is_empty() { 6.0 } else { 3.0 },
                    Role::Node,
                ));
                let st = if g.stereo.is_empty() { 0.0 } else { size * 1.3 };
                if st > 0.0 {
                    items.push(text(
                        n.x + n.w * 0.5,
                        n.y + size * 1.3,
                        &g.stereo,
                        size * 0.85,
                        1,
                        false,
                        Role::Text,
                    ));
                }
                items.push(text(
                    n.x + n.w * 0.5,
                    n.y + size * 1.45 + st,
                    &g.title,
                    size,
                    1,
                    !g.rows.is_empty(),
                    Role::Text,
                ));
                if !g.rows.is_empty() {
                    let sep = n.y + size * 2.2 + st;
                    items.push(line(
                        alloc::vec![(n.x, sep), (n.x + n.w, sep)],
                        false,
                        Head::None,
                    ));
                    for (k, row) in g.rows.iter().enumerate() {
                        let ry = sep + size * 1.3 + k as f32 * size * 1.4;
                        if row == "\u{1}" {
                            items.push(line(
                                alloc::vec![(n.x, ry - size * 0.55), (n.x + n.w, ry - size * 0.55)],
                                false,
                                Head::None,
                            ));
                            continue;
                        }
                        items.push(text(
                            n.x + size * 0.7,
                            ry,
                            row,
                            size * 0.9,
                            0,
                            false,
                            Role::Text,
                        ));
                    }
                }
            }
        }
    }
    Chart {
        w: cw,
        h: ch,
        items,
    }
}

fn class(src: &str, width: &dyn Fn(&str, f32) -> f32, size: f32) -> Option<Chart> {
    let mut nodes: Vec<GNode> = Vec::new();
    let mut edges: Vec<GEdge> = Vec::new();
    let all = lines_of(src);
    let mut i = 1;
    let rels: [(&str, Head, Head, bool); 10] = [
        ("<|--", Head::None, Head::Hollow, false),
        ("--|>", Head::Hollow, Head::None, false),
        ("<|..", Head::None, Head::Hollow, true),
        ("..|>", Head::Hollow, Head::None, true),
        ("*--", Head::None, Head::FilledDiamond, false),
        ("o--", Head::None, Head::Diamond, false),
        ("-->", Head::Arrow, Head::None, false),
        ("..>", Head::Arrow, Head::None, true),
        ("..", Head::None, Head::None, true),
        ("--", Head::None, Head::None, false),
    ];
    while i < all.len() {
        let l = all[i];
        i += 1;
        if let Some(rest) = l.strip_prefix("class ") {
            let name = rest.trim_end_matches('{').trim();
            let name = name.split(['~', '<']).next().unwrap_or(name).trim();
            let id = find_or_add(&mut nodes, name);
            if l.ends_with('{') {
                while i < all.len() && all[i] != "}" {
                    nodes[id].rows.push(String::from(all[i]));
                    i += 1;
                }
                i += 1;
            }
            continue;
        }
        if let Some((rel, head, tail, dashed)) = rels.iter().find(|r| l.contains(r.0)) {
            let p = l.find(rel).unwrap_or(0);
            let (left, right) = (&l[..p], &l[p + rel.len()..]);
            let (right, label) = match right.split_once(':') {
                Some((a, b)) => (a, b.trim()),
                None => (right, ""),
            };
            let strip_card = |s: &str| -> (String, String) {
                let s = s.trim();
                if let Some(q) = s.find('"') {
                    let rest = &s[q + 1..];
                    if let Some(e) = rest.find('"') {
                        let card = &rest[..e];
                        let joined = String::from(s[..q].trim()) + rest[e + 1..].trim();
                        let name = String::from(joined.trim());
                        return (name, String::from(card));
                    }
                }
                (String::from(s), String::new())
            };
            let (a, ac) = strip_card(left);
            let (b, bc) = strip_card(right);
            if a.is_empty() || b.is_empty() {
                continue;
            }
            let ia = find_or_add(&mut nodes, &a);
            let ib = find_or_add(&mut nodes, &b);
            edges.push(GEdge {
                a: ia,
                b: ib,
                label: String::from(label),
                dashed: *dashed,
                head: *head,
                tail: *tail,
                a_card: ac,
                b_card: bc,
            });
            continue;
        }
        if let Some((name, member)) = l.split_once(" : ") {
            let id = find_or_add(&mut nodes, name.trim());
            nodes[id].rows.push(String::from(member.trim()));
        }
    }
    if nodes.is_empty() {
        return None;
    }
    // Attributes, then methods in their own compartment (a row of "\u{1}"
    // is the line between them); an empty class keeps its empty box.
    for n in nodes.iter_mut() {
        let (methods, attrs): (Vec<String>, Vec<String>) =
            n.rows.drain(..).partition(|r| r.contains('('));
        n.rows = attrs;
        if n.rows.is_empty() {
            n.rows.push(String::new());
        }
        n.rows.push(String::from("\u{1}"));
        n.rows.extend(methods);
    }
    Some(graph_chart(&nodes, &edges, width, size, false))
}

fn state(src: &str, width: &dyn Fn(&str, f32) -> f32, size: f32) -> Option<Chart> {
    let mut nodes: Vec<GNode> = Vec::new();
    let mut edges: Vec<GEdge> = Vec::new();
    for l in lines_of(src).into_iter().skip(1) {
        if l.starts_with("direction") || l == "}" || l.starts_with("note") || l.starts_with("--") {
            continue;
        }
        if let Some(rest) = l.strip_prefix("state ") {
            // state "A long name" as S  /  state S {
            if let Some((desc, id)) = rest.split_once(" as ") {
                let id = id.trim_end_matches('{').trim();
                let n = find_or_add(&mut nodes, id);
                nodes[n].title = String::from(unquote(desc));
            } else {
                find_or_add(&mut nodes, rest.trim_end_matches('{').trim());
            }
            continue;
        }
        if let Some(p) = l.find("-->") {
            let a = l[..p].trim();
            let (b, label) = match l[p + 3..].split_once(':') {
                Some((x, y)) => (x.trim(), y.trim()),
                None => (l[p + 3..].trim(), ""),
            };
            let a = if a == "[*]" { "[*]start" } else { a };
            let b = if b == "[*]" { "[*]end" } else { b };
            let ia = find_or_add(&mut nodes, a);
            let ib = find_or_add(&mut nodes, b);
            edges.push(GEdge {
                a: ia,
                b: ib,
                label: String::from(label),
                dashed: false,
                head: Head::Arrow,
                tail: Head::None,
                a_card: String::new(),
                b_card: String::new(),
            });
            continue;
        }
        if let Some((id, desc)) = l.split_once(':') {
            let n = find_or_add(&mut nodes, id.trim());
            nodes[n].rows.push(String::from(desc.trim()));
        }
    }
    if nodes.is_empty() {
        return None;
    }
    Some(graph_chart(&nodes, &edges, width, size, false))
}

fn er(src: &str, width: &dyn Fn(&str, f32) -> f32, size: f32) -> Option<Chart> {
    let mut nodes: Vec<GNode> = Vec::new();
    let mut edges: Vec<GEdge> = Vec::new();
    let all = lines_of(src);
    let mut i = 1;
    let card = |s: &str, left: bool| -> String {
        // ||, |o, o|, }|, |{, }o, o{  -> words Mermaid's crow's feet mean.
        let s = if left {
            s.chars().rev().collect::<String>()
        } else {
            String::from(s)
        };
        String::from(match s.as_str() {
            "||" => "1",
            "|o" | "o|" => "0..1",
            "}|" | "|{" => "1..*",
            "}o" | "o{" => "0..*",
            _ => "",
        })
    };
    while i < all.len() {
        let l = all[i];
        i += 1;
        if l.ends_with('{') {
            let id = find_or_add(&mut nodes, l.trim_end_matches('{').trim());
            while i < all.len() && all[i] != "}" {
                let mut w = all[i].split_whitespace();
                let (t, n) = (w.next().unwrap_or(""), w.next().unwrap_or(""));
                let mut row = String::from(t);
                row.push(' ');
                row.push_str(n);
                nodes[id].rows.push(row);
                i += 1;
            }
            i += 1;
            continue;
        }
        // A ||--o{ B : label
        let Some(p) = l.find("--").or_else(|| l.find("..")) else {
            continue;
        };
        let dashed = l[p..].starts_with("..");
        let left = l[..p].trim();
        let (a, lc) = left.rsplit_once(' ').unwrap_or((left, ""));
        let rest = &l[p + 2..];
        let (right, label) = match rest.split_once(':') {
            Some((x, y)) => (x.trim(), unquote(y)),
            None => (rest.trim(), ""),
        };
        let (rc, b) = right.split_once(' ').unwrap_or(("", right));
        if a.trim().is_empty() || b.trim().is_empty() {
            continue;
        }
        let ia = find_or_add(&mut nodes, a.trim());
        let ib = find_or_add(&mut nodes, b.trim());
        edges.push(GEdge {
            a: ia,
            b: ib,
            label: String::from(label),
            dashed,
            head: Head::None,
            tail: Head::None,
            a_card: card(lc, true),
            b_card: card(rc, false),
        });
    }
    if nodes.is_empty() {
        return None;
    }
    Some(graph_chart(&nodes, &edges, width, size, false))
}

// ---- time and proportion: gantt, pie, journey, timeline ---------------------------------------------

/// Days since 1970-01-01 of a YYYY-MM-DD date.
fn days(d: &str) -> Option<i64> {
    let y: i64 = d.get(..4)?.parse().ok()?;
    let m: i64 = d.get(5..7)?.parse().ok()?;
    let dd: i64 = d.get(8..10)?.parse().ok()?;
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + dd - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

fn date_of(days: i64) -> String {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    alloc::format!("{y:04}-{m:02}-{d:02}")
}

fn duration(s: &str) -> Option<i64> {
    let s = s.trim();
    let n: f32 = s
        .trim_end_matches(|c: char| c.is_alphabetic())
        .parse()
        .ok()?;
    let unit = s.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.');
    let d = match unit {
        "d" | "" => n,
        "w" => n * 7.0,
        "h" => n / 24.0,
        "m" | "min" => n / 1440.0,
        _ => return None,
    };
    Some(d.ceil().max(1.0) as i64)
}

fn gantt(src: &str, width: &dyn Fn(&str, f32) -> f32, size: f32, max_w: f32) -> Option<Chart> {
    let mut title = String::new();
    let mut section = String::new();
    // (section, name, id, start day, end day, status)
    let mut tasks: Vec<(String, String, String, i64, i64, Role)> = Vec::new();
    for l in lines_of(src).into_iter().skip(1) {
        if let Some(t) = l.strip_prefix("title") {
            title = String::from(t.trim());
            continue;
        }
        if let Some(s) = l.strip_prefix("section") {
            section = String::from(s.trim());
            continue;
        }
        if l.starts_with("dateFormat")
            || l.starts_with("axisFormat")
            || l.starts_with("excludes")
            || l.starts_with("todayMarker")
            || l.starts_with("tickInterval")
        {
            continue;
        }
        let Some((name, spec)) = l.split_once(':') else {
            continue;
        };
        let mut parts: Vec<&str> = spec.split(',').map(str::trim).collect();
        let mut status = Role::Series(0);
        parts.retain(|p| match *p {
            "done" => {
                status = Role::Done;
                false
            }
            "active" => {
                status = Role::Active;
                false
            }
            "crit" => {
                status = Role::Critical;
                false
            }
            "milestone" => false,
            _ => true,
        });
        let prev_end = tasks.last().map_or(0, |t| t.4);
        let (mut id, mut start, mut end) = (String::new(), None, None);
        for p in &parts {
            if let Some(d) = days(p) {
                if start.is_none() {
                    start = Some(d);
                } else {
                    end = Some(d);
                }
            } else if let Some(after) = p.strip_prefix("after ") {
                let e = after
                    .split_whitespace()
                    .filter_map(|a| tasks.iter().find(|t| t.2 == a).map(|t| t.4))
                    .max();
                start = e.or(Some(prev_end));
            } else if let Some(d) = duration(p) {
                end = Some(start.unwrap_or(prev_end) + d);
            } else if !p.is_empty() {
                id = String::from(*p);
            }
        }
        let start = start.unwrap_or(prev_end);
        let end = end.unwrap_or(start + 1).max(start + 1);
        let st = if status == Role::Series(0) {
            Role::Series(
                (tasks
                    .iter()
                    .map(|t| &t.0)
                    .collect::<alloc::collections::BTreeSet<_>>()
                    .len()
                    % 4) as u8,
            )
        } else {
            status
        };
        tasks.push((
            section.clone(),
            String::from(name.trim()),
            id,
            start,
            end,
            st,
        ));
    }
    if tasks.is_empty() {
        return None;
    }
    let first = tasks.iter().map(|t| t.3).min().unwrap_or(0);
    let last = tasks.iter().map(|t| t.4).max().unwrap_or(first + 1);
    let label_w = tasks
        .iter()
        .map(|t| width(&t.0, size * 0.9))
        .fold(size * 4.0, f32::max)
        + size;
    let span = (last - first).max(1) as f32;
    let plot_w = (max_w - label_w - size * 6.0).max(size * 16.0);
    let px = |d: i64| label_w + (d - first) as f32 / span * plot_w;
    let row_h = size * 2.0;
    let top = if title.is_empty() { size } else { size * 3.0 };
    let mut items = Vec::new();
    if !title.is_empty() {
        items.push(text(
            label_w + plot_w * 0.5,
            size * 1.6,
            &title,
            size * 1.1,
            1,
            true,
            Role::Text,
        ));
    }
    let bottom = top + tasks.len() as f32 * row_h;
    // Section bands, then the grid and axis dates.
    let mut k = 0;
    while k < tasks.len() {
        let sec = &tasks[k].0;
        let mut e = k;
        while e + 1 < tasks.len() && &tasks[e + 1].0 == sec {
            e += 1;
        }
        if k % 2 == 0 {
            items.push(Item::Box {
                x: 0.0,
                y: top + k as f32 * row_h,
                w: label_w + plot_w,
                h: (e - k + 1) as f32 * row_h,
                r: 0.0,
                fill: Role::Cluster,
                stroke: Role::None,
                dashed: false,
            });
        }
        items.push(text(
            size * 0.4,
            top + k as f32 * row_h + row_h * 0.65,
            sec,
            size * 0.9,
            0,
            true,
            Role::Text,
        ));
        k = e + 1;
    }
    let ticks = 6;
    for t in 0..=ticks {
        let d = first + (last - first) * t / ticks;
        let x = px(d);
        items.push(Item::Line {
            pts: alloc::vec![(x, top), (x, bottom)],
            role: Role::Muted,
            width: 0.8,
            dashed: false,
            head: Head::None,
            tail: Head::None,
        });
        items.push(text(
            x,
            bottom + size * 1.3,
            date_of(d).get(5..).unwrap_or(""),
            size * 0.75,
            1,
            false,
            Role::Muted,
        ));
    }
    for (i, t) in tasks.iter().enumerate() {
        let y = top + i as f32 * row_h + row_h * 0.15;
        let (x0, x1) = (px(t.3), px(t.4));
        items.push(Item::Box {
            x: x0,
            y,
            w: (x1 - x0).max(2.0),
            h: row_h * 0.7,
            r: 3.0,
            fill: t.5,
            stroke: Role::None,
            dashed: false,
        });
        let tw = width(&t.1, size * 0.85);
        if tw + size < x1 - x0 {
            items.push(text(
                (x0 + x1) * 0.5,
                y + row_h * 0.5,
                &t.1,
                size * 0.85,
                1,
                false,
                Role::Text,
            ));
        } else {
            items.push(text(
                x1 + size * 0.4,
                y + row_h * 0.5,
                &t.1,
                size * 0.85,
                0,
                false,
                Role::Text,
            ));
        }
    }
    Some(Chart {
        w: label_w + plot_w + size * 6.0,
        h: bottom + size * 2.0,
        items,
    })
}

fn pie(src: &str, width: &dyn Fn(&str, f32) -> f32, size: f32) -> Option<Chart> {
    let all = lines_of(src);
    let mut title = String::from(
        all.first()?
            .trim_start_matches("pie")
            .trim()
            .trim_start_matches("showData")
            .trim()
            .trim_start_matches("title")
            .trim(),
    );
    let mut slices: Vec<(String, f32)> = Vec::new();
    for l in all.into_iter().skip(1) {
        if let Some(t) = l.strip_prefix("title") {
            title = String::from(t.trim());
            continue;
        }
        if let Some((k, v)) = l.rsplit_once(':') {
            if let Ok(v) = v.trim().parse::<f32>() {
                if v > 0.0 {
                    slices.push((String::from(unquote(k)), v));
                }
            }
        }
    }
    if slices.is_empty() {
        return None;
    }
    let total: f32 = slices.iter().map(|s| s.1).sum();
    let r = size * 7.0;
    let top = if title.is_empty() { size } else { size * 3.0 };
    let (cx, cy) = (r + size, top + r);
    let mut items = Vec::new();
    if !title.is_empty() {
        items.push(text(
            cx,
            size * 1.6,
            &title,
            size * 1.1,
            1,
            true,
            Role::Text,
        ));
    }
    let mut a = -90.0f32;
    for (i, (label, v)) in slices.iter().enumerate() {
        let sweep = v / total * 360.0;
        items.push(Item::Wedge {
            cx,
            cy,
            r,
            start: a,
            sweep,
            fill: Role::Series((i % 12) as u8),
        });
        let mid = (a + sweep * 0.5).to_radians();
        let pct = alloc::format!("{:.0}%", v / total * 100.0);
        items.push(text(
            cx + mid.cos() * r * 0.65,
            cy + mid.sin() * r * 0.65 + size * 0.3,
            &pct,
            size * 0.85,
            1,
            false,
            Role::Text,
        ));
        a += sweep;
        let ly = top + i as f32 * size * 1.6 + size;
        let lx = cx + r + size * 2.0;
        items.push(Item::Box {
            x: lx,
            y: ly - size * 0.8,
            w: size * 0.9,
            h: size * 0.9,
            r: 2.0,
            fill: Role::Series((i % 12) as u8),
            stroke: Role::None,
            dashed: false,
        });
        items.push(text(
            lx + size * 1.4,
            ly,
            label,
            size * 0.9,
            0,
            false,
            Role::Text,
        ));
    }
    let legend_w = slices
        .iter()
        .map(|s| width(&s.0, size * 0.9))
        .fold(0.0, f32::max)
        + size * 3.0;
    Some(Chart {
        w: cx + r + size * 2.0 + legend_w,
        h: (cy + r + size).max(top + slices.len() as f32 * size * 1.6 + size),
        items,
    })
}

fn journey(src: &str, width: &dyn Fn(&str, f32) -> f32, size: f32) -> Option<Chart> {
    let mut title = String::new();
    let mut section = String::new();
    let mut tasks: Vec<(String, String, u8)> = Vec::new();
    for l in lines_of(src).into_iter().skip(1) {
        if let Some(t) = l.strip_prefix("title") {
            title = String::from(t.trim());
        } else if let Some(s) = l.strip_prefix("section") {
            section = String::from(s.trim());
        } else if let Some((name, rest)) = l.split_once(':') {
            let score = rest
                .split(':')
                .next()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(3u8);
            tasks.push((
                section.clone(),
                String::from(name.trim()),
                score.clamp(1, 5),
            ));
        }
    }
    if tasks.is_empty() {
        return None;
    }
    let col_w = tasks
        .iter()
        .map(|t| width(&t.1, size * 0.85))
        .fold(size * 6.0, f32::max)
        + size;
    let top = if title.is_empty() { size } else { size * 3.0 };
    let mut items = Vec::new();
    if !title.is_empty() {
        items.push(text(
            size * 0.5,
            size * 1.6,
            &title,
            size * 1.1,
            0,
            true,
            Role::Text,
        ));
    }
    let band = top + size * 2.0;
    let face_y = |s: u8| band + size * 3.0 + (5 - s) as f32 * size * 1.6;
    let mut k = 0;
    while k < tasks.len() {
        let sec = tasks[k].0.clone();
        let mut e = k;
        while e + 1 < tasks.len() && tasks[e + 1].0 == sec {
            e += 1;
        }
        let x = k as f32 * col_w;
        let w = (e - k + 1) as f32 * col_w - size * 0.4;
        items.push(Item::Box {
            x,
            y: top,
            w,
            h: size * 1.6,
            r: 3.0,
            fill: Role::Cluster,
            stroke: Role::None,
            dashed: false,
        });
        items.push(text(
            x + w * 0.5,
            top + size * 1.1,
            &sec,
            size * 0.9,
            1,
            true,
            Role::Text,
        ));
        k = e + 1;
    }
    let mut prev: Option<(f32, f32)> = None;
    for (i, t) in tasks.iter().enumerate() {
        let x = i as f32 * col_w;
        items.push(bx(x, band, col_w - size * 0.4, size * 2.0, 3.0, Role::Node));
        items.push(text(
            x + col_w * 0.5 - size * 0.2,
            band + size * 1.35,
            &t.1,
            size * 0.85,
            1,
            false,
            Role::Text,
        ));
        let (fx, fy) = (x + col_w * 0.5, face_y(t.2));
        if let Some(p) = prev {
            items.push(Item::Line {
                pts: alloc::vec![p, (fx, fy)],
                role: Role::Line,
                width: 1.2,
                dashed: false,
                head: Head::None,
                tail: Head::None,
            });
        }
        items.push(Item::Line {
            pts: alloc::vec![(fx, band + size * 2.0), (fx, fy)],
            role: Role::Muted,
            width: 0.8,
            dashed: true,
            head: Head::None,
            tail: Head::None,
        });
        prev = Some((fx, fy));
        let mood = match t.2 {
            5 | 4 => Role::Done,
            3 => Role::Active,
            _ => Role::Critical,
        };
        items.push(Item::Circle {
            cx: fx,
            cy: fy,
            r: size * 0.7,
            fill: mood,
            stroke: Role::Line,
        });
        items.push(text(
            fx,
            fy + size * 0.3,
            &alloc::format!("{}", t.2),
            size * 0.8,
            1,
            true,
            Role::Text,
        ));
    }
    // The score axis, 5 at the top.
    let mut axis = Vec::new();
    for sc in 1..=5u8 {
        axis.push(text(
            -size * 0.8,
            face_y(sc) + size * 0.3,
            &alloc::format!("{sc}"),
            size * 0.75,
            2,
            false,
            Role::Muted,
        ));
    }
    axis.extend(items);
    let items: Vec<Item> = axis
        .into_iter()
        .map(|i| shift_item(i, size * 1.6, 0.0))
        .collect();
    Some(Chart {
        w: tasks.len() as f32 * col_w + size * 1.6,
        h: face_y(1) + size * 1.5,
        items,
    })
}

fn timeline(src: &str, width: &dyn Fn(&str, f32) -> f32, size: f32) -> Option<Chart> {
    let mut title = String::new();
    let mut periods: Vec<(String, Vec<String>)> = Vec::new();
    for l in lines_of(src).into_iter().skip(1) {
        if let Some(t) = l.strip_prefix("title") {
            title = String::from(t.trim());
            continue;
        }
        if l.starts_with("section") {
            continue;
        }
        let mut parts = l.split(" : ").map(str::trim);
        let first = parts.next().unwrap_or("");
        if let Some(ev) = first.strip_prefix(':') {
            if let Some(p) = periods.last_mut() {
                p.1.push(String::from(ev.trim()));
            }
            continue;
        }
        let evs: Vec<String> = parts.map(String::from).collect();
        periods.push((String::from(first.trim_end_matches(':').trim()), evs));
    }
    if periods.is_empty() {
        return None;
    }
    let col_w = periods
        .iter()
        .flat_map(|p| core::iter::once(&p.0).chain(p.1.iter()))
        .map(|s| width(s, size * 0.85))
        .fold(size * 6.0, f32::max)
        + size * 1.5;
    let top = if title.is_empty() { size } else { size * 3.0 };
    let mut items = Vec::new();
    if !title.is_empty() {
        items.push(text(
            periods.len() as f32 * col_w * 0.5,
            size * 1.6,
            &title,
            size * 1.1,
            1,
            true,
            Role::Text,
        ));
    }
    let axis = top + size * 3.4;
    items.push(Item::Line {
        pts: alloc::vec![(0.0, axis), (periods.len() as f32 * col_w, axis)],
        role: Role::Line,
        width: 2.0,
        dashed: false,
        head: Head::Arrow,
        tail: Head::None,
    });
    let mut deepest = axis;
    for (i, (period, evs)) in periods.iter().enumerate() {
        let x = i as f32 * col_w + size * 0.2;
        let w = col_w - size * 0.4;
        items.push(Item::Box {
            x,
            y: top,
            w,
            h: size * 2.2,
            r: 4.0,
            fill: Role::Series((i % 12) as u8),
            stroke: Role::None,
            dashed: false,
        });
        items.push(text(
            x + w * 0.5,
            top + size * 1.45,
            period,
            size * 0.9,
            1,
            true,
            Role::Text,
        ));
        items.push(Item::Line {
            pts: alloc::vec![
                (x + w * 0.5, top + size * 2.2),
                (
                    x + w * 0.5,
                    axis + size * 1.2 + evs.len() as f32 * size * 2.4
                )
            ],
            role: Role::Muted,
            width: 1.0,
            dashed: true,
            head: Head::None,
            tail: Head::None,
        });
        for (k, ev) in evs.iter().enumerate() {
            let y = axis + size * 1.2 + k as f32 * size * 2.4;
            items.push(Item::Box {
                x,
                y,
                w,
                h: size * 1.9,
                r: 4.0,
                fill: Role::Series((i % 12) as u8),
                stroke: Role::None,
                dashed: false,
            });
            items.push(text(
                x + w * 0.5,
                y + size * 1.25,
                ev,
                size * 0.85,
                1,
                false,
                Role::Text,
            ));
            deepest = deepest.max(y + size * 1.9);
        }
    }
    Some(Chart {
        w: periods.len() as f32 * col_w,
        h: deepest + size,
        items,
    })
}

fn shift_item(i: Item, dx: f32, dy: f32) -> Item {
    match i {
        Item::Turned {
            x,
            y,
            s,
            size,
            turns,
            role,
        } => Item::Turned {
            x: x + dx,
            y: y + dy,
            s,
            size,
            turns,
            role,
        },
        Item::Band {
            pts,
            from,
            to,
            x0,
            x1,
        } => Item::Band {
            pts: pts.into_iter().map(|(x, y)| (x + dx, y + dy)).collect(),
            from,
            to,
            x0: x0 + dx,
            x1: x1 + dx,
        },
        Item::Poly { pts, fill, stroke } => Item::Poly {
            pts: pts.into_iter().map(|(x, y)| (x + dx, y + dy)).collect(),
            fill,
            stroke,
        },
        Item::Box {
            x,
            y,
            w,
            h,
            r,
            fill,
            stroke,
            dashed,
        } => Item::Box {
            x: x + dx,
            y: y + dy,
            w,
            h,
            r,
            fill,
            stroke,
            dashed,
        },
        Item::Diamond {
            x,
            y,
            w,
            h,
            fill,
            stroke,
        } => Item::Diamond {
            x: x + dx,
            y: y + dy,
            w,
            h,
            fill,
            stroke,
        },
        Item::Circle {
            cx,
            cy,
            r,
            fill,
            stroke,
        } => Item::Circle {
            cx: cx + dx,
            cy: cy + dy,
            r,
            fill,
            stroke,
        },
        Item::Wedge {
            cx,
            cy,
            r,
            start,
            sweep,
            fill,
        } => Item::Wedge {
            cx: cx + dx,
            cy: cy + dy,
            r,
            start,
            sweep,
            fill,
        },
        Item::Line {
            pts,
            role,
            width,
            dashed,
            head,
            tail,
        } => Item::Line {
            pts: pts.into_iter().map(|(x, y)| (x + dx, y + dy)).collect(),
            role,
            width,
            dashed,
            head,
            tail,
        },
        Item::Text {
            x,
            y,
            s,
            size,
            anchor,
            bold,
            role,
        } => Item::Text {
            x: x + dx,
            y: y + dy,
            s,
            size,
            anchor,
            bold,
            role,
        },
    }
}

// ---- mindmap ------------------------------------------------------------------------------------

fn mindmap(src: &str, width: &dyn Fn(&str, f32) -> f32, size: f32) -> Option<Chart> {
    // (depth by indentation, label)
    let mut nodes: Vec<(usize, String)> = Vec::new();
    for raw in src.lines().skip_while(|l| l.trim() != "mindmap").skip(1) {
        if raw.trim().is_empty() || raw.trim().starts_with("%%") || raw.trim().starts_with("::") {
            continue;
        }
        let ind = raw.len() - raw.trim_start().len();
        let t = raw.trim();
        // id((text)), id[text], id(text): the text inside.
        let label = match t.find(['(', '[', '{']) {
            Some(p) => t[p..].trim_matches(|c| matches!(c, '(' | ')' | '[' | ']' | '{' | '}')),
            None => t,
        };
        nodes.push((ind, String::from(label)));
    }
    if nodes.is_empty() {
        return None;
    }
    // Depth from indentation steps.
    let mut levels: Vec<usize> = Vec::new();
    let mut depth = Vec::new();
    for (ind, _) in &nodes {
        while levels.last().is_some_and(|l| l >= ind) {
            levels.pop();
        }
        depth.push(levels.len());
        levels.push(*ind);
    }
    // Parents; the root's branches go right and left of it, as Mermaid
    // spreads them, each side a tidy tree (leaves stacked, a parent centred
    // on its children).
    let mut parent = alloc::vec![usize::MAX; nodes.len()];
    for i in 1..nodes.len() {
        parent[i] = (0..i).rev().find(|j| depth[*j] < depth[i]).unwrap_or(0);
    }
    let kids_of =
        |i: usize| -> Vec<usize> { (0..parent.len()).filter(|k| parent[*k] == i).collect() };
    let top_kids = kids_of(0);
    let split = top_kids.len().div_ceil(2);
    let mut side = alloc::vec![1i8; nodes.len()];
    for (k, &c) in top_kids.iter().enumerate() {
        let sd = if k < split { 1 } else { -1 };
        let mut stack = alloc::vec![c];
        while let Some(n) = stack.pop() {
            side[n] = sd;
            stack.extend(kids_of(n));
        }
    }
    let row = size * 2.6;
    let mut y_of = alloc::vec![0.0f32; nodes.len()];
    fn place(i: usize, parent: &[usize], y_of: &mut [f32], next: &mut f32, row: f32) -> f32 {
        let kids: Vec<usize> = (0..parent.len()).filter(|k| parent[*k] == i).collect();
        let y = if kids.is_empty() {
            let y = *next;
            *next += row;
            y
        } else {
            let ys: Vec<f32> = kids
                .iter()
                .map(|k| place(*k, parent, y_of, next, row))
                .collect();
            (ys.first().copied().unwrap_or(0.0) + ys.last().copied().unwrap_or(0.0)) * 0.5
        };
        y_of[i] = y;
        y
    }
    let (mut right_h, mut left_h) = (0.0f32, 0.0f32);
    for (k, &c) in top_kids.iter().enumerate() {
        if k < split {
            place(c, &parent, &mut y_of, &mut right_h, row);
        } else {
            place(c, &parent, &mut y_of, &mut left_h, row);
        }
    }
    // Centre the shorter side against the taller.
    let tall = right_h.max(left_h).max(row);
    for i in 1..nodes.len() {
        let h = if side[i] > 0 { right_h } else { left_h };
        y_of[i] += (tall - h) * 0.5;
    }
    let ys: Vec<f32> = top_kids.iter().map(|c| y_of[*c]).collect();
    y_of[0] = if ys.is_empty() {
        0.0
    } else {
        (tall - row) * 0.5
    };
    let maxd = depth.iter().copied().max().unwrap_or(0);
    let mut col_w = alloc::vec![0.0f32; maxd + 1];
    for (i, (_, l)) in nodes.iter().enumerate() {
        col_w[depth[i]] = col_w[depth[i]].max(width(l, size) + size * 2.0);
    }
    let gap = size * 2.5;
    // x of each node's near edge, measured out from the root's centre.
    let mut reach = alloc::vec![0.0f32; maxd + 1];
    for d in 1..=maxd {
        reach[d] = reach[d - 1] + if d == 1 { col_w[0] * 0.5 } else { col_w[d - 1] } + gap;
    }
    let node_w = |i: usize| width(&nodes[i].1, size) + size * 2.0;
    let x_of = |i: usize| -> f32 {
        if i == 0 {
            return -node_w(0) * 0.5;
        }
        let d = depth[i];
        if side[i] > 0 {
            reach[d]
        } else {
            -reach[d] - node_w(i)
        }
    };
    let min_x = (0..nodes.len()).map(x_of).fold(0.0f32, f32::min);
    let max_x = (0..nodes.len())
        .map(|i| x_of(i) + node_w(i))
        .fold(0.0f32, f32::max);
    let mut items = Vec::new();
    for i in 1..nodes.len() {
        let p = parent[i];
        let (px0, pw) = (x_of(p), node_w(p));
        let (cx0, cw) = (x_of(i), node_w(i));
        let (x0, x1) = if side[i] > 0 {
            (px0 + pw, cx0)
        } else {
            (px0, cx0 + cw)
        };
        let (y0, y1) = (y_of[p] + size * 1.1, y_of[i] + size * 1.1);
        let mid = (x0 + x1) * 0.5;
        items.push(Item::Line {
            pts: alloc::vec![
                (x0 - min_x, y0),
                (mid - min_x, y0),
                (mid - min_x, y1),
                (x1 - min_x, y1)
            ],
            role: Role::Series(((i % 11) + 1) as u8),
            width: 2.0,
            dashed: false,
            head: Head::None,
            tail: Head::None,
        });
    }
    for (i, (_, l)) in nodes.iter().enumerate() {
        let w = node_w(i);
        let x = x_of(i) - min_x;
        let (fill, r) = if i == 0 {
            (Role::Node, size * 1.1)
        } else {
            (Role::None, 5.0)
        };
        items.push(Item::Box {
            x,
            y: y_of[i],
            w,
            h: size * 2.2,
            r,
            fill,
            stroke: if i == 0 { Role::Line } else { Role::None },
            dashed: false,
        });
        items.push(text(
            x + w * 0.5,
            y_of[i] + size * 1.45,
            l,
            size,
            1,
            i == 0,
            Role::Text,
        ));
    }
    Some(Chart {
        w: max_x - min_x,
        h: tall,
        items,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str, size: f32) -> f32 {
        s.chars().count() as f32 * size * 0.5
    }

    fn texts(c: &Chart) -> Vec<String> {
        c.items
            .iter()
            .filter_map(|i| match i {
                Item::Text { s, .. } => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn every_kind_draws() {
        let cases = [
            ("graph TD; A-->B;", "A"),
            ("sequenceDiagram\nparticipant A as Alice\nactor B\nA->>B: Hello\nloop Every minute\nB-->>A: Hi\nend\nNote right of B: thinks", "Hello"),
            ("classDiagram\nclass Animal {\n  +String name\n  +eat()\n}\nAnimal <|-- Duck\nDuck : +swim()", "+swim()"),
            ("stateDiagram-v2\n[*] --> Still\nStill --> Moving : push\nMoving --> [*]", "push"),
            ("erDiagram\nCUSTOMER ||--o{ ORDER : places\nORDER {\n  int id\n}", "places"),
            ("gantt\ntitle Plan\ndateFormat YYYY-MM-DD\nsection Build\nDesign :done, des, 2026-01-01, 5d\nCode :active, after des, 10d\nsection Ship\nLaunch :crit, 2026-01-20, 2d", "Code"),
            ("pie title Pets\n\"Dogs\" : 386\n\"Cats\" : 85", "Dogs"),
            ("journey\ntitle My day\nsection Work\nMake tea: 5: Me\nDo work: 1: Me", "Make tea"),
            ("mindmap\n  root((Krate))\n    Apps\n      Vault\n    Runtime", "Vault"),
            ("timeline\ntitle History\n2002 : LinkedIn\n2004 : Facebook : Google", "Google"),
        ];
        for (src, want) in cases {
            let c = chart(src, &w, 14.0, 700.0).unwrap_or_else(|| panic!("{src}"));
            assert!(c.w > 0.0 && c.h > 0.0, "{src}");
            assert!(
                texts(&c).iter().any(|t| t == want),
                "{want} in {:?}",
                texts(&c)
            );
        }
        assert!(chart("gitGraph\ncommit", &w, 14.0, 700.0).is_some());
        assert!(chart("zenuml\nA.b()", &w, 14.0, 700.0).is_none());
        assert_eq!(kind("sequenceDiagram\nA->>B: x"), Some("sequence"));
    }

    #[test]
    fn layouts_are_in_order() {
        let s = chart("sequenceDiagram\nA->>B: one\nB->>A: two", &w, 14.0, 700.0).unwrap();
        let ys: Vec<f32> = s
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Text { s, y, .. } if s == "one" || s == "two" => Some(*y),
                _ => None,
            })
            .collect();
        assert!(ys[0] < ys[1], "messages go down the page");
        let p = chart("pie\n\"A\" : 3\n\"B\" : 1", &w, 14.0, 700.0).unwrap();
        let sweeps: Vec<f32> = p
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Wedge { sweep, .. } => Some(*sweep),
                _ => None,
            })
            .collect();
        assert_eq!(sweeps, [270.0, 90.0]);
        let g = chart(
            "gantt\nsection S\nA : a1, 2026-01-01, 3d\nB : after a1, 2d",
            &w,
            14.0,
            700.0,
        )
        .unwrap();
        let bars: Vec<(f32, f32)> = g
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Box { x, w, r, .. } if *r == 3.0 => Some((*x, *w)),
                _ => None,
            })
            .collect();
        assert!(
            (bars[0].0 + bars[0].1 - bars[1].0).abs() < 0.5,
            "B starts where A ends: {bars:?}"
        );
        assert_eq!(days("1970-01-02"), Some(1));
        assert_eq!(date_of(days("2026-11-22").unwrap()), "2026-11-22");
    }
}
