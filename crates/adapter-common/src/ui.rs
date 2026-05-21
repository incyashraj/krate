//! Shared Phase 3 UI adapter draft types.
//!
//! This module is intentionally host-neutral. It does not create real native
//! windows yet. It gives the Linux, macOS, and Windows adapter work a shared
//! shape for ids, size validation, lifecycle state, and early event routing.

use std::{
    collections::BTreeMap,
    sync::{Mutex, MutexGuard},
};

use thiserror::Error;

/// Stable host-side id for a window in one runtime session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowId(u64);

impl WindowId {
    /// Return the raw id value used by WIT and logs.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Logical size in device-independent pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowSize {
    pub width: u32,
    pub height: u32,
}

impl WindowSize {
    /// Create a validated logical window size.
    pub fn new(width: u32, height: u32) -> Result<Self, UiAdapterError> {
        if width == 0 || height == 0 {
            return Err(UiAdapterError::InvalidSize { width, height });
        }
        if width > MAX_WINDOW_EDGE || height > MAX_WINDOW_EDGE {
            return Err(UiAdapterError::InvalidSize { width, height });
        }

        Ok(Self { width, height })
    }
}

/// Stable app-owned id for one widget node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WidgetId(u64);

impl WidgetId {
    /// Create a validated widget id.
    pub fn new(id: u64) -> Result<Self, UiAdapterError> {
        if id == 0 {
            return Err(UiAdapterError::InvalidWidgetId { id });
        }

        Ok(Self(id))
    }

    /// Return the raw id value used by WIT and logs.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Host window state requested by an app or reported by an adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowState {
    Normal,
    Minimized,
    Maximized,
    Fullscreen,
}

/// Host color preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Light,
    Dark,
    Unknown,
}

/// Mouse or touch button in the portable UI event stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerButton {
    Primary,
    Secondary,
    Middle,
    Other,
}

/// Keyboard modifier state attached to input events.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub meta: bool,
}

/// Pointer input event after runtime routing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointerEvent {
    pub window: WindowId,
    pub widget: Option<WidgetId>,
    pub x: f32,
    pub y: f32,
    pub button: Option<PointerButton>,
    pub pressed: bool,
    pub modifiers: Modifiers,
}

/// Options used when creating a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowOptions {
    pub title: String,
    pub size: WindowSize,
    pub state: WindowState,
}

impl WindowOptions {
    /// Create a validated window request.
    pub fn new(title: impl Into<String>, size: WindowSize) -> Result<Self, UiAdapterError> {
        let title = title.into();
        validate_title(&title)?;

        Ok(Self {
            title,
            size,
            state: WindowState::Normal,
        })
    }
}

/// State record tracked by the draft window registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowRecord {
    pub id: WindowId,
    pub title: String,
    pub size: WindowSize,
    pub state: WindowState,
    pub visible: bool,
    pub closed: bool,
}

/// First host-neutral widget set for Phase 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetKind {
    Stack,
    Grid,
    Scroll,
    Tabs,
    Button,
    Checkbox,
    Radio,
    Switch,
    Slider,
    Progress,
    Text,
    TextField,
    TextArea,
    ListView,
    TreeView,
    Image,
    Canvas,
}

/// Minimal layout hints attached to a widget node.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WidgetStyle {
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub grow: f32,
    pub padding: f32,
}

impl Default for WidgetStyle {
    fn default() -> Self {
        Self {
            width: None,
            height: None,
            grow: 0.0,
            padding: 0.0,
        }
    }
}

impl WidgetStyle {
    /// Validate a small style block before it enters the host adapter path.
    pub fn validate(self) -> Result<Self, UiAdapterError> {
        validate_optional_f32("width", self.width)?;
        validate_optional_f32("height", self.height)?;
        validate_f32("grow", self.grow)?;
        validate_f32("padding", self.padding)?;
        Ok(self)
    }
}

/// One node in the portable widget tree.
#[derive(Debug, Clone, PartialEq)]
pub struct WidgetNode {
    pub id: WidgetId,
    pub parent: Option<WidgetId>,
    pub kind: WidgetKind,
    pub label: Option<String>,
    pub role: Option<String>,
    pub style: WidgetStyle,
}

impl WidgetNode {
    /// Create one validated widget node with default style.
    pub fn new(id: WidgetId, kind: WidgetKind) -> Self {
        Self {
            id,
            parent: None,
            kind,
            label: None,
            role: None,
            style: WidgetStyle::default(),
        }
    }

    /// Attach a parent id.
    pub fn with_parent(mut self, parent: WidgetId) -> Self {
        self.parent = Some(parent);
        self
    }

    /// Attach visible text.
    pub fn with_label(mut self, label: impl Into<String>) -> Result<Self, UiAdapterError> {
        let label = label.into();
        validate_short_text("widget label", &label)?;
        self.label = Some(label);
        Ok(self)
    }

    /// Attach an accessibility role hint.
    pub fn with_role(mut self, role: impl Into<String>) -> Result<Self, UiAdapterError> {
        let role = role.into();
        validate_short_text("widget role", &role)?;
        self.role = Some(role);
        Ok(self)
    }

    /// Attach validated style.
    pub fn with_style(mut self, style: WidgetStyle) -> Result<Self, UiAdapterError> {
        self.style = style.validate()?;
        Ok(self)
    }
}

/// Flat validated widget tree for one window.
#[derive(Debug, Clone, PartialEq)]
pub struct WidgetTree {
    root: WidgetId,
    nodes: BTreeMap<WidgetId, WidgetNode>,
}

impl WidgetTree {
    /// Create a tree with one root node.
    pub fn new(mut root: WidgetNode) -> Result<Self, UiAdapterError> {
        root.parent = None;
        root.style = root.style.validate()?;

        let root_id = root.id;
        let mut nodes = BTreeMap::new();
        nodes.insert(root_id, root);

        Ok(Self {
            root: root_id,
            nodes,
        })
    }

    /// Return the root widget id.
    pub fn root(&self) -> WidgetId {
        self.root
    }

    /// Insert or replace a non-root widget.
    pub fn upsert(&mut self, node: WidgetNode) -> Result<(), UiAdapterError> {
        if node.id == self.root {
            return Err(UiAdapterError::DuplicateWidget { id: node.id.get() });
        }

        let parent = node.parent.ok_or(UiAdapterError::MissingWidgetParent {
            id: node.id.get(),
            parent: 0,
        })?;
        if parent == node.id {
            return Err(UiAdapterError::WidgetParentCycle { id: node.id.get() });
        }
        let mut cursor = Some(parent);
        while let Some(ancestor) = cursor {
            if ancestor == node.id {
                return Err(UiAdapterError::WidgetParentCycle { id: node.id.get() });
            }
            cursor = self.nodes.get(&ancestor).and_then(|node| node.parent);
        }
        if !self.nodes.contains_key(&parent) {
            return Err(UiAdapterError::MissingWidgetParent {
                id: node.id.get(),
                parent: parent.get(),
            });
        }

        let mut node = node;
        node.style = node.style.validate()?;
        self.nodes.insert(node.id, node);
        Ok(())
    }

    /// Remove a widget and all descendants from the tree.
    pub fn remove(&mut self, id: WidgetId) -> Result<Vec<WidgetId>, UiAdapterError> {
        if id == self.root {
            return Err(UiAdapterError::CannotRemoveRootWidget { id: id.get() });
        }
        if !self.nodes.contains_key(&id) {
            return Err(UiAdapterError::InvalidWidgetId { id: id.get() });
        }

        let mut removed = vec![id];
        let mut cursor = 0;
        while let Some(parent) = removed.get(cursor).copied() {
            let children = self
                .nodes
                .values()
                .filter(|node| node.parent == Some(parent))
                .map(|node| node.id)
                .collect::<Vec<_>>();
            removed.extend(children);
            cursor += 1;
        }

        for id in &removed {
            self.nodes.remove(id);
        }

        Ok(removed)
    }

    /// Return one widget node.
    pub fn node(&self, id: WidgetId) -> Option<&WidgetNode> {
        self.nodes.get(&id)
    }

    /// Return all nodes keyed by stable widget id.
    pub fn nodes(&self) -> &BTreeMap<WidgetId, WidgetNode> {
        &self.nodes
    }
}

/// Small host-neutral event shape for the first Phase 3 prototype.
#[derive(Debug, Clone, PartialEq)]
pub enum UiEvent {
    WindowCreated(WindowId),
    WindowShown(WindowId),
    WindowClosed(WindowId),
    RedrawRequested(WindowId),
    Resized { id: WindowId, size: WindowSize },
    TitleChanged { id: WindowId, title: String },
    WidgetRootSet { window: WindowId, root: WidgetId },
    WidgetUpdated { window: WindowId, widget: WidgetId },
    WidgetRemoved { window: WindowId, widget: WidgetId },
    FocusChanged { window: WindowId, widget: WidgetId },
    Pointer(PointerEvent),
}

/// Static capability summary for one UI adapter build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiAdapterInfo {
    pub host_family: String,
    pub backend: String,
    pub native_windows: bool,
    pub native_event_loop: bool,
}

impl UiAdapterInfo {
    /// Create a UI adapter capability summary.
    pub fn new(
        host_family: impl Into<String>,
        backend: impl Into<String>,
        native_windows: bool,
        native_event_loop: bool,
    ) -> Self {
        Self {
            host_family: host_family.into(),
            backend: backend.into(),
            native_windows,
            native_event_loop,
        }
    }
}

/// Shared host UI adapter contract.
///
/// Native adapters on macOS, Windows, and Linux will implement this trait. The
/// draft adapter below implements the same contract with in-memory state, so
/// runtime code can be tested before OS windows exist.
pub trait UiAdapter: Send + Sync {
    /// Return host and backend capability information for this adapter.
    fn info(&self) -> UiAdapterInfo;

    /// Create a host window and return its stable session id.
    fn create_window(&self, options: WindowOptions) -> Result<WindowId, UiAdapterError>;

    /// Show an existing window.
    fn show_window(&self, id: WindowId) -> Result<(), UiAdapterError>;

    /// Close an existing window.
    fn close_window(&self, id: WindowId) -> Result<(), UiAdapterError>;

    /// Change an existing window title.
    fn set_title(&self, id: WindowId, title: String) -> Result<(), UiAdapterError>;

    /// Change an existing window size.
    fn set_size(&self, id: WindowId, size: WindowSize) -> Result<(), UiAdapterError>;

    /// Ask the host to redraw a window.
    fn request_redraw(&self, id: WindowId) -> Result<(), UiAdapterError>;

    /// Set or replace the root widget tree for a window.
    fn set_root(&self, window: WindowId, root: WidgetNode) -> Result<(), UiAdapterError>;

    /// Insert or update a widget node for a window.
    fn upsert_node(&self, window: WindowId, node: WidgetNode) -> Result<(), UiAdapterError>;

    /// Remove a widget node and its descendants.
    fn remove_node(&self, window: WindowId, widget: WidgetId) -> Result<(), UiAdapterError>;

    /// Move focus to a widget node.
    fn focus_node(&self, window: WindowId, widget: WidgetId) -> Result<(), UiAdapterError>;

    /// Return a snapshot of a tracked window.
    fn window(&self, id: WindowId) -> Result<Option<WindowRecord>, UiAdapterError>;

    /// Return a snapshot of a window's widget tree.
    fn widget_tree(&self, window: WindowId) -> Result<Option<WidgetTree>, UiAdapterError>;

    /// Return the focused widget for a window.
    fn focused_widget(&self, window: WindowId) -> Result<Option<WidgetId>, UiAdapterError>;

    /// Drain queued adapter events.
    fn drain_events(&self) -> Result<Vec<UiEvent>, UiAdapterError>;

    /// Queue a routed pointer event.
    fn queue_pointer_event(&self, event: PointerEvent) -> Result<(), UiAdapterError>;

    /// Read host clipboard text.
    fn read_clipboard_text(&self) -> Result<String, UiAdapterError> {
        Err(UiAdapterError::Unsupported(
            "clipboard read is not implemented by this UI adapter".to_string(),
        ))
    }

    /// Write host clipboard text.
    fn write_clipboard_text(&self, _text: &str) -> Result<(), UiAdapterError> {
        Err(UiAdapterError::Unsupported(
            "clipboard write is not implemented by this UI adapter".to_string(),
        ))
    }
}

/// In-memory implementation of [`UiAdapter`] used while native backends land.
#[derive(Debug, Default)]
pub struct DraftUiAdapter {
    registry: Mutex<DraftWindowRegistry>,
}

impl DraftUiAdapter {
    /// Create an empty draft UI adapter.
    pub fn new() -> Self {
        Self::default()
    }

    fn registry(&self) -> Result<MutexGuard<'_, DraftWindowRegistry>, UiAdapterError> {
        self.registry
            .lock()
            .map_err(|_| UiAdapterError::Internal("draft UI adapter lock is poisoned".to_string()))
    }
}

impl UiAdapter for DraftUiAdapter {
    fn info(&self) -> UiAdapterInfo {
        UiAdapterInfo::new("generic", "headless-draft", false, false)
    }

    fn create_window(&self, options: WindowOptions) -> Result<WindowId, UiAdapterError> {
        Ok(self.registry()?.create_window(options))
    }

    fn show_window(&self, id: WindowId) -> Result<(), UiAdapterError> {
        self.registry()?.show_window(id)
    }

    fn close_window(&self, id: WindowId) -> Result<(), UiAdapterError> {
        self.registry()?.close_window(id)
    }

    fn set_title(&self, id: WindowId, title: String) -> Result<(), UiAdapterError> {
        self.registry()?.set_title(id, title)
    }

    fn set_size(&self, id: WindowId, size: WindowSize) -> Result<(), UiAdapterError> {
        self.registry()?.set_size(id, size)
    }

    fn request_redraw(&self, id: WindowId) -> Result<(), UiAdapterError> {
        self.registry()?.request_redraw(id)
    }

    fn set_root(&self, window: WindowId, root: WidgetNode) -> Result<(), UiAdapterError> {
        self.registry()?.set_root(window, root)
    }

    fn upsert_node(&self, window: WindowId, node: WidgetNode) -> Result<(), UiAdapterError> {
        self.registry()?.upsert_node(window, node)
    }

    fn remove_node(&self, window: WindowId, widget: WidgetId) -> Result<(), UiAdapterError> {
        self.registry()?.remove_node(window, widget)
    }

    fn focus_node(&self, window: WindowId, widget: WidgetId) -> Result<(), UiAdapterError> {
        self.registry()?.focus_node(window, widget)
    }

    fn window(&self, id: WindowId) -> Result<Option<WindowRecord>, UiAdapterError> {
        Ok(self.registry()?.window(id).cloned())
    }

    fn widget_tree(&self, window: WindowId) -> Result<Option<WidgetTree>, UiAdapterError> {
        Ok(self.registry()?.widget_tree(window).cloned())
    }

    fn focused_widget(&self, window: WindowId) -> Result<Option<WidgetId>, UiAdapterError> {
        self.registry()?.focused_widget(window)
    }

    fn drain_events(&self) -> Result<Vec<UiEvent>, UiAdapterError> {
        Ok(self.registry()?.drain_events())
    }

    fn queue_pointer_event(&self, event: PointerEvent) -> Result<(), UiAdapterError> {
        self.registry()?.queue_pointer_event(event)
    }
}

/// Draft in-memory window registry used until OS-backed adapters land.
#[derive(Debug, Default)]
pub struct DraftWindowRegistry {
    next_id: u64,
    windows: BTreeMap<WindowId, WindowRecord>,
    widget_trees: BTreeMap<WindowId, WidgetTree>,
    focused_widgets: BTreeMap<WindowId, WidgetId>,
    events: Vec<UiEvent>,
}

impl DraftWindowRegistry {
    /// Create a window record and return its id.
    pub fn create_window(&mut self, options: WindowOptions) -> WindowId {
        self.next_id += 1;
        let id = WindowId(self.next_id);
        let record = WindowRecord {
            id,
            title: options.title,
            size: options.size,
            state: options.state,
            visible: false,
            closed: false,
        };
        self.windows.insert(id, record);
        self.events.push(UiEvent::WindowCreated(id));
        id
    }

    /// Mark a created window as visible.
    pub fn show_window(&mut self, id: WindowId) -> Result<(), UiAdapterError> {
        let window = self.open_window_mut(id)?;
        window.visible = true;
        self.events.push(UiEvent::WindowShown(id));
        Ok(())
    }

    /// Mark a window as closed.
    pub fn close_window(&mut self, id: WindowId) -> Result<(), UiAdapterError> {
        let window = self.open_window_mut(id)?;
        window.closed = true;
        window.visible = false;
        self.widget_trees.remove(&id);
        self.focused_widgets.remove(&id);
        self.events.push(UiEvent::WindowClosed(id));
        Ok(())
    }

    /// Change a window title.
    pub fn set_title(
        &mut self,
        id: WindowId,
        title: impl Into<String>,
    ) -> Result<(), UiAdapterError> {
        let title = title.into();
        validate_title(&title)?;
        let window = self.open_window_mut(id)?;
        window.title = title.clone();
        self.events.push(UiEvent::TitleChanged { id, title });
        Ok(())
    }

    /// Change a window size.
    pub fn set_size(&mut self, id: WindowId, size: WindowSize) -> Result<(), UiAdapterError> {
        let window = self.open_window_mut(id)?;
        window.size = size;
        self.events.push(UiEvent::Resized { id, size });
        Ok(())
    }

    /// Queue a redraw request for a window.
    pub fn request_redraw(&mut self, id: WindowId) -> Result<(), UiAdapterError> {
        self.open_window(id)?;
        self.events.push(UiEvent::RedrawRequested(id));
        Ok(())
    }

    /// Set or replace a window's root widget tree.
    pub fn set_root(&mut self, window: WindowId, root: WidgetNode) -> Result<(), UiAdapterError> {
        self.open_window(window)?;
        let tree = WidgetTree::new(root)?;
        let root = tree.root();
        self.widget_trees.insert(window, tree);
        self.focused_widgets.remove(&window);
        self.events.push(UiEvent::WidgetRootSet { window, root });
        Ok(())
    }

    /// Insert or update one node in a window's widget tree.
    pub fn upsert_node(
        &mut self,
        window: WindowId,
        node: WidgetNode,
    ) -> Result<(), UiAdapterError> {
        self.open_window(window)?;
        let widget = node.id;
        let tree = self
            .widget_trees
            .get_mut(&window)
            .ok_or(UiAdapterError::MissingWidgetTree {
                window: window.get(),
            })?;
        tree.upsert(node)?;
        self.events.push(UiEvent::WidgetUpdated { window, widget });
        Ok(())
    }

    /// Remove one node and its descendants from a window's widget tree.
    pub fn remove_node(
        &mut self,
        window: WindowId,
        widget: WidgetId,
    ) -> Result<(), UiAdapterError> {
        self.open_window(window)?;
        let tree = self
            .widget_trees
            .get_mut(&window)
            .ok_or(UiAdapterError::MissingWidgetTree {
                window: window.get(),
            })?;
        let removed = tree.remove(widget)?;
        if self
            .focused_widgets
            .get(&window)
            .is_some_and(|focused| removed.contains(focused))
        {
            self.focused_widgets.remove(&window);
        }
        self.events.push(UiEvent::WidgetRemoved { window, widget });
        Ok(())
    }

    /// Move focus to a widget in a window's widget tree.
    pub fn focus_node(&mut self, window: WindowId, widget: WidgetId) -> Result<(), UiAdapterError> {
        self.open_window(window)?;
        let tree = self
            .widget_trees
            .get(&window)
            .ok_or(UiAdapterError::MissingWidgetTree {
                window: window.get(),
            })?;
        if !tree.nodes.contains_key(&widget) {
            return Err(UiAdapterError::InvalidWidgetId { id: widget.get() });
        }

        self.focused_widgets.insert(window, widget);
        self.events.push(UiEvent::FocusChanged { window, widget });
        Ok(())
    }

    /// Read one window record.
    pub fn window(&self, id: WindowId) -> Option<&WindowRecord> {
        self.windows.get(&id)
    }

    /// Read a window's widget tree.
    pub fn widget_tree(&self, window: WindowId) -> Option<&WidgetTree> {
        self.widget_trees.get(&window)
    }

    /// Read a window's focused widget.
    pub fn focused_widget(&self, window: WindowId) -> Result<Option<WidgetId>, UiAdapterError> {
        self.open_window(window)?;
        Ok(self.focused_widgets.get(&window).copied())
    }

    /// Drain queued draft events.
    pub fn drain_events(&mut self) -> Vec<UiEvent> {
        std::mem::take(&mut self.events)
    }

    /// Queue a pointer event after runtime hit testing has assigned a target.
    pub fn queue_pointer_event(&mut self, event: PointerEvent) -> Result<(), UiAdapterError> {
        self.open_window(event.window)?;
        if let Some(widget) = event.widget {
            let tree =
                self.widget_trees
                    .get(&event.window)
                    .ok_or(UiAdapterError::MissingWidgetTree {
                        window: event.window.get(),
                    })?;
            if !tree.nodes.contains_key(&widget) {
                return Err(UiAdapterError::InvalidWidgetId { id: widget.get() });
            }
        }

        self.events.push(UiEvent::Pointer(event));
        Ok(())
    }

    fn open_window(&self, id: WindowId) -> Result<&WindowRecord, UiAdapterError> {
        let window = self
            .windows
            .get(&id)
            .ok_or(UiAdapterError::InvalidWindow { id: id.get() })?;
        if window.closed {
            return Err(UiAdapterError::WindowClosed { id: id.get() });
        }
        Ok(window)
    }

    fn open_window_mut(&mut self, id: WindowId) -> Result<&mut WindowRecord, UiAdapterError> {
        let window = self
            .windows
            .get_mut(&id)
            .ok_or(UiAdapterError::InvalidWindow { id: id.get() })?;
        if window.closed {
            return Err(UiAdapterError::WindowClosed { id: id.get() });
        }
        Ok(window)
    }
}

/// Errors surfaced by the Phase 3 UI adapter draft.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum UiAdapterError {
    #[error("invalid window id {id}")]
    InvalidWindow { id: u64 },
    #[error("invalid widget id {id}")]
    InvalidWidgetId { id: u64 },
    #[error("duplicate widget id {id}")]
    DuplicateWidget { id: u64 },
    #[error("widget {id} references missing parent {parent}")]
    MissingWidgetParent { id: u64, parent: u64 },
    #[error("widget {id} cannot be its own parent")]
    WidgetParentCycle { id: u64 },
    #[error("window {window} has no widget tree")]
    MissingWidgetTree { window: u64 },
    #[error("cannot remove root widget {id}")]
    CannotRemoveRootWidget { id: u64 },
    #[error("invalid widget style: {0}")]
    InvalidWidgetStyle(String),
    #[error("window {id} is closed")]
    WindowClosed { id: u64 },
    #[error("invalid window size {width}x{height}")]
    InvalidSize { width: u32, height: u32 },
    #[error("window title is empty")]
    EmptyTitle,
    #[error("window title is too long")]
    TitleTooLong,
    #[error("unsupported UI feature: {0}")]
    Unsupported(String),
    #[error("internal UI adapter error: {0}")]
    Internal(String),
}

const MAX_WINDOW_EDGE: u32 = 16_384;
const MAX_TITLE_CHARS: usize = 512;
const MAX_WIDGET_TEXT_CHARS: usize = 1_024;

fn validate_title(title: &str) -> Result<(), UiAdapterError> {
    if title.trim().is_empty() {
        return Err(UiAdapterError::EmptyTitle);
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(UiAdapterError::TitleTooLong);
    }
    Ok(())
}

fn validate_short_text(field: &str, value: &str) -> Result<(), UiAdapterError> {
    if value.chars().count() > MAX_WIDGET_TEXT_CHARS {
        return Err(UiAdapterError::InvalidWidgetStyle(format!(
            "{field} is too long"
        )));
    }

    Ok(())
}

fn validate_optional_f32(field: &str, value: Option<f32>) -> Result<(), UiAdapterError> {
    if let Some(value) = value {
        validate_f32(field, value)?;
    }

    Ok(())
}

fn validate_f32(field: &str, value: f32) -> Result<(), UiAdapterError> {
    if !value.is_finite() || value < 0.0 {
        return Err(UiAdapterError::InvalidWidgetStyle(format!(
            "{field} must be a finite non-negative number"
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_registry_allocates_stable_window_ids() {
        let mut registry = DraftWindowRegistry::default();
        let size = WindowSize::new(800, 600).expect("size");
        let first =
            registry.create_window(WindowOptions::new("Notes", size).expect("window options"));
        let second =
            registry.create_window(WindowOptions::new("Preview", size).expect("window options"));

        assert_eq!(first.get(), 1);
        assert_eq!(second.get(), 2);
        assert_eq!(registry.window(first).expect("first").title, "Notes");
        assert_eq!(registry.window(second).expect("second").title, "Preview");
    }

    #[test]
    fn draft_registry_tracks_visibility_resize_redraw_and_close() {
        let mut registry = DraftWindowRegistry::default();
        let size = WindowSize::new(800, 600).expect("size");
        let id = registry.create_window(WindowOptions::new("Notes", size).expect("options"));

        registry.show_window(id).expect("show");
        let resized = WindowSize::new(1024, 768).expect("resized");
        registry.set_size(id, resized).expect("resize");
        registry.request_redraw(id).expect("redraw");
        registry.close_window(id).expect("close");

        let window = registry.window(id).expect("window");
        assert_eq!(window.size, resized);
        assert!(!window.visible);
        assert!(window.closed);
        assert_eq!(
            registry.drain_events(),
            vec![
                UiEvent::WindowCreated(id),
                UiEvent::WindowShown(id),
                UiEvent::Resized { id, size: resized },
                UiEvent::RedrawRequested(id),
                UiEvent::WindowClosed(id),
            ]
        );
    }

    #[test]
    fn draft_registry_rejects_invalid_window_operations() {
        let mut registry = DraftWindowRegistry::default();
        let id = WindowId(42);

        assert_eq!(
            registry.show_window(id),
            Err(UiAdapterError::InvalidWindow { id: 42 })
        );

        let size = WindowSize::new(640, 480).expect("size");
        let id = registry.create_window(WindowOptions::new("Notes", size).expect("options"));
        registry.close_window(id).expect("close");

        assert_eq!(
            registry.request_redraw(id),
            Err(UiAdapterError::WindowClosed { id: id.get() })
        );
    }

    #[test]
    fn validates_window_size_and_title() {
        assert_eq!(
            WindowSize::new(0, 10),
            Err(UiAdapterError::InvalidSize {
                width: 0,
                height: 10,
            })
        );
        assert_eq!(
            WindowOptions::new(" ", WindowSize::new(100, 100).expect("size")),
            Err(UiAdapterError::EmptyTitle)
        );
    }

    #[test]
    fn widget_tree_tracks_stable_ids_and_parent_links() {
        let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Stack)
            .with_role("group")
            .expect("role");
        let mut tree = WidgetTree::new(root).expect("tree");

        let button = WidgetNode::new(WidgetId::new(2).expect("button"), WidgetKind::Button)
            .with_parent(tree.root())
            .with_label("Save")
            .expect("label");
        tree.upsert(button).expect("insert button");

        assert_eq!(tree.nodes().len(), 2);
        assert_eq!(
            tree.node(WidgetId::new(2).expect("button lookup"))
                .expect("button")
                .label
                .as_deref(),
            Some("Save")
        );
    }

    #[test]
    fn widget_tree_rejects_bad_ids_missing_parents_and_bad_style() {
        assert_eq!(
            WidgetId::new(0),
            Err(UiAdapterError::InvalidWidgetId { id: 0 })
        );

        let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Stack);
        let mut tree = WidgetTree::new(root).expect("tree");
        let orphan = WidgetNode::new(WidgetId::new(2).expect("orphan"), WidgetKind::Text)
            .with_parent(WidgetId::new(99).expect("parent"));

        assert_eq!(
            tree.upsert(orphan),
            Err(UiAdapterError::MissingWidgetParent { id: 2, parent: 99 })
        );

        let bad_style = WidgetStyle {
            width: Some(f32::NAN),
            ..WidgetStyle::default()
        };
        let styled = WidgetNode::new(WidgetId::new(3).expect("styled"), WidgetKind::Text)
            .with_parent(tree.root())
            .with_style(bad_style);

        assert!(matches!(styled, Err(UiAdapterError::InvalidWidgetStyle(_))));
    }

    #[test]
    fn widget_tree_rejects_reparenting_that_would_create_cycle() {
        let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Stack);
        let mut tree = WidgetTree::new(root).expect("tree");
        let parent = WidgetNode::new(WidgetId::new(2).expect("parent"), WidgetKind::Stack)
            .with_parent(tree.root());
        let child = WidgetNode::new(WidgetId::new(3).expect("child"), WidgetKind::Text)
            .with_parent(parent.id);
        tree.upsert(parent.clone()).expect("parent");
        tree.upsert(child).expect("child");

        let cyclic_parent = parent.with_parent(WidgetId::new(3).expect("child"));

        assert_eq!(
            tree.upsert(cyclic_parent),
            Err(UiAdapterError::WidgetParentCycle { id: 2 })
        );
    }

    #[test]
    fn draft_registry_tracks_widget_tree_focus_and_removal() {
        let mut registry = DraftWindowRegistry::default();
        let size = WindowSize::new(800, 600).expect("size");
        let window = registry.create_window(WindowOptions::new("Notes", size).expect("options"));
        let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Stack);
        let button = WidgetNode::new(WidgetId::new(2).expect("button"), WidgetKind::Button)
            .with_parent(root.id)
            .with_label("Save")
            .expect("label");
        let nested = WidgetNode::new(WidgetId::new(3).expect("nested"), WidgetKind::Text)
            .with_parent(button.id)
            .with_label("Saved")
            .expect("label");

        registry.set_root(window, root).expect("root");
        registry.upsert_node(window, button).expect("button");
        registry.upsert_node(window, nested).expect("nested");
        registry
            .focus_node(window, WidgetId::new(3).expect("nested"))
            .expect("focus");
        registry
            .remove_node(window, WidgetId::new(2).expect("button"))
            .expect("remove subtree");

        let tree = registry.widget_tree(window).expect("tree");
        assert_eq!(tree.nodes().len(), 1);
        assert_eq!(registry.focused_widget(window).expect("focus"), None);
        assert_eq!(
            registry.drain_events(),
            vec![
                UiEvent::WindowCreated(window),
                UiEvent::WidgetRootSet {
                    window,
                    root: WidgetId::new(1).expect("root"),
                },
                UiEvent::WidgetUpdated {
                    window,
                    widget: WidgetId::new(2).expect("button"),
                },
                UiEvent::WidgetUpdated {
                    window,
                    widget: WidgetId::new(3).expect("nested"),
                },
                UiEvent::FocusChanged {
                    window,
                    widget: WidgetId::new(3).expect("nested"),
                },
                UiEvent::WidgetRemoved {
                    window,
                    widget: WidgetId::new(2).expect("button"),
                },
            ]
        );
    }

    #[test]
    fn draft_registry_queues_routed_pointer_events() {
        let mut registry = DraftWindowRegistry::default();
        let size = WindowSize::new(800, 600).expect("size");
        let window = registry.create_window(WindowOptions::new("Notes", size).expect("options"));
        let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Stack);
        let button = WidgetNode::new(WidgetId::new(2).expect("button"), WidgetKind::Button)
            .with_parent(root.id)
            .with_label("Save")
            .expect("label");

        registry.set_root(window, root).expect("root");
        registry.upsert_node(window, button).expect("button");
        registry
            .queue_pointer_event(PointerEvent {
                window,
                widget: Some(WidgetId::new(2).expect("button")),
                x: 8.0,
                y: 10.0,
                button: Some(PointerButton::Primary),
                pressed: true,
                modifiers: Modifiers::default(),
            })
            .expect("pointer");

        assert_eq!(
            registry.drain_events(),
            vec![
                UiEvent::WindowCreated(window),
                UiEvent::WidgetRootSet {
                    window,
                    root: WidgetId::new(1).expect("root"),
                },
                UiEvent::WidgetUpdated {
                    window,
                    widget: WidgetId::new(2).expect("button"),
                },
                UiEvent::Pointer(PointerEvent {
                    window,
                    widget: Some(WidgetId::new(2).expect("button")),
                    x: 8.0,
                    y: 10.0,
                    button: Some(PointerButton::Primary),
                    pressed: true,
                    modifiers: Modifiers::default(),
                }),
            ]
        );
    }

    #[test]
    fn draft_registry_rejects_pointer_events_for_missing_widgets() {
        let mut registry = DraftWindowRegistry::default();
        let size = WindowSize::new(800, 600).expect("size");
        let window = registry.create_window(WindowOptions::new("Notes", size).expect("options"));
        let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Stack);
        registry.set_root(window, root).expect("root");

        assert_eq!(
            registry.queue_pointer_event(PointerEvent {
                window,
                widget: Some(WidgetId::new(99).expect("missing")),
                x: 8.0,
                y: 10.0,
                button: Some(PointerButton::Primary),
                pressed: true,
                modifiers: Modifiers::default(),
            }),
            Err(UiAdapterError::InvalidWidgetId { id: 99 })
        );
    }

    #[test]
    fn draft_adapter_implements_shared_ui_contract() {
        let adapter = DraftUiAdapter::new();
        let size = WindowSize::new(700, 500).expect("size");
        let id = adapter
            .create_window(WindowOptions::new("Layer36", size).expect("options"))
            .expect("create");

        adapter.show_window(id).expect("show");
        adapter
            .set_title(id, "Layer36 Preview".to_string())
            .expect("title");
        adapter.request_redraw(id).expect("redraw");

        let window = adapter.window(id).expect("window lookup").expect("window");
        assert_eq!(window.title, "Layer36 Preview");
        assert!(window.visible);
        assert_eq!(
            adapter.drain_events().expect("events"),
            vec![
                UiEvent::WindowCreated(id),
                UiEvent::WindowShown(id),
                UiEvent::TitleChanged {
                    id,
                    title: "Layer36 Preview".to_string(),
                },
                UiEvent::RedrawRequested(id),
            ]
        );
    }

    #[test]
    fn draft_adapter_reports_headless_info() {
        let adapter = DraftUiAdapter::new();
        let info = adapter.info();

        assert_eq!(info.host_family, "generic");
        assert_eq!(info.backend, "headless-draft");
        assert!(!info.native_windows);
        assert!(!info.native_event_loop);
    }

    #[test]
    fn draft_adapter_reports_clipboard_as_unsupported() {
        let adapter = DraftUiAdapter::new();

        assert!(matches!(
            adapter.read_clipboard_text(),
            Err(UiAdapterError::Unsupported(_))
        ));
        assert!(matches!(
            adapter.write_clipboard_text("copied text"),
            Err(UiAdapterError::Unsupported(_))
        ));
    }
}
