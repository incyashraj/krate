//! The tree-to-widgets step every drawn Krate host shares (IC-956).
//!
//! An app sends a widget tree; a host lays it out, decides what is on
//! screen and where, and paints it. That middle step used to live inside
//! the desktop runtime (`phase3_gui_host.rs`), next to wasmtime, the GPU
//! presenter and the OS adapters, so nothing else could reuse it -- and the
//! browser Player would have needed a second copy that drifted from the
//! first. It lives here now, with no GPU and no OS underneath, and both the
//! runtime and the Player call it:
//!
//! - [`measure_label`] and [`layout_inside`]: the layout a window gets, with
//!   real text measurement (K-412) and the edges its host keeps clear.
//! - [`placements`]: what is on screen, where, clipped and shifted by each
//!   scroll container's offset, with a list's selected row resolved.
//! - [`scroll_by`]: a wheel turn over a scroll container, clamped to its
//!   content, the way the platform scrolls without the app writing code.
//! - [`press_focuses`] and [`activation_point`]: which presses move keyboard
//!   focus, and where Enter or Space on a focused control presses it.
//! - [`node_from_parts`]: the rules a node the app sends must pass.
//! - [`canvas_raster`]: the `gfx.canvas2d` rasterizer, the pixels an app
//!   draws with commands, published through the image-widget path.

use std::sync::Arc;

pub mod canvas_raster;

use krate_adapter_common::painter::drawn_kind;
use krate_adapter_common::ui::{
    kind_is_selectable, ImagePixels, WidgetId, WidgetKind, WidgetNode, WidgetPlacement,
    WidgetStyle, WidgetTree,
};
use krate_layout::{
    absolute_rect, compute_layout_measured, LayoutError, LayoutSnapshot, LayoutViewport,
};

/// Measure one label for layout.
///
/// The layout crate owns geometry and deliberately knows nothing about fonts,
/// so it asks for this. Without it every Text widget is zero wide, and a
/// centred label is placed with its left edge exactly on the centre line and
/// drawn off to the right of it (K-412).
///
/// `measure_canvas_text` returns `None` when this host has no usable fonts and
/// the painter will fall back to the 5x7 bitmap face; the fallback here is
/// that face's own metrics, so the number layout is told always matches the
/// pixels that get drawn.
pub fn measure_label(text: &str, size: f32) -> Option<(f32, f32)> {
    if let Some(m) = krate_adapter_common::vector_text::measure_canvas_text(text, size) {
        return Some((m.width, m.height));
    }
    let scale = (size / 7.0).max(1.0);
    Some((text.chars().count() as f32 * 6.0 * scale, 8.0 * scale))
}

/// Lay a window's tree out inside the edges its host keeps clear (a phone's
/// status bar and gesture bar), offset past them (K-1060). A tree with a
/// canvas is left full-bleed: such an app reads the covered edges from
/// krate:ui/screen and lays itself out. `insets` is (top, right, bottom,
/// left); a host that covers nothing passes zeros.
pub fn layout_inside(
    tree: &WidgetTree,
    viewport: LayoutViewport,
    insets: (f32, f32, f32, f32),
) -> Result<LayoutSnapshot, LayoutError> {
    let draws_itself = tree
        .nodes()
        .values()
        .any(|node| node.kind == WidgetKind::Canvas);
    let (top, right, bottom, left) = if draws_itself {
        (0.0, 0.0, 0.0, 0.0)
    } else {
        insets
    };
    let clear = |v: f32| if v.is_finite() { v.max(0.0) } else { 0.0 };
    let (top, right, bottom, left) = (clear(top), clear(right), clear(bottom), clear(left));
    let inner = LayoutViewport::new(
        (viewport.width - left - right).max(1.0),
        (viewport.height - top - bottom).max(1.0),
    )?;
    let mut layout = compute_layout_measured(tree, inner, &measure_label)?;
    layout.offset(left, top);
    Ok(layout)
}

/// The nearest Scroll container above `id`, if any.
pub fn nearest_scroll_ancestor(tree: &WidgetTree, id: WidgetId) -> Option<WidgetId> {
    let mut current = tree.node(id)?.parent;
    while let Some(parent_id) = current {
        let parent = tree.node(parent_id)?;
        if parent.kind == WidgetKind::Scroll {
            return Some(parent_id);
        }
        current = parent.parent;
    }
    None
}

/// Topmost Scroll container whose rectangle contains the logical point.
pub fn scroll_container_at(
    tree: &WidgetTree,
    layout: &LayoutSnapshot,
    x: f32,
    y: f32,
) -> Option<WidgetId> {
    tree.nodes()
        .iter()
        .rev()
        .filter(|(_, node)| node.kind == WidgetKind::Scroll)
        .find(|(id, _)| {
            absolute_rect(tree, layout, **id)
                .is_some_and(|r| x >= r.x && y >= r.y && x < r.x + r.width && y < r.y + r.height)
        })
        .map(|(id, _)| *id)
}

/// Clamp a scroll offset after applying a wheel delta: never negative,
/// never past the point where the last content row is visible.
pub fn clamped_scroll_offset(
    current: f32,
    dy: f32,
    content_height: f32,
    viewport_height: f32,
) -> f32 {
    let max_offset = (content_height - viewport_height).max(0.0);
    (current + dy).clamp(0.0, max_offset)
}

/// Apply a wheel turn at (`x`, `y`) to the scroll container under it.
///
/// Returns the container and its new offset when the offset moved, `None`
/// when nothing scrolls there or the container is already at that end.
/// `current` reads a container's offset now. The guest still receives the
/// wheel event either way: an app that draws its own content has no scroll
/// container to find, and swallowing the event there was the bug where a
/// list of 32 items showed 6 with the rest out of reach.
pub fn scroll_by(
    tree: &WidgetTree,
    layout: &LayoutSnapshot,
    x: f32,
    y: f32,
    dy: f32,
    current: impl Fn(WidgetId) -> f32,
) -> Option<(WidgetId, f32)> {
    let scroll_id = scroll_container_at(tree, layout, x, y)?;
    let scroll_rect = absolute_rect(tree, layout, scroll_id)?;
    let content_bottom = tree
        .nodes()
        .iter()
        .filter(|(child, _)| nearest_scroll_ancestor(tree, **child) == Some(scroll_id))
        .filter_map(|(child, _)| absolute_rect(tree, layout, *child))
        .map(|r| r.y + r.height)
        .fold(scroll_rect.y, f32::max);
    let content_height = content_bottom - scroll_rect.y;
    let now = current(scroll_id);
    let updated = clamped_scroll_offset(now, dy, content_height, scroll_rect.height);
    if (updated - now).abs() > f32::EPSILON {
        Some((scroll_id, updated))
    } else {
        None
    }
}

/// Widget kinds that take keyboard focus from a pointer press.
pub fn press_focuses(kind: WidgetKind) -> bool {
    matches!(kind, WidgetKind::TextField | WidgetKind::TextArea)
}

/// Where Enter or Space on the focused `widget` presses it, if it is a
/// control a press activates.
///
/// Every native toolkit activates a focused button on those keys (IC-743,
/// test 1548). The same things a pointer press activates: the control
/// kinds, and a row of a list, which is a Text node whose parent is a
/// ListView. The answer is the control's centre; the host routes a press
/// and a release there exactly as a pointer would have delivered them.
pub fn activation_point(
    tree: &WidgetTree,
    layout: &LayoutSnapshot,
    widget: WidgetId,
    key: &str,
) -> Option<(f32, f32)> {
    if !matches!(key, "Enter" | "Space") {
        return None;
    }
    let activates = tree.node(widget).is_some_and(|node| {
        let control = matches!(
            node.kind,
            WidgetKind::Button | WidgetKind::Checkbox | WidgetKind::Radio | WidgetKind::Switch
        );
        let list_row = node.kind == WidgetKind::Text
            && node
                .parent
                .and_then(|parent| tree.node(parent))
                .is_some_and(|parent| parent.kind == WidgetKind::ListView);
        control || list_row
    });
    if !activates {
        return None;
    }
    let rect = absolute_rect(tree, layout, widget)?;
    Some((rect.x + rect.width / 2.0, rect.y + rect.height / 2.0))
}

/// Build the drawn-widget placement list for a laid-out tree: what is on
/// screen, where, and how.
///
/// `offset` reads a scroll container's host-side offset; `image` the
/// picture an image widget holds. One shared list decides what the drawn
/// painters support, so placement filtering and painting can never drift
/// apart.
pub fn placements(
    tree: &WidgetTree,
    layout: &LayoutSnapshot,
    offset: impl Fn(WidgetId) -> f32,
    image: impl Fn(WidgetId) -> Option<Arc<ImagePixels>>,
) -> Vec<WidgetPlacement> {
    let mut placements = Vec::new();
    // One pass for every node's children, used below per node. Asking the
    // tree per node walked all of it each time, so this function was
    // quadratic in the widget count (K-1135).
    let children = krate_layout::child_index(tree);
    let nth_child = |parent: WidgetId, index: u32| -> Option<WidgetId> {
        children.get(&parent)?.get(index as usize).copied()
    };
    for (id, node) in tree.nodes() {
        // A CONTAINER passes too when the app gave it a box. `stack`, `grid`
        // and `scroll` draw nothing of their own and are rightly absent from
        // `drawn_kind` -- but a stack with a background and a corner radius
        // is a card, and dropping it here would mean the painter never sees
        // it.
        let has_box = node
            .style
            .r#box
            .map(|style| style.sanitized().paints_anything())
            .unwrap_or(false);
        if !drawn_kind(node.kind) && !has_box {
            continue;
        }
        // A widget inside an unselected tab panel is not on screen. The
        // layout collapses the panel, but a nested control still resolves to
        // a rectangle, so without this every tab's contents would paint on
        // top of each other.
        if krate_layout::is_hidden_by_tabs_in(tree, &children, *id) {
            continue;
        }
        let Some(rect) = absolute_rect(tree, layout, *id) else {
            continue;
        };
        // Widgets inside a Scroll container shift by the container's
        // host-side offset and clip to the container's rectangle.
        let mut y = rect.y;
        let mut clip = None;
        if let Some(scroll_id) = nearest_scroll_ancestor(tree, *id) {
            if let Some(scroll_rect) = absolute_rect(tree, layout, scroll_id) {
                y -= offset(scroll_id);
                clip = Some((
                    scroll_rect.x,
                    scroll_rect.y,
                    scroll_rect.width,
                    scroll_rect.height,
                ));
            }
        }
        // Resolve a selectable container's selected index to the child's
        // rect here, where the tree and layout are both in hand; the painters
        // only ever see rectangles. Out-of-range indices and children that
        // failed layout simply draw no highlight.
        let selection = node.selected.and_then(|index| {
            let child = nth_child(*id, index)?;
            let child_rect = absolute_rect(tree, layout, child)?;
            Some((
                child_rect.x,
                child_rect.y - (rect.y - y),
                child_rect.width,
                child_rect.height,
            ))
        });
        // A Text row directly inside a ListView is a selectable row, not a
        // passive label, so mark it clickable. Native hosts lower clickable
        // rows as buttons so a click routes back with the row's widget id;
        // drawn hosts already hit-test every placement and ignore this.
        let list_parent = node
            .parent
            .filter(|_| node.kind == WidgetKind::Text)
            .and_then(|parent| tree.node(parent).map(|p| (parent, p)))
            .filter(|(_, parent)| parent.kind == WidgetKind::ListView);
        let clickable = list_parent.is_some();
        // For a native host, mark the selected row via `checked` so its
        // button can be tinted; the drawn painters use the container's
        // selection wash instead and leave `checked` alone here.
        let row_selected = list_parent.and_then(|(parent_id, parent)| {
            let index = parent.selected?;
            let selected_child = nth_child(parent_id, index)?;
            Some(selected_child == *id)
        });
        placements.push(WidgetPlacement {
            widget: *id,
            kind: node.kind,
            label: node.label.clone(),
            checked: row_selected.or(node.checked),
            value: node.value,
            selection,
            text_cursor: node.text_cursor,
            clip,
            x: rect.x,
            y,
            width: rect.width,
            height: rect.height,
            clickable,
            role: node.role.clone(),
            // Shared, not copied: this runs once per widget per frame, and a
            // photograph is a quarter-gigabyte of pixels.
            pixels: image(*id),
            // Sanitized here rather than at the painters, so every host gets
            // the same bounded numbers and a guest cannot hand one of them a
            // NaN or a thousand-pixel outline.
            text: node.style.text.map(|style| style.sanitized()),
            r#box: node.style.r#box.map(|style| style.sanitized()),
        });
    }
    placements
}

/// Why a node the app sent was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeError {
    /// The id (or the parent's id) is not a valid widget id.
    InvalidWidget,
    /// The node carries something its kind cannot carry, in words.
    Unsupported(String),
}

/// Build and validate a widget node from parts already in the host's own
/// types.
///
/// Every phase and every host lands here. Phase 4's `widget-kind` has a case
/// Phase 3's does not, so the two cannot share a WIT record -- but they share
/// every RULE, which is what this function holds (K-403).
///
/// The arguments ARE the widget's fields, one per WIT record member.
/// Grouping them into a struct would mean a second shape beside the WIT
/// record that has to be kept in step with it by hand, which is the drift
/// K-403 exists to prevent.
#[allow(clippy::too_many_arguments)]
pub fn node_from_parts(
    raw_id: u64,
    raw_parent: Option<u64>,
    kind: WidgetKind,
    label: Option<String>,
    role: Option<String>,
    style: WidgetStyle,
    checked: Option<bool>,
    value: Option<f32>,
    selected: Option<u32>,
    text_cursor: Option<(u32, u32)>,
) -> Result<WidgetNode, NodeError> {
    let widget_id = |raw: u64| WidgetId::new(raw).map_err(|_| NodeError::InvalidWidget);
    let id = widget_id(raw_id)?;
    let parent = raw_parent.map(widget_id).transpose()?;
    if let Some(value) = value {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(NodeError::Unsupported(
                "widget value must be a finite number in 0..=1".to_string(),
            ));
        }
    }
    if selected.is_some() && !kind_is_selectable(kind) {
        return Err(NodeError::Unsupported(format!(
            "widget kind {kind:?} cannot carry a selected index"
        )));
    }
    // A caret only means something on an editable text widget. Rejecting it
    // elsewhere keeps a stray value from silently riding on, say, a button.
    if text_cursor.is_some() && !matches!(kind, WidgetKind::TextArea | WidgetKind::TextField) {
        return Err(NodeError::Unsupported(format!(
            "widget kind {kind:?} cannot carry a text caret"
        )));
    }

    Ok(WidgetNode {
        id,
        parent,
        kind,
        label,
        role,
        style,
        checked,
        value,
        selected,
        text_cursor,
        // A picture arrives through `krate:ui/image`, keyed by widget id, not
        // as a field here. The node the app sends must stay the exact record
        // it was compiled against.
        pixels: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(raw: u64) -> WidgetId {
        WidgetId::new(raw).expect("id")
    }

    fn node(raw: u64, parent: Option<u64>, kind: WidgetKind) -> WidgetNode {
        let mut node = WidgetNode::new(id(raw), kind);
        node.parent = parent.map(id);
        node
    }

    /// A scroll container with more rows than fit: the wheel moves it, the
    /// offset stops at the last row, and the rows inside shift and clip.
    #[test]
    fn a_wheel_scrolls_the_container_under_it_and_stops_at_the_end() {
        let mut tree = WidgetTree::new(node(1, None, WidgetKind::Stack)).expect("tree");
        let mut scroll = node(2, Some(1), WidgetKind::Scroll);
        scroll.style.height = Some(100.0);
        tree.upsert(scroll).expect("scroll");
        for row in 0..20u64 {
            let mut text = node(10 + row, Some(2), WidgetKind::Text);
            text.label = Some(format!("row {row}"));
            text.style.height = Some(20.0);
            tree.upsert(text).expect("row");
        }
        let viewport = LayoutViewport::new(200.0, 300.0).expect("viewport");
        let layout = layout_inside(&tree, viewport, (0.0, 0.0, 0.0, 0.0)).expect("layout");

        let (container, offset) =
            scroll_by(&tree, &layout, 50.0, 50.0, 30.0, |_| 0.0).expect("scrolls");
        assert_eq!(container, id(2));
        assert_eq!(offset, 30.0);
        // Far past the end clamps to the last row's bottom less the
        // container's own height: twenty 20-pixel rows are at least 400.
        let (_, end) = scroll_by(&tree, &layout, 50.0, 50.0, 10_000.0, |_| 0.0).expect("end");
        let last = absolute_rect(&tree, &layout, id(29)).expect("last row");
        let scroll_rect = absolute_rect(&tree, &layout, id(2)).expect("scroll");
        assert_eq!(
            end,
            last.y + last.height - scroll_rect.y - scroll_rect.height
        );
        assert!(end >= 300.0, "{end}");
        // Already at the end: nothing moves.
        assert_eq!(scroll_by(&tree, &layout, 50.0, 50.0, 10.0, |_| end), None);
        // Outside the container: nothing scrolls.
        assert_eq!(scroll_by(&tree, &layout, 50.0, 250.0, 10.0, |_| 0.0), None);

        let placed = placements(&tree, &layout, |_| 30.0, |_| None);
        let first = placed.iter().find(|p| p.widget == id(10)).expect("row 0");
        assert!(first.clip.is_some());
        let unshifted = absolute_rect(&tree, &layout, id(10)).expect("rect");
        assert_eq!(first.y, unshifted.y - 30.0);
    }

    /// Placing a long list costs time in proportion to its rows (K-1135).
    ///
    /// Measured before: every widget rebuilt the whole tree's child index
    /// to ask whether a tab hid it, so one frame of N rows cost N^2: 4,000
    /// rows took 1.7 s here in a debug build, 8,000 four times that. Built
    /// once per frame it is milliseconds, so the bound is generous for a
    /// slow machine and still far below what the quadratic version needs.
    #[test]
    fn placing_a_long_list_is_linear_in_its_rows() {
        const ROWS: u64 = 8_000;
        let mut tree = WidgetTree::new(node(1, None, WidgetKind::Stack)).expect("tree");
        tree.upsert(node(2, Some(1), WidgetKind::Scroll))
            .expect("scroll");
        for row in 0..ROWS {
            let mut text = node(100 + row, Some(2), WidgetKind::Text);
            text.style.height = Some(28.0);
            tree.upsert(text).expect("row");
        }
        let viewport = LayoutViewport::new(360.0, 520.0).expect("viewport");
        let layout = krate_layout::compute_layout(&tree, viewport).expect("layout");

        let started = std::time::Instant::now();
        let placed = placements(&tree, &layout, |_| 0.0, |_| None);
        let took = started.elapsed();
        assert_eq!(placed.len() as u64, ROWS, "every row is placed");
        assert!(
            took < std::time::Duration::from_secs(1),
            "placing {ROWS} rows took {took:?}: something is asking the whole tree per widget"
        );
    }

    /// Enter on a focused button presses its centre; on a text field it
    /// does nothing; any other key does nothing.
    #[test]
    fn enter_activates_a_focused_control_only() {
        let mut tree = WidgetTree::new(node(1, None, WidgetKind::Stack)).expect("tree");
        let mut button = node(2, Some(1), WidgetKind::Button);
        button.label = Some("Go".to_string());
        tree.upsert(button).expect("button");
        tree.upsert(node(3, Some(1), WidgetKind::TextField))
            .expect("field");
        let viewport = LayoutViewport::new(200.0, 200.0).expect("viewport");
        let layout = layout_inside(&tree, viewport, (0.0, 0.0, 0.0, 0.0)).expect("layout");
        let (x, y) = activation_point(&tree, &layout, id(2), "Enter").expect("press");
        let rect = absolute_rect(&tree, &layout, id(2)).expect("rect");
        assert!(rect.contains(krate_layout::LayoutPoint::new(x, y).expect("point")));
        assert!(activation_point(&tree, &layout, id(2), "a").is_none());
        assert!(activation_point(&tree, &layout, id(3), "Enter").is_none());
        assert!(press_focuses(WidgetKind::TextField));
        assert!(!press_focuses(WidgetKind::Button));
    }

    /// The node rules: a value outside 0..=1, a selection on a kind that
    /// cannot select, and a caret on a button are each refused in words.
    #[test]
    fn a_node_breaking_the_rules_is_refused_in_words() {
        let style = WidgetStyle::default();
        let bad_value = node_from_parts(
            1,
            None,
            WidgetKind::Progress,
            None,
            None,
            style,
            None,
            Some(1.5),
            None,
            None,
        );
        assert!(matches!(bad_value, Err(NodeError::Unsupported(m)) if m.contains("0..=1")));
        let caret = node_from_parts(
            1,
            None,
            WidgetKind::Button,
            None,
            None,
            style,
            None,
            None,
            None,
            Some((0, 0)),
        );
        assert!(matches!(caret, Err(NodeError::Unsupported(m)) if m.contains("caret")));
        assert_eq!(
            node_from_parts(
                0,
                None,
                WidgetKind::Text,
                None,
                None,
                style,
                None,
                None,
                None,
                None
            ),
            Err(NodeError::InvalidWidget)
        );
    }
}
