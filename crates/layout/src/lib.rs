//! Phase 3 layout wrapper.
//!
//! This crate owns the first runtime-facing layout path. It maps the shared
//! widget tree model into Taffy, then returns stable widget-id keyed rectangles
//! that native widgets and drawn fallbacks can both use.

use std::collections::BTreeMap;

use layer36_adapter_common::ui::{WidgetId, WidgetKind, WidgetNode, WidgetTree};
use taffy::prelude::*;
use taffy::TaffyError;
use thiserror::Error;

/// Logical window content size used for a layout pass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutViewport {
    pub width: f32,
    pub height: f32,
}

impl LayoutViewport {
    /// Create a validated logical viewport.
    pub fn new(width: f32, height: f32) -> Result<Self, LayoutError> {
        validate_dimension("viewport width", width)?;
        validate_dimension("viewport height", height)?;
        Ok(Self { width, height })
    }
}

/// Computed rectangle in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ComputedRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Stable layout result keyed by widget id.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutSnapshot {
    root: WidgetId,
    rects: BTreeMap<WidgetId, ComputedRect>,
}

impl LayoutSnapshot {
    /// Return the root widget id for this layout.
    pub fn root(&self) -> WidgetId {
        self.root
    }

    /// Return a rectangle for one widget id.
    pub fn rect(&self, id: WidgetId) -> Option<ComputedRect> {
        self.rects.get(&id).copied()
    }

    /// Return every computed rectangle.
    pub fn rects(&self) -> &BTreeMap<WidgetId, ComputedRect> {
        &self.rects
    }
}

/// Errors from the Phase 3 layout wrapper.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LayoutError {
    #[error("invalid layout dimension for {field}: {value}")]
    InvalidDimension { field: &'static str, value: String },
    #[error("widget {id} is missing from the widget tree")]
    MissingWidget { id: u64 },
    #[error("layout engine error: {0}")]
    Engine(String),
}

/// Compute layout for one window widget tree.
pub fn compute_layout(
    tree: &WidgetTree,
    viewport: LayoutViewport,
) -> Result<LayoutSnapshot, LayoutError> {
    let mut taffy = TaffyTree::<()>::new();
    let root = tree.root();
    let mut node_map = BTreeMap::new();
    let root_node = build_taffy_node(tree, root, viewport, &mut taffy, &mut node_map)?;

    taffy
        .compute_layout(
            root_node,
            Size {
                width: AvailableSpace::Definite(viewport.width),
                height: AvailableSpace::Definite(viewport.height),
            },
        )
        .map_err(map_taffy)?;

    let mut rects = BTreeMap::new();
    for (widget, node) in node_map {
        let layout = taffy.layout(node).map_err(map_taffy)?;
        rects.insert(
            widget,
            ComputedRect {
                x: layout.location.x,
                y: layout.location.y,
                width: layout.size.width,
                height: layout.size.height,
            },
        );
    }

    Ok(LayoutSnapshot { root, rects })
}

fn build_taffy_node(
    tree: &WidgetTree,
    widget: WidgetId,
    viewport: LayoutViewport,
    taffy: &mut TaffyTree<()>,
    node_map: &mut BTreeMap<WidgetId, NodeId>,
) -> Result<NodeId, LayoutError> {
    let node = tree
        .node(widget)
        .ok_or(LayoutError::MissingWidget { id: widget.get() })?;
    let children = tree
        .nodes()
        .values()
        .filter(|candidate| candidate.parent == Some(widget))
        .map(|child| build_taffy_node(tree, child.id, viewport, taffy, node_map))
        .collect::<Result<Vec<_>, _>>()?;
    let style = taffy_style_for(node, widget == tree.root(), viewport);
    let taffy_node = if children.is_empty() {
        taffy.new_leaf(style).map_err(map_taffy)?
    } else {
        taffy
            .new_with_children(style, &children)
            .map_err(map_taffy)?
    };

    node_map.insert(widget, taffy_node);
    Ok(taffy_node)
}

fn taffy_style_for(node: &WidgetNode, is_root: bool, viewport: LayoutViewport) -> Style {
    let mut size = Size {
        width: dimension_from_option(node.style.width),
        height: dimension_from_option(node.style.height),
    };
    if is_root {
        size = Size {
            width: Dimension::from_length(viewport.width),
            height: Dimension::from_length(viewport.height),
        };
    }

    Style {
        display: Display::Flex,
        flex_direction: flex_direction_for(node.kind),
        flex_grow: node.style.grow,
        size,
        padding: Rect {
            left: LengthPercentage::length(node.style.padding),
            right: LengthPercentage::length(node.style.padding),
            top: LengthPercentage::length(node.style.padding),
            bottom: LengthPercentage::length(node.style.padding),
        },
        ..Default::default()
    }
}

fn flex_direction_for(kind: WidgetKind) -> FlexDirection {
    match kind {
        WidgetKind::Stack | WidgetKind::Scroll | WidgetKind::ListView | WidgetKind::TreeView => {
            FlexDirection::Column
        }
        _ => FlexDirection::Row,
    }
}

fn dimension_from_option(value: Option<f32>) -> Dimension {
    value.map_or(Dimension::AUTO, Dimension::from_length)
}

fn validate_dimension(field: &'static str, value: f32) -> Result<(), LayoutError> {
    if !value.is_finite() || value <= 0.0 {
        return Err(LayoutError::InvalidDimension {
            field,
            value: value.to_string(),
        });
    }

    Ok(())
}

fn map_taffy(err: TaffyError) -> LayoutError {
    LayoutError::Engine(err.to_string())
}

#[cfg(test)]
mod tests {
    use layer36_adapter_common::ui::WidgetStyle;

    use super::*;

    #[test]
    fn lays_out_stack_children_in_stable_order() {
        let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Stack);
        let mut tree = WidgetTree::new(root).expect("tree");
        tree.upsert(fixed_child(2, tree.root(), 100.0, 40.0))
            .expect("first child");
        tree.upsert(fixed_child(3, tree.root(), 100.0, 60.0))
            .expect("second child");

        let layout = compute_layout(&tree, LayoutViewport::new(300.0, 200.0).expect("viewport"))
            .expect("layout");

        assert_eq!(
            layout.rect(WidgetId::new(1).expect("root")),
            Some(ComputedRect {
                x: 0.0,
                y: 0.0,
                width: 300.0,
                height: 200.0,
            })
        );
        assert_eq!(
            layout.rect(WidgetId::new(2).expect("first")),
            Some(ComputedRect {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 40.0,
            })
        );
        assert_eq!(
            layout.rect(WidgetId::new(3).expect("second")),
            Some(ComputedRect {
                x: 0.0,
                y: 40.0,
                width: 100.0,
                height: 60.0,
            })
        );
    }

    #[test]
    fn grows_children_to_fill_remaining_stack_space() {
        let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Stack);
        let mut tree = WidgetTree::new(root).expect("tree");
        tree.upsert(growing_child(2, tree.root()))
            .expect("first child");
        tree.upsert(growing_child(3, tree.root()))
            .expect("second child");

        let layout = compute_layout(&tree, LayoutViewport::new(100.0, 120.0).expect("viewport"))
            .expect("layout");

        assert_eq!(
            layout.rect(WidgetId::new(2).expect("first")),
            Some(ComputedRect {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 60.0,
            })
        );
        assert_eq!(
            layout.rect(WidgetId::new(3).expect("second")),
            Some(ComputedRect {
                x: 0.0,
                y: 60.0,
                width: 100.0,
                height: 60.0,
            })
        );
    }

    #[test]
    fn rejects_invalid_viewports() {
        assert_eq!(
            LayoutViewport::new(0.0, 100.0),
            Err(LayoutError::InvalidDimension {
                field: "viewport width",
                value: "0".to_string(),
            })
        );
    }

    #[test]
    fn lays_out_nested_children_with_parent_offsets() {
        let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Stack);
        let mut tree = WidgetTree::new(root).expect("tree");
        let container = WidgetNode::new(WidgetId::new(2).expect("container"), WidgetKind::Stack)
            .with_parent(tree.root())
            .with_style(WidgetStyle {
                width: Some(160.0),
                height: Some(80.0),
                padding: 8.0,
                ..WidgetStyle::default()
            })
            .expect("style");
        tree.upsert(container).expect("container");
        tree.upsert(fixed_child(
            3,
            WidgetId::new(2).expect("container"),
            100.0,
            24.0,
        ))
        .expect("child");

        let layout = compute_layout(&tree, LayoutViewport::new(300.0, 200.0).expect("viewport"))
            .expect("layout");

        assert_eq!(
            layout.rect(WidgetId::new(2).expect("container")),
            Some(ComputedRect {
                x: 0.0,
                y: 0.0,
                width: 160.0,
                height: 80.0,
            })
        );
        assert_eq!(
            layout.rect(WidgetId::new(3).expect("child")),
            Some(ComputedRect {
                x: 8.0,
                y: 8.0,
                width: 100.0,
                height: 24.0,
            })
        );
    }

    fn fixed_child(id: u64, parent: WidgetId, width: f32, height: f32) -> WidgetNode {
        WidgetNode::new(WidgetId::new(id).expect("id"), WidgetKind::Text)
            .with_parent(parent)
            .with_style(WidgetStyle {
                width: Some(width),
                height: Some(height),
                ..WidgetStyle::default()
            })
            .expect("style")
    }

    fn growing_child(id: u64, parent: WidgetId) -> WidgetNode {
        WidgetNode::new(WidgetId::new(id).expect("id"), WidgetKind::Text)
            .with_parent(parent)
            .with_style(WidgetStyle {
                grow: 1.0,
                ..WidgetStyle::default()
            })
            .expect("style")
    }
}
