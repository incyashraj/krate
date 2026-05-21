//! Phase 3 layout wrapper.
//!
//! This crate owns the first runtime-facing layout path. It maps the shared
//! widget tree model into Taffy, then returns stable widget-id keyed rectangles
//! that native widgets and drawn fallbacks can both use.

use std::collections::BTreeMap;

use layer36_adapter_common::ui::{WidgetId, WidgetKind, WidgetNode, WidgetStyle, WidgetTree};
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

/// Logical point used for hit testing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutPoint {
    pub x: f32,
    pub y: f32,
}

impl LayoutPoint {
    /// Create a validated logical point.
    pub fn new(x: f32, y: f32) -> Result<Self, LayoutError> {
        validate_finite("point x", x)?;
        validate_finite("point y", y)?;
        Ok(Self { x, y })
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

impl ComputedRect {
    /// Return whether a point is inside this rectangle.
    pub fn contains(self, point: LayoutPoint) -> bool {
        point.x >= self.x
            && point.y >= self.y
            && point.x < self.x + self.width
            && point.y < self.y + self.height
    }
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

/// Hit-test result for a computed layout snapshot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HitTestResult {
    pub widget: WidgetId,
    pub rect: ComputedRect,
    pub depth: usize,
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
    let mut prepared = PreparedLayoutTree::new(tree)?;
    prepared.compute(viewport)
}

/// Prepared Taffy tree for repeated layout passes of the same widget tree.
pub struct PreparedLayoutTree {
    root_widget: WidgetId,
    root_node: NodeId,
    root_kind: WidgetKind,
    root_style: WidgetStyle,
    taffy: TaffyTree<()>,
    node_map: BTreeMap<WidgetId, NodeId>,
}

impl PreparedLayoutTree {
    /// Build a reusable layout tree from the shared widget tree model.
    pub fn new(tree: &WidgetTree) -> Result<Self, LayoutError> {
        let root_widget = tree.root();
        let root = tree.node(root_widget).ok_or(LayoutError::MissingWidget {
            id: root_widget.get(),
        })?;
        let root_kind = root.kind;
        let root_style = root.style;
        let prepared_viewport = LayoutViewport {
            width: 1.0,
            height: 1.0,
        };
        let mut taffy = TaffyTree::<()>::new();
        let mut node_map = BTreeMap::new();
        let child_index = child_index(tree);
        let root_node = build_taffy_node(
            tree,
            root_widget,
            prepared_viewport,
            &child_index,
            &mut taffy,
            &mut node_map,
        )?;

        Ok(Self {
            root_widget,
            root_node,
            root_kind,
            root_style,
            taffy,
            node_map,
        })
    }

    /// Compute layout using the prepared tree.
    pub fn compute(&mut self, viewport: LayoutViewport) -> Result<LayoutSnapshot, LayoutError> {
        let root_style = taffy_style_from_parts(self.root_kind, self.root_style, true, viewport);
        self.taffy
            .set_style(self.root_node, root_style)
            .map_err(map_taffy)?;
        self.taffy
            .compute_layout(
                self.root_node,
                Size {
                    width: AvailableSpace::Definite(viewport.width),
                    height: AvailableSpace::Definite(viewport.height),
                },
            )
            .map_err(map_taffy)?;

        let mut rects = BTreeMap::new();
        for (&widget, &node) in &self.node_map {
            let layout = self.taffy.layout(node).map_err(map_taffy)?;
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

        Ok(LayoutSnapshot {
            root: self.root_widget,
            rects,
        })
    }
}

/// Compute layout by rebuilding the engine tree for one pass.
///
/// This is useful for smoke paths and cold benchmarks. Repeated layout should
/// use [`PreparedLayoutTree`] instead.
pub fn compute_layout_cold(
    tree: &WidgetTree,
    viewport: LayoutViewport,
) -> Result<LayoutSnapshot, LayoutError> {
    let mut taffy = TaffyTree::<()>::new();
    let root = tree.root();
    let mut node_map = BTreeMap::new();
    let child_index = child_index(tree);
    let root_node = build_taffy_node(
        tree,
        root,
        viewport,
        &child_index,
        &mut taffy,
        &mut node_map,
    )?;

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

/// Return a widget rectangle in root-window coordinates.
pub fn absolute_rect(
    tree: &WidgetTree,
    snapshot: &LayoutSnapshot,
    widget: WidgetId,
) -> Option<ComputedRect> {
    let mut rect = snapshot.rect(widget)?;
    let mut cursor = tree.node(widget)?.parent;
    while let Some(parent) = cursor {
        let parent_rect = snapshot.rect(parent)?;
        rect.x += parent_rect.x;
        rect.y += parent_rect.y;
        cursor = tree.node(parent)?.parent;
    }
    Some(rect)
}

/// Find the deepest widget that contains the point.
pub fn hit_test(
    tree: &WidgetTree,
    snapshot: &LayoutSnapshot,
    point: LayoutPoint,
) -> Option<HitTestResult> {
    let mut best = None;
    for &widget in snapshot.rects().keys() {
        let rect = absolute_rect(tree, snapshot, widget)?;
        if !rect.contains(point) {
            continue;
        }
        let depth = widget_depth(tree, widget)?;
        let candidate = HitTestResult {
            widget,
            rect,
            depth,
        };
        if best.is_none_or(|current: HitTestResult| {
            candidate.depth > current.depth
                || (candidate.depth == current.depth && candidate.widget > current.widget)
        }) {
            best = Some(candidate);
        }
    }
    best
}

fn child_index(tree: &WidgetTree) -> BTreeMap<WidgetId, Vec<WidgetId>> {
    let mut children: BTreeMap<WidgetId, Vec<WidgetId>> = BTreeMap::new();
    for node in tree.nodes().values() {
        if let Some(parent) = node.parent {
            children.entry(parent).or_default().push(node.id);
        }
    }
    children
}

fn build_taffy_node(
    tree: &WidgetTree,
    widget: WidgetId,
    viewport: LayoutViewport,
    child_index: &BTreeMap<WidgetId, Vec<WidgetId>>,
    taffy: &mut TaffyTree<()>,
    node_map: &mut BTreeMap<WidgetId, NodeId>,
) -> Result<NodeId, LayoutError> {
    let node = tree
        .node(widget)
        .ok_or(LayoutError::MissingWidget { id: widget.get() })?;
    let children = child_index
        .get(&widget)
        .into_iter()
        .flatten()
        .map(|&child| build_taffy_node(tree, child, viewport, child_index, taffy, node_map))
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
    taffy_style_from_parts(node.kind, node.style, is_root, viewport)
}

fn taffy_style_from_parts(
    kind: WidgetKind,
    widget_style: WidgetStyle,
    is_root: bool,
    viewport: LayoutViewport,
) -> Style {
    let mut size = Size {
        width: dimension_from_option(widget_style.width),
        height: dimension_from_option(widget_style.height),
    };
    if is_root {
        size = Size {
            width: Dimension::from_length(viewport.width),
            height: Dimension::from_length(viewport.height),
        };
    }

    Style {
        display: Display::Flex,
        flex_direction: flex_direction_for(kind),
        flex_grow: widget_style.grow,
        size,
        padding: Rect {
            left: LengthPercentage::length(widget_style.padding),
            right: LengthPercentage::length(widget_style.padding),
            top: LengthPercentage::length(widget_style.padding),
            bottom: LengthPercentage::length(widget_style.padding),
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
    validate_finite(field, value)?;
    if value <= 0.0 {
        return Err(LayoutError::InvalidDimension {
            field,
            value: value.to_string(),
        });
    }

    Ok(())
}

fn validate_finite(field: &'static str, value: f32) -> Result<(), LayoutError> {
    if !value.is_finite() {
        return Err(LayoutError::InvalidDimension {
            field,
            value: value.to_string(),
        });
    }

    Ok(())
}

fn widget_depth(tree: &WidgetTree, widget: WidgetId) -> Option<usize> {
    let mut depth = 0;
    let mut cursor = tree.node(widget)?.parent;
    while let Some(parent) = cursor {
        depth += 1;
        cursor = tree.node(parent)?.parent;
    }
    Some(depth)
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

    #[test]
    fn returns_absolute_rects_for_nested_children() {
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
            absolute_rect(&tree, &layout, WidgetId::new(3).expect("child")),
            Some(ComputedRect {
                x: 8.0,
                y: 8.0,
                width: 100.0,
                height: 24.0,
            })
        );
    }

    #[test]
    fn hit_test_returns_deepest_widget_containing_point() {
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
        let hit =
            hit_test(&tree, &layout, LayoutPoint::new(16.0, 16.0).expect("point")).expect("hit");

        assert_eq!(hit.widget, WidgetId::new(3).expect("child"));
        assert_eq!(hit.depth, 2);
    }

    #[test]
    fn hit_test_returns_none_outside_root() {
        let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Stack);
        let tree = WidgetTree::new(root).expect("tree");
        let layout = compute_layout(&tree, LayoutViewport::new(100.0, 100.0).expect("viewport"))
            .expect("layout");

        assert_eq!(
            hit_test(
                &tree,
                &layout,
                LayoutPoint::new(120.0, 40.0).expect("point")
            ),
            None
        );
    }

    #[test]
    fn computes_100_generated_layout_shapes() {
        for shape in 0..100 {
            let tree = generated_tree(shape);
            let viewport = LayoutViewport::new(
                240.0 + f32::from(shape % 9) * 13.0,
                180.0 + f32::from(shape % 7) * 17.0,
            )
            .expect("viewport");
            let layout = compute_layout(&tree, viewport).expect("layout");
            let root_rect = layout.rect(tree.root()).expect("root rect");

            assert_eq!(layout.rects().len(), tree.nodes().len(), "shape {shape}");
            assert_eq!(root_rect.width, viewport.width, "shape {shape}");
            assert_eq!(root_rect.height, viewport.height, "shape {shape}");
        }
    }

    #[test]
    fn prepared_tree_recomputes_with_new_viewport() {
        let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Stack);
        let mut tree = WidgetTree::new(root).expect("tree");
        tree.upsert(growing_child(2, tree.root())).expect("child");
        let mut prepared = PreparedLayoutTree::new(&tree).expect("prepared");

        let first = prepared
            .compute(LayoutViewport::new(100.0, 120.0).expect("viewport"))
            .expect("first layout");
        let second = prepared
            .compute(LayoutViewport::new(240.0, 300.0).expect("viewport"))
            .expect("second layout");

        assert_eq!(
            first.rect(tree.root()).expect("first root"),
            ComputedRect {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 120.0,
            }
        );
        assert_eq!(
            second.rect(tree.root()).expect("second root"),
            ComputedRect {
                x: 0.0,
                y: 0.0,
                width: 240.0,
                height: 300.0,
            }
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

    fn generated_tree(shape: u8) -> WidgetTree {
        let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Stack);
        let mut tree = WidgetTree::new(root).expect("tree");
        let node_count = 8 + usize::from(shape % 17);
        let branch = 2 + u64::from(shape % 4);

        for id in 2..=node_count as u64 {
            let parent = WidgetId::new(((id - 2) / branch) + 1).expect("parent");
            let kind = match (id + u64::from(shape)) % 9 {
                0 => WidgetKind::Stack,
                1 => WidgetKind::Scroll,
                2 => WidgetKind::ListView,
                3 => WidgetKind::Button,
                4 => WidgetKind::TextField,
                5 => WidgetKind::TextArea,
                6 => WidgetKind::Checkbox,
                7 => WidgetKind::Canvas,
                _ => WidgetKind::Text,
            };
            let style = WidgetStyle {
                width: ((id + u64::from(shape)) % 3 == 0)
                    .then_some(40.0 + f32::from((id % 7) as u8) * 11.0),
                height: ((id + u64::from(shape)) % 4 == 0)
                    .then_some(20.0 + f32::from((id % 5) as u8) * 7.0),
                grow: if id % 5 == 0 { 1.0 } else { 0.0 },
                padding: f32::from(((id + u64::from(shape)) % 3) as u8),
            };
            let node = WidgetNode::new(WidgetId::new(id).expect("id"), kind)
                .with_parent(parent)
                .with_style(style)
                .expect("style");
            tree.upsert(node).expect("node");
        }

        tree
    }
}
