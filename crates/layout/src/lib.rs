//! Phase 3 layout wrapper.
//!
//! This crate owns the first runtime-facing layout path. It maps the shared
//! widget tree model into Taffy, then returns stable widget-id keyed rectangles
//! that native widgets and drawn fallbacks can both use.

use std::collections::BTreeMap;

use krate_adapter_common::ui::{WidgetId, WidgetKind, WidgetNode, WidgetStyle, WidgetTree};
use taffy::prelude::*;
use taffy::TaffyError;
use thiserror::Error;

/// Height a content-sized widget falls back to when it has no explicit height
/// and the layout cannot measure its text. One standard control row: enough
/// that a label or a checkbox lowers to a real native view rather than a
/// zero-height one the host refuses.
const DEFAULT_CONTENT_ROW_HEIGHT: f32 = 24.0;

/// Space between a container's children when the app sets none. Zero-gap
/// stacks read as amateur -- rows touch, a title sits flat on the control
/// below it. One comfortable line of separation makes every app breathe
/// without any app asking. Overridden the moment an app sets its own padding,
/// which signals it is laying things out deliberately.
const DEFAULT_CONTAINER_GAP: f32 = 10.0;

/// Inset from the window edge for a top-level layout that is not a full-bleed
/// canvas. Content flush against the frame is the single biggest tell that a
/// UI was not designed; a margin fixes it everywhere at once.
const DEFAULT_ROOT_INSET: f32 = 14.0;

/// Whether a container's sole child is a drawing that should fill it edge to
/// edge. A canvas-only or image-only parent (a 3D game, a full-window photo)
/// must not get a border inset or an internal gap that would shrink the
/// drawing and leave a frame around it.
fn is_full_bleed_parent(
    tree: &WidgetTree,
    child_index: &BTreeMap<WidgetId, Vec<WidgetId>>,
    widget: WidgetId,
) -> bool {
    let Some(children) = child_index.get(&widget) else {
        return false;
    };
    children.len() == 1
        && tree
            .node(children[0])
            .map(|n| matches!(n.kind, WidgetKind::Canvas | WidgetKind::Image))
            .unwrap_or(false)
}

/// Widgets that are meant to fill the room given to them when the app sets no
/// size: a canvas an app paints, an image widget. Both collapse to a sliver
/// with zero grow and no size, which reads as a blank window.
fn wants_to_fill(kind: WidgetKind) -> bool {
    matches!(kind, WidgetKind::Canvas | WidgetKind::Image)
}

/// Containers whose children flow in a line and therefore benefit from a gap.
fn is_gap_container(kind: WidgetKind) -> bool {
    matches!(
        kind,
        WidgetKind::Stack
            | WidgetKind::Scroll
            | WidgetKind::ListView
            | WidgetKind::TreeView
            | WidgetKind::Grid
    )
}

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
    root_full_bleed: bool,
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
        let root_full_bleed = is_full_bleed_parent(tree, &child_index, root_widget);
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
            root_full_bleed,
            taffy,
            node_map,
        })
    }

    /// Compute layout using the prepared tree.
    pub fn compute(&mut self, viewport: LayoutViewport) -> Result<LayoutSnapshot, LayoutError> {
        let root_style = taffy_style_from_parts(
            self.root_kind,
            self.root_style,
            true,
            viewport,
            self.root_full_bleed,
        );
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
    // A tab strip shows one panel at a time. Everything else in the tree is
    // always laid out, so this is the only place visibility is decided.
    let hidden = parent_hides_this_child(tree, child_index, node);
    let full_bleed = is_full_bleed_parent(tree, child_index, widget);
    let mut style = taffy_style_for(node, widget == tree.root(), viewport, full_bleed);
    if hidden {
        style.display = Display::None;
    }
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

/// Whether this node is a `tabs` panel that is not the selected one.
///
/// `tabs` was declared in the widget set and implemented by no host, so an app
/// that asked for one was refused everywhere. The only thing it needs beyond
/// any other container is that unselected panels take no space -- the node
/// model already carries `selected`, and Taffy already has `Display::None`, so
/// the widget is a visibility rule rather than new drawing on three hosts.
///
/// A `tabs` with no `selected` shows its first panel, because a tab strip that
/// shows nothing looks broken rather than empty.
fn parent_hides_this_child(
    tree: &WidgetTree,
    child_index: &BTreeMap<WidgetId, Vec<WidgetId>>,
    node: &WidgetNode,
) -> bool {
    let Some(parent_id) = node.parent else {
        return false;
    };
    let Some(parent) = tree.node(parent_id) else {
        return false;
    };
    if parent.kind != WidgetKind::Tabs {
        return false;
    }
    let Some(siblings) = child_index.get(&parent_id) else {
        return false;
    };
    let Some(position) = siblings.iter().position(|id| *id == node.id) else {
        return false;
    };
    let selected = parent.selected.unwrap_or(0) as usize;
    position != selected
}

fn taffy_style_for(
    node: &WidgetNode,
    is_root: bool,
    viewport: LayoutViewport,
    full_bleed: bool,
) -> Style {
    taffy_style_from_parts(node.kind, node.style, is_root, viewport, full_bleed)
}

fn taffy_style_from_parts(
    kind: WidgetKind,
    widget_style: WidgetStyle,
    is_root: bool,
    viewport: LayoutViewport,
    full_bleed: bool,
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
        flex_wrap: flex_wrap_for(kind),
        // Content-sized widgets do not grow, whatever the app's boilerplate
        // asked for. A Text label given flex-grow eats half its column and
        // leaves a dead band -- which is exactly what a title above a canvas
        // did, because the common `node()` helper stamps grow: 1.0 on every
        // widget. Text hugs its line; a container or a canvas fills the room.
        // An explicit height still wins for anyone who genuinely wants a tall
        // label.
        flex_grow: if hugs_content(kind) && widget_style.height.is_none() {
            0.0
        } else if wants_to_fill(kind) && widget_style.grow == 0.0 && widget_style.height.is_none() {
            // A canvas or image with no size and no grow is almost certainly
            // meant to fill its space -- a full-window game, a photo viewer.
            // Left at grow 0 it collapses to nothing on the cross axis (a
            // 480x1 strip) and renders blank, which is a trap an app author
            // hits once and cannot see without a screenshot. Default it to
            // fill; an explicit size or grow still wins.
            1.0
        } else {
            widget_style.grow
        },
        // Widgets with an explicit height keep it: Taffy's default
        // flex_shrink of 1 would compress Scroll children to fit their
        // container instead of overflowing it, which makes scrolling
        // impossible.
        flex_shrink: if widget_style.height.is_some() && !is_root {
            0.0
        } else {
            1.0
        },
        // A content-sized widget that no longer grows must not collapse to
        // nothing. The layout has no font metrics, so it cannot measure a
        // line, but a control that lowers to a native view needs a real
        // height or the lowering is refused ("placement needs a non-zero
        // size"). This floor is one standard control row; a Text with its own
        // explicit height overrides it, and a growing container ignores it.
        min_size: if hugs_content(kind) && widget_style.height.is_none() {
            Size {
                width: auto(),
                height: Dimension::from_length(DEFAULT_CONTENT_ROW_HEIGHT),
            }
        } else {
            Size {
                width: auto(),
                height: auto(),
            }
        },
        size,
        // Padding: the app's own value wins. When it sets none, a top-level
        // non-canvas layout gets a window inset so content does not touch the
        // frame; a full-bleed canvas root and every inner widget get nothing,
        // so a game still fills the window and a button is not silently fattened.
        padding: {
            let inset = if widget_style.padding > 0.0 {
                widget_style.padding
            } else if is_root && !full_bleed {
                DEFAULT_ROOT_INSET
            } else {
                0.0
            };
            Rect {
                left: LengthPercentage::length(inset),
                right: LengthPercentage::length(inset),
                top: LengthPercentage::length(inset),
                bottom: LengthPercentage::length(inset),
            }
        },
        // Gap: a default line of space between a flowing container's children,
        // unless this is a full-bleed canvas (no gap around a single drawing)
        // or the app set its own padding (it is spacing things itself).
        gap: {
            let gap = if !full_bleed && widget_style.padding == 0.0 && is_gap_container(kind) {
                DEFAULT_CONTAINER_GAP
            } else {
                0.0
            };
            Size {
                width: LengthPercentage::length(gap),
                height: LengthPercentage::length(gap),
            }
        },
        ..Default::default()
    }
}

fn flex_direction_for(kind: WidgetKind) -> FlexDirection {
    if is_region_container(kind) {
        // A region the guest paints into stacks its children like any other
        // column container; the host simply never draws over them.
        return FlexDirection::Column;
    }
    match kind {
        WidgetKind::Stack | WidgetKind::Scroll | WidgetKind::ListView | WidgetKind::TreeView => {
            FlexDirection::Column
        }
        _ => FlexDirection::Row,
    }
}

/// Whether a container lets its children flow onto another line.
///
/// Only `grid` does. It was declared in the widget set and implemented by no
/// host, so an app that asked for one was refused on every system -- a promise
/// sitting in the codebase with nothing behind it. A grid is a row that wraps,
/// which the same layout engine and the same host container code already
/// handle, so it needs no new drawing anywhere.
fn flex_wrap_for(kind: WidgetKind) -> FlexWrap {
    match kind {
        WidgetKind::Grid => FlexWrap::Wrap,
        _ => FlexWrap::NoWrap,
    }
}

/// Containers that hold a region rather than painting one.
///
/// `canvas` is here because the guest draws its contents itself: the host gives
/// it a rectangle and stays out of it. It was accepted on macOS and refused on
/// Windows and Linux, which is worse than refusing it everywhere -- an app
/// built on the machine that allowed it would fail when it was shared, which is
/// the exact failure Krate exists to remove. Laying it out like any other
/// container is all three hosts need, because none of them paint it.
fn is_region_container(kind: WidgetKind) -> bool {
    matches!(kind, WidgetKind::Canvas)
}

/// Widgets sized by their content, which therefore should not flex-grow.
///
/// Text and its editable cousins have an intrinsic size; a checkbox, a radio,
/// a switch and a progress bar are fixed controls. None of them should stretch
/// to fill a column just because a boilerplate helper set grow on every node.
/// Containers, canvases, lists and images are not here: they are meant to fill
/// the space they are given.
fn hugs_content(kind: WidgetKind) -> bool {
    matches!(
        kind,
        WidgetKind::Text
            | WidgetKind::TextField
            | WidgetKind::Checkbox
            | WidgetKind::Radio
            | WidgetKind::Switch
            | WidgetKind::Progress
            | WidgetKind::Slider
    )
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
    use krate_adapter_common::ui::WidgetStyle;

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
        let first = layout
            .rect(WidgetId::new(2).expect("first"))
            .expect("first");
        let second = layout
            .rect(WidgetId::new(3).expect("second"))
            .expect("second");
        // Children are inset from the window frame and keep their sizes; order
        // is preserved. The exact numbers come from the default inset and gap,
        // asserted through the constants so the intent stays legible.
        assert_eq!((first.x, first.y), (DEFAULT_ROOT_INSET, DEFAULT_ROOT_INSET));
        assert_eq!((first.width, first.height), (100.0, 40.0));
        assert_eq!(second.x, DEFAULT_ROOT_INSET);
        assert_eq!(second.y, DEFAULT_ROOT_INSET + 40.0 + DEFAULT_CONTAINER_GAP);
        assert_eq!((second.width, second.height), (100.0, 60.0));
    }

    #[test]
    fn a_text_title_above_a_canvas_hugs_its_line_and_gives_the_rest_to_the_canvas() {
        // The chart bug, pinned. A title and a canvas, both stamped grow: 1.0
        // by the common node() helper. Before the fix they split the window
        // in half and the canvas drew into a dead band; now the text hugs its
        // line and the canvas fills what is left.
        let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Stack);
        let mut tree = WidgetTree::new(root).expect("tree");
        tree.upsert(
            WidgetNode::new(WidgetId::new(2).expect("title"), WidgetKind::Text)
                .with_parent(tree.root())
                .with_style(WidgetStyle {
                    grow: 1.0,
                    ..WidgetStyle::default()
                })
                .expect("title style"),
        )
        .expect("title");
        tree.upsert(
            WidgetNode::new(WidgetId::new(3).expect("canvas"), WidgetKind::Canvas)
                .with_parent(tree.root())
                .with_style(WidgetStyle {
                    grow: 1.0,
                    ..WidgetStyle::default()
                })
                .expect("canvas style"),
        )
        .expect("canvas");

        let layout = compute_layout(&tree, LayoutViewport::new(240.0, 200.0).expect("viewport"))
            .expect("layout");
        let title = layout
            .rect(WidgetId::new(2).expect("title"))
            .expect("title rect");
        let canvas = layout
            .rect(WidgetId::new(3).expect("canvas"))
            .expect("canvas rect");

        assert!(
            title.height < 40.0,
            "the title should hug its line, got {}",
            title.height
        );
        // The canvas fills what the title leaves inside the window inset. Even
        // after the default margin and gap it takes the great majority of the
        // height -- the point of the fix is that it is not a half-window band.
        assert!(
            canvas.height > 120.0,
            "the canvas should fill what the title left, got {}",
            canvas.height
        );
        assert!(
            canvas.height > title.height * 3.0,
            "the canvas should dwarf the title, got canvas {} vs title {}",
            canvas.height,
            title.height
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

        let first = layout
            .rect(WidgetId::new(2).expect("first"))
            .expect("first");
        let second = layout
            .rect(WidgetId::new(3).expect("second"))
            .expect("second");
        // Two equal growing children split the height that is left inside the
        // window inset, once the gap between them is taken out. They stay the
        // same size as each other, sit inside the inset, and the second starts
        // one gap below the first.
        let inner_width = 100.0 - 2.0 * DEFAULT_ROOT_INSET;
        let available = 120.0 - 2.0 * DEFAULT_ROOT_INSET - DEFAULT_CONTAINER_GAP;
        assert_eq!((first.x, first.y), (DEFAULT_ROOT_INSET, DEFAULT_ROOT_INSET));
        assert_eq!(first.width, inner_width);
        assert_eq!(first.height, available / 2.0);
        assert_eq!(second.height, first.height);
        assert_eq!(second.x, DEFAULT_ROOT_INSET);
        assert_eq!(second.y, first.y + first.height + DEFAULT_CONTAINER_GAP);
    }

    #[test]
    fn a_grid_wraps_its_children_onto_the_next_row() {
        // `grid` was declared in the widget set and implemented by no host, so
        // an app that asked for one was refused everywhere. It is a row that
        // wraps, which the layout engine already does -- this proves the
        // children actually move to a second row rather than the kind merely
        // being accepted.
        let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Grid);
        let mut tree = WidgetTree::new(root).expect("tree");
        // Three 40-wide children with a gap between them: in a 140-wide viewport
        // two fit inside the inset (40 + gap + 40), the third wraps to a new row.
        for id in 2..=4 {
            tree.upsert(fixed_child(id, tree.root(), 40.0, 20.0))
                .expect("child");
        }

        let layout = compute_layout(&tree, LayoutViewport::new(140.0, 200.0).expect("viewport"))
            .expect("layout");

        let first = layout.rect(WidgetId::new(2).expect("id")).expect("first");
        let second = layout.rect(WidgetId::new(3).expect("id")).expect("second");
        let third = layout.rect(WidgetId::new(4).expect("id")).expect("third");

        assert_eq!(first.y, second.y, "the first two share a row");
        assert!(
            third.y > first.y,
            "the third child should wrap to a second row, got y={} against {}",
            third.y,
            first.y
        );
        assert_eq!(third.x, first.x, "a wrapped child starts a new row");
    }

    #[test]
    fn a_tab_strip_lays_out_only_the_selected_panel() {
        // `tabs` was declared and implemented by no host. The only thing it
        // needs beyond any other container is that unselected panels take no
        // space -- the node model already carries `selected`, so this is a
        // visibility rule rather than new drawing on three hosts.
        let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Tabs)
            .with_selected(1)
            .expect("selected");
        let mut tree = WidgetTree::new(root).expect("tree");
        for id in 2..=4 {
            tree.upsert(fixed_child(id, tree.root(), 40.0, 20.0))
                .expect("panel");
        }

        let layout = compute_layout(&tree, LayoutViewport::new(200.0, 200.0).expect("viewport"))
            .expect("layout");

        let first = layout.rect(WidgetId::new(2).expect("id")).expect("first");
        let second = layout.rect(WidgetId::new(3).expect("id")).expect("second");
        let third = layout.rect(WidgetId::new(4).expect("id")).expect("third");

        // Panel index 1 is selected, so it is the one with size.
        assert!(
            second.width > 0.0 && second.height > 0.0,
            "the selected panel is laid out"
        );
        assert_eq!(first.width, 0.0, "an unselected panel takes no width");
        assert_eq!(third.width, 0.0, "an unselected panel takes no width");
    }

    #[test]
    fn a_tab_strip_with_no_selection_shows_its_first_panel() {
        // A tab strip that shows nothing looks broken rather than empty, so
        // the absence of a selection means the first panel.
        let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Tabs);
        let mut tree = WidgetTree::new(root).expect("tree");
        for id in 2..=3 {
            tree.upsert(fixed_child(id, tree.root(), 40.0, 20.0))
                .expect("panel");
        }

        let layout = compute_layout(&tree, LayoutViewport::new(200.0, 200.0).expect("viewport"))
            .expect("layout");

        let first = layout.rect(WidgetId::new(2).expect("id")).expect("first");
        let second = layout.rect(WidgetId::new(3).expect("id")).expect("second");
        assert!(first.width > 0.0, "the first panel shows by default");
        assert_eq!(second.width, 0.0, "the rest do not");
    }

    #[test]
    fn a_canvas_with_no_size_or_grow_still_fills_rather_than_collapsing() {
        // The trap a paint app hit: a canvas with grow 0 and no height collapsed
        // to a one-pixel strip and rendered blank. A drawing with no size is
        // meant to fill; default it to grow so an author who forgets grow: 1.0
        // still gets a canvas they can see.
        let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Stack);
        let mut tree = WidgetTree::new(root).expect("tree");
        tree.upsert(
            WidgetNode::new(WidgetId::new(2).expect("canvas"), WidgetKind::Canvas)
                .with_parent(tree.root())
                // grow defaults to 0, no width or height set: the trap shape.
                .with_style(WidgetStyle::default())
                .expect("canvas style"),
        )
        .expect("canvas");

        let layout = compute_layout(&tree, LayoutViewport::new(480.0, 360.0).expect("viewport"))
            .expect("layout");
        let canvas = layout
            .rect(WidgetId::new(2).expect("canvas"))
            .expect("canvas rect");
        assert_eq!(
            (canvas.x, canvas.y, canvas.width, canvas.height),
            (0.0, 0.0, 480.0, 360.0),
            "a bare canvas must fill the window, not collapse to a strip"
        );
    }

    #[test]
    fn a_full_bleed_canvas_fills_the_window_but_a_normal_layout_is_inset() {
        // A game or 3D scene fills a window whose only child is a canvas: no
        // border inset, no gap, the drawing reaches every edge. The moment the
        // window holds ordinary widgets instead, they sit inside a margin so
        // nothing touches the frame. Both in one test so the two halves of the
        // rule cannot drift apart.
        let canvas_root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Stack);
        let mut canvas_tree = WidgetTree::new(canvas_root).expect("tree");
        canvas_tree
            .upsert(
                WidgetNode::new(WidgetId::new(2).expect("canvas"), WidgetKind::Canvas)
                    .with_parent(canvas_tree.root())
                    .with_style(WidgetStyle {
                        grow: 1.0,
                        ..WidgetStyle::default()
                    })
                    .expect("canvas style"),
            )
            .expect("canvas");
        let canvas_layout = compute_layout(
            &canvas_tree,
            LayoutViewport::new(320.0, 240.0).expect("viewport"),
        )
        .expect("layout");
        let canvas = canvas_layout
            .rect(WidgetId::new(2).expect("canvas"))
            .expect("canvas rect");
        assert_eq!(
            (canvas.x, canvas.y, canvas.width, canvas.height),
            (0.0, 0.0, 320.0, 240.0),
            "a canvas-only window must be filled edge to edge"
        );

        let widget_root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Stack);
        let mut widget_tree = WidgetTree::new(widget_root).expect("tree");
        widget_tree
            .upsert(fixed_child(2, widget_tree.root(), 100.0, 30.0))
            .expect("child");
        let widget_layout = compute_layout(
            &widget_tree,
            LayoutViewport::new(320.0, 240.0).expect("viewport"),
        )
        .expect("layout");
        let child = widget_layout
            .rect(WidgetId::new(2).expect("child"))
            .expect("child rect");
        assert_eq!(
            (child.x, child.y),
            (DEFAULT_ROOT_INSET, DEFAULT_ROOT_INSET),
            "an ordinary widget must sit inside the window inset"
        );
    }

    #[test]
    fn a_stack_does_not_wrap() {
        // The other half of the same claim: wrapping is specific to grid, not
        // something the change turned on for every container.
        let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Stack);
        let mut tree = WidgetTree::new(root).expect("tree");
        for id in 2..=4 {
            tree.upsert(fixed_child(id, tree.root(), 40.0, 20.0))
                .expect("child");
        }

        let layout = compute_layout(&tree, LayoutViewport::new(100.0, 200.0).expect("viewport"))
            .expect("layout");

        // A stack is a column, so every child is on its own row already and
        // they share a left edge.
        let first = layout.rect(WidgetId::new(2).expect("id")).expect("first");
        let second = layout.rect(WidgetId::new(3).expect("id")).expect("second");
        assert_eq!(first.x, second.x);
        assert!(second.y > first.y);
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

        // The container sits inside the root's default window inset; its own
        // fixed size is unchanged. The child sits at the container's own 8px
        // padding, measured from the container's inset origin.
        assert_eq!(
            layout.rect(WidgetId::new(2).expect("container")),
            Some(ComputedRect {
                x: DEFAULT_ROOT_INSET,
                y: DEFAULT_ROOT_INSET,
                width: 160.0,
                height: 80.0,
            })
        );
        // This rect is relative to the container, so it reflects the
        // container's own 8px padding only -- the root inset shifts the
        // container, not the child within it.
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

        // Absolute position folds in the root inset and the container's own
        // padding: the child lands at inset + 8 on both axes.
        assert_eq!(
            absolute_rect(&tree, &layout, WidgetId::new(3).expect("child")),
            Some(ComputedRect {
                x: DEFAULT_ROOT_INSET + 8.0,
                y: DEFAULT_ROOT_INSET + 8.0,
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
        // The child now sits at inset + container padding, so aim a few pixels
        // inside that combined offset to land on it rather than in the frame or
        // the container's own padding band.
        let inside = DEFAULT_ROOT_INSET + 8.0 + 4.0;
        let hit = hit_test(
            &tree,
            &layout,
            LayoutPoint::new(inside, inside).expect("point"),
        )
        .expect("hit");

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
        // A Stack, not Text: content-sized widgets no longer grow, so a helper
        // that must fill space uses a container that is meant to.
        WidgetNode::new(WidgetId::new(id).expect("id"), WidgetKind::Stack)
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
