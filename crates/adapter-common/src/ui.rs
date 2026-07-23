//! Shared Phase 3 UI adapter draft types.
//!
//! This module is intentionally host-neutral. It does not create real native
//! windows yet. It gives the Linux, macOS, and Windows adapter work a shared
//! shape for ids, size validation, lifecycle state, and early event routing.

use std::{
    collections::{BTreeMap, VecDeque},
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

/// Window backend family used by a host adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowBackendKind {
    HeadlessDraft,
    AppKit,
    Winit,
    Win32,
    Unknown,
}

/// Opaque host window handle associated with a Krate window id.
///
/// The raw value is owned by the host adapter. Krate only stores it so a
/// native backend can keep the stable `WindowId` and the OS-level window object
/// connected while events move through the shared queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeWindowHandle {
    pub backend: WindowBackendKind,
    pub raw_handle: u64,
}

impl NativeWindowHandle {
    /// Create a validated native window handle token.
    pub fn new(backend: WindowBackendKind, raw_handle: u64) -> Result<Self, UiAdapterError> {
        if raw_handle == 0 || !backend.accepts_native_window_handles() {
            return Err(UiAdapterError::InvalidNativeWindowHandle {
                backend: format!("{backend:?}"),
                raw_handle,
            });
        }

        Ok(Self {
            backend,
            raw_handle,
        })
    }
}

impl WindowBackendKind {
    /// Return whether this backend kind can own native host window handles.
    pub fn accepts_native_window_handles(self) -> bool {
        matches!(self, Self::AppKit | Self::Winit | Self::Win32)
    }
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

/// Keyboard input event after runtime focus routing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    pub window: WindowId,
    pub widget: Option<WidgetId>,
    pub key: String,
    pub pressed: bool,
    pub modifiers: Modifiers,
}

impl KeyEvent {
    /// Create a validated key event.
    pub fn new(
        window: WindowId,
        widget: Option<WidgetId>,
        key: impl Into<String>,
        pressed: bool,
        modifiers: Modifiers,
    ) -> Result<Self, UiAdapterError> {
        let key = key.into();
        validate_key_name(&key)?;
        Ok(Self {
            window,
            widget,
            key,
            pressed,
            modifiers,
        })
    }
}

/// Text input after keyboard layout or IME commit processing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextInputEvent {
    pub window: WindowId,
    pub widget: Option<WidgetId>,
    pub text: String,
}

impl TextInputEvent {
    /// Create a validated text input event.
    pub fn new(
        window: WindowId,
        widget: Option<WidgetId>,
        text: impl Into<String>,
    ) -> Result<Self, UiAdapterError> {
        let text = text.into();
        validate_text_input(&text)?;
        Ok(Self {
            window,
            widget,
            text,
        })
    }
}

/// A natively lowered control's complete text after a person edited it.
///
/// Unlike [`TextInputEvent`], which appends, this replaces: the widget's whole
/// text is carried, which is the only faithful representation of deleting,
/// selecting, or pasting inside a real OS control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextChangedEvent {
    pub window: WindowId,
    pub widget: WidgetId,
    pub text: String,
}

impl TextChangedEvent {
    pub fn new(
        window: WindowId,
        widget: WidgetId,
        text: impl Into<String>,
    ) -> Result<Self, UiAdapterError> {
        let text = text.into();
        validate_text_input(&text)?;
        Ok(Self {
            window,
            widget,
            text,
        })
    }
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
    pub focused: bool,
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

/// Whether this widget kind can carry a `selected` child index.
/// Selection belongs to container kinds that present a list of choices;
/// every other kind rejects it at construction time.
pub fn kind_is_selectable(kind: WidgetKind) -> bool {
    matches!(kind, WidgetKind::ListView | WidgetKind::TreeView)
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
    /// On/off state for checkable kinds (checkbox, radio, switch).
    pub checked: Option<bool>,
    /// Normalized 0..=1 value for slider and progress kinds.
    pub value: Option<f32>,
    /// Zero-based index of the selected child, for selectable container
    /// kinds (list view, tree view).
    pub selected: Option<u32>,
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
            checked: None,
            value: None,
            selected: None,
        }
    }

    /// Attach on/off state (checkbox, radio, switch).
    pub fn with_checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }

    /// Attach a selected child index (list view, tree view). The index is
    /// bounds-checked against the real child count at lowering time, when
    /// the tree is known; here we only reject kinds that cannot carry a
    /// selection at all.
    pub fn with_selected(mut self, selected: u32) -> Result<Self, UiAdapterError> {
        if !kind_is_selectable(self.kind) {
            return Err(UiAdapterError::Unsupported(format!(
                "widget kind {:?} cannot carry a selected index",
                self.kind
            )));
        }
        self.selected = Some(selected);
        Ok(self)
    }

    /// Attach a normalized 0..=1 value (slider, progress).
    pub fn with_value(mut self, value: f32) -> Result<Self, UiAdapterError> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(UiAdapterError::Unsupported(format!(
                "widget value must be a finite number in 0..=1, got {value}"
            )));
        }
        self.value = Some(value);
        Ok(self)
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

    /// Return the children of one widget, ordered by widget id.
    ///
    /// The tree is stored flat with parent pointers, so this walks the map;
    /// ordering follows the `BTreeMap` key order, which is the same order
    /// layout assigns rows. Callers that index into the result (selection,
    /// for one) therefore see stable indices across re-lowering.
    pub fn children(&self, parent: WidgetId) -> Vec<WidgetId> {
        self.nodes
            .iter()
            .filter(|(_, node)| node.parent == Some(parent))
            .map(|(id, _)| *id)
            .collect()
    }
}

/// Small host-neutral event shape for the first Phase 3 prototype.
#[derive(Debug, Clone, PartialEq)]
pub enum UiEvent {
    WindowCreated(WindowId),
    WindowShown(WindowId),
    WindowClosed(WindowId),
    NativeWindowAttached {
        id: WindowId,
        backend: WindowBackendKind,
    },
    NativeWindowDetached {
        id: WindowId,
        backend: WindowBackendKind,
    },
    WindowCloseRequested(WindowId),
    WindowFocused {
        id: WindowId,
        focused: bool,
    },
    ThemeChanged {
        theme: Theme,
    },
    ScaleChanged {
        id: WindowId,
        scale: f32,
    },
    RedrawRequested(WindowId),
    Resized {
        id: WindowId,
        size: WindowSize,
    },
    TitleChanged {
        id: WindowId,
        title: String,
    },
    WidgetRootSet {
        window: WindowId,
        root: WidgetId,
    },
    WidgetUpdated {
        window: WindowId,
        widget: WidgetId,
    },
    WidgetRemoved {
        window: WindowId,
        widget: WidgetId,
    },
    FocusChanged {
        window: WindowId,
        widget: WidgetId,
    },
    Pointer(PointerEvent),
    Key(KeyEvent),
    TextInput(TextInputEvent),
    TextChanged(TextChangedEvent),
}

/// Summary from one non-blocking host event-loop tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiEventLoopTick {
    pub window: WindowId,
    pub callbacks_handled: usize,
    pub snapshot_refreshed: bool,
    pub redraw_requested: bool,
}

/// Current state read from a future winit-backed native window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WinitWindowSnapshot {
    pub window: WindowId,
    pub size: WindowSize,
    pub visible: bool,
    pub focused: bool,
    pub scale: f32,
}

impl WinitWindowSnapshot {
    /// Create a validated winit snapshot.
    pub fn new(
        window: WindowId,
        size: WindowSize,
        visible: bool,
        focused: bool,
        scale: f32,
    ) -> Result<Self, UiAdapterError> {
        validate_scale_factor(scale)?;
        Ok(Self {
            window,
            size,
            visible,
            focused,
            scale,
        })
    }
}

/// Native event shape accepted by the first Linux and Windows winit session owner.
#[derive(Debug, Clone, PartialEq)]
pub enum WinitWindowNativeEvent {
    CloseRequested,
    Resized(WindowSize),
    Focused(bool),
    ScaleChanged(f32),
    RedrawRequested,
    Snapshot(WinitWindowSnapshot),
}

/// One non-blocking unit of future winit event-loop work.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WinitWindowEventLoopStep {
    callbacks: Vec<WinitWindowNativeEvent>,
}

impl WinitWindowEventLoopStep {
    /// Create an empty winit event-loop step.
    pub fn new() -> Self {
        Self::default()
    }

    /// Include native callbacks collected from winit.
    pub fn with_callbacks(
        mut self,
        callbacks: impl IntoIterator<Item = WinitWindowNativeEvent>,
    ) -> Self {
        self.callbacks.extend(callbacks);
        self
    }

    /// Return queued native callbacks.
    pub fn callbacks(&self) -> &[WinitWindowNativeEvent] {
        &self.callbacks
    }
}

/// FIFO queue used by Linux and Windows winit callback bridges.
#[derive(Debug, Clone, PartialEq)]
pub struct WinitWindowEventCollector {
    window: WindowId,
    callbacks: VecDeque<WinitWindowNativeEvent>,
}

impl WinitWindowEventCollector {
    /// Create an empty callback collector for one Krate window.
    pub fn new(window: WindowId) -> Self {
        Self {
            window,
            callbacks: VecDeque::new(),
        }
    }

    /// Return the stable Krate window id this collector feeds.
    pub fn window(&self) -> WindowId {
        self.window
    }

    /// Return the number of callbacks waiting to be pumped.
    pub fn pending_callbacks(&self) -> usize {
        self.callbacks.len()
    }

    /// Return whether no native callbacks are waiting.
    pub fn is_empty(&self) -> bool {
        self.callbacks.is_empty()
    }

    /// Record one native callback in delivery order.
    pub fn push_event(&mut self, event: WinitWindowNativeEvent) -> Result<(), UiAdapterError> {
        if let WinitWindowNativeEvent::Snapshot(snapshot) = &event {
            if snapshot.window != self.window {
                return Err(UiAdapterError::InvalidWindow {
                    id: snapshot.window.get(),
                });
            }
        }

        self.callbacks.push_back(event);
        Ok(())
    }

    /// Drain all pending callbacks into one event-loop step.
    pub fn drain_step(&mut self) -> WinitWindowEventLoopStep {
        let callbacks = self.callbacks.drain(..).collect::<Vec<_>>();
        WinitWindowEventLoopStep::new().with_callbacks(callbacks)
    }
}

/// Result from one non-blocking winit event-loop step.
#[derive(Debug, Clone, PartialEq)]
pub struct WinitWindowEventLoopStepReport {
    pub callbacks_handled: usize,
    pub snapshot: Option<WinitWindowSnapshot>,
    pub redraw_requested: bool,
}

/// State owned by a future winit-backed host window session.
#[derive(Debug, Clone, PartialEq)]
pub struct WinitWindowSession {
    id: WindowId,
    handle: NativeWindowHandle,
    last_snapshot: WinitWindowSnapshot,
}

impl WinitWindowSession {
    /// Create session state for a winit-backed Krate window.
    pub fn new(
        id: WindowId,
        handle: NativeWindowHandle,
        initial_snapshot: WinitWindowSnapshot,
    ) -> Result<Self, UiAdapterError> {
        if handle.backend != WindowBackendKind::Winit {
            return Err(UiAdapterError::InvalidNativeWindowHandle {
                backend: format!("{:?}", handle.backend),
                raw_handle: handle.raw_handle,
            });
        }
        if initial_snapshot.window != id {
            return Err(UiAdapterError::InvalidWindow {
                id: initial_snapshot.window.get(),
            });
        }

        Ok(Self {
            id,
            handle,
            last_snapshot: initial_snapshot,
        })
    }

    /// Return the stable Krate window id.
    pub fn id(&self) -> WindowId {
        self.id
    }

    /// Return the opaque winit handle token attached to this session.
    pub fn native_handle(&self) -> NativeWindowHandle {
        self.handle
    }

    /// Return the latest native snapshot seen by Krate.
    pub fn last_snapshot(&self) -> WinitWindowSnapshot {
        self.last_snapshot
    }

    /// Refresh this session from a full native winit snapshot.
    pub fn sync_snapshot(
        &mut self,
        adapter: &dyn WindowAdapter,
        snapshot: WinitWindowSnapshot,
    ) -> Result<WinitWindowSnapshot, UiAdapterError> {
        if snapshot.window != self.id {
            return Err(UiAdapterError::InvalidWindow {
                id: snapshot.window.get(),
            });
        }

        if snapshot.size != self.last_snapshot.size {
            adapter.queue_host_resize(self.id, snapshot.size)?;
        }
        if snapshot.focused != self.last_snapshot.focused {
            adapter.queue_window_focused(self.id, snapshot.focused)?;
        }
        if (snapshot.scale - self.last_snapshot.scale).abs() > f32::EPSILON {
            adapter.queue_scale_changed(self.id, snapshot.scale)?;
        }

        self.last_snapshot = snapshot;
        Ok(snapshot)
    }

    /// Queue one native winit event through the shared window adapter path.
    pub fn handle_native_event(
        &mut self,
        adapter: &dyn WindowAdapter,
        event: WinitWindowNativeEvent,
    ) -> Result<Option<WinitWindowSnapshot>, UiAdapterError> {
        match event {
            WinitWindowNativeEvent::CloseRequested => {
                adapter.queue_close_requested(self.id)?;
                Ok(None)
            }
            WinitWindowNativeEvent::Resized(size) => {
                let snapshot = WinitWindowSnapshot::new(
                    self.id,
                    size,
                    self.last_snapshot.visible,
                    self.last_snapshot.focused,
                    self.last_snapshot.scale,
                )?;
                self.sync_snapshot(adapter, snapshot).map(Some)
            }
            WinitWindowNativeEvent::Focused(focused) => {
                let snapshot = WinitWindowSnapshot::new(
                    self.id,
                    self.last_snapshot.size,
                    self.last_snapshot.visible,
                    focused,
                    self.last_snapshot.scale,
                )?;
                self.sync_snapshot(adapter, snapshot).map(Some)
            }
            WinitWindowNativeEvent::ScaleChanged(scale) => {
                let snapshot = WinitWindowSnapshot::new(
                    self.id,
                    self.last_snapshot.size,
                    self.last_snapshot.visible,
                    self.last_snapshot.focused,
                    scale,
                )?;
                self.sync_snapshot(adapter, snapshot).map(Some)
            }
            WinitWindowNativeEvent::RedrawRequested => {
                adapter.request_redraw(self.id)?;
                Ok(None)
            }
            WinitWindowNativeEvent::Snapshot(snapshot) => {
                self.sync_snapshot(adapter, snapshot).map(Some)
            }
        }
    }

    /// Apply one non-blocking winit event-loop step to this session.
    pub fn pump_event_loop_once(
        &mut self,
        adapter: &dyn WindowAdapter,
        step: &WinitWindowEventLoopStep,
    ) -> Result<WinitWindowEventLoopStepReport, UiAdapterError> {
        let mut snapshot = None;
        let mut redraw_requested = false;

        for callback in step.callbacks() {
            if matches!(callback, WinitWindowNativeEvent::RedrawRequested) {
                redraw_requested = true;
            }
            if let Some(updated) = self.handle_native_event(adapter, callback.clone())? {
                snapshot = Some(updated);
            }
        }

        Ok(WinitWindowEventLoopStepReport {
            callbacks_handled: step.callbacks().len(),
            snapshot,
            redraw_requested,
        })
    }
}

/// Static capability summary for one UI adapter build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiAdapterInfo {
    pub host_family: String,
    pub backend: String,
    pub window_backend: WindowBackendKind,
    pub planned_window_backend: WindowBackendKind,
    pub native_windows: bool,
    pub native_event_loop: bool,
}

impl UiAdapterInfo {
    /// Create a UI adapter capability summary.
    pub fn new(
        host_family: impl Into<String>,
        backend: impl Into<String>,
        window_backend: WindowBackendKind,
        planned_window_backend: WindowBackendKind,
        native_windows: bool,
        native_event_loop: bool,
    ) -> Self {
        Self {
            host_family: host_family.into(),
            backend: backend.into(),
            window_backend,
            planned_window_backend,
            native_windows,
            native_event_loop,
        }
    }

    /// Create an adapter summary for a headless draft backend.
    pub fn headless_draft(
        host_family: impl Into<String>,
        backend: impl Into<String>,
        planned_window_backend: WindowBackendKind,
    ) -> Self {
        Self::new(
            host_family,
            backend,
            WindowBackendKind::HeadlessDraft,
            planned_window_backend,
            false,
            false,
        )
    }
}

/// Shared host window contract for Phase 3.
///
/// `UiAdapter` builds on this. Keeping the named window boundary visible lets
/// the first AppKit and winit backends land without coupling them to widget
/// lowering work too early.
pub trait WindowAdapter: Send + Sync {
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

    /// Return a snapshot of a tracked window.
    fn window(&self, id: WindowId) -> Result<Option<WindowRecord>, UiAdapterError>;

    /// Attach an opaque host-native window handle to a tracked window.
    fn attach_native_window(
        &self,
        id: WindowId,
        handle: NativeWindowHandle,
    ) -> Result<(), UiAdapterError>;

    /// Return the native handle currently attached to a tracked window.
    fn native_window(&self, id: WindowId) -> Result<Option<NativeWindowHandle>, UiAdapterError>;

    /// Remove the native handle currently attached to a tracked window.
    fn detach_native_window(
        &self,
        id: WindowId,
    ) -> Result<Option<NativeWindowHandle>, UiAdapterError>;

    /// Drain queued window and UI events.
    fn drain_events(&self) -> Result<Vec<UiEvent>, UiAdapterError>;

    /// Poll one queued window or UI event in FIFO order.
    fn poll_event(&self) -> Result<Option<UiEvent>, UiAdapterError>;

    /// Queue a host close request without closing the window yet.
    fn queue_close_requested(&self, id: WindowId) -> Result<(), UiAdapterError>;

    /// Queue a host resize event and update the tracked window size.
    fn queue_host_resize(&self, id: WindowId, size: WindowSize) -> Result<(), UiAdapterError>;

    /// Queue a host window focus change.
    fn queue_window_focused(&self, id: WindowId, focused: bool) -> Result<(), UiAdapterError>;

    /// Queue a host theme preference change.
    fn queue_theme_changed(&self, theme: Theme) -> Result<(), UiAdapterError>;

    /// Queue a host scale factor change for a window.
    fn queue_scale_changed(&self, id: WindowId, scale: f32) -> Result<(), UiAdapterError>;
}

/// Shared host UI adapter contract.
///
/// Native adapters on macOS, Windows, and Linux will implement this trait. The
/// draft adapter below implements the same contract with in-memory state, so
/// runtime code can be tested before OS windows exist.
/// Host-neutral placement of one widget for native lowering.
///
/// Coordinates are absolute logical layout units with a top-left origin,
/// exactly as the layout snapshot reports them. Host backends convert to
/// their own coordinate systems (AppKit flips Y).
#[derive(Debug, Clone, PartialEq)]
pub struct WidgetPlacement {
    /// Stable widget id being lowered.
    pub widget: WidgetId,
    /// Widget kind being lowered.
    pub kind: WidgetKind,
    /// Label or text content for the native control.
    pub label: Option<String>,
    /// On/off state for checkable kinds.
    pub checked: Option<bool>,
    /// Normalized 0..=1 value for slider and progress kinds.
    pub value: Option<f32>,
    /// Row rect of the selected child, in logical pixels, resolved at
    /// lowering time for selectable containers. Painters draw the
    /// selection highlight here; `None` means nothing is selected.
    pub selection: Option<(f32, f32, f32, f32)>,
    /// Clip rectangle (x, y, w, h) in logical pixels, set for widgets
    /// inside Scroll containers; painters must not draw outside it.
    pub clip: Option<(f32, f32, f32, f32)>,
    /// Left edge in logical pixels.
    pub x: f32,
    /// Top edge in logical pixels.
    pub y: f32,
    /// Width in logical pixels.
    pub width: f32,
    /// Height in logical pixels.
    pub height: f32,
    /// True when this placement should behave as a clickable target even
    /// though its kind would normally be passive (a Text row inside a list).
    /// Hosts that lower to native controls use it to make list rows selectable.
    pub clickable: bool,
}

/// One raw pointer sample from a native backend, before hit testing.
///
/// Backends report where and whether the primary button changed; the
/// runtime routes samples through layout hit testing so the app-facing
/// event carries a widget id. Raw samples never enter the UI event queue
/// directly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawPointerSample {
    /// Window the sample belongs to.
    pub window: WindowId,
    /// X in logical pixels.
    pub x: f32,
    /// Y in logical pixels.
    pub y: f32,
    /// True on primary-button press, false on release.
    pub pressed: bool,
}

/// One raw keyboard sample from a native backend, before focus routing.
///
/// Backends report the normalized key name, transition, modifier state,
/// and any text the platform produced for the press. The runtime attaches
/// the focused widget and queues portable key/text events; raw samples
/// never enter the UI event queue directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawKeySample {
    /// Window the sample belongs to.
    pub window: WindowId,
    /// Host-normalized key name (for example "a", "Enter").
    pub key: String,
    /// True on key press, false on release.
    pub pressed: bool,
    /// Modifier state at the time of the sample.
    pub modifiers: Modifiers,
    /// Text produced by the press after keyboard layout processing.
    pub text: Option<String>,
}

/// One raw mouse-wheel sample from a native backend, before routing.
///
/// Deltas are in logical pixels (line-based wheels are pre-scaled by the
/// backend). The runtime hit-tests the position against Scroll
/// containers and adjusts host-side scroll offsets; raw samples never
/// enter the UI event queue.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawWheelSample {
    /// Window the sample belongs to.
    pub window: WindowId,
    /// Cursor X in logical pixels at wheel time.
    pub x: f32,
    /// Cursor Y in logical pixels at wheel time.
    pub y: f32,
    /// Vertical scroll delta in logical pixels (positive scrolls down).
    pub dy: f32,
}

pub trait UiAdapter: WindowAdapter {
    /// Set or replace the root widget tree for a window.
    fn set_root(&self, window: WindowId, root: WidgetNode) -> Result<(), UiAdapterError>;

    /// Insert or update a widget node for a window.
    fn upsert_node(&self, window: WindowId, node: WidgetNode) -> Result<(), UiAdapterError>;

    /// Remove a widget node and its descendants.
    fn remove_node(&self, window: WindowId, widget: WidgetId) -> Result<(), UiAdapterError>;

    /// Move focus to a widget node.
    fn focus_node(&self, window: WindowId, widget: WidgetId) -> Result<(), UiAdapterError>;

    /// Return a snapshot of a window's widget tree.
    fn widget_tree(&self, window: WindowId) -> Result<Option<WidgetTree>, UiAdapterError>;

    /// Return the focused widget for a window.
    fn focused_widget(&self, window: WindowId) -> Result<Option<WidgetId>, UiAdapterError>;

    /// Queue a routed pointer event.
    fn queue_pointer_event(&self, event: PointerEvent) -> Result<(), UiAdapterError>;

    /// Queue a routed key event.
    fn queue_key_event(&self, event: KeyEvent) -> Result<(), UiAdapterError>;

    /// Queue committed text input.
    fn queue_text_input(&self, event: TextInputEvent) -> Result<(), UiAdapterError>;

    /// Queue a native control's complete text after a person edited it.
    fn queue_text_changed(&self, event: TextChangedEvent) -> Result<(), UiAdapterError>;

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

    /// Pump one non-blocking native event-loop tick if this adapter has one.
    fn pump_event_loop_once(
        &self,
        _window: WindowId,
    ) -> Result<Option<UiEventLoopTick>, UiAdapterError> {
        Ok(None)
    }

    /// Drain raw pointer input captured by a native backend since the last
    /// call. Headless adapters return nothing.
    fn drain_raw_pointer_input(&self) -> Vec<RawPointerSample> {
        Vec::new()
    }

    /// Drain raw keyboard samples captured by the native backend since
    /// the last call. Headless adapters have no native input and return
    /// nothing.
    fn drain_raw_key_input(&self) -> Vec<RawKeySample> {
        Vec::new()
    }

    /// Drain raw mouse-wheel samples captured by the native backend
    /// since the last call. Headless adapters return nothing.
    fn drain_raw_wheel_input(&self) -> Vec<RawWheelSample> {
        Vec::new()
    }

    /// Read the current text of a natively lowered editable control.
    ///
    /// Hosts that lower to real OS controls keep typed text inside those
    /// controls, where the guest cannot see it. The gui host reads it back
    /// after each pump so the component learns what the person typed.
    ///
    /// Returns `None` when the host draws its own widgets (Linux and Windows
    /// today, where keystrokes already arrive as portable events) or when the
    /// widget is not a lowered editable control.
    fn native_widget_text(&self, _window: WindowId, _widget: WidgetId) -> Option<String> {
        None
    }

    /// Widgets currently lowered to native editable controls for a window.
    ///
    /// The gui host polls exactly these after each pump rather than every
    /// widget in the tree.
    fn native_editable_widgets(&self, _window: WindowId) -> Vec<WidgetId> {
        Vec::new()
    }

    /// Lower widget placements to native host controls, replacing any
    /// previously lowered set for the window.
    ///
    /// Returns how many widgets were lowered natively. Headless and
    /// drawn-only backends return `Ok(0)`, which means "no native lowering
    /// available" — that is a valid state, not an error, because the portable
    /// widget tree and event routes keep working without it.
    fn lower_widget_placements(
        &self,
        _window: WindowId,
        _placements: &[WidgetPlacement],
    ) -> Result<usize, UiAdapterError> {
        Ok(0)
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

impl WindowAdapter for DraftUiAdapter {
    fn info(&self) -> UiAdapterInfo {
        UiAdapterInfo::headless_draft(
            "generic",
            "headless-draft",
            WindowBackendKind::HeadlessDraft,
        )
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

    fn window(&self, id: WindowId) -> Result<Option<WindowRecord>, UiAdapterError> {
        Ok(self.registry()?.window(id).cloned())
    }

    fn attach_native_window(
        &self,
        id: WindowId,
        handle: NativeWindowHandle,
    ) -> Result<(), UiAdapterError> {
        self.registry()?.attach_native_window(id, handle)
    }

    fn native_window(&self, id: WindowId) -> Result<Option<NativeWindowHandle>, UiAdapterError> {
        self.registry()?.native_window(id)
    }

    fn detach_native_window(
        &self,
        id: WindowId,
    ) -> Result<Option<NativeWindowHandle>, UiAdapterError> {
        self.registry()?.detach_native_window(id)
    }

    fn drain_events(&self) -> Result<Vec<UiEvent>, UiAdapterError> {
        Ok(self.registry()?.drain_events())
    }

    fn poll_event(&self) -> Result<Option<UiEvent>, UiAdapterError> {
        Ok(self.registry()?.poll_event())
    }

    fn queue_close_requested(&self, id: WindowId) -> Result<(), UiAdapterError> {
        self.registry()?.queue_close_requested(id)
    }

    fn queue_host_resize(&self, id: WindowId, size: WindowSize) -> Result<(), UiAdapterError> {
        self.registry()?.queue_host_resize(id, size)
    }

    fn queue_window_focused(&self, id: WindowId, focused: bool) -> Result<(), UiAdapterError> {
        self.registry()?.queue_window_focused(id, focused)
    }

    fn queue_theme_changed(&self, theme: Theme) -> Result<(), UiAdapterError> {
        self.registry()?.queue_theme_changed(theme)
    }

    fn queue_scale_changed(&self, id: WindowId, scale: f32) -> Result<(), UiAdapterError> {
        self.registry()?.queue_scale_changed(id, scale)
    }
}

impl UiAdapter for DraftUiAdapter {
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

    fn widget_tree(&self, window: WindowId) -> Result<Option<WidgetTree>, UiAdapterError> {
        Ok(self.registry()?.widget_tree(window).cloned())
    }

    fn focused_widget(&self, window: WindowId) -> Result<Option<WidgetId>, UiAdapterError> {
        self.registry()?.focused_widget(window)
    }

    fn queue_pointer_event(&self, event: PointerEvent) -> Result<(), UiAdapterError> {
        self.registry()?.queue_pointer_event(event)
    }

    fn queue_key_event(&self, event: KeyEvent) -> Result<(), UiAdapterError> {
        self.registry()?.queue_key_event(event)
    }

    fn queue_text_input(&self, event: TextInputEvent) -> Result<(), UiAdapterError> {
        self.registry()?.queue_text_input(event)
    }

    fn queue_text_changed(&self, event: TextChangedEvent) -> Result<(), UiAdapterError> {
        self.registry()?.queue_text_changed(event)
    }
}

/// Draft in-memory window registry used until OS-backed adapters land.
#[derive(Debug, Default)]
pub struct DraftWindowRegistry {
    next_id: u64,
    windows: BTreeMap<WindowId, WindowRecord>,
    native_windows: BTreeMap<WindowId, NativeWindowHandle>,
    widget_trees: BTreeMap<WindowId, WidgetTree>,
    focused_widgets: BTreeMap<WindowId, WidgetId>,
    events: VecDeque<UiEvent>,
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
            focused: false,
            closed: false,
        };
        self.windows.insert(id, record);
        self.events.push_back(UiEvent::WindowCreated(id));
        id
    }

    /// Mark a created window as visible.
    pub fn show_window(&mut self, id: WindowId) -> Result<(), UiAdapterError> {
        let window = self.open_window_mut(id)?;
        window.visible = true;
        self.events.push_back(UiEvent::WindowShown(id));
        Ok(())
    }

    /// Mark a window as closed.
    pub fn close_window(&mut self, id: WindowId) -> Result<(), UiAdapterError> {
        let window = self.open_window_mut(id)?;
        window.closed = true;
        window.visible = false;
        self.native_windows.remove(&id);
        self.widget_trees.remove(&id);
        self.focused_widgets.remove(&id);
        self.events.push_back(UiEvent::WindowClosed(id));
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
        self.events.push_back(UiEvent::TitleChanged { id, title });
        Ok(())
    }

    /// Change a window size.
    pub fn set_size(&mut self, id: WindowId, size: WindowSize) -> Result<(), UiAdapterError> {
        let window = self.open_window_mut(id)?;
        window.size = size;
        self.events.push_back(UiEvent::Resized { id, size });
        Ok(())
    }

    /// Queue a redraw request for a window.
    pub fn request_redraw(&mut self, id: WindowId) -> Result<(), UiAdapterError> {
        self.open_window(id)?;
        self.events.push_back(UiEvent::RedrawRequested(id));
        Ok(())
    }

    /// Set or replace a window's root widget tree.
    pub fn set_root(&mut self, window: WindowId, root: WidgetNode) -> Result<(), UiAdapterError> {
        self.open_window(window)?;
        let tree = WidgetTree::new(root)?;
        let root = tree.root();
        self.widget_trees.insert(window, tree);
        self.focused_widgets.remove(&window);
        self.events
            .push_back(UiEvent::WidgetRootSet { window, root });
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
        self.events
            .push_back(UiEvent::WidgetUpdated { window, widget });
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
        self.events
            .push_back(UiEvent::WidgetRemoved { window, widget });
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
        self.events
            .push_back(UiEvent::FocusChanged { window, widget });
        Ok(())
    }

    /// Read one window record.
    pub fn window(&self, id: WindowId) -> Option<&WindowRecord> {
        self.windows.get(&id)
    }

    /// Attach an opaque native host handle to a tracked window.
    pub fn attach_native_window(
        &mut self,
        id: WindowId,
        handle: NativeWindowHandle,
    ) -> Result<(), UiAdapterError> {
        self.open_window(id)?;
        if self.native_windows.contains_key(&id) {
            return Err(UiAdapterError::NativeWindowAlreadyAttached { id: id.get() });
        }

        self.native_windows.insert(id, handle);
        self.events.push_back(UiEvent::NativeWindowAttached {
            id,
            backend: handle.backend,
        });
        Ok(())
    }

    /// Return the native handle for a tracked window.
    pub fn native_window(
        &self,
        id: WindowId,
    ) -> Result<Option<NativeWindowHandle>, UiAdapterError> {
        self.open_window(id)?;
        Ok(self.native_windows.get(&id).copied())
    }

    /// Detach the native handle for a tracked window.
    pub fn detach_native_window(
        &mut self,
        id: WindowId,
    ) -> Result<Option<NativeWindowHandle>, UiAdapterError> {
        self.open_window(id)?;
        let handle = self.native_windows.remove(&id);
        if let Some(handle) = handle {
            self.events.push_back(UiEvent::NativeWindowDetached {
                id,
                backend: handle.backend,
            });
        }
        Ok(handle)
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
        self.events.drain(..).collect()
    }

    /// Poll one queued draft event in FIFO order.
    pub fn poll_event(&mut self) -> Option<UiEvent> {
        self.events.pop_front()
    }

    /// Queue a host close request without closing the window yet.
    pub fn queue_close_requested(&mut self, id: WindowId) -> Result<(), UiAdapterError> {
        self.open_window(id)?;
        self.events.push_back(UiEvent::WindowCloseRequested(id));
        Ok(())
    }

    /// Queue a host resize event and update the tracked logical size.
    pub fn queue_host_resize(
        &mut self,
        id: WindowId,
        size: WindowSize,
    ) -> Result<(), UiAdapterError> {
        let window = self.open_window_mut(id)?;
        window.size = size;
        self.events.push_back(UiEvent::Resized { id, size });
        Ok(())
    }

    /// Queue a host window focus change.
    pub fn queue_window_focused(
        &mut self,
        id: WindowId,
        focused: bool,
    ) -> Result<(), UiAdapterError> {
        let window = self.open_window_mut(id)?;
        window.focused = focused;
        self.events
            .push_back(UiEvent::WindowFocused { id, focused });
        Ok(())
    }

    /// Queue a host theme preference change.
    pub fn queue_theme_changed(&mut self, theme: Theme) -> Result<(), UiAdapterError> {
        self.events.push_back(UiEvent::ThemeChanged { theme });
        Ok(())
    }

    /// Queue a host scale factor change for a window.
    pub fn queue_scale_changed(&mut self, id: WindowId, scale: f32) -> Result<(), UiAdapterError> {
        self.open_window(id)?;
        validate_scale_factor(scale)?;
        self.events.push_back(UiEvent::ScaleChanged { id, scale });
        Ok(())
    }

    /// Queue a pointer event after runtime hit testing has assigned a target.
    pub fn queue_pointer_event(&mut self, event: PointerEvent) -> Result<(), UiAdapterError> {
        self.validate_event_target(event.window, event.widget)?;
        self.events.push_back(UiEvent::Pointer(event));
        Ok(())
    }

    /// Queue a key event after runtime focus routing has assigned a target.
    pub fn queue_key_event(&mut self, event: KeyEvent) -> Result<(), UiAdapterError> {
        validate_key_name(&event.key)?;
        self.validate_event_target(event.window, event.widget)?;
        self.events.push_back(UiEvent::Key(event));
        Ok(())
    }

    /// Queue committed text input after runtime focus routing has assigned a target.
    pub fn queue_text_input(&mut self, event: TextInputEvent) -> Result<(), UiAdapterError> {
        validate_text_input(&event.text)?;
        self.validate_event_target(event.window, event.widget)?;
        self.events.push_back(UiEvent::TextInput(event));
        Ok(())
    }

    pub fn queue_text_changed(&mut self, event: TextChangedEvent) -> Result<(), UiAdapterError> {
        validate_text_input(&event.text)?;
        self.validate_event_target(event.window, Some(event.widget))?;
        self.events.push_back(UiEvent::TextChanged(event));
        Ok(())
    }

    fn validate_event_target(
        &self,
        window: WindowId,
        widget: Option<WidgetId>,
    ) -> Result<(), UiAdapterError> {
        self.open_window(window)?;
        if let Some(widget) = widget {
            let tree = self
                .widget_trees
                .get(&window)
                .ok_or(UiAdapterError::MissingWidgetTree {
                    window: window.get(),
                })?;
            if !tree.nodes.contains_key(&widget) {
                return Err(UiAdapterError::InvalidWidgetId { id: widget.get() });
            }
        }

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
    #[error("invalid input event: {0}")]
    InvalidInputEvent(String),
    #[error("invalid scale factor: {0}")]
    InvalidScaleFactor(String),
    #[error("window {id} is closed")]
    WindowClosed { id: u64 },
    #[error("invalid window size {width}x{height}")]
    InvalidSize { width: u32, height: u32 },
    #[error("window title is empty")]
    EmptyTitle,
    #[error("window title is too long")]
    TitleTooLong,
    #[error("invalid native window handle {raw_handle} for backend {backend}")]
    InvalidNativeWindowHandle { backend: String, raw_handle: u64 },
    #[error("window {id} already has a native handle attached")]
    NativeWindowAlreadyAttached { id: u64 },
    #[error("unsupported UI feature: {0}")]
    Unsupported(String),
    #[error("internal UI adapter error: {0}")]
    Internal(String),
}

const MAX_WINDOW_EDGE: u32 = 16_384;
const MAX_TITLE_CHARS: usize = 512;
const MAX_WIDGET_TEXT_CHARS: usize = 1_024;
const MAX_KEY_NAME_CHARS: usize = 128;
const MAX_TEXT_INPUT_CHARS: usize = 4_096;
const MAX_SCALE_FACTOR: f32 = 8.0;

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

fn validate_key_name(value: &str) -> Result<(), UiAdapterError> {
    if value.trim().is_empty() {
        return Err(UiAdapterError::InvalidInputEvent(
            "key name is empty".to_string(),
        ));
    }
    if value.chars().count() > MAX_KEY_NAME_CHARS {
        return Err(UiAdapterError::InvalidInputEvent(
            "key name is too long".to_string(),
        ));
    }
    if value.chars().any(char::is_control) {
        return Err(UiAdapterError::InvalidInputEvent(
            "key name contains control characters".to_string(),
        ));
    }

    Ok(())
}

fn validate_text_input(value: &str) -> Result<(), UiAdapterError> {
    if value.is_empty() {
        return Err(UiAdapterError::InvalidInputEvent(
            "text input is empty".to_string(),
        ));
    }
    if value.chars().count() > MAX_TEXT_INPUT_CHARS {
        return Err(UiAdapterError::InvalidInputEvent(
            "text input is too long".to_string(),
        ));
    }

    Ok(())
}

fn validate_scale_factor(value: f32) -> Result<(), UiAdapterError> {
    if !value.is_finite() || value <= 0.0 || value > MAX_SCALE_FACTOR {
        return Err(UiAdapterError::InvalidScaleFactor(format!(
            "{value} must be finite and between 0 and {MAX_SCALE_FACTOR}"
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
        assert!(!window.focused);
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
    fn draft_registry_polls_events_in_fifo_order() {
        let mut registry = DraftWindowRegistry::default();
        let size = WindowSize::new(800, 600).expect("size");
        let id = registry.create_window(WindowOptions::new("Notes", size).expect("options"));
        registry.show_window(id).expect("show");

        assert_eq!(registry.poll_event(), Some(UiEvent::WindowCreated(id)));
        assert_eq!(registry.poll_event(), Some(UiEvent::WindowShown(id)));
        assert_eq!(registry.poll_event(), None);
    }

    #[test]
    fn draft_registry_queues_host_window_events_without_closing() {
        let mut registry = DraftWindowRegistry::default();
        let size = WindowSize::new(800, 600).expect("size");
        let id = registry.create_window(WindowOptions::new("Notes", size).expect("options"));
        let resized = WindowSize::new(900, 700).expect("resized");

        registry.queue_window_focused(id, true).expect("focus");
        registry.queue_host_resize(id, resized).expect("resize");
        registry.queue_close_requested(id).expect("close request");

        let window = registry.window(id).expect("window");
        assert_eq!(window.size, resized);
        assert!(window.focused);
        assert!(!window.closed);
        assert_eq!(
            registry.drain_events(),
            vec![
                UiEvent::WindowCreated(id),
                UiEvent::WindowFocused { id, focused: true },
                UiEvent::Resized { id, size: resized },
                UiEvent::WindowCloseRequested(id),
            ]
        );
    }

    #[test]
    fn draft_registry_queues_theme_and_scale_events() {
        let mut registry = DraftWindowRegistry::default();
        let size = WindowSize::new(800, 600).expect("size");
        let id = registry.create_window(WindowOptions::new("Notes", size).expect("options"));

        registry
            .queue_theme_changed(Theme::Dark)
            .expect("theme changed");
        registry
            .queue_scale_changed(id, 2.0)
            .expect("scale changed");

        assert_eq!(
            registry.drain_events(),
            vec![
                UiEvent::WindowCreated(id),
                UiEvent::ThemeChanged { theme: Theme::Dark },
                UiEvent::ScaleChanged { id, scale: 2.0 },
            ]
        );
    }

    #[test]
    fn draft_registry_tracks_native_window_handles() {
        let mut registry = DraftWindowRegistry::default();
        let size = WindowSize::new(800, 600).expect("size");
        let id = registry.create_window(WindowOptions::new("Notes", size).expect("options"));
        let handle = NativeWindowHandle::new(WindowBackendKind::AppKit, 0xCAFE).expect("handle");

        registry
            .attach_native_window(id, handle)
            .expect("attach native handle");

        assert_eq!(registry.native_window(id).expect("lookup"), Some(handle));
        assert!(matches!(
            registry.attach_native_window(id, handle),
            Err(UiAdapterError::NativeWindowAlreadyAttached { id: 1 })
        ));
        assert_eq!(
            registry.detach_native_window(id).expect("detach"),
            Some(handle)
        );
        assert_eq!(registry.native_window(id).expect("lookup"), None);
        assert_eq!(
            registry.drain_events(),
            vec![
                UiEvent::WindowCreated(id),
                UiEvent::NativeWindowAttached {
                    id,
                    backend: WindowBackendKind::AppKit,
                },
                UiEvent::NativeWindowDetached {
                    id,
                    backend: WindowBackendKind::AppKit,
                },
            ]
        );
    }

    #[test]
    fn draft_registry_removes_native_window_handle_on_close() {
        let mut registry = DraftWindowRegistry::default();
        let size = WindowSize::new(800, 600).expect("size");
        let id = registry.create_window(WindowOptions::new("Notes", size).expect("options"));
        let handle = NativeWindowHandle::new(WindowBackendKind::Winit, 0xBEEF).expect("handle");

        registry.attach_native_window(id, handle).expect("attach");
        registry.close_window(id).expect("close");

        assert!(matches!(
            registry.native_window(id),
            Err(UiAdapterError::WindowClosed { id: 1 })
        ));
    }

    #[test]
    fn native_window_handle_rejects_non_native_backends() {
        assert!(matches!(
            NativeWindowHandle::new(WindowBackendKind::HeadlessDraft, 0xCAFE),
            Err(UiAdapterError::InvalidNativeWindowHandle { .. })
        ));
        assert!(matches!(
            NativeWindowHandle::new(WindowBackendKind::Unknown, 0xCAFE),
            Err(UiAdapterError::InvalidNativeWindowHandle { .. })
        ));
        assert!(matches!(
            NativeWindowHandle::new(WindowBackendKind::AppKit, 0),
            Err(UiAdapterError::InvalidNativeWindowHandle { .. })
        ));
    }

    #[test]
    fn draft_registry_rejects_invalid_scale_events() {
        let mut registry = DraftWindowRegistry::default();
        let size = WindowSize::new(800, 600).expect("size");
        let id = registry.create_window(WindowOptions::new("Notes", size).expect("options"));

        assert!(matches!(
            registry.queue_scale_changed(id, 0.0),
            Err(UiAdapterError::InvalidScaleFactor(_))
        ));
        assert!(matches!(
            registry.queue_scale_changed(id, f32::INFINITY),
            Err(UiAdapterError::InvalidScaleFactor(_))
        ));
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
    fn selected_index_is_accepted_only_by_selectable_kinds() {
        let list = WidgetNode::new(WidgetId::new(1).expect("list"), WidgetKind::ListView)
            .with_selected(2)
            .expect("list view accepts a selection");
        assert_eq!(list.selected, Some(2));

        WidgetNode::new(WidgetId::new(2).expect("tree"), WidgetKind::TreeView)
            .with_selected(0)
            .expect("tree view accepts a selection");

        // A selection on a leaf widget is meaningless and must be refused
        // at construction rather than silently ignored at paint time.
        let err = WidgetNode::new(WidgetId::new(3).expect("button"), WidgetKind::Button)
            .with_selected(0)
            .expect_err("button must reject a selection");
        assert!(matches!(err, UiAdapterError::Unsupported(_)));
    }

    #[test]
    fn children_are_returned_in_stable_id_order() {
        let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::ListView);
        let mut tree = WidgetTree::new(root).expect("tree");
        // Insert out of order; the accessor must still report id order, so
        // a selected index means the same row across re-lowering.
        for id in [30u64, 10, 20] {
            let row = WidgetNode::new(WidgetId::new(id).expect("row"), WidgetKind::Text)
                .with_parent(tree.root());
            tree.upsert(row).expect("insert row");
        }
        let children: Vec<u64> = tree
            .children(tree.root())
            .into_iter()
            .map(|id| id.get())
            .collect();
        assert_eq!(children, vec![10, 20, 30]);
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
    fn draft_registry_queues_key_and_text_events_for_focused_widget() {
        let mut registry = DraftWindowRegistry::default();
        let size = WindowSize::new(800, 600).expect("size");
        let window = registry.create_window(WindowOptions::new("Notes", size).expect("options"));
        let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Stack);
        let input = WidgetNode::new(WidgetId::new(2).expect("input"), WidgetKind::TextField)
            .with_parent(root.id)
            .with_label("Title")
            .expect("label");

        registry.set_root(window, root).expect("root");
        registry.upsert_node(window, input).expect("input");
        registry
            .focus_node(window, WidgetId::new(2).expect("input"))
            .expect("focus");
        registry
            .queue_key_event(
                KeyEvent::new(
                    window,
                    Some(WidgetId::new(2).expect("input")),
                    "Enter",
                    true,
                    Modifiers {
                        meta: true,
                        ..Modifiers::default()
                    },
                )
                .expect("key"),
            )
            .expect("queue key");
        registry
            .queue_text_input(
                TextInputEvent::new(window, Some(WidgetId::new(2).expect("input")), "hello")
                    .expect("text"),
            )
            .expect("queue text");

        assert_eq!(
            registry.drain_events().last_chunk::<2>(),
            Some(&[
                UiEvent::Key(
                    KeyEvent::new(
                        window,
                        Some(WidgetId::new(2).expect("input")),
                        "Enter",
                        true,
                        Modifiers {
                            meta: true,
                            ..Modifiers::default()
                        },
                    )
                    .expect("key")
                ),
                UiEvent::TextInput(
                    TextInputEvent::new(window, Some(WidgetId::new(2).expect("input")), "hello")
                        .expect("text"),
                ),
            ])
        );
    }

    #[test]
    fn input_event_constructors_reject_bad_shapes() {
        let window = WindowId(1);

        assert!(matches!(
            KeyEvent::new(window, None, "", true, Modifiers::default()),
            Err(UiAdapterError::InvalidInputEvent(_))
        ));
        assert!(matches!(
            KeyEvent::new(window, None, "A\nB", true, Modifiers::default()),
            Err(UiAdapterError::InvalidInputEvent(_))
        ));
        assert!(matches!(
            TextInputEvent::new(window, None, ""),
            Err(UiAdapterError::InvalidInputEvent(_))
        ));
    }

    #[test]
    fn draft_adapter_implements_shared_ui_contract() {
        let adapter = DraftUiAdapter::new();
        let size = WindowSize::new(700, 500).expect("size");
        let id = adapter
            .create_window(WindowOptions::new("Krate", size).expect("options"))
            .expect("create");

        adapter.show_window(id).expect("show");
        adapter
            .set_title(id, "Krate Preview".to_string())
            .expect("title");
        adapter.request_redraw(id).expect("redraw");

        let window = adapter.window(id).expect("window lookup").expect("window");
        assert_eq!(window.title, "Krate Preview");
        assert!(window.visible);
        assert_eq!(
            adapter.drain_events().expect("events"),
            vec![
                UiEvent::WindowCreated(id),
                UiEvent::WindowShown(id),
                UiEvent::TitleChanged {
                    id,
                    title: "Krate Preview".to_string(),
                },
                UiEvent::RedrawRequested(id),
            ]
        );
    }

    #[test]
    fn draft_adapter_polls_before_drain() {
        let adapter = DraftUiAdapter::new();
        let size = WindowSize::new(700, 500).expect("size");
        let id = adapter
            .create_window(WindowOptions::new("Krate", size).expect("options"))
            .expect("create");
        adapter.show_window(id).expect("show");

        assert_eq!(
            adapter.poll_event().expect("poll"),
            Some(UiEvent::WindowCreated(id))
        );
        assert_eq!(
            adapter.drain_events().expect("events"),
            vec![UiEvent::WindowShown(id)]
        );
        assert_eq!(adapter.poll_event().expect("poll"), None);
    }

    #[test]
    fn draft_adapter_event_loop_pump_is_noop() {
        let adapter = DraftUiAdapter::new();
        let id = adapter
            .create_window(
                WindowOptions::new("Krate no native loop", WindowSize::new(640, 480).unwrap())
                    .unwrap(),
            )
            .expect("create");

        assert_eq!(adapter.pump_event_loop_once(id).expect("pump"), None);
    }

    #[test]
    fn winit_session_syncs_snapshot_changes_through_window_adapter() {
        let adapter = DraftUiAdapter::new();
        let size = WindowSize::new(640, 480).expect("size");
        let id = adapter
            .create_window(WindowOptions::new("Krate winit", size).expect("options"))
            .expect("create");
        let handle = NativeWindowHandle::new(WindowBackendKind::Winit, 0xD06).expect("handle");
        adapter
            .attach_native_window(id, handle)
            .expect("attach native");
        let initial =
            WinitWindowSnapshot::new(id, size, false, false, 1.0).expect("initial snapshot");
        let mut session = WinitWindowSession::new(id, handle, initial).expect("session");
        let resized = WindowSize::new(800, 600).expect("resized");
        let updated = WinitWindowSnapshot::new(id, resized, true, true, 2.0).expect("updated");

        assert_eq!(
            session
                .sync_snapshot(&adapter, updated)
                .expect("sync snapshot"),
            updated
        );
        assert_eq!(session.id(), id);
        assert_eq!(session.native_handle(), handle);
        assert_eq!(session.last_snapshot(), updated);
        assert_eq!(
            adapter.drain_events().expect("events"),
            vec![
                UiEvent::WindowCreated(id),
                UiEvent::NativeWindowAttached {
                    id,
                    backend: WindowBackendKind::Winit,
                },
                UiEvent::Resized { id, size: resized },
                UiEvent::WindowFocused { id, focused: true },
                UiEvent::ScaleChanged { id, scale: 2.0 },
            ]
        );
    }

    #[test]
    fn winit_event_loop_step_routes_native_callbacks() {
        let adapter = DraftUiAdapter::new();
        let size = WindowSize::new(640, 480).expect("size");
        let id = adapter
            .create_window(WindowOptions::new("Krate winit", size).expect("options"))
            .expect("create");
        let handle = NativeWindowHandle::new(WindowBackendKind::Winit, 0xD06).expect("handle");
        adapter
            .attach_native_window(id, handle)
            .expect("attach native");
        let initial =
            WinitWindowSnapshot::new(id, size, false, false, 1.0).expect("initial snapshot");
        let mut session = WinitWindowSession::new(id, handle, initial).expect("session");
        let resized = WindowSize::new(900, 700).expect("resized");
        let step = WinitWindowEventLoopStep::new().with_callbacks([
            WinitWindowNativeEvent::Focused(true),
            WinitWindowNativeEvent::Resized(resized),
            WinitWindowNativeEvent::RedrawRequested,
            WinitWindowNativeEvent::CloseRequested,
        ]);

        let report = session
            .pump_event_loop_once(&adapter, &step)
            .expect("pump event loop");

        assert_eq!(report.callbacks_handled, 4);
        assert_eq!(
            report.snapshot,
            Some(WinitWindowSnapshot::new(id, resized, false, true, 1.0).expect("snapshot"))
        );
        assert!(report.redraw_requested);
        assert_eq!(
            adapter.drain_events().expect("events"),
            vec![
                UiEvent::WindowCreated(id),
                UiEvent::NativeWindowAttached {
                    id,
                    backend: WindowBackendKind::Winit,
                },
                UiEvent::WindowFocused { id, focused: true },
                UiEvent::Resized { id, size: resized },
                UiEvent::RedrawRequested(id),
                UiEvent::WindowCloseRequested(id),
            ]
        );
    }

    #[test]
    fn winit_event_collector_drains_callbacks_in_order() {
        let adapter = DraftUiAdapter::new();
        let size = WindowSize::new(640, 480).expect("size");
        let id = adapter
            .create_window(WindowOptions::new("Krate winit", size).expect("options"))
            .expect("create");
        let handle = NativeWindowHandle::new(WindowBackendKind::Winit, 0xD06).expect("handle");
        adapter
            .attach_native_window(id, handle)
            .expect("attach native");
        let initial =
            WinitWindowSnapshot::new(id, size, false, false, 1.0).expect("initial snapshot");
        let mut session = WinitWindowSession::new(id, handle, initial).expect("session");
        let resized = WindowSize::new(760, 540).expect("resized");
        let mut collector = WinitWindowEventCollector::new(id);

        collector
            .push_event(WinitWindowNativeEvent::Focused(true))
            .expect("focus");
        collector
            .push_event(WinitWindowNativeEvent::Resized(resized))
            .expect("resize");
        collector
            .push_event(WinitWindowNativeEvent::RedrawRequested)
            .expect("redraw");
        assert_eq!(collector.window(), id);
        assert_eq!(collector.pending_callbacks(), 3);
        assert!(!collector.is_empty());

        let step = collector.drain_step();
        let report = session
            .pump_event_loop_once(&adapter, &step)
            .expect("pump event loop");

        assert!(collector.is_empty());
        assert_eq!(report.callbacks_handled, 3);
        assert_eq!(
            report.snapshot,
            Some(WinitWindowSnapshot::new(id, resized, false, true, 1.0).expect("snapshot"))
        );
        assert!(report.redraw_requested);
        assert_eq!(
            adapter.drain_events().expect("events"),
            vec![
                UiEvent::WindowCreated(id),
                UiEvent::NativeWindowAttached {
                    id,
                    backend: WindowBackendKind::Winit,
                },
                UiEvent::WindowFocused { id, focused: true },
                UiEvent::Resized { id, size: resized },
                UiEvent::RedrawRequested(id),
            ]
        );
    }

    #[test]
    fn winit_event_collector_rejects_foreign_snapshots() {
        let adapter = DraftUiAdapter::new();
        let size = WindowSize::new(640, 480).expect("size");
        let first = adapter
            .create_window(WindowOptions::new("Krate first", size).expect("options"))
            .expect("first");
        let second = adapter
            .create_window(WindowOptions::new("Krate second", size).expect("options"))
            .expect("second");
        let mut collector = WinitWindowEventCollector::new(first);
        let foreign = WinitWindowSnapshot::new(second, size, false, false, 1.0).expect("snapshot");

        assert!(matches!(
            collector.push_event(WinitWindowNativeEvent::Snapshot(foreign)),
            Err(UiAdapterError::InvalidWindow { id }) if id == second.get()
        ));
        assert!(collector.is_empty());
    }

    #[test]
    fn draft_adapter_reports_headless_info() {
        let adapter = DraftUiAdapter::new();
        let info = adapter.info();
        let window_adapter: &dyn WindowAdapter = &adapter;
        let window_info = window_adapter.info();

        assert_eq!(info.host_family, "generic");
        assert_eq!(info.backend, "headless-draft");
        assert_eq!(info.window_backend, WindowBackendKind::HeadlessDraft);
        assert_eq!(
            info.planned_window_backend,
            WindowBackendKind::HeadlessDraft
        );
        assert!(!info.native_windows);
        assert!(!info.native_event_loop);
        assert_eq!(window_info, info);
    }

    #[test]
    fn draft_adapter_exposes_native_window_handle_boundary() {
        let adapter = DraftUiAdapter::new();
        let size = WindowSize::new(640, 480).expect("size");
        let id = adapter
            .create_window(WindowOptions::new("Krate", size).expect("options"))
            .expect("window");
        let handle = NativeWindowHandle::new(WindowBackendKind::Winit, 0x1234).expect("handle");

        adapter
            .attach_native_window(id, handle)
            .expect("attach native handle");

        assert_eq!(adapter.native_window(id).expect("lookup"), Some(handle));
        assert_eq!(
            adapter.detach_native_window(id).expect("detach"),
            Some(handle)
        );
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
