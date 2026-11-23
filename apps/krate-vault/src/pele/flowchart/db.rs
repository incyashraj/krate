//! Ports diagrams/flowchart/db.ts: what the parser builds -- the nodes,
//! edges, subgraphs, classes and links of a flowchart, as Mermaid's
//! FlowDB keeps them.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use super::shapes::is_valid_shape;
use super::types::{
    DocItem, FlowClassDef, FlowEdge, FlowNode, FlowSubgraph, FlowText, LabelType, LinkInfo,
};
use crate::json::Value;
use crate::pele::errors::{PeleError, PeleErrorCode};
use crate::pele::js;
use crate::pele::preprocess::{to_js_string, truthy};
use crate::pele::util::url::safe_url;
use crate::pele::util::yaml::parse_yaml;

/// Ports `interface FlowDbOptions` (without `warn`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FlowDbOptions {
    pub max_edges: Option<usize>,
    pub inherit_dir: bool,
    pub direction: Option<String>,
}

/// Ports `labelType`.
fn label_type(kind: Option<&str>) -> LabelType {
    match kind {
        Some("text") => LabelType::Text,
        Some("string") => LabelType::String,
        _ => LabelType::Markdown,
    }
}

/// Ports `unquote`.
fn unquote(text: &str) -> String {
    // A lone '"' starts and ends with one, and substring(1, 0) gives it back.
    if text.len() >= 2 && text.starts_with('"') && text.ends_with('"') {
        text[1..text.len() - 1].to_string()
    } else {
        text.to_string()
    }
}

/// Ports `destructStartLink`.
fn destruct_start_link(raw: &str) -> (String, String) {
    let mut s = js::trim(raw);
    let mut kind = "arrow_open";
    match s.as_bytes().first() {
        Some(b'<') => {
            kind = "arrow_point";
            s = &s[1..];
        }
        Some(b'x') => {
            kind = "arrow_cross";
            s = &s[1..];
        }
        Some(b'o') => {
            kind = "arrow_circle";
            s = &s[1..];
        }
        _ => {}
    }
    let mut stroke = "normal";
    if s.contains('=') {
        stroke = "thick";
    }
    if s.contains('.') {
        stroke = "dotted";
    }
    (kind.to_string(), stroke.to_string())
}

/// Ports `destructEndLink`.
fn destruct_end_link(raw: &str) -> LinkInfo {
    let s = js::trim(raw);
    let last = s.chars().next_back();
    let mut line: &str = match s.char_indices().next_back() {
        Some((i, _)) => &s[..i],
        None => s,
    };
    let mut kind = String::from("arrow_open");
    let mut double = |k: &str, first: char, line: &mut &str| {
        kind = k.to_string();
        if s.starts_with(first) {
            kind = format!("double_{}", k);
            *line = &line[first.len_utf8().min(line.len())..];
        }
    };
    match last {
        Some('x') => double("arrow_cross", 'x', &mut line),
        Some('>') => double("arrow_point", '<', &mut line),
        Some('o') => double("arrow_circle", 'o', &mut line),
        _ => {}
    }
    let mut stroke = "normal";
    let mut length = js::len16(line) as f64 - 1.0;
    if line.starts_with('=') {
        stroke = "thick";
    }
    if line.starts_with('~') {
        stroke = "invisible";
    }
    let dots = line.matches('.').count();
    if dots > 0 {
        stroke = "dotted";
        length = dots as f64;
    }
    LinkInfo {
        kind,
        stroke: stroke.to_string(),
        length: Some(length),
        text: None,
        id: None,
    }
}

/// Ports `class FlowDb`. Maps keep their insertion order: `nodes` and
/// `classes` as vectors with an index by id.
#[derive(Debug, Clone, Default)]
pub struct FlowDb {
    pub direction: Option<String>,
    pub nodes: Vec<FlowNode>,
    node_index: BTreeMap<String, usize>,
    pub edges: Vec<FlowEdge>,
    pub subgraphs: Vec<FlowSubgraph>,
    pub classes: Vec<FlowClassDef>,
    class_index: BTreeMap<String, usize>,
    pub tooltips: BTreeMap<String, String>,
    pub default_edge_style: Option<Vec<String>>,
    pub default_interpolate: Option<String>,
    pub title: Option<String>,
    pub acc_title: Option<String>,
    pub acc_descr: Option<String>,

    subgraph_lookup: BTreeMap<String, usize>,
    members: BTreeSet<String>,
    edge_by_id: BTreeMap<String, usize>,
    pair_count: BTreeMap<String, usize>,
    sub_count: usize,
    options: FlowDbOptions,
}

fn sem(message: String) -> PeleError {
    PeleError::of(message, PeleErrorCode::Semantic, "flowchart")
}

impl FlowDb {
    pub fn new(options: FlowDbOptions) -> FlowDb {
        FlowDb {
            options,
            ..FlowDb::default()
        }
    }

    pub fn node(&self, id: &str) -> Option<&FlowNode> {
        self.node_index.get(id).map(|&i| &self.nodes[i])
    }

    pub fn class(&self, id: &str) -> Option<&FlowClassDef> {
        self.class_index.get(id).map(|&i| &self.classes[i])
    }

    /// Ports `addVertex`.
    #[allow(clippy::too_many_arguments)]
    pub fn add_vertex(
        &mut self,
        id: &str,
        text_obj: Option<FlowText>,
        kind: Option<&str>,
        style: Option<Vec<String>>,
        classes: Option<Vec<String>>,
        dir: Option<String>,
        props: Option<Vec<(String, String)>>,
        metadata: Option<String>,
    ) -> Result<(), PeleError> {
        if id.is_empty() || js::trim(id).is_empty() {
            return Ok(());
        }

        let mut doc: Option<Vec<(String, Value)>> = None;
        if let Some(m) = metadata {
            let yaml = if m.contains('\n') {
                format!("{}\n", m)
            } else {
                format!("{{\n{}\n}}", m)
            };
            let parsed = parse_yaml(&yaml).map_err(|e| {
                PeleError::of(
                    format!("Metadata of \"{}\" is not valid YAML. {}", id, e),
                    PeleErrorCode::Syntax,
                    "flowchart",
                )
            })?;
            if let Value::Obj(o) = parsed {
                doc = Some(o);
            }
        }
        let get = |k: &str| {
            doc.as_ref()
                .and_then(|d| d.iter().find(|(n, _)| n == k).map(|(_, v)| v))
        };

        if let Some(&si) = self.subgraph_lookup.get(id) {
            if let Some(d) = &doc {
                let sg = &mut self.subgraphs[si];
                let mut merged = sg.metadata.clone().unwrap_or_default();
                for (k, v) in d {
                    match merged.iter_mut().find(|(n, _)| n == k) {
                        Some(e) => e.1 = v.clone(),
                        None => merged.push((k.clone(), v.clone())),
                    }
                }
                sg.metadata = Some(merged);
                return Ok(());
            }
        }

        if let Some(&ei) = self.edge_by_id.get(id) {
            let edge = &mut self.edges[ei];
            if let Some(v) = get("animate") {
                edge.animate = Some(v.clone());
            }
            if let Some(v) = get("animation") {
                edge.animation = Some(v.clone());
            }
            match get("curve") {
                None => {}
                Some(Value::Null) => edge.interpolate = None,
                Some(v) => edge.interpolate = Some(to_js_string(v)),
            }
            return Ok(());
        }

        let vi = match self.node_index.get(id) {
            Some(&i) => i,
            None => {
                self.nodes.push(FlowNode {
                    id: id.to_string(),
                    text: None,
                    label_type: LabelType::Text,
                    kind: None,
                    styles: Vec::new(),
                    classes: Vec::new(),
                    dir: None,
                    props: None,
                    link: None,
                    link_target: None,
                    icon: None,
                    form: None,
                    pos: None,
                    img: None,
                    constraint: None,
                    asset_width: None,
                    asset_height: None,
                });
                self.node_index.insert(id.to_string(), self.nodes.len() - 1);
                self.nodes.len() - 1
            }
        };
        let vertex = &mut self.nodes[vi];

        if let Some(t) = text_obj {
            vertex.label_type = t.kind;
            vertex.text = Some(unquote(js::trim(&t.text)));
        } else if vertex.text.is_none() {
            vertex.text = Some(id.to_string());
        }
        if let Some(k) = kind {
            vertex.kind = Some(k.to_string());
        }
        if let Some(s) = style {
            vertex.styles.extend(s);
        }
        if let Some(c) = classes {
            vertex.classes.extend(c);
        }
        if dir.is_some() {
            vertex.dir = dir;
        }
        match (&mut vertex.props, props) {
            (None, p) => vertex.props = Some(p.unwrap_or_default()),
            (Some(existing), Some(p)) => {
                for (k, v) in p {
                    match existing.iter_mut().find(|(n, _)| *n == k) {
                        Some(e) => e.1 = v,
                        None => existing.push((k, v)),
                    }
                }
            }
            _ => {}
        }

        if doc.is_none() {
            return Ok(());
        }

        if truthy(get("shape")) {
            let shape = to_js_string(get("shape").unwrap_or(&Value::Null));
            if shape != shape.to_lowercase() || shape.contains('_') {
                return Err(sem(format!(
                    "No such shape: {}. Shape names should be lowercase.",
                    shape
                )));
            }
            if !is_valid_shape(&shape) {
                return Err(sem(format!("No such shape: {}.", shape)));
            }
            vertex.kind = Some(shape);
        }
        let label = match get("label") {
            Some(Value::Str(s)) => Some(s.clone()),
            _ => None,
        };
        if truthy(get("label")) {
            vertex.text = Some(to_js_string(get("label").unwrap_or(&Value::Null)));
            let lt = match get("labelType") {
                Some(Value::Str(s)) => Some(s.as_str()),
                _ => None,
            };
            vertex.label_type = label_type(lt);
        }
        let blank_label = label.as_deref().is_none_or(|l| js::trim(l).is_empty());
        if truthy(get("icon")) {
            vertex.icon = Some(to_js_string(get("icon").unwrap_or(&Value::Null)));
            if blank_label && vertex.text.as_deref() == Some(id) {
                vertex.text = Some(String::new());
            }
        }
        if truthy(get("form")) {
            vertex.form = Some(to_js_string(get("form").unwrap_or(&Value::Null)));
        }
        if truthy(get("pos")) {
            vertex.pos = Some(to_js_string(get("pos").unwrap_or(&Value::Null)));
        }
        if truthy(get("img")) {
            vertex.img = Some(to_js_string(get("img").unwrap_or(&Value::Null)));
            if blank_label && vertex.text.as_deref() == Some(id) {
                vertex.text = Some(String::new());
            }
        }
        if truthy(get("constraint")) {
            vertex.constraint = Some(to_js_string(get("constraint").unwrap_or(&Value::Null)));
        }
        if truthy(get("w")) {
            vertex.asset_width = Some(to_number(get("w")));
        }
        if truthy(get("h")) {
            vertex.asset_height = Some(to_number(get("h")));
        }
        Ok(())
    }

    /// Ports `addSingleLink`.
    pub fn add_single_link(
        &mut self,
        start: &str,
        end: &str,
        info: Option<&LinkInfo>,
        id: Option<&str>,
    ) -> Result<(), PeleError> {
        let mut edge = FlowEdge {
            start: start.to_string(),
            end: end.to_string(),
            kind: None,
            text: String::new(),
            label_type: LabelType::Text,
            stroke: None,
            length: None,
            style: None,
            classes: Vec::new(),
            id: None,
            is_user_defined_id: false,
            interpolate: self.default_interpolate.clone(),
            animate: None,
            animation: None,
        };
        if let Some(text) = info.and_then(|i| i.text.as_ref()) {
            edge.text = unquote(js::trim(&text.text));
            edge.label_type = label_type(Some(text.kind.as_str()));
        }
        if let Some(i) = info {
            edge.kind = Some(i.kind.clone());
            edge.stroke = Some(i.stroke.clone());
            edge.length = match i.length {
                Some(l) if l > 10.0 => Some(10.0),
                l => l,
            };
        }
        let pair = format!("{}\0{}", start, end);
        let count = self.pair_count.get(&pair).copied().unwrap_or(0);
        match id {
            Some(id) if !id.is_empty() && !self.edge_by_id.contains_key(id) => {
                edge.id = Some(id.to_string());
                edge.is_user_defined_id = true;
            }
            _ => {
                edge.id = Some(format!(
                    "L_{}_{}_{}",
                    start,
                    end,
                    if count == 0 { 0 } else { count + 1 }
                ));
            }
        }
        if let Some(max) = self.options.max_edges {
            if self.edges.len() >= max {
                return Err(PeleError::of(
                    format!("Edge limit exceeded. The limit is {}.", max),
                    PeleErrorCode::Limit,
                    "flowchart",
                ));
            }
        }
        let eid = edge.id.clone().unwrap_or_default();
        self.edges.push(edge);
        self.pair_count.insert(pair, count + 1);
        if !self.edge_by_id.contains_key(&eid) {
            self.edge_by_id.insert(eid, self.edges.len() - 1);
        }
        Ok(())
    }

    /// Ports `addLink`.
    pub fn add_link(
        &mut self,
        starts: &[String],
        ends: &[String],
        link: Option<&LinkInfo>,
    ) -> Result<(), PeleError> {
        let id = link
            .and_then(|l| l.id.as_deref())
            .map(|s| js::replace_first(s, "@", ""));
        let last_start = starts.last().cloned().unwrap_or_default();
        let first_end = ends.first().cloned().unwrap_or_default();
        for start in starts {
            for end in ends {
                let this_id = if *start == last_start && *end == first_end {
                    id.as_deref()
                } else {
                    None
                };
                self.add_single_link(start, end, link, this_id)?;
            }
        }
        Ok(())
    }

    /// Ports `updateLinkInterpolate`.
    pub fn update_link_interpolate(
        &mut self,
        positions: &[String],
        interpolate: &str,
    ) -> Result<(), PeleError> {
        for pos in positions {
            if pos == "default" {
                self.default_interpolate = Some(interpolate.to_string());
            } else {
                let i = self.edge_at(pos)?;
                self.edges[i].interpolate = Some(interpolate.to_string());
            }
        }
        Ok(())
    }

    /// Ports `updateLink`. The style list is one array shared by every
    /// position in Pele, so 'fill:none' added for one is there for all.
    pub fn update_link(
        &mut self,
        positions: &[String],
        mut style: Vec<String>,
    ) -> Result<(), PeleError> {
        let numeric = positions.iter().any(|p| p != "default");
        if numeric && !style.is_empty() && !style.iter().any(|s| s.starts_with("fill")) {
            style.push("fill:none".to_string());
        }
        for pos in positions {
            if pos == "default" {
                self.default_edge_style = Some(style.clone());
                continue;
            }
            let i = self.edge_at(pos)?;
            self.edges[i].style = Some(style.clone());
        }
        Ok(())
    }

    /// Ports `edgeAt`.
    fn edge_at(&self, pos: &str) -> Result<usize, PeleError> {
        let n = js::number(pos);
        if n >= 0.0 && n.fract() == 0.0 && (n as usize) < self.edges.len() {
            return Ok(n as usize);
        }
        Err(sem(format!(
            "The index {} for linkStyle is out of bounds. Valid indices for linkStyle are between 0 and {}. (Help: Ensure that the index is within the range of existing edges.)",
            pos,
            self.edges.len() as i64 - 1
        )))
    }

    /// Ports `addClass`.
    pub fn add_class(&mut self, ids: &str, styles: &[String]) {
        let joined = styles.join(",");
        let style: Vec<String> = joined
            .replace("\\,", "§§§")
            .replace(',', ";")
            .replace("§§§", ",")
            .split(';')
            .map(|s| s.to_string())
            .collect();
        for id in ids.split(',') {
            let ci = match self.class_index.get(id) {
                Some(&i) => i,
                None => {
                    self.classes.push(FlowClassDef {
                        id: id.to_string(),
                        styles: Vec::new(),
                        text_styles: Vec::new(),
                    });
                    self.class_index
                        .insert(id.to_string(), self.classes.len() - 1);
                    self.classes.len() - 1
                }
            };
            let class = &mut self.classes[ci];
            for s in &style {
                if s.contains("color") {
                    class
                        .text_styles
                        .push(js::replace_first(s, "fill", "bgFill"));
                }
                class.styles.push(s.clone());
            }
        }
    }

    /// Ports `setDirection`.
    pub fn set_direction(&mut self, dir: &str) {
        let mut d = js::trim(dir).to_string();
        if d.contains('<') {
            d = "RL".to_string();
        }
        if d.contains('^') {
            d = "BT".to_string();
        }
        if d.contains('>') {
            d = "LR".to_string();
        }
        if d.contains('v') {
            d = "TB".to_string();
        }
        if d == "TD" {
            d = "TB".to_string();
        }
        self.direction = Some(d);
    }

    /// Ports `setClass`.
    pub fn set_class(&mut self, ids: &str, class_name: &str) {
        for id in ids.split(',') {
            if let Some(&i) = self.node_index.get(id) {
                self.nodes[i].classes.push(class_name.to_string());
            }
            if let Some(&i) = self.edge_by_id.get(id) {
                self.edges[i].classes.push(class_name.to_string());
            }
            if let Some(&i) = self.subgraph_lookup.get(id) {
                self.subgraphs[i].classes.push(class_name.to_string());
            }
        }
    }

    /// Ports `setTooltip`.
    pub fn set_tooltip(&mut self, ids: &str, tooltip: Option<&str>) {
        let Some(tooltip) = tooltip else { return };
        for id in ids.split(',') {
            self.tooltips.insert(id.to_string(), tooltip.to_string());
        }
    }

    /// Ports `setLink`.
    pub fn set_link(&mut self, ids: &str, link: &str, target: Option<&str>) {
        let url = js::trim(link);
        for id in ids.split(',') {
            if let Some(&i) = self.node_index.get(id) {
                let v = &mut self.nodes[i];
                v.link = if url.is_empty() {
                    None
                } else {
                    Some(safe_url(url))
                };
                v.link_target = target.map(|t| t.to_string());
            }
        }
        self.set_class(ids, "clickable");
    }

    /// Ports `setClickEvent`: callbacks are never executed; the node is
    /// only marked clickable, as in Mermaid's strict mode.
    pub fn set_click_event(&mut self, ids: &str) {
        self.set_class(ids, "clickable");
    }

    /// Ports `setAccTitle`.
    pub fn set_acc_title(&mut self, text: &str) {
        self.acc_title = Some(js::trim_start(text).to_string());
    }

    /// Ports `setAccDescription`: text.replace(/\n\s+/g, '\n').
    pub fn set_acc_description(&mut self, text: &str) {
        let mut out = String::with_capacity(text.len());
        let mut it = text.chars().peekable();
        while let Some(c) = it.next() {
            out.push(c);
            if c == '\n' {
                while it.peek().is_some_and(|c| js::is_space(*c)) {
                    it.next();
                }
            }
        }
        self.acc_descr = Some(out);
    }

    /// Ports `addSubGraph`.
    pub fn add_sub_graph(
        &mut self,
        id_text: Option<&FlowText>,
        list: &[DocItem],
        title_text: Option<&FlowText>,
    ) -> String {
        let mut id: Option<String> = id_text.map(|t| js::trim(&t.text).to_string());
        let mut title = title_text.map(|t| t.text.clone()).unwrap_or_default();
        if id_text.is_some_and(|t| t.same_as_title) && title.chars().any(js::is_space) {
            id = None;
        }

        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut node_list: Vec<String> = Vec::new();
        let mut dir: Option<String> = None;
        for entry in list {
            match entry {
                DocItem::Text(s) => {
                    if !js::trim(s).is_empty() && !seen.contains(s) {
                        seen.insert(s.clone());
                        node_list.push(s.clone());
                    }
                }
                DocItem::List(items) => {
                    for item in items {
                        if !js::trim(item).is_empty() && !seen.contains(item) {
                            seen.insert(item.clone());
                            node_list.push(item.clone());
                        }
                    }
                }
                DocItem::Dir(v) => dir = Some(v.clone()),
            }
        }
        if dir.is_none() && self.options.inherit_dir {
            dir = self
                .direction
                .clone()
                .or_else(|| self.options.direction.clone());
        }

        let id = id.unwrap_or_else(|| format!("subGraph{}", self.sub_count));
        if title.is_empty() {
            title = String::new();
        }
        self.sub_count += 1;

        // A node belongs to the first subgraph that lists it, and a subgraph never contains itself.
        let nodes: Vec<String> = node_list
            .into_iter()
            .filter(|n| !self.members.contains(n) && *n != id)
            .collect();
        for n in &nodes {
            self.members.insert(n.clone());
        }
        match self.subgraph_lookup.get(&id) {
            Some(&existing) => {
                self.subgraphs[existing].nodes.extend(nodes);
            }
            None => {
                self.subgraphs.push(FlowSubgraph {
                    id: id.clone(),
                    nodes,
                    title: js::trim(&title).to_string(),
                    classes: Vec::new(),
                    dir,
                    label_type: label_type(title_text.map(|t| t.kind.as_str())),
                    metadata: None,
                });
                self.subgraph_lookup
                    .insert(id.clone(), self.subgraphs.len() - 1);
            }
        }
        id
    }

    /// Ports `destructLink`.
    pub fn destruct_link(&self, end: &str, start: Option<&str>) -> LinkInfo {
        let info = destruct_end_link(end);
        let Some(start) = start.filter(|s| !s.is_empty()) else {
            return info;
        };
        let invalid = LinkInfo {
            kind: "INVALID".to_string(),
            stroke: "INVALID".to_string(),
            length: None,
            text: None,
            id: None,
        };
        let (mut kind, stroke) = destruct_start_link(start);
        if stroke != info.stroke {
            return invalid;
        }
        if kind == "arrow_open" {
            kind = info.kind.clone();
        } else {
            if kind != info.kind {
                return invalid;
            }
            kind = format!("double_{}", kind);
        }
        if kind == "double_arrow" {
            kind = "double_arrow_point".to_string();
        }
        LinkInfo {
            kind,
            stroke,
            length: info.length,
            text: None,
            id: None,
        }
    }

    /// Ports `subgraph`.
    pub fn subgraph(&self, id: &str) -> Option<&FlowSubgraph> {
        self.subgraph_lookup.get(id).map(|&i| &self.subgraphs[i])
    }
}

/// `Number(value)`.
fn to_number(v: Option<&Value>) -> f64 {
    match v {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(b)) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        Some(Value::Num(n)) => *n,
        Some(Value::Str(s)) => js::number(s),
        Some(other) => js::number(&to_js_string(other)),
    }
}
