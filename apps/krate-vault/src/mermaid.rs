//! Mermaid flowcharts (` ```mermaid ` blocks starting `graph` or
//! `flowchart`), read into boxes and edges and laid out as Mermaid's dagre
//! lays them out (src/layered.rs): ranks top to bottom or left to right,
//! the order with fewest crossings, curved edges with their labels.
//! The other kinds are in src/charts.rs.

use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Rect,
    Round,
    Stadium,
    Circle,
    Diamond,
    /// `[(Store)]`: a database drum.
    Cylinder,
    /// `[[Step]]`: a box with a line down each side.
    Subroutine,
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
    /// The curve drawn, border to border.
    pub pts: Vec<(f32, f32)>,
    /// Where its label is centred, and how wide its patch is.
    pub label_at: (f32, f32),
    pub label_w: f32,
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
        ("[[", "]]", Shape::Subroutine),
        ("[(", ")]", Shape::Cylinder),
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
                pts: Vec::new(),
                label_at: (0.0, 0.0),
                label_w: 0.0,
            });
            prev = next;
            rest = after2;
        }
    }
    if nodes.is_empty() {
        return None;
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
            // Mermaid 11's rhombus: twice a box's height, the label's width
            // and most of that height again across (measured in Obsidian).
            Shape::Diamond => {
                let h = (size * 1.4 + pad_y * 2.0) * 2.0;
                (tw + h * 0.68, h)
            }
            Shape::Cylinder => (tw + pad_x * 2.0, size * 1.4 + pad_y * 2.0 + size * 0.9),
            Shape::Subroutine => (tw + pad_x * 3.0, size * 1.4 + pad_y * 2.0),
            _ => (tw + pad_x * 2.0, size * 1.4 + pad_y * 2.0),
        };
        node.w = w;
        node.h = h;
    }
    // Laid out as dagre lays out Mermaid's (src/layered.rs), with Mermaid's
    // 50 px between nodes and ranks at its 16 px text.
    let sizes: Vec<(f32, f32)> = nodes.iter().map(|n| (n.w, n.h)).collect();
    let pairs: Vec<(usize, usize)> = edges.iter().map(|e| (e.from, e.to)).collect();
    let labels: Vec<(f32, f32)> = edges
        .iter()
        .map(|e| {
            if e.label.is_empty() {
                (0.0, 0.0)
            } else {
                (width(&e.label) * 0.85 + size * 0.6, size * 1.5)
            }
        })
        .collect();
    let lay = crate::layered::layout(&crate::layered::Input {
        sizes: &sizes,
        edges: &pairs,
        labels: &labels,
        shapes: &nodes
            .iter()
            .map(|n| match n.shape {
                Shape::Diamond => 1,
                Shape::Circle => 2,
                _ => 0,
            })
            .collect::<Vec<u8>>(),
        dir: crate::layered::Dir::of(dir),
        node_sep: size * 3.125,
        rank_sep: size * 3.125,
        label_gap: size * 1.25,
    });
    for (node, p) in nodes.iter_mut().zip(lay.pos.iter()) {
        node.x = p.0;
        node.y = p.1;
    }
    for (i, e) in edges.iter_mut().enumerate() {
        let pts = lay.paths.get(i).cloned().unwrap_or_default();
        if let (Some(f), Some(l)) = (pts.first(), pts.last()) {
            e.x0 = f.0;
            e.y0 = f.1;
            e.x1 = l.0;
            e.y1 = l.1;
        }
        e.label_at = lay.label_at.get(i).copied().unwrap_or((0.0, 0.0));
        e.label_w = labels.get(i).map_or(0.0, |l| l.0);
        e.pts = pts;
    }
    let (w, h) = (lay.w, lay.h);
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
