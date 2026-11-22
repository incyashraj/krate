//! Mermaid flowcharts (` ```mermaid ` blocks starting `graph` or
//! `flowchart`), drawn as Obsidian draws them: boxes in ranks, top to
//! bottom or left to right, joined by arrows with their labels.
//!
//! Only flowcharts. Sequence, class, state, Gantt and pie diagrams return
//! `None` and stay a code block; that is said in parity.md.

use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Rect,
    Round,
    Stadium,
    Circle,
    Diamond,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: String,
    pub label: String,
    pub shape: Shape,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub label: String,
    pub dotted: bool,
    pub thick: bool,
    pub arrow: bool,
    /// The segment drawn, from border to border.
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Diagram {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub w: f32,
    pub h: f32,
}

/// A node as written: its id, its label and shape if given, what follows.
type NodeRef<'a> = (String, Option<(String, Shape)>, &'a str);

fn node_ref(s: &str) -> Option<NodeRef<'_>> {
    let s = s.trim_start();
    let id_len = s
        .char_indices()
        .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
        .map(|(i, c)| i + c.len_utf8())
        .last()?;
    let id = String::from(s.get(..id_len)?);
    let rest = s.get(id_len..).unwrap_or("");
    let shapes: [(&str, &str, Shape); 8] = [
        ("((", "))", Shape::Circle),
        ("([", "])", Shape::Stadium),
        ("[[", "]]", Shape::Rect),
        ("[(", ")]", Shape::Rect),
        ("[", "]", Shape::Rect),
        ("(", ")", Shape::Round),
        ("{", "}", Shape::Diamond),
        (">", "]", Shape::Rect),
    ];
    for (open, close, shape) in shapes {
        if let Some(inner) = rest.strip_prefix(open) {
            if let Some(end) = inner.find(close) {
                let label = inner.get(..end).unwrap_or("").trim().trim_matches('"');
                return Some((
                    id,
                    Some((String::from(label), shape)),
                    inner.get(end + close.len()..).unwrap_or(""),
                ));
            }
        }
    }
    Some((id, None, rest))
}

/// An edge at the start of `s`: (label, dotted, thick, arrow, rest).
/// An edge as written: label, dotted, thick, arrow, what follows.
type EdgeRef<'a> = (String, bool, bool, bool, &'a str);

fn edge(s: &str) -> Option<EdgeRef<'_>> {
    let s = s.trim_start();
    // `-- text -->` puts the label inside the arrow.
    if let Some(after) = s.strip_prefix("-- ") {
        if let Some(i) = after.find("-->") {
            let label = String::from(after.get(..i).unwrap_or("").trim());
            return Some((label, false, false, true, after.get(i + 3..).unwrap_or("")));
        }
    }
    let kinds: [(&str, bool, bool, bool); 6] = [
        ("-.->", true, false, true),
        ("==>", false, true, true),
        ("-->", false, false, true),
        ("-.-", true, false, false),
        ("===", false, true, false),
        ("---", false, false, false),
    ];
    for (tok, dotted, thick, arrow) in kinds {
        if let Some(rest) = s.strip_prefix(tok) {
            let rest = rest.trim_start();
            if let Some(lab) = rest.strip_prefix('|') {
                if let Some(end) = lab.find('|') {
                    let label = String::from(lab.get(..end).unwrap_or("").trim());
                    return Some((
                        label,
                        dotted,
                        thick,
                        arrow,
                        lab.get(end + 1..).unwrap_or(""),
                    ));
                }
            }
            return Some((String::new(), dotted, thick, arrow, rest));
        }
    }
    None
}

/// Read and lay out a flowchart. `width` measures a label at the size it
/// is drawn.
pub fn flowchart(src: &str, width: &dyn Fn(&str) -> f32, size: f32) -> Option<Diagram> {
    let mut lines = src
        .split(['\n', ';'])
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("%%"));
    let head = lines.next()?;
    let mut words = head.split_whitespace();
    let kind = words.next()?;
    if kind != "graph" && kind != "flowchart" {
        return None;
    }
    let dir = words.next().unwrap_or("TD");
    let across = matches!(dir, "LR" | "RL");
    let mut nodes: Vec<Node> = Vec::new();
    let mut edges: Vec<Edge> = Vec::new();
    let find_or_add = |nodes: &mut Vec<Node>, id: String, def: Option<(String, Shape)>| -> usize {
        if let Some(i) = nodes.iter().position(|n| n.id == id) {
            if let (Some((label, shape)), Some(n)) = (def, nodes.get_mut(i)) {
                n.label = label;
                n.shape = shape;
            }
            return i;
        }
        let (label, shape) = def.unwrap_or_else(|| (id.clone(), Shape::Rect));
        nodes.push(Node {
            id,
            label,
            shape,
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
        });
        nodes.len() - 1
    };
    for line in lines {
        let first = line.split_whitespace().next().unwrap_or("");
        if matches!(
            first,
            "style"
                | "classDef"
                | "class"
                | "click"
                | "linkStyle"
                | "subgraph"
                | "end"
                | "direction"
        ) {
            continue;
        }
        let Some((id, def, mut rest)) = node_ref(line) else {
            continue;
        };
        let mut prev = find_or_add(&mut nodes, id, def);
        while let Some((label, dotted, thick, arrow, after)) = edge(rest) {
            let Some((id2, def2, after2)) = node_ref(after) else {
                break;
            };
            let next = find_or_add(&mut nodes, id2, def2);
            edges.push(Edge {
                from: prev,
                to: next,
                label,
                dotted,
                thick,
                arrow,
                x0: 0.0,
                y0: 0.0,
                x1: 0.0,
                y1: 0.0,
            });
            prev = next;
            rest = after2;
        }
    }
    if nodes.is_empty() {
        return None;
    }
    // Ranks: the longest path from the top, after setting aside the edges
    // that close a cycle (found by a walk in the order the nodes appear).
    let n = nodes.len();
    let mut back = alloc::vec![false; edges.len()];
    let mut state = alloc::vec![0u8; n]; // 0 new, 1 on the walk, 2 done
    for root in 0..n {
        if state.get(root).copied() != Some(0) {
            continue;
        }
        let mut stack: Vec<(usize, usize)> = alloc::vec![(root, 0)];
        if let Some(s) = state.get_mut(root) {
            *s = 1;
        }
        while let Some(&(v, k)) = stack.last() {
            let next = edges.iter().enumerate().filter(|(_, e)| e.from == v).nth(k);
            if let Some(top) = stack.last_mut() {
                top.1 += 1;
            }
            match next {
                Some((ei, e)) => match state.get(e.to).copied() {
                    Some(0) => {
                        if let Some(s) = state.get_mut(e.to) {
                            *s = 1;
                        }
                        stack.push((e.to, 0));
                    }
                    Some(1) => {
                        if let Some(b) = back.get_mut(ei) {
                            *b = true;
                        }
                    }
                    _ => {}
                },
                None => {
                    if let Some(s) = state.get_mut(v) {
                        *s = 2;
                    }
                    stack.pop();
                }
            }
        }
    }
    let mut rank = alloc::vec![0usize; n];
    for _ in 0..n {
        let mut changed = false;
        for (ei, e) in edges.iter().enumerate() {
            if back.get(ei).copied().unwrap_or(false) || e.from == e.to {
                continue;
            }
            let want = rank.get(e.from).copied().unwrap_or(0) + 1;
            if let Some(r) = rank.get_mut(e.to) {
                if *r < want {
                    *r = want;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    let pad_x = size * 1.0;
    let pad_y = size * 0.75;
    for node in nodes.iter_mut() {
        let tw = width(&node.label);
        let (w, h) = match node.shape {
            Shape::Circle => {
                let d = tw + pad_x * 1.2;
                (d, d)
            }
            Shape::Diamond => (tw + pad_x * 2.6, size * 1.4 + pad_y * 2.6),
            _ => (tw + pad_x * 2.0, size * 1.4 + pad_y * 2.0),
        };
        node.w = w;
        node.h = h;
    }
    let ranks = rank.iter().copied().max().unwrap_or(0) + 1;
    let gap_main = size * 3.0;
    let gap_cross = size * 1.6;
    // The size of each rank along the main axis, and its total across.
    let mut main_size = alloc::vec![0.0f32; ranks];
    let mut cross_total = alloc::vec![0.0f32; ranks];
    for (i, node) in nodes.iter().enumerate() {
        let r = rank.get(i).copied().unwrap_or(0);
        let (m, c) = if across {
            (node.w, node.h)
        } else {
            (node.h, node.w)
        };
        if let Some(s) = main_size.get_mut(r) {
            *s = s.max(m);
        }
        if let Some(t) = cross_total.get_mut(r) {
            *t += c + if *t > 0.0 { gap_cross } else { 0.0 };
        }
    }
    let widest = cross_total.iter().copied().fold(0.0f32, f32::max);
    let mut cursor = alloc::vec![0.0f32; ranks];
    for (r, c) in cursor.iter_mut().enumerate() {
        *c = (widest - cross_total.get(r).copied().unwrap_or(0.0)) * 0.5;
    }
    let mut main_at = alloc::vec![0.0f32; ranks];
    let mut acc = 0.0;
    for r in 0..ranks {
        if let Some(m) = main_at.get_mut(r) {
            *m = acc;
        }
        acc += main_size.get(r).copied().unwrap_or(0.0) + gap_main;
    }
    let total_main = acc - gap_main;
    let reverse = matches!(dir, "BT" | "RL");
    for (i, node) in nodes.iter_mut().enumerate() {
        let r = rank.get(i).copied().unwrap_or(0);
        let ms = main_size.get(r).copied().unwrap_or(0.0);
        let (m, c) = if across {
            (node.w, node.h)
        } else {
            (node.h, node.w)
        };
        let mut main = main_at.get(r).copied().unwrap_or(0.0) + (ms - m) * 0.5;
        if reverse {
            main = total_main - main - m;
        }
        let cross = cursor.get(r).copied().unwrap_or(0.0);
        if let Some(cu) = cursor.get_mut(r) {
            *cu += c + gap_cross;
        }
        if across {
            node.x = main;
            node.y = cross;
        } else {
            node.x = cross;
            node.y = main;
        }
    }
    // Each edge from the border of one box toward the centre of the other.
    for e in edges.iter_mut() {
        let (Some(a), Some(b)) = (nodes.get(e.from), nodes.get(e.to)) else {
            continue;
        };
        let (ax, ay) = (a.x + a.w * 0.5, a.y + a.h * 0.5);
        let (bx, by) = (b.x + b.w * 0.5, b.y + b.h * 0.5);
        let clip = |cx: f32, cy: f32, w: f32, h: f32, dx: f32, dy: f32| -> (f32, f32) {
            let tx = if dx.abs() > 1e-3 {
                (w * 0.5) / dx.abs()
            } else {
                f32::MAX
            };
            let ty = if dy.abs() > 1e-3 {
                (h * 0.5) / dy.abs()
            } else {
                f32::MAX
            };
            let t = tx.min(ty).min(1.0);
            (cx + dx * t, cy + dy * t)
        };
        let (dx, dy) = (bx - ax, by - ay);
        let (x0, y0) = clip(ax, ay, a.w, a.h, dx, dy);
        let (x1, y1) = clip(bx, by, b.w, b.h, -dx, -dy);
        e.x0 = x0;
        e.y0 = y0;
        e.x1 = x1;
        e.y1 = y1;
    }
    let (w, h) = if across {
        (total_main, widest)
    } else {
        (widest, total_main)
    };
    Some(Diagram { nodes, edges, w, h })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> f32 {
        s.chars().count() as f32 * 8.0
    }

    #[test]
    fn flowcharts_rank_their_boxes() {
        let d = flowchart("graph TD; A-->B;", &w, 16.0).unwrap();
        assert_eq!(d.nodes.len(), 2);
        assert!(d.nodes[1].y > d.nodes[0].y + d.nodes[0].h, "B below A");
        assert!((d.edges[0].y0 - (d.nodes[0].y + d.nodes[0].h)).abs() < 0.5);
        let lr = flowchart(
            "flowchart LR\n  start([Start]) --> check{Ready?}\n  check -->|yes| go[Go]\n  check -- no --> wait(Wait)\n  wait -.-> check",
            &w,
            16.0,
        )
        .unwrap();
        let names: Vec<&str> = lr.nodes.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(names, ["Start", "Ready?", "Go", "Wait"]);
        assert_eq!(lr.nodes[1].shape, Shape::Diamond);
        assert_eq!(lr.nodes[0].shape, Shape::Stadium);
        assert!(lr.nodes[2].x > lr.nodes[1].x, "left to right");
        assert_eq!(lr.edges[1].label, "yes");
        assert_eq!(lr.edges[2].label, "no");
        assert!(lr.edges[3].dotted);
        let chain = flowchart("graph TD\nA --> B --> C\nstyle A fill:#f9f", &w, 16.0).unwrap();
        assert_eq!(chain.edges.len(), 2);
        assert!(chain.nodes[2].y > chain.nodes[1].y);
        assert!(flowchart("sequenceDiagram\nA->>B: hi", &w, 16.0).is_none());
        assert!(flowchart("graph TD", &w, 16.0).is_none());
    }
}
