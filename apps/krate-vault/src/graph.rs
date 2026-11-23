//! The graph view: every note a dot, every link a line, laid out by a
//! force simulation (links pull, dots push apart, a weak pull to the
//! middle) and drawn into a picture of its own.
//!
//! It has to stay alive at 10,000 notes, so the push between dots is
//! Barnes-Hut (a quadtree: far groups of dots act as one), the simulation
//! runs a few steps per frame and cools until it stops, and the lines and
//! dots are drawn here into one RGBA buffer the guest hands over in a
//! single call -- tens of thousands of lines as one picture, not tens of
//! thousands of host calls.

use crate::json::Value;
use crate::text::lower;
use crate::vault::Vault;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Note,
    /// A link to a note that is not there.
    Unresolved,
    Tag,
    /// A picture, PDF, sound or other file, when attachments are shown.
    Attachment,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub x: f32,
    pub y: f32,
    vx: f32,
    vy: f32,
    pub kind: Kind,
    pub label: String,
    /// The file it is, for a click to open.
    pub path: Option<String>,
    pub degree: u32,
    /// A colour group's colour (`0xRRGGBB`), if one matches.
    pub color: Option<u32>,
    /// Held where it is (being dragged).
    pub fixed: bool,
}

impl Node {
    /// The dot's radius in graph units.
    pub fn radius(&self, size: f32) -> f32 {
        (3.0 + 1.6 * (self.degree as f32).sqrt()) * size
    }
}

/// What the person set in the graph's controls (`graph.json`).
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub search: String,
    pub show_tags: bool,
    pub show_attachments: bool,
    pub hide_unresolved: bool,
    pub show_orphans: bool,
    pub show_arrow: bool,
    pub text_fade: f32,
    pub node_size: f32,
    pub line_size: f32,
    pub center: f32,
    pub repel: f32,
    pub link: f32,
    pub distance: f32,
    /// Colour groups: a search and an `0xRRGGBB` colour.
    pub groups: Vec<(String, u32)>,
    pub open_filters: bool,
    pub open_groups: bool,
    pub open_display: bool,
    pub open_forces: bool,
    /// A local graph's filters: notes that link to it, that it links to,
    /// and the links among those notes.
    pub incoming: bool,
    pub outgoing: bool,
    pub neighbors: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            search: String::new(),
            show_tags: false,
            show_attachments: false,
            hide_unresolved: false,
            show_orphans: true,
            show_arrow: false,
            text_fade: 0.0,
            node_size: 1.0,
            line_size: 1.0,
            center: 0.518_713_25,
            repel: 10.0,
            link: 1.0,
            distance: 250.0,
            groups: Vec::new(),
            open_filters: false,
            open_groups: false,
            open_display: false,
            open_forces: false,
            incoming: true,
            outgoing: true,
            neighbors: true,
        }
    }
}

impl Options {
    /// Read `graph.json`.
    pub fn from_json(v: &Value) -> Options {
        let d = Options::default();
        let b = |k: &str, or: bool| v.get_bool(k).unwrap_or(or);
        let n = |k: &str, or: f32| v.get_num(k).map_or(or, |x| x as f32);
        Options {
            search: String::from(v.get_str("search").unwrap_or("")),
            show_tags: b("showTags", d.show_tags),
            show_attachments: b("showAttachments", d.show_attachments),
            hide_unresolved: b("hideUnresolved", d.hide_unresolved),
            show_orphans: b("showOrphans", d.show_orphans),
            show_arrow: b("showArrow", d.show_arrow),
            text_fade: n("textFadeMultiplier", d.text_fade),
            node_size: n("nodeSizeMultiplier", d.node_size),
            line_size: n("lineSizeMultiplier", d.line_size),
            center: n("centerStrength", d.center),
            repel: n("repelStrength", d.repel),
            link: n("linkStrength", d.link),
            distance: n("linkDistance", d.distance),
            groups: v
                .get("colorGroups")
                .map(|g| {
                    g.arr()
                        .iter()
                        .filter_map(|e| {
                            let q = e.get_str("query")?;
                            let rgb = e.get("color")?.get_num("rgb")? as u32;
                            Some((String::from(q), rgb))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            open_filters: !b("collapse-filter", true),
            open_groups: !b("collapse-color-groups", true),
            open_display: !b("collapse-display", true),
            open_forces: !b("collapse-forces", true),
            incoming: true,
            outgoing: true,
            neighbors: true,
        }
    }

    /// Write back into `graph.json`, keeping the keys it does not know.
    pub fn to_json(&self, v: &mut Value) {
        let s = |v: &mut Value, k: &str, x: Value| v.set(k, x);
        s(v, "collapse-filter", Value::Bool(!self.open_filters));
        s(v, "search", Value::s(&self.search));
        s(v, "showTags", Value::Bool(self.show_tags));
        s(v, "showAttachments", Value::Bool(self.show_attachments));
        s(v, "hideUnresolved", Value::Bool(self.hide_unresolved));
        s(v, "showOrphans", Value::Bool(self.show_orphans));
        s(v, "collapse-color-groups", Value::Bool(!self.open_groups));
        let groups = self
            .groups
            .iter()
            .map(|(q, rgb)| {
                let mut c = Value::obj();
                c.set("a", Value::Num(1.0));
                c.set("rgb", Value::Num(f64::from(*rgb)));
                let mut g = Value::obj();
                g.set("query", Value::s(q));
                g.set("color", c);
                g
            })
            .collect();
        s(v, "colorGroups", Value::Arr(groups));
        s(v, "collapse-display", Value::Bool(!self.open_display));
        s(v, "showArrow", Value::Bool(self.show_arrow));
        s(
            v,
            "textFadeMultiplier",
            Value::Num(f64::from(self.text_fade)),
        );
        s(
            v,
            "nodeSizeMultiplier",
            Value::Num(f64::from(self.node_size)),
        );
        s(
            v,
            "lineSizeMultiplier",
            Value::Num(f64::from(self.line_size)),
        );
        s(v, "collapse-forces", Value::Bool(!self.open_forces));
        s(v, "centerStrength", Value::Num(f64::from(self.center)));
        s(v, "repelStrength", Value::Num(f64::from(self.repel)));
        s(v, "linkStrength", Value::Num(f64::from(self.link)));
        s(v, "linkDistance", Value::Num(f64::from(self.distance)));
    }
}

/// A search the graph understands: words in the path, `path:`, `file:`,
/// `tag:#x`, `-word` to leave out.
pub fn matches(query: &str, path: &str, tags: &[String]) -> bool {
    let p = lower(path);
    for raw in query.split_whitespace() {
        let (neg, term) = match raw.strip_prefix('-') {
            Some(t) => (true, t),
            None => (false, raw),
        };
        let term = lower(term.trim_matches('"'));
        let hit = if let Some(t) = term.strip_prefix("tag:") {
            let t = t.trim_start_matches('#');
            tags.iter().any(|x| {
                let x = lower(x);
                x == t
                    || x.starts_with(&{
                        let mut s = String::from(t);
                        s.push('/');
                        s
                    })
            })
        } else if let Some(t) = term.strip_prefix("path:") {
            p.contains(t)
        } else if let Some(t) = term.strip_prefix("file:") {
            lower(crate::vfs::file_name(path)).contains(t)
        } else {
            p.contains(term.as_str())
        };
        if hit == neg {
            return false;
        }
    }
    true
}

pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<(u32, u32)>,
    pub adj: Vec<Vec<u32>>,
    pub alpha: f32,
    /// The vault generation it was built from.
    pub generation: u64,
}

impl Graph {
    /// The graph of the whole vault (`local: None`), or of the notes within
    /// `depth` links of one note.
    pub fn build(v: &Vault, opt: &Options, local: Option<(&str, u32)>) -> Graph {
        let mut ids: BTreeMap<String, u32> = BTreeMap::new();
        let mut nodes: Vec<Node> = Vec::new();
        let mut edges: Vec<(u32, u32)> = Vec::new();
        // Which way each link goes, for a local graph's in and out filters.
        let mut dir: Vec<(u32, u32)> = Vec::new();
        let empty = Vec::new();
        let add = |nodes: &mut Vec<Node>,
                   ids: &mut BTreeMap<String, u32>,
                   key: String,
                   kind: Kind,
                   label: String,
                   path: Option<String>|
         -> u32 {
            if let Some(&i) = ids.get(&key) {
                return i;
            }
            let i = nodes.len() as u32;
            nodes.push(Node {
                x: 0.0,
                y: 0.0,
                vx: 0.0,
                vy: 0.0,
                kind,
                label,
                path,
                degree: 0,
                color: None,
                fixed: false,
            });
            ids.insert(key, i);
            i
        };
        for n in &v.notes {
            let attachment = crate::vault::is_attachment(&n.path);
            let shown = n.path.ends_with(".md")
                || n.path.ends_with(".canvas")
                || (attachment && opt.show_attachments);
            if !shown {
                continue;
            }
            let tags = v.meta.get(&n.path).map_or(&empty, |m| &m.tags);
            if !opt.search.trim().is_empty() && !matches(&opt.search, &n.path, tags) {
                continue;
            }
            let label = if n.path.ends_with(".md") {
                n.name.clone()
            } else {
                String::from(crate::vfs::file_name(&n.path))
            };
            add(
                &mut nodes,
                &mut ids,
                n.path.clone(),
                if attachment && !n.path.ends_with(".canvas") {
                    Kind::Attachment
                } else {
                    Kind::Note
                },
                label,
                Some(n.path.clone()),
            );
        }
        for n in &v.notes {
            let Some(&from) = ids.get(&n.path) else {
                continue;
            };
            let Some(m) = v.meta.get(&n.path) else {
                continue;
            };
            for (target, _) in &m.links {
                let to = match v.resolve(target) {
                    Some(t) => match v.notes.get(t).and_then(|t| ids.get(&t.path)) {
                        Some(&i) => i,
                        None => continue,
                    },
                    None => {
                        if opt.hide_unresolved {
                            continue;
                        }
                        let mut key = String::from("?");
                        key.push_str(target);
                        let label = String::from(target.rsplit('/').next().unwrap_or(target));
                        add(&mut nodes, &mut ids, key, Kind::Unresolved, label, None)
                    }
                };
                if to != from {
                    edges.push((from.min(to), from.max(to)));
                    dir.push((from, to));
                }
            }
            if opt.show_tags {
                for t in &m.tags {
                    let mut key = String::from("#");
                    key.push_str(&lower(t));
                    let mut label = String::from("#");
                    label.push_str(t);
                    let to = add(&mut nodes, &mut ids, key, Kind::Tag, label, None);
                    edges.push((from.min(to), from.max(to)));
                    dir.push((from, to));
                }
            }
        }
        edges.sort_unstable();
        edges.dedup();
        dir.sort_unstable();
        dir.dedup();
        let mut g = Graph::from_parts(nodes, edges);
        if let Some((path, depth)) = local {
            g.keep_near(path, depth, &dir, opt);
        }
        if !opt.show_orphans {
            g.keep(|_, n, deg| deg > 0 || n.kind != Kind::Note);
        }
        for n in g.nodes.iter_mut() {
            if let Some(p) = &n.path {
                let tags = v.meta.get(p).map_or(&empty, |m| &m.tags);
                n.color = opt
                    .groups
                    .iter()
                    .find(|(q, _)| !q.trim().is_empty() && matches(q, p, tags))
                    .map(|(_, c)| *c);
            }
        }
        g.generation = v.generation;
        g
    }

    pub fn from_parts(mut nodes: Vec<Node>, edges: Vec<(u32, u32)>) -> Graph {
        let mut adj: Vec<Vec<u32>> = alloc::vec![Vec::new(); nodes.len()];
        for &(a, b) in &edges {
            if let (Some(x), Some(_)) = (adj.get_mut(a as usize), nodes.get(b as usize)) {
                x.push(b);
            }
            if let Some(y) = adj.get_mut(b as usize) {
                y.push(a);
            }
        }
        for (n, a) in nodes.iter_mut().zip(adj.iter()) {
            n.degree = a.len() as u32;
        }
        // A sunflower spiral to start from, so the same vault always lays
        // out the same way.
        let golden = core::f32::consts::PI * (3.0 - 5.0f32.sqrt());
        for (i, n) in nodes.iter_mut().enumerate() {
            let r = 30.0 * ((i as f32) + 0.5).sqrt();
            let a = i as f32 * golden;
            n.x = r * a.cos();
            n.y = r * a.sin();
        }
        Graph {
            nodes,
            edges,
            adj,
            alpha: 1.0,
            generation: 0,
        }
    }

    /// Keep the nodes `keep` says yes to.
    fn keep(&mut self, keep: impl Fn(usize, &Node, usize) -> bool) {
        let alive: Vec<bool> = self
            .nodes
            .iter()
            .zip(self.adj.iter())
            .enumerate()
            .map(|(i, (n, a))| keep(i, n, a.len()))
            .collect();
        let mut map: Vec<Option<u32>> = Vec::with_capacity(self.nodes.len());
        let mut nodes = Vec::new();
        for (n, &a) in self.nodes.iter().zip(alive.iter()) {
            if a {
                map.push(Some(nodes.len() as u32));
                nodes.push(n.clone());
            } else {
                map.push(None);
            }
        }
        let edges: Vec<(u32, u32)> = self
            .edges
            .iter()
            .filter_map(|&(a, b)| Some(((*map.get(a as usize)?)?, (*map.get(b as usize)?)?)))
            .collect();
        let gen = self.generation;
        *self = Graph::from_parts(nodes, edges);
        self.generation = gen;
    }

    /// Keep the note at `path` and what is within `depth` links of it,
    /// following links in (to it) and out (from it) as the filters say;
    /// without neighbour links, only links between notes one step apart.
    fn keep_near(&mut self, path: &str, depth: u32, dir: &[(u32, u32)], opt: &Options) {
        let Some(start) = self
            .nodes
            .iter()
            .position(|n| n.path.as_deref() == Some(path))
        else {
            *self = Graph::from_parts(Vec::new(), Vec::new());
            return;
        };
        let mut dist: Vec<u32> = alloc::vec![u32::MAX; self.nodes.len()];
        dist[start] = 0;
        let mut queue = alloc::collections::VecDeque::new();
        queue.push_back(start);
        while let Some(i) = queue.pop_front() {
            if dist[i] >= depth {
                continue;
            }
            for &j in &self.adj[i] {
                let j = j as usize;
                if dist[j] != u32::MAX {
                    continue;
                }
                let (a, b) = (i as u32, j as u32);
                let out = dir.binary_search(&(a, b)).is_ok();
                let inc = dir.binary_search(&(b, a)).is_ok();
                if (out && opt.outgoing) || (inc && opt.incoming) {
                    dist[j] = dist[i] + 1;
                    queue.push_back(j);
                }
            }
        }
        if !opt.neighbors {
            let d = dist.clone();
            self.edges.retain(|&(a, b)| {
                let (da, db) = (d[a as usize], d[b as usize]);
                da != u32::MAX && db != u32::MAX && da.abs_diff(db) == 1
            });
        }
        self.keep(|i, _, _| dist.get(i).is_some_and(|d| *d != u32::MAX));
    }

    pub fn find(&self, path: &str) -> Option<usize> {
        self.nodes
            .iter()
            .position(|n| n.path.as_deref() == Some(path))
    }

    /// Whether the layout is still moving.
    pub fn running(&self) -> bool {
        self.alpha > 0.005
    }

    /// Warm the simulation back up (a drag, a change of forces).
    pub fn reheat(&mut self, to: f32) {
        self.alpha = self.alpha.max(to);
    }

    /// One step of the simulation.
    pub fn tick(&mut self, opt: &Options) {
        let n = self.nodes.len();
        if n == 0 {
            self.alpha = 0.0;
            return;
        }
        let alpha = self.alpha;
        // Links: pull toward their length, weaker for well-linked notes.
        let dist = (opt.distance * 0.4).max(10.0);
        for &(a, b) in &self.edges {
            let (a, b) = (a as usize, b as usize);
            let (Some(na), Some(nb)) = (self.nodes.get(a), self.nodes.get(b)) else {
                continue;
            };
            let dx = (nb.x + nb.vx) - (na.x + na.vx);
            let dy = (nb.y + nb.vy) - (na.y + na.vy);
            let l = (dx * dx + dy * dy).sqrt().max(0.01);
            let da = na.degree.max(1) as f32;
            let db = nb.degree.max(1) as f32;
            let strength = opt.link.clamp(0.0, 1.0) / da.min(db);
            let f = (l - dist) / l * alpha * strength;
            let bias = da / (da + db);
            let (fx, fy) = (dx * f, dy * f);
            if let Some(nb) = self.nodes.get_mut(b) {
                nb.vx -= fx * bias;
                nb.vy -= fy * bias;
            }
            if let Some(na) = self.nodes.get_mut(a) {
                na.vx += fx * (1.0 - bias);
                na.vy += fy * (1.0 - bias);
            }
        }
        // Many-body push, through a quadtree.
        let tree = Quad::build(&self.nodes);
        let k = -40.0 * opt.repel.max(0.0);
        for i in 0..n {
            let (x, y) = (self.nodes[i].x, self.nodes[i].y);
            let (mut fx, mut fy) = (0.0f32, 0.0f32);
            tree.force(0, x, y, i as u32, k * alpha, &mut fx, &mut fy);
            self.nodes[i].vx += fx;
            self.nodes[i].vy += fy;
        }
        // A pull to the middle, on each dot.
        let c = opt.center.clamp(0.0, 1.0) * 0.02 * alpha;
        for nd in self.nodes.iter_mut() {
            nd.vx -= nd.x * c;
            nd.vy -= nd.y * c;
        }
        for nd in self.nodes.iter_mut() {
            if nd.fixed {
                nd.vx = 0.0;
                nd.vy = 0.0;
                continue;
            }
            nd.vx *= 0.6;
            nd.vy *= 0.6;
            nd.x += nd.vx;
            nd.y += nd.vy;
        }
        self.alpha += (0.0 - self.alpha) * 0.0228;
        if self.alpha < 0.005 {
            self.alpha = 0.0;
        }
    }

    /// The node nearest (x, y) in graph units, if one is within reach.
    pub fn hit(&self, x: f32, y: f32, slack: f32, size: f32) -> Option<usize> {
        let mut best: Option<(usize, f32)> = None;
        for (i, n) in self.nodes.iter().enumerate() {
            let dx = n.x - x;
            let dy = n.y - y;
            let d = (dx * dx + dy * dy).sqrt();
            let reach = n.radius(size) + slack;
            if d <= reach && best.is_none_or(|(_, bd)| d < bd) {
                best = Some((i, d));
            }
        }
        best.map(|(i, _)| i)
    }

    /// The box the nodes take, as (min x, min y, max x, max y).
    pub fn bounds(&self) -> (f32, f32, f32, f32) {
        let mut b = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for n in &self.nodes {
            b.0 = b.0.min(n.x);
            b.1 = b.1.min(n.y);
            b.2 = b.2.max(n.x);
            b.3 = b.3.max(n.y);
        }
        if self.nodes.is_empty() {
            (0.0, 0.0, 0.0, 0.0)
        } else {
            b
        }
    }
}

/// A Barnes-Hut quadtree over the dots.
struct Quad {
    cells: Vec<Cell>,
    xs: Vec<(f32, f32)>,
}

#[derive(Clone)]
struct Cell {
    x0: f32,
    y0: f32,
    size: f32,
    /// Centre of mass and how many dots are inside.
    cx: f32,
    cy: f32,
    count: u32,
    /// A leaf holds one dot; an inner cell four children.
    dot: Option<u32>,
    kids: [u32; 4],
}

const NONE: u32 = u32::MAX;

impl Quad {
    fn build(nodes: &[Node]) -> Quad {
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for n in nodes {
            x0 = x0.min(n.x);
            y0 = y0.min(n.y);
            x1 = x1.max(n.x);
            y1 = y1.max(n.y);
        }
        let size = (x1 - x0).max(y1 - y0).max(1.0) + 1.0;
        let mut q = Quad {
            cells: Vec::with_capacity(nodes.len() * 2),
            xs: nodes.iter().map(|n| (n.x, n.y)).collect(),
        };
        q.cells.push(Cell {
            x0,
            y0,
            size,
            cx: 0.0,
            cy: 0.0,
            count: 0,
            dot: None,
            kids: [NONE; 4],
        });
        for i in 0..nodes.len() {
            q.insert(0, i as u32, 0);
        }
        q
    }

    fn insert(&mut self, c: usize, i: u32, depth: u32) {
        let (x, y) = self.xs[i as usize];
        {
            let cell = &mut self.cells[c];
            let n = cell.count as f32;
            cell.cx = (cell.cx * n + x) / (n + 1.0);
            cell.cy = (cell.cy * n + y) / (n + 1.0);
            cell.count += 1;
            if cell.count == 1 {
                cell.dot = Some(i);
                return;
            }
        }
        if depth > 40 {
            return; // dots on top of each other: keep them together
        }
        if let Some(old) = self.cells[c].dot.take() {
            self.push_down(c, old, depth);
        }
        self.push_down(c, i, depth);
    }

    fn push_down(&mut self, c: usize, i: u32, depth: u32) {
        let (x, y) = self.xs[i as usize];
        let (x0, y0, half) = {
            let cell = &self.cells[c];
            (cell.x0, cell.y0, cell.size * 0.5)
        };
        let qx = usize::from(x >= x0 + half);
        let qy = usize::from(y >= y0 + half);
        let q = qy * 2 + qx;
        let mut kid = self.cells[c].kids[q];
        if kid == NONE {
            kid = self.cells.len() as u32;
            self.cells.push(Cell {
                x0: x0 + qx as f32 * half,
                y0: y0 + qy as f32 * half,
                size: half,
                cx: 0.0,
                cy: 0.0,
                count: 0,
                dot: None,
                kids: [NONE; 4],
            });
            self.cells[c].kids[q] = kid;
        }
        self.insert(kid as usize, i, depth + 1);
    }

    #[allow(clippy::too_many_arguments)]
    fn force(&self, c: usize, x: f32, y: f32, me: u32, k: f32, fx: &mut f32, fy: &mut f32) {
        let Some(cell) = self.cells.get(c) else {
            return;
        };
        if cell.count == 0 || (cell.count == 1 && cell.dot == Some(me)) {
            return;
        }
        let dx = cell.cx - x;
        let dy = cell.cy - y;
        let d2 = (dx * dx + dy * dy).max(1.0);
        let leaf = cell.kids.iter().all(|k| *k == NONE);
        // Far enough away (size / distance < 0.9): the cell acts as one.
        if leaf || cell.size * cell.size < 0.81 * d2 {
            if leaf && cell.dot == Some(me) {
                return;
            }
            let w = k * cell.count as f32 / d2;
            *fx += dx * w;
            *fy += dy * w;
            return;
        }
        for &kid in &cell.kids {
            if kid != NONE {
                self.force(kid as usize, x, y, me, k, fx, fy);
            }
        }
    }
}

/// An RGBA picture drawn in memory: the graph's lines and dots.
pub struct Raster {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

impl Raster {
    pub fn new(w: u32, h: u32) -> Raster {
        Raster {
            w,
            h,
            px: alloc::vec![0; (w as usize) * (h as usize) * 4],
        }
    }

    pub fn clear(&mut self, rgb: u32) {
        let (r, g, b) = ((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8);
        for p in self.px.chunks_exact_mut(4) {
            p[0] = r;
            p[1] = g;
            p[2] = b;
            p[3] = 255;
        }
    }

    #[inline]
    fn blend(&mut self, x: i32, y: i32, rgb: u32, a: f32) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 || a <= 0.0 {
            return;
        }
        let i = ((y as usize) * (self.w as usize) + x as usize) * 4;
        let Some(p) = self.px.get_mut(i..i + 3) else {
            return;
        };
        // Integer blending: this runs for every pixel of every line.
        let a = (a.min(1.0) * 256.0) as u32;
        let inv = 256 - a;
        p[0] = ((u32::from(p[0]) * inv + ((rgb >> 16) & 0xff) * a) >> 8) as u8;
        p[1] = ((u32::from(p[1]) * inv + ((rgb >> 8) & 0xff) * a) >> 8) as u8;
        p[2] = ((u32::from(p[2]) * inv + (rgb & 0xff) * a) >> 8) as u8;
    }

    pub fn get(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y as usize) * (self.w as usize) + x as usize) * 4;
        match self.px.get(i..i + 4) {
            Some(p) => [p[0], p[1], p[2], p[3]],
            None => [0; 4],
        }
    }

    /// An anti-aliased line (Wu's), `width` about 1 px, at `alpha`.
    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, rgb: u32, alpha: f32) {
        let w = self.w as f32;
        let h = self.h as f32;
        if (x0 < 0.0 && x1 < 0.0)
            || (y0 < 0.0 && y1 < 0.0)
            || (x0 >= w && x1 >= w)
            || (y0 >= h && y1 >= h)
        {
            return;
        }
        let steep = (y1 - y0).abs() > (x1 - x0).abs();
        let (mut x0, mut y0, mut x1, mut y1) = if steep {
            (y0, x0, y1, x1)
        } else {
            (x0, y0, x1, y1)
        };
        if x0 > x1 {
            core::mem::swap(&mut x0, &mut x1);
            core::mem::swap(&mut y0, &mut y1);
        }
        let dx = x1 - x0;
        let grad = if dx.abs() < 1e-6 { 1.0 } else { (y1 - y0) / dx };
        // Clip the run to the picture along the major axis.
        let limit = if steep { h } else { w };
        let start = x0.max(0.0).floor() as i32;
        let end = x1.min(limit - 1.0).ceil() as i32;
        let mut y = y0 + grad * (start as f32 - x0);
        for x in start..=end {
            let yf = y.floor();
            let frac = y - yf;
            let yi = yf as i32;
            if steep {
                self.blend(yi, x, rgb, (1.0 - frac) * alpha);
                self.blend(yi + 1, x, rgb, frac * alpha);
            } else {
                self.blend(x, yi, rgb, (1.0 - frac) * alpha);
                self.blend(x, yi + 1, rgb, frac * alpha);
            }
            y += grad;
        }
    }

    /// A filled, anti-aliased disc.
    pub fn disc(&mut self, cx: f32, cy: f32, r: f32, rgb: u32, alpha: f32) {
        if r <= 0.0 {
            return;
        }
        let x0 = (cx - r - 1.0).floor() as i32;
        let x1 = (cx + r + 1.0).ceil() as i32;
        let y0 = (cy - r - 1.0).floor() as i32;
        let y1 = (cy + r + 1.0).ceil() as i32;
        if x1 < 0 || y1 < 0 || x0 >= self.w as i32 || y0 >= self.h as i32 {
            return;
        }
        for y in y0.max(0)..=y1.min(self.h as i32 - 1) {
            for x in x0.max(0)..=x1.min(self.w as i32 - 1) {
                let dx = x as f32 + 0.5 - cx;
                let dy = y as f32 + 0.5 - cy;
                let d = (dx * dx + dy * dy).sqrt();
                let cov = (r + 0.5 - d).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend(x, y, rgb, cov * alpha);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::mem_vault;

    fn vault() -> Vault {
        let mut v = mem_vault(
            "G",
            &[
                ("Home.md", "[[A]] [[B]] [[Missing]] #hub"),
                ("A.md", "[[B]]"),
                ("B.md", "back to [[Home]]"),
                ("Lonely.md", "no links"),
                ("Projects/C.md", "[[A]] #work"),
                ("Board.canvas", "{\"nodes\":[{\"id\":\"1\",\"type\":\"file\",\"file\":\"Home.md\",\"x\":0,\"y\":0,\"width\":10,\"height\":10}],\"edges\":[]}"),
            ],
        );
        v.index_some(&mut || true);
        v
    }

    #[test]
    fn notes_links_unresolved_and_orphans() {
        let v = vault();
        let g = Graph::build(&v, &Options::default(), None);
        let labels: Vec<&str> = g.nodes.iter().map(|n| n.label.as_str()).collect();
        assert!(labels.contains(&"Home"));
        assert!(labels.contains(&"Lonely"));
        assert!(labels.contains(&"Board.canvas"));
        assert!(labels.contains(&"missing"));
        // Home-A, Home-B, Home-Missing, A-B, C-A, Board-Home: one line per pair.
        assert_eq!(g.edges.len(), 6);
        let home = g.find("Home.md").unwrap();
        assert_eq!(g.nodes[home].degree, 4);
        let opt = Options {
            hide_unresolved: true,
            show_orphans: false,
            ..Options::default()
        };
        let g = Graph::build(&v, &opt, None);
        assert!(g
            .nodes
            .iter()
            .all(|n| n.label != "missing" && n.label != "Lonely"));
        let opt = Options {
            show_tags: true,
            search: String::from("-path:projects"),
            ..Options::default()
        };
        let g = Graph::build(&v, &opt, None);
        assert!(g.nodes.iter().any(|n| n.label == "#hub"));
        assert!(g.nodes.iter().all(|n| n.label != "C"));
    }

    #[test]
    fn local_graph_keeps_the_neighbourhood() {
        let v = vault();
        let g = Graph::build(&v, &Options::default(), Some(("Projects/C.md", 1)));
        let mut labels: Vec<&str> = g.nodes.iter().map(|n| n.label.as_str()).collect();
        labels.sort_unstable();
        assert_eq!(labels, ["A", "C"]);
        let g = Graph::build(&v, &Options::default(), Some(("Projects/C.md", 2)));
        assert_eq!(g.nodes.len(), 4, "C, A, and A's links Home and B");
    }

    #[test]
    fn attachments_show_when_asked_in_yellow_kind() {
        let mut v = mem_vault(
            "G",
            &[
                ("Home.md", "![[photo.png]] and [[Report.pdf]]"),
                ("photo.png", "png"),
                ("Report.pdf", "pdf"),
            ],
        );
        v.index_some(&mut || true);
        let g = Graph::build(&v, &Options::default(), None);
        assert!(g.find("photo.png").is_none(), "hidden by default");
        let opt = Options {
            show_attachments: true,
            ..Options::default()
        };
        let g = Graph::build(&v, &opt, None);
        let p = g.find("photo.png").expect("shown");
        assert_eq!(g.nodes[p].kind, Kind::Attachment);
        assert!(g.find("Report.pdf").is_some());
        assert!(g.nodes[p].degree > 0, "linked from Home");
    }

    #[test]
    fn groups_colour_matching_notes_and_options_round_trip() {
        let v = vault();
        let mut opt = Options::default();
        opt.groups.push((String::from("path:Projects"), 0xE04050));
        opt.groups.push((String::from("tag:#hub"), 0x40A0E0));
        let g = Graph::build(&v, &opt, None);
        let c = g.find("Projects/C.md").unwrap();
        assert_eq!(g.nodes[c].color, Some(0xE04050));
        let h = g.find("Home.md").unwrap();
        assert_eq!(g.nodes[h].color, Some(0x40A0E0));
        let mut j = Value::obj();
        opt.show_arrow = true;
        opt.to_json(&mut j);
        assert_eq!(Options::from_json(&j), opt);
    }

    #[test]
    fn the_layout_settles_and_spreads() {
        let v = vault();
        let opt = Options::default();
        let mut g = Graph::build(&v, &opt, None);
        let mut steps = 0;
        while g.running() && steps < 2000 {
            g.tick(&opt);
            steps += 1;
        }
        assert!(!g.running(), "settles");
        assert!(steps < 400, "in {steps} steps");
        // No two dots on top of each other, linked dots nearer than the rest.
        for i in 0..g.nodes.len() {
            for j in i + 1..g.nodes.len() {
                let d = ((g.nodes[i].x - g.nodes[j].x).powi(2)
                    + (g.nodes[i].y - g.nodes[j].y).powi(2))
                .sqrt();
                assert!(
                    d > 5.0,
                    "{} and {} overlap",
                    g.nodes[i].label,
                    g.nodes[j].label
                );
            }
        }
        let h = g.find("Home.md").unwrap();
        assert_eq!(g.hit(g.nodes[h].x + 2.0, g.nodes[h].y, 3.0, 1.0), Some(h));
    }

    #[test]
    fn ten_thousand_notes_step_quickly() {
        // A ring of 10,000 notes, each linked to the next and to a hub.
        let n = 10_000u32;
        let nodes: Vec<Node> = (0..n)
            .map(|i| Node {
                x: 0.0,
                y: 0.0,
                vx: 0.0,
                vy: 0.0,
                kind: Kind::Note,
                label: String::new(),
                path: None,
                degree: 0,
                color: None,
                fixed: i == 0,
            })
            .collect();
        let mut edges: Vec<(u32, u32)> = (0..n - 1).map(|i| (i, i + 1)).collect();
        edges.extend((1..n).step_by(50).map(|i| (0, i)));
        let mut g = Graph::from_parts(nodes, edges);
        let opt = Options::default();
        let t = std::time::Instant::now();
        for _ in 0..5 {
            g.tick(&opt);
        }
        let each = t.elapsed().as_secs_f32() / 5.0;
        assert!(each < 0.25, "a step took {each}s");
        assert!(g.nodes.iter().all(|n| n.x.is_finite() && n.y.is_finite()));
    }

    #[test]
    fn the_raster_draws_lines_and_dots() {
        let mut r = Raster::new(40, 20);
        r.clear(0x1C1C1C);
        r.line(2.0, 10.0, 38.0, 10.0, 0xFFFFFF, 1.0);
        assert!(r.get(20, 10)[0] > 200, "the line is there");
        assert_eq!(r.get(20, 3), [0x1C, 0x1C, 0x1C, 255]);
        r.disc(10.0, 5.0, 3.0, 0xFF0000, 1.0);
        assert_eq!(r.get(9, 4), [255, 0, 0, 255]);
        // Lines off the picture are skipped, not drawn wrapped.
        r.line(-50.0, -50.0, -10.0, -5.0, 0x00FF00, 1.0);
        assert!(r.px.chunks(4).all(|p| p[..3] != [0, 255, 0]));
        r.line(5.0, -100.0, 5.0, 100.0, 0x0000FF, 1.0);
        assert!(r.get(5, 0)[2] > 200 && r.get(5, 19)[2] > 200);
    }
}

#[cfg(test)]
mod bench {
    use super::*;

    /// `cargo test --release graph::bench -- --ignored --nocapture`: one
    /// step and one picture at 10,000 notes, timed.
    #[test]
    #[ignore]
    fn ten_thousand_notes_frame_cost() {
        let n = 10_000u32;
        let nodes: Vec<Node> = (0..n)
            .map(|_| Node {
                x: 0.0,
                y: 0.0,
                vx: 0.0,
                vy: 0.0,
                kind: Kind::Note,
                label: String::new(),
                path: None,
                degree: 0,
                color: None,
                fixed: false,
            })
            .collect();
        let mut edges = Vec::new();
        let mut z: u64 = 7;
        for i in 0..n {
            for _ in 0..5 {
                z = z
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let j = ((z >> 33) % u64::from(n)) as u32;
                if j != i {
                    edges.push((i.min(j), i.max(j)));
                }
            }
        }
        edges.sort_unstable();
        edges.dedup();
        let mut g = Graph::from_parts(nodes, edges);
        let opt = Options::default();
        let t = std::time::Instant::now();
        for _ in 0..10 {
            g.tick(&opt);
        }
        let tick = t.elapsed().as_secs_f64() / 10.0;
        let mut r = Raster::new(724, 720);
        let t = std::time::Instant::now();
        r.clear(0x1C1C1C);
        let (x0, y0, x1, y1) = g.bounds();
        let zoom = (724.0 / (x1 - x0)).min(720.0 / (y1 - y0));
        for &(a, b) in &g.edges {
            let (na, nb) = (&g.nodes[a as usize], &g.nodes[b as usize]);
            r.line(
                (na.x - x0) * zoom,
                (na.y - y0) * zoom,
                (nb.x - x0) * zoom,
                (nb.y - y0) * zoom,
                0x3F3F3F,
                1.0,
            );
        }
        for nd in &g.nodes {
            r.disc((nd.x - x0) * zoom, (nd.y - y0) * zoom, 2.0, 0xB3B3B3, 1.0);
        }
        let draw = t.elapsed().as_secs_f64();
        std::println!(
            "edges {} tick {:.1} ms picture {:.1} ms",
            g.edges.len(),
            tick * 1000.0,
            draw * 1000.0
        );
    }
}
