//! Ports diagrams/flowchart/graph.ts: resolves a parsed flowchart into the
//! nodes and edges that are actually drawn -- group membership, collapsed
//! subgraphs, class styles and edge defaults.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use super::db::FlowDb;
use super::types::{FlowNode, LabelType};
use crate::json::Value;

/// Ports `interface GraphNode`.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphNode {
    pub id: String,
    pub label: Option<String>,
    pub label_type: LabelType,
    pub shape: String,
    pub is_group: bool,
    pub parent_id: Option<String>,
    pub css_styles: Vec<String>,
    pub css_compiled_styles: Vec<String>,
    pub css_classes: String,
    pub dir: Option<String>,
    pub link: Option<String>,
    pub link_target: Option<String>,
    pub tooltip: Option<String>,
    pub icon: Option<String>,
    pub pos: Option<String>,
    pub img: Option<String>,
    pub asset_width: Option<f64>,
    pub asset_height: Option<f64>,
    pub constraint: Option<String>,
    pub color_index: Option<usize>,
    pub metadata: Option<Vec<(String, Value)>>,
}

/// Ports `interface GraphEdge`. `kind` is `type`.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphEdge {
    pub id: String,
    pub is_user_defined_id: bool,
    pub start: String,
    pub end: String,
    pub kind: String,
    pub label: String,
    pub label_type: LabelType,
    pub thickness: Option<String>,
    pub pattern: Option<String>,
    pub minlen: Option<f64>,
    pub arrow_type_start: String,
    pub arrow_type_end: String,
    pub css_compiled_styles: Vec<String>,
    pub style: Vec<String>,
    pub classes: String,
    pub curve: Option<String>,
}

/// Ports `interface FlowGraph`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FlowGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

/// Ports `shapeOf`.
fn shape_of(vertex: &FlowNode) -> String {
    if vertex.img.as_deref().is_some_and(|s| !s.is_empty()) {
        return "imageSquare".to_string();
    }
    if vertex.icon.as_deref().is_some_and(|s| !s.is_empty()) {
        return match vertex.form.as_deref() {
            Some("circle") => "iconCircle",
            Some("square") => "iconSquare",
            Some("rounded") => "iconRounded",
            _ => "icon",
        }
        .to_string();
    }
    match vertex.kind.as_deref() {
        Some("square") | None => "squareRect".to_string(),
        Some("round") => "roundedRect".to_string(),
        Some(k) => k.to_string(),
    }
}

/// Ports `arrows`.
fn arrows(kind: Option<&str>) -> (String, String) {
    match kind {
        Some(k @ ("arrow_point" | "arrow_circle" | "arrow_cross")) => {
            ("none".to_string(), k.to_string())
        }
        Some(k @ ("double_arrow_point" | "double_arrow_circle" | "double_arrow_cross")) => {
            let single = k.replacen("double_", "", 1);
            (single.clone(), single)
        }
        _ => ("none".to_string(), "arrow_point".to_string()),
    }
}

fn is_collapsed_meta(meta: &Option<Vec<(String, Value)>>) -> bool {
    meta.as_ref()
        .and_then(|m| m.iter().find(|(k, _)| k == "view"))
        .is_some_and(|(_, v)| *v == Value::Str("collapsed".to_string()))
}

/// Ports `buildFlowGraph`.
pub fn build_flow_graph(model: &FlowDb, default_curve: Option<&str>) -> FlowGraph {
    let mut nodes: Vec<GraphNode> = Vec::new();
    let mut edges: Vec<GraphEdge> = Vec::new();
    let subgraphs = &model.subgraphs;

    let compiled = |class_names: &[String]| -> Vec<String> {
        let mut out = Vec::new();
        for name in class_names {
            let Some(def) = model.class(name) else {
                continue;
            };
            for s in &def.styles {
                out.push(crate::pele::js::trim(s).to_string());
            }
            for s in &def.text_styles {
                out.push(crate::pele::js::trim(s).to_string());
            }
        }
        out
    };

    let mut subgraph_parent: BTreeMap<String, String> = BTreeMap::new();
    for sg in subgraphs {
        for child in &sg.nodes {
            if model.subgraph(child).is_some() {
                subgraph_parent.insert(child.clone(), sg.id.clone());
            }
        }
    }

    let mut declaration_index: BTreeMap<String, usize> = BTreeMap::new();
    let mut children_of: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for sg in subgraphs {
        let Some(parent) = subgraph_parent.get(&sg.id) else {
            continue;
        };
        children_of
            .entry(parent.clone())
            .or_default()
            .push(sg.id.clone());
    }
    let mut next_index = 0;
    for sg in subgraphs {
        if subgraph_parent.contains_key(&sg.id) {
            continue;
        }
        let mut pending = vec![sg.id.clone()];
        while let Some(id) = pending.pop() {
            if declaration_index.contains_key(&id) {
                continue;
            }
            declaration_index.insert(id.clone(), next_index);
            next_index += 1;
            if let Some(kids) = children_of.get(&id) {
                for k in kids.iter().rev() {
                    pending.push(k.clone());
                }
            }
        }
    }

    let is_collapsed = |id: &str| {
        model
            .subgraph(id)
            .is_some_and(|sg| is_collapsed_meta(&sg.metadata))
    };
    let any_collapsed = subgraphs.iter().any(|sg| is_collapsed_meta(&sg.metadata));
    // The outermost collapsed subgraph around each subgraph, resolved once per chain of parents.
    let mut outermost: BTreeMap<String, Option<String>> = BTreeMap::new();
    let outermost_collapsed =
        |id: &str, outermost: &mut BTreeMap<String, Option<String>>| -> Option<String> {
            let mut chain: Vec<String> = Vec::new();
            let mut seen: BTreeSet<String> = BTreeSet::new();
            let mut current: Option<String> = Some(id.to_string());
            while let Some(c) = current.clone() {
                if outermost.contains_key(&c) || seen.contains(&c) {
                    break;
                }
                seen.insert(c.clone());
                chain.push(c.clone());
                current = subgraph_parent.get(&c).cloned();
            }
            let mut result = match &current {
                Some(c) => outermost.get(c).cloned().flatten(),
                None => None,
            };
            for c in chain.iter().rev() {
                if result.is_none() && is_collapsed(c) {
                    result = Some(c.clone());
                }
                outermost.insert(c.clone(), result.clone());
            }
            result
        };

    let mut hidden: BTreeSet<String> = BTreeSet::new();
    let mut collapsed_into: BTreeMap<String, String> = BTreeMap::new();
    if any_collapsed {
        for sg in subgraphs {
            let Some(ancestor) = outermost_collapsed(&sg.id, &mut outermost) else {
                continue;
            };
            if sg.id != ancestor {
                hidden.insert(sg.id.clone());
                collapsed_into.insert(sg.id.clone(), ancestor.clone());
            }
            for child in &sg.nodes {
                if *child == ancestor {
                    continue;
                }
                hidden.insert(child.clone());
                collapsed_into.insert(child.clone(), ancestor.clone());
            }
        }
    }

    let mut parent_of: BTreeMap<String, String> = BTreeMap::new();
    let mut groups: BTreeSet<String> = BTreeSet::new();
    for sg in subgraphs.iter().rev() {
        if hidden.contains(&sg.id) {
            continue;
        }
        if !sg.nodes.is_empty() {
            groups.insert(sg.id.clone());
        }
        for id in &sg.nodes {
            parent_of.insert(id.clone(), sg.id.clone());
        }
    }

    let mut by_id: BTreeMap<String, usize> = BTreeMap::new();
    for sg in subgraphs.iter().rev() {
        if hidden.contains(&sg.id) {
            continue;
        }
        let collapsed = is_collapsed_meta(&sg.metadata);
        nodes.push(GraphNode {
            id: sg.id.clone(),
            label: Some(sg.title.clone()),
            label_type: sg.label_type,
            parent_id: parent_of.get(&sg.id).cloned(),
            css_styles: Vec::new(),
            css_compiled_styles: compiled(&sg.classes),
            css_classes: sg.classes.join(" "),
            shape: if collapsed { "collapsedGroup" } else { "rect" }.to_string(),
            dir: sg.dir.clone(),
            is_group: !collapsed,
            color_index: declaration_index.get(&sg.id).copied(),
            metadata: if collapsed { None } else { sg.metadata.clone() },
            link: None,
            link_target: None,
            tooltip: None,
            icon: None,
            pos: None,
            img: None,
            asset_width: None,
            asset_height: None,
            constraint: None,
        });
        by_id.insert(sg.id.clone(), nodes.len() - 1);
    }

    for vertex in &model.nodes {
        if hidden.contains(&vertex.id) {
            continue;
        }
        if let Some(&existing) = by_id.get(&vertex.id) {
            let n = &mut nodes[existing];
            n.css_styles = vertex.styles.clone();
            n.css_compiled_styles = compiled(&vertex.classes);
            n.css_classes = vertex.classes.join(" ");
            continue;
        }
        let is_group = groups.contains(&vertex.id);
        let mut names = vec!["default".to_string(), "node".to_string()];
        names.extend(vertex.classes.iter().cloned());
        nodes.push(GraphNode {
            id: vertex.id.clone(),
            label: vertex.text.clone(),
            label_type: vertex.label_type,
            parent_id: parent_of.get(&vertex.id).cloned(),
            css_styles: vertex.styles.clone(),
            css_compiled_styles: compiled(&names),
            css_classes: format!("default {}", vertex.classes.join(" ")),
            dir: vertex.dir.clone(),
            link: vertex.link.clone(),
            link_target: vertex.link_target.clone(),
            tooltip: model.tooltips.get(&vertex.id).cloned(),
            icon: vertex.icon.clone(),
            pos: vertex.pos.clone(),
            img: vertex.img.clone(),
            asset_width: vertex.asset_width,
            asset_height: vertex.asset_height,
            constraint: vertex.constraint.clone(),
            is_group,
            shape: if is_group {
                "rect".to_string()
            } else {
                shape_of(vertex)
            },
            color_index: None,
            metadata: None,
        });
    }

    for (index, raw) in model.edges.iter().enumerate() {
        let start = collapsed_into
            .get(&raw.start)
            .cloned()
            .unwrap_or_else(|| raw.start.clone());
        let end = collapsed_into
            .get(&raw.end)
            .cloned()
            .unwrap_or_else(|| raw.end.clone());
        if start == end
            && (collapsed_into.contains_key(&raw.start) || collapsed_into.contains_key(&raw.end))
        {
            continue;
        }
        let (arrow_start, arrow_end) = arrows(raw.kind.as_deref());
        let mut styles: Vec<String> = model.default_edge_style.clone().unwrap_or_default();
        if let Some(s) = &raw.style {
            styles.extend(s.iter().cloned());
        }
        let plain =
            raw.stroke.as_deref() == Some("invisible") || raw.kind.as_deref() == Some("arrow_open");
        let id = match raw.id.as_deref() {
            Some(id) if !id.is_empty() => id.to_string(),
            _ => format!("L_{}_{}_{}", start, end, index),
        };
        let curve = [
            raw.interpolate.as_deref(),
            model.default_interpolate.as_deref(),
        ]
        .into_iter()
        .flatten()
        .find(|c| !c.is_empty())
        .or(default_curve)
        .map(|s| s.to_string());
        edges.push(GraphEdge {
            id,
            is_user_defined_id: raw.is_user_defined_id,
            start,
            end,
            kind: raw.kind.clone().unwrap_or_else(|| "normal".to_string()),
            label: raw.text.clone(),
            label_type: raw.label_type,
            thickness: raw.stroke.clone(),
            pattern: raw.stroke.clone(),
            minlen: raw.length,
            classes: if raw.stroke.as_deref() == Some("invisible") {
                String::new()
            } else {
                "edge-thickness-normal edge-pattern-solid flowchart-link".to_string()
            },
            arrow_type_start: if plain {
                "none".to_string()
            } else {
                arrow_start
            },
            arrow_type_end: if plain { "none".to_string() } else { arrow_end },
            css_compiled_styles: compiled(&raw.classes),
            style: styles,
            curve,
        });
    }

    FlowGraph { nodes, edges }
}
