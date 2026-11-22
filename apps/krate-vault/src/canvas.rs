//! Canvas: `.canvas` files in the JSON Canvas format -- cards of text,
//! cards that show a note, web links, groups, and arrows between them on an
//! endless board. Read and written the way the notes app writes them (tab
//! indents, one card per line, keys in their order), so a board made in one
//! app opens in the other and a board only looked at is never rewritten.

use crate::json::{self, Value};
use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardKind {
    Text,
    File,
    Link,
    Group,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Top,
    Right,
    Bottom,
    Left,
}

impl Side {
    pub fn name(self) -> &'static str {
        match self {
            Side::Top => "top",
            Side::Right => "right",
            Side::Bottom => "bottom",
            Side::Left => "left",
        }
    }

    fn parse(s: &str) -> Option<Side> {
        Some(match s {
            "top" => Side::Top,
            "right" => Side::Right,
            "bottom" => Side::Bottom,
            "left" => Side::Left,
            _ => return None,
        })
    }
}

/// A card. Its JSON is kept whole (`raw`), so keys Vault does not know
/// are written back as they were.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub id: String,
    pub kind: CardKind,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub text: Option<String>,
    pub file: Option<String>,
    pub url: Option<String>,
    pub label: Option<String>,
    /// `"1"`..`"6"` (the preset colours) or `#rrggbb`.
    pub color: Option<String>,
    raw: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub id: String,
    pub from: String,
    pub from_side: Option<Side>,
    pub to: String,
    pub to_side: Option<Side>,
    pub from_arrow: bool,
    pub to_arrow: bool,
    pub label: Option<String>,
    pub color: Option<String>,
    raw: Value,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Canvas {
    pub nodes: Vec<Card>,
    pub edges: Vec<Edge>,
    /// Ids handed out so far, to make new ones that do not clash.
    seq: u64,
}

fn num(v: &Value, k: &str) -> f32 {
    v.get_num(k).unwrap_or(0.0) as f32
}

fn opt(v: &Value, k: &str) -> Option<String> {
    v.get_str(k).map(String::from)
}

impl Card {
    fn from_json(v: Value) -> Option<Card> {
        let kind = match v.get_str("type")? {
            "text" => CardKind::Text,
            "file" => CardKind::File,
            "link" => CardKind::Link,
            "group" => CardKind::Group,
            _ => return None,
        };
        Some(Card {
            id: opt(&v, "id")?,
            kind,
            x: num(&v, "x"),
            y: num(&v, "y"),
            w: num(&v, "width").max(1.0),
            h: num(&v, "height").max(1.0),
            text: opt(&v, "text"),
            file: opt(&v, "file"),
            url: opt(&v, "url"),
            label: opt(&v, "label"),
            color: opt(&v, "color"),
            raw: v,
        })
    }

    fn to_json(&self) -> Value {
        let mut v = self.raw.clone();
        let opt = |v: &mut Value, k: &str, x: &Option<String>| match x {
            Some(s) => v.set(k, Value::s(s)),
            None => v.remove(k),
        };
        v.set("id", Value::s(&self.id));
        v.set(
            "type",
            Value::s(match self.kind {
                CardKind::Text => "text",
                CardKind::File => "file",
                CardKind::Link => "link",
                CardKind::Group => "group",
            }),
        );
        opt(&mut v, "text", &self.text);
        opt(&mut v, "file", &self.file);
        opt(&mut v, "url", &self.url);
        opt(&mut v, "label", &self.label);
        // Numbers are written only when they changed, whole as the app
        // writes them; one only looked at keeps its exact text.
        for (k, x) in [
            ("x", self.x),
            ("y", self.y),
            ("width", self.w),
            ("height", self.h),
        ] {
            if v.get_num(k)
                .is_none_or(|old| (old as f32 - x).abs() > 0.001)
            {
                v.set(k, Value::Num(f64::from(x.round())));
            }
        }
        opt(&mut v, "color", &self.color);
        v
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x <= self.x + self.w && y <= self.y + self.h
    }

    /// Where an arrow meets this card on `side`.
    pub fn anchor(&self, side: Side) -> (f32, f32) {
        match side {
            Side::Top => (self.x + self.w * 0.5, self.y),
            Side::Right => (self.x + self.w, self.y + self.h * 0.5),
            Side::Bottom => (self.x + self.w * 0.5, self.y + self.h),
            Side::Left => (self.x, self.y + self.h * 0.5),
        }
    }

    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w * 0.5, self.y + self.h * 0.5)
    }
}

impl Edge {
    fn from_json(v: Value) -> Option<Edge> {
        Some(Edge {
            id: opt(&v, "id")?,
            from: opt(&v, "fromNode")?,
            from_side: v.get_str("fromSide").and_then(Side::parse),
            to: opt(&v, "toNode")?,
            to_side: v.get_str("toSide").and_then(Side::parse),
            from_arrow: v.get_str("fromEnd") == Some("arrow"),
            to_arrow: v.get_str("toEnd") != Some("none"),
            label: opt(&v, "label"),
            color: opt(&v, "color"),
            raw: v,
        })
    }

    fn to_json(&self) -> Value {
        let mut v = self.raw.clone();
        v.set("id", Value::s(&self.id));
        v.set("fromNode", Value::s(&self.from));
        match self.from_side {
            Some(s) => v.set("fromSide", Value::s(s.name())),
            None => v.remove("fromSide"),
        }
        if self.from_arrow {
            v.set("fromEnd", Value::s("arrow"));
        } else if v.get("fromEnd").is_some() {
            v.set("fromEnd", Value::s("none"));
        }
        v.set("toNode", Value::s(&self.to));
        match self.to_side {
            Some(s) => v.set("toSide", Value::s(s.name())),
            None => v.remove("toSide"),
        }
        if !self.to_arrow {
            v.set("toEnd", Value::s("none"));
        } else if v.get("toEnd").is_some() {
            v.set("toEnd", Value::s("arrow"));
        }
        match &self.label {
            Some(l) => v.set("label", Value::s(l)),
            None => v.remove("label"),
        }
        match &self.color {
            Some(c) => v.set("color", Value::s(c)),
            None => v.remove("color"),
        }
        v
    }
}

/// The side of `a` that faces `b`.
pub fn facing(a: &Card, b: &Card) -> Side {
    let (ax, ay) = a.center();
    let (bx, by) = b.center();
    let dx = (bx - ax) / a.w.max(1.0);
    let dy = (by - ay) / a.h.max(1.0);
    if dx.abs() > dy.abs() {
        if dx > 0.0 {
            Side::Right
        } else {
            Side::Left
        }
    } else if dy > 0.0 {
        Side::Bottom
    } else {
        Side::Top
    }
}

impl Canvas {
    /// Read a `.canvas` file. An empty file is an empty board.
    pub fn parse(text: &str) -> Option<Canvas> {
        if text.trim().is_empty() {
            return Some(Canvas::default());
        }
        let v = json::parse(text)?;
        let nodes: Vec<Card> = v
            .get("nodes")
            .map(|a| {
                a.arr()
                    .iter()
                    .cloned()
                    .filter_map(Card::from_json)
                    .collect()
            })
            .unwrap_or_default();
        let edges: Vec<Edge> = v
            .get("edges")
            .map(|a| {
                a.arr()
                    .iter()
                    .cloned()
                    .filter_map(Edge::from_json)
                    .collect()
            })
            .unwrap_or_default();
        Some(Canvas {
            nodes,
            edges,
            seq: 0,
        })
    }

    /// Write it out as the notes app does.
    pub fn to_text(&self) -> String {
        let mut out = String::from("{\n\t\"nodes\":[");
        for (i, n) in self.nodes.iter().enumerate() {
            out.push_str(if i == 0 { "\n\t\t" } else { ",\n\t\t" });
            out.push_str(&json::compact(&n.to_json()));
        }
        out.push_str(if self.nodes.is_empty() {
            "],"
        } else {
            "\n\t],"
        });
        out.push_str("\n\t\"edges\":[");
        for (i, e) in self.edges.iter().enumerate() {
            out.push_str(if i == 0 { "\n\t\t" } else { ",\n\t\t" });
            out.push_str(&json::compact(&e.to_json()));
        }
        out.push_str(if self.edges.is_empty() { "]" } else { "\n\t]" });
        out.push_str("\n}");
        out
    }

    pub fn card(&self, id: &str) -> Option<&Card> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn card_mut(&mut self, id: &str) -> Option<&mut Card> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    /// A new id: 16 hex digits, like the app's, from `seed` and a count.
    pub fn new_id(&mut self, seed: u64) -> String {
        loop {
            self.seq += 1;
            let mut z = seed ^ self.seq.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            let mut id = String::with_capacity(16);
            for i in (0..16).rev() {
                let d = ((z >> (i * 4)) & 15) as u32;
                id.push(char::from_digit(d, 16).unwrap_or('0'));
            }
            if self.card(&id).is_none() && !self.edges.iter().any(|e| e.id == id) {
                return id;
            }
        }
    }

    /// Add a card; returns its id.
    #[allow(clippy::too_many_arguments)]
    pub fn add(
        &mut self,
        kind: CardKind,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        body: &str,
        seed: u64,
    ) -> String {
        let id = self.new_id(seed);
        let mut raw = Value::obj();
        raw.set("id", Value::s(&id));
        raw.set(
            "type",
            Value::s(match kind {
                CardKind::Text => "text",
                CardKind::File => "file",
                CardKind::Link => "link",
                CardKind::Group => "group",
            }),
        );
        let (mut text, mut file, mut url, mut label) = (None, None, None, None);
        match kind {
            CardKind::Text => {
                text = Some(String::from(body));
                raw.set("text", Value::s(body));
            }
            CardKind::File => {
                file = Some(String::from(body));
                raw.set("file", Value::s(body));
            }
            CardKind::Link => {
                url = Some(String::from(body));
                raw.set("url", Value::s(body));
            }
            CardKind::Group => {
                if !body.is_empty() {
                    label = Some(String::from(body));
                    raw.set("label", Value::s(body));
                }
            }
        }
        self.nodes.push(Card {
            id: id.clone(),
            kind,
            x,
            y,
            w,
            h,
            text,
            file,
            url,
            label,
            color: None,
            raw,
        });
        id
    }

    /// Join two cards with an arrow, side to side.
    pub fn connect(
        &mut self,
        from: &str,
        from_side: Side,
        to: &str,
        to_side: Side,
        seed: u64,
    ) -> Option<String> {
        if from == to || self.card(from).is_none() || self.card(to).is_none() {
            return None;
        }
        let id = self.new_id(seed);
        let mut raw = Value::obj();
        raw.set("id", Value::s(&id));
        raw.set("fromNode", Value::s(from));
        raw.set("fromSide", Value::s(from_side.name()));
        raw.set("toNode", Value::s(to));
        raw.set("toSide", Value::s(to_side.name()));
        self.edges.push(Edge {
            id: id.clone(),
            from: String::from(from),
            from_side: Some(from_side),
            to: String::from(to),
            to_side: Some(to_side),
            from_arrow: false,
            to_arrow: true,
            label: None,
            color: None,
            raw,
        });
        Some(id)
    }

    /// Take a card away, with its arrows.
    pub fn remove(&mut self, id: &str) {
        self.nodes.retain(|n| n.id != id);
        self.edges
            .retain(|e| e.from != id && e.to != id && e.id != id);
    }

    /// Cards inside a group (their whole box within it), to move with it.
    pub fn inside(&self, group: &str) -> Vec<String> {
        let Some(g) = self.card(group) else {
            return Vec::new();
        };
        self.nodes
            .iter()
            .filter(|n| {
                n.id != g.id
                    && n.x >= g.x
                    && n.y >= g.y
                    && n.x + n.w <= g.x + g.w
                    && n.y + n.h <= g.y + g.h
            })
            .map(|n| n.id.clone())
            .collect()
    }

    /// The topmost card under a point: groups lie under everything else.
    pub fn hit(&self, x: f32, y: f32) -> Option<usize> {
        let top = self
            .nodes
            .iter()
            .enumerate()
            .rev()
            .find(|(_, n)| n.kind != CardKind::Group && n.contains(x, y));
        top.or_else(|| {
            self.nodes
                .iter()
                .enumerate()
                .rev()
                .find(|(_, n)| n.kind == CardKind::Group && n.contains(x, y))
        })
        .map(|(i, _)| i)
    }

    /// The box all cards take: (x, y, w, h).
    pub fn bounds(&self) -> Option<(f32, f32, f32, f32)> {
        let first = self.nodes.first()?;
        let mut b = (first.x, first.y, first.x + first.w, first.y + first.h);
        for n in &self.nodes {
            b.0 = b.0.min(n.x);
            b.1 = b.1.min(n.y);
            b.2 = b.2.max(n.x + n.w);
            b.3 = b.3.max(n.y + n.h);
        }
        Some((b.0, b.1, b.2 - b.0, b.3 - b.1))
    }

    /// An edge's two ends, as points on the board.
    pub fn ends(&self, e: &Edge) -> Option<(End, End)> {
        let a = self.card(&e.from)?;
        let b = self.card(&e.to)?;
        let sa = e.from_side.unwrap_or_else(|| facing(a, b));
        let sb = e.to_side.unwrap_or_else(|| facing(b, a));
        let (ax, ay) = a.anchor(sa);
        let (bx, by) = b.anchor(sb);
        Some(((ax, ay, sa), (bx, by, sb)))
    }
}

/// One end of an arrow: where it meets a card, and which side.
pub type End = (f32, f32, Side);

/// Points along an arrow's curve: a cubic from each end straight out of
/// its side, as the app draws them.
pub fn curve(a: End, b: End, steps: usize) -> Vec<(f32, f32)> {
    let out_of = |s: Side| match s {
        Side::Top => (0.0, -1.0),
        Side::Right => (1.0, 0.0),
        Side::Bottom => (0.0, 1.0),
        Side::Left => (-1.0, 0.0),
    };
    let d = (((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt() * 0.4).clamp(20.0, 150.0);
    let (ax, ay) = out_of(a.2);
    let (bx, by) = out_of(b.2);
    let p0 = (a.0, a.1);
    let p1 = (a.0 + ax * d, a.1 + ay * d);
    let p2 = (b.0 + bx * d, b.1 + by * d);
    let p3 = (b.0, b.1);
    (0..=steps)
        .map(|i| {
            let t = i as f32 / steps.max(1) as f32;
            let u = 1.0 - t;
            (
                u * u * u * p0.0
                    + 3.0 * u * u * t * p1.0
                    + 3.0 * u * t * t * p2.0
                    + t * t * t * p3.0,
                u * u * u * p0.1
                    + 3.0 * u * u * t * p1.1
                    + 3.0 * u * t * t * p2.1
                    + t * t * t * p3.1,
            )
        })
        .collect()
}

/// What a link card shows of a web page: its title, its description and
/// its site, read from the page's HTML (`og:` tags first, as previews do).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Preview {
    pub title: String,
    pub description: String,
    pub site: String,
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name).map(|i| i + from) {
        let rest = &tag[i + name.len()..];
        let rest_t = rest.trim_start();
        if let Some(r) = rest_t.strip_prefix('=') {
            let r = r.trim_start();
            let q = r.chars().next()?;
            if q == '"' || q == '\'' {
                let body = &r[1..];
                return body.find(q).map(|e| &body[..e]);
            }
            let end = r.find([' ', '>', '/']).unwrap_or(r.len());
            return Some(&r[..end]);
        }
        from = i + name.len();
    }
    None
}

fn unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&nbsp;", " ")
}

/// Read a page's preview from its HTML.
pub fn preview(html: &str, url: &str) -> Preview {
    let mut p = Preview::default();
    let lower = html.to_ascii_lowercase();
    let mut at = 0;
    let mut og_title = None;
    let mut og_desc = None;
    let mut desc = None;
    let mut site = None;
    while let Some(i) = lower[at..].find("<meta").map(|i| i + at) {
        let end = lower[i..].find('>').map_or(lower.len(), |e| i + e);
        let tag = &html[i..end];
        let key = attr(tag, "property")
            .or_else(|| attr(tag, "name"))
            .map(str::to_ascii_lowercase);
        let content = attr(tag, "content").map(unescape);
        match (key.as_deref(), content) {
            (Some("og:title"), Some(c)) => og_title = Some(c),
            (Some("og:description"), Some(c)) => og_desc = Some(c),
            (Some("description"), Some(c)) => desc = Some(c),
            (Some("og:site_name"), Some(c)) => site = Some(c),
            _ => {}
        }
        at = end;
    }
    let title_tag = lower.find("<title").and_then(|i| {
        let open = lower[i..].find('>')? + i + 1;
        let close = lower[open..].find("</title")? + open;
        Some(unescape(html[open..close].trim()))
    });
    p.title = og_title.or(title_tag).unwrap_or_default();
    p.description = og_desc.or(desc).unwrap_or_default();
    let host = url
        .split("://")
        .nth(1)
        .unwrap_or(url)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("");
    p.site = site.unwrap_or_else(|| String::from(host.trim_start_matches("www.")));
    if p.title.is_empty() {
        p.title = String::from(url);
    }
    p
}

/// The preset colours `"1"`..`"6"` (red, orange, yellow, green, cyan,
/// purple), or a `#rrggbb`, as `0xRRGGBB` for the theme (dark or light).
pub fn color_of(c: &str, dark: bool) -> Option<u32> {
    let presets_dark = [0xFB464C, 0xE9973F, 0xE0DE71, 0x44CF6E, 0x53DFDD, 0xA882FF];
    let presets_light = [0xE93147, 0xEC7500, 0xE0AC00, 0x08B94E, 0x00BFBC, 0x7852EE];
    if let Some(hex) = c.strip_prefix('#') {
        return u32::from_str_radix(hex.get(..6)?, 16).ok();
    }
    let i: usize = c.parse().ok()?;
    let set = if dark { presets_dark } else { presets_light };
    set.get(i.checked_sub(1)?).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOARD: &str = "{\n\t\"nodes\":[\n\t\t{\"id\":\"a1b2c3d4e5f60001\",\"type\":\"text\",\"text\":\"# Plan\\nShip it.\",\"x\":-300,\"y\":-160,\"width\":260,\"height\":140},\n\t\t{\"id\":\"a1b2c3d4e5f60002\",\"type\":\"file\",\"file\":\"Home.md\",\"x\":60,\"y\":-200,\"width\":400,\"height\":300,\"custom\":{\"k\":[1,2]}},\n\t\t{\"id\":\"g1\",\"type\":\"group\",\"label\":\"Ideas\",\"x\":-340,\"y\":260,\"width\":820,\"height\":260},\n\t\t{\"id\":\"c1\",\"type\":\"text\",\"text\":\"inside\",\"x\":-300,\"y\":320,\"width\":250,\"height\":60,\"color\":\"4\"}\n\t],\n\t\"edges\":[\n\t\t{\"id\":\"e1\",\"fromNode\":\"a1b2c3d4e5f60001\",\"fromSide\":\"right\",\"toNode\":\"a1b2c3d4e5f60002\",\"toSide\":\"left\"},\n\t\t{\"id\":\"e2\",\"fromNode\":\"c1\",\"toNode\":\"a1b2c3d4e5f60002\",\"toEnd\":\"none\",\"label\":\"see\"}\n\t]\n}";

    #[test]
    fn a_board_reads_and_writes_back_byte_for_byte() {
        let c = Canvas::parse(BOARD).unwrap();
        assert_eq!(c.nodes.len(), 4);
        assert_eq!(c.edges.len(), 2);
        assert_eq!(c.nodes[1].file.as_deref(), Some("Home.md"));
        assert!(c.edges[0].to_arrow && !c.edges[1].to_arrow);
        assert_eq!(c.to_text(), BOARD);
        assert_eq!(
            Canvas::parse("").unwrap().to_text(),
            "{\n\t\"nodes\":[],\n\t\"edges\":[]\n}"
        );
    }

    #[test]
    fn a_link_card_reads_the_pages_title() {
        let html = "<html><head><title>Krate &amp; friends</title><meta name=\"description\" content=\"Apps that run anywhere\"><meta property='og:site_name' content='Krate'></head></html>";
        let p = preview(html, "https://www.krate.tech/x");
        assert_eq!(p.title, "Krate & friends");
        assert_eq!(p.description, "Apps that run anywhere");
        assert_eq!(p.site, "Krate");
        let p = preview(
            "<meta property=\"og:title\" content=\"OG\">",
            "https://example.com/a",
        );
        assert_eq!((p.title.as_str(), p.site.as_str()), ("OG", "example.com"));
        assert_eq!(preview("", "https://e.org").title, "https://e.org");
    }

    #[test]
    fn cards_move_resize_connect_and_go() {
        let mut c = Canvas::parse(BOARD).unwrap();
        let id = c.add(CardKind::Text, 100.0, 600.0, 250.0, 60.0, "new", 7);
        assert_eq!(id.len(), 16);
        let e = c.connect("c1", Side::Right, &id, Side::Left, 7).unwrap();
        assert_eq!(c.edges.len(), 3);
        if let Some(n) = c.card_mut("c1") {
            n.x += 10.4;
            n.w = 300.0;
        }
        assert_eq!(c.inside("g1"), alloc::vec![String::from("c1")]);
        let text = c.to_text();
        assert!(text.contains("{\"id\":\"c1\",\"type\":\"text\",\"text\":\"inside\",\"x\":-290,\"y\":320,\"width\":300,\"height\":60,\"color\":\"4\"}"), "{text}");
        let again = Canvas::parse(&text).unwrap();
        assert_eq!(again.edges[2].id, e);
        assert!(again.edges[2].to_arrow);
        c.remove("c1");
        assert_eq!(c.edges.len(), 1, "its arrows go with it");
        // The card on top wins over the group under it.
        let c = Canvas::parse(BOARD).unwrap();
        assert_eq!(
            c.hit(-250.0, 340.0).map(|i| c.nodes[i].id.as_str()),
            Some("c1")
        );
        assert_eq!(
            c.hit(300.0, 400.0).map(|i| c.nodes[i].id.as_str()),
            Some("g1")
        );
        assert_eq!(color_of("4", true), Some(0x44CF6E));
        assert_eq!(color_of("#ff8800", true), Some(0xFF8800));
        let pts = curve((0.0, 0.0, Side::Right), (100.0, 50.0, Side::Left), 8);
        assert_eq!(pts.len(), 9);
        assert_eq!(pts[8], (100.0, 50.0));
    }
}
