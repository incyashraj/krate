//! Host implementation for the Phase 3 `gui` world's new imports.
//!
//! `Phase3GuiHost` backs the `krate:ui` interfaces with the UCap-gated
//! Phase 3 UI dispatcher. Window, widget-tree, and event calls are real;
//! after every tree change the host recomputes layout and re-lowers the
//! supported widgets to native controls when the selected adapter can (the
//! opt-in macOS AppKit prototype today — headless adapters lower nothing and
//! that is a valid state). The `gfx`, `audio`, `dialog`, and `menu` surfaces
//! return honest `unsupported` errors until their runtimes exist.

use krate_adapter_common::painter::drawn_kind;
use krate_adapter_common::ui::{
    kind_is_selectable, Modifiers, PointerButton, Theme, UiAdapterError, UiEvent, WidgetId,
    WidgetKind, WidgetNode, WidgetPlacement, WidgetStyle, WindowId, WindowOptions, WindowSize,
};
use krate_layout::{absolute_rect, LayoutViewport};

use crate::{
    phase3_gui_bindings::krate::{audio, gfx, ui},
    phase3_ui::{Phase3HostUiMode, Phase3UiDispatcher, Phase3UiRuntime, UiDispatchError},
    uapi::UapiGuard,
};

/// How long `events.wait` sleeps between polls.
const WAIT_POLL_INTERVAL_MILLIS: u64 = 10;

/// Host state for the Phase 3 `gui` world imports.
pub struct Phase3GuiHost {
    runtime: Phase3UiRuntime,
    windows: Vec<WindowId>,
    /// Host-side vertical scroll offsets per (window, Scroll widget).
    /// Scrolling never involves the guest: wheel input adjusts these and
    /// re-lowers placements, matching native platform feel.
    scroll_offsets: std::cell::RefCell<std::collections::BTreeMap<(WindowId, WidgetId), f32>>,
    /// Last text the host observed in each natively lowered editable control.
    /// AppKit keeps typed characters inside the control, so the guest only
    /// learns about them by the host reading the control back and comparing.
    native_text: std::cell::RefCell<std::collections::BTreeMap<(WindowId, WidgetId), String>>,
}

impl Phase3GuiHost {
    /// Create the GUI host with the requested host UI mode.
    pub fn new(guard: UapiGuard, mode: Phase3HostUiMode) -> Result<Self, UiDispatchError> {
        let runtime = Phase3UiRuntime::try_with_host_adapter_mode(guard, mode)?;
        Ok(Self {
            runtime,
            windows: Vec::new(),
            scroll_offsets: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            native_text: std::cell::RefCell::new(std::collections::BTreeMap::new()),
        })
    }

    fn dispatcher(&self) -> Phase3UiDispatcher<'_> {
        self.runtime.dispatcher()
    }

    /// Recompute layout and re-lower supported widgets to native controls.
    ///
    /// This is the naive vertical-slice strategy: every tree change replaces
    /// the whole native widget set. Reconciler diffing comes later.
    fn sync_native_widgets(&self, window: WindowId) -> Result<(), UiDispatchError> {
        let dispatcher = self.dispatcher();
        let Some(tree) = dispatcher.widget_tree(window)? else {
            return Ok(());
        };
        let Some(record) = dispatcher.window(window)? else {
            return Ok(());
        };

        let viewport = LayoutViewport::new(record.size.width as f32, record.size.height as f32)
            .map_err(|err| UiDispatchError::Layout(err.to_string()))?;
        let layout = dispatcher.compute_layout(window, viewport)?;

        let offsets = self.scroll_offsets.borrow();
        let mut placements = Vec::new();
        for (id, node) in tree.nodes() {
            // One shared list decides what the drawn painters support, so
            // placement filtering and painting can never drift apart.
            if !drawn_kind(node.kind) {
                continue;
            }
            let Some(rect) = absolute_rect(&tree, &layout, *id) else {
                continue;
            };
            // Widgets inside a Scroll container shift by the container's
            // host-side offset and clip to the container's rectangle.
            let mut y = rect.y;
            let mut clip = None;
            if let Some(scroll_id) = nearest_scroll_ancestor(&tree, *id) {
                if let Some(scroll_rect) = absolute_rect(&tree, &layout, scroll_id) {
                    let offset = offsets.get(&(window, scroll_id)).copied().unwrap_or(0.0);
                    y -= offset;
                    clip = Some((
                        scroll_rect.x,
                        scroll_rect.y,
                        scroll_rect.width,
                        scroll_rect.height,
                    ));
                }
            }
            // Resolve a selectable container's selected index to the child's
            // rect here, where the tree and layout are both in hand; the
            // painters only ever see rectangles. Out-of-range indices and
            // children that failed layout simply draw no highlight.
            let selection = node.selected.and_then(|index| {
                let child = *tree.children(*id).get(index as usize)?;
                let child_rect = absolute_rect(&tree, &layout, child)?;
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
            let clickable = node.kind == WidgetKind::Text
                && node
                    .parent
                    .and_then(|parent| tree.node(parent))
                    .is_some_and(|parent| parent.kind == WidgetKind::ListView);
            placements.push(WidgetPlacement {
                widget: *id,
                kind: node.kind,
                label: node.label.clone(),
                checked: node.checked,
                value: node.value,
                selection,
                clip,
                x: rect.x,
                y,
                width: rect.width,
                height: rect.height,
                clickable,
            });
        }
        drop(offsets);

        dispatcher.lower_widget_placements(window, &placements)?;
        Ok(())
    }

    /// Report a natively lowered control's text whenever a person changes it.
    ///
    /// On hosts that lower to real OS controls, the control holds the text and
    /// the component never sees it. Reading each editable control back after a
    /// pump closes that loop.
    ///
    /// This sends the control's **complete** text, not the part that was added.
    /// An append cannot describe deleting, selecting, or pasting, and trying to
    /// derive one leaves two copies of the text drifting apart. The control is
    /// the single owner; the component mirrors it.
    fn sync_native_text(&self, window: WindowId, dispatcher: &Phase3UiDispatcher<'_>) {
        for widget in dispatcher.native_editable_widgets(window) {
            let Some(current) = dispatcher.native_widget_text(window, widget) else {
                continue;
            };

            let changed = {
                let mut seen = self.native_text.borrow_mut();
                if seen.get(&(window, widget)).map(String::as_str) == Some(current.as_str()) {
                    false
                } else {
                    seen.insert((window, widget), current.clone());
                    true
                }
            };

            if changed {
                let _ = dispatcher.queue_text_changed(window, widget, current);
            }
        }
    }

    fn poll_one_event(&self) -> Result<Option<ui::types::Event>, UiDispatchError> {
        let dispatcher = self.dispatcher();
        for window in &self.windows {
            // Native pumps refresh window state and drain delegate callbacks;
            // headless adapters return no tick. Ignore per-window pump errors
            // so one closed window cannot wedge event delivery.
            let _ = dispatcher.pump_event_loop_once(*window);
            self.sync_native_text(*window, &dispatcher);
        }

        // Route raw native pointer input through layout hit testing so the
        // app-facing event carries a widget id. Raw samples never reach the
        // queue directly, so this cannot loop.
        for sample in dispatcher.drain_raw_pointer_input() {
            if let Some(record) = dispatcher.window(sample.window)? {
                if let Ok(viewport) =
                    LayoutViewport::new(record.size.width as f32, record.size.height as f32)
                {
                    let routed =
                        dispatcher.route_pointer_event(crate::phase3_ui::PointerRouteRequest {
                            window: sample.window,
                            viewport,
                            x: sample.x,
                            y: sample.y,
                            button: Some(PointerButton::Primary),
                            pressed: sample.pressed,
                            modifiers: Modifiers::default(),
                        });
                    // Click-to-focus: a press routed onto a text-entry
                    // widget moves keyboard focus there (queues the
                    // portable focus-changed event through the dispatcher).
                    if sample.pressed {
                        if let Ok(Some(widget)) = routed {
                            if let Ok(Some(tree)) = dispatcher.widget_tree(sample.window) {
                                let focusable = tree
                                    .node(widget)
                                    .is_some_and(|node| press_focuses(node.kind));
                                if focusable
                                    && dispatcher.focused_widget(sample.window).ok().flatten()
                                        != Some(widget)
                                {
                                    let _ = dispatcher.focus_node(sample.window, widget);
                                }
                            }
                        }
                    }
                }
            }
        }

        // Attach keyboard focus to raw key samples and queue portable
        // key/text events. Raw samples never enter the queue directly.
        for sample in dispatcher.drain_raw_key_input() {
            let focused = dispatcher.focused_widget(sample.window).ok().flatten();
            if let Ok(event) = krate_adapter_common::ui::KeyEvent::new(
                sample.window,
                focused,
                sample.key.clone(),
                sample.pressed,
                sample.modifiers,
            ) {
                let _ = dispatcher.queue_key_event(event);
            }
            if sample.pressed {
                if let Some(text) = sample.text.as_deref() {
                    if let Ok(event) =
                        krate_adapter_common::ui::TextInputEvent::new(sample.window, focused, text)
                    {
                        let _ = dispatcher.queue_text_input(event);
                    }
                }
            }
        }

        // Wheel input scrolls host-side: hit-test the topmost Scroll
        // container under the cursor, clamp its offset to the content
        // extent, and re-lower. The guest never sees wheel events.
        for sample in dispatcher.drain_raw_wheel_input() {
            let Ok(Some(record)) = dispatcher.window(sample.window) else {
                continue;
            };
            let Ok(viewport) =
                LayoutViewport::new(record.size.width as f32, record.size.height as f32)
            else {
                continue;
            };
            let Ok(Some(tree)) = dispatcher.widget_tree(sample.window) else {
                continue;
            };
            let Ok(layout) = dispatcher.compute_layout(sample.window, viewport) else {
                continue;
            };
            let Some(scroll_id) = scroll_container_at(&tree, &layout, sample.x, sample.y) else {
                continue;
            };
            let Some(scroll_rect) = absolute_rect(&tree, &layout, scroll_id) else {
                continue;
            };
            let content_bottom = tree
                .nodes()
                .iter()
                .filter(|(child, _)| nearest_scroll_ancestor(&tree, **child) == Some(scroll_id))
                .filter_map(|(child, _)| absolute_rect(&tree, &layout, *child))
                .map(|r| r.y + r.height)
                .fold(scroll_rect.y, f32::max);
            let content_height = content_bottom - scroll_rect.y;
            let mut offsets = self.scroll_offsets.borrow_mut();
            let entry = offsets.entry((sample.window, scroll_id)).or_insert(0.0);
            let updated =
                clamped_scroll_offset(*entry, sample.dy, content_height, scroll_rect.height);
            if (updated - *entry).abs() > f32::EPSILON {
                *entry = updated;
                drop(offsets);
                let _ = self.sync_native_widgets(sample.window);
            }
        }

        // Skip host-side bookkeeping events that have no portable WIT shape.
        while let Some(event) = dispatcher.poll_event()? {
            if let Some(event) = event_to_wit(event) {
                return Ok(Some(event));
            }
        }
        Ok(None)
    }
}

/// Map a dispatch error into the portable `ui-error` shape.
fn dispatch_error_to_ui_error(err: UiDispatchError) -> ui::types::UiError {
    match err {
        UiDispatchError::PermissionDenied => ui::types::UiError::PermissionDenied,
        UiDispatchError::Adapter(UiAdapterError::Unsupported(message)) => {
            ui::types::UiError::Unsupported(message)
        }
        UiDispatchError::Adapter(UiAdapterError::InvalidWindow { .. }) => {
            ui::types::UiError::InvalidWindow
        }
        UiDispatchError::Adapter(UiAdapterError::InvalidWidgetId { .. }) => {
            ui::types::UiError::InvalidWidget
        }
        other => ui::types::UiError::Platform(other.to_string()),
    }
}

impl Phase3GuiHost {
    /// Resolve a guest-supplied raw window id against the windows this
    /// component created. Guests cannot reference windows they do not own.
    fn window_id(&self, raw: u64) -> Result<WindowId, ui::types::UiError> {
        self.windows
            .iter()
            .copied()
            .find(|window| window.get() == raw)
            .ok_or(ui::types::UiError::InvalidWindow)
    }
}

/// Nearest Scroll ancestor of a widget, if any.
fn nearest_scroll_ancestor(
    tree: &krate_adapter_common::ui::WidgetTree,
    id: WidgetId,
) -> Option<WidgetId> {
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
fn scroll_container_at(
    tree: &krate_adapter_common::ui::WidgetTree,
    layout: &krate_layout::LayoutSnapshot,
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
fn clamped_scroll_offset(current: f32, dy: f32, content_height: f32, viewport_height: f32) -> f32 {
    let max_offset = (content_height - viewport_height).max(0.0);
    (current + dy).clamp(0.0, max_offset)
}

/// Widget kinds that take keyboard focus from a pointer press.
fn press_focuses(kind: WidgetKind) -> bool {
    matches!(kind, WidgetKind::TextField | WidgetKind::TextArea)
}

fn widget_id(raw: u64) -> Result<WidgetId, ui::types::UiError> {
    WidgetId::new(raw).map_err(|_| ui::types::UiError::InvalidWidget)
}

fn widget_kind_from_wit(kind: ui::types::WidgetKind) -> WidgetKind {
    match kind {
        ui::types::WidgetKind::Stack => WidgetKind::Stack,
        ui::types::WidgetKind::Grid => WidgetKind::Grid,
        ui::types::WidgetKind::Scroll => WidgetKind::Scroll,
        ui::types::WidgetKind::Tabs => WidgetKind::Tabs,
        ui::types::WidgetKind::Button => WidgetKind::Button,
        ui::types::WidgetKind::Checkbox => WidgetKind::Checkbox,
        ui::types::WidgetKind::Radio => WidgetKind::Radio,
        ui::types::WidgetKind::Switch => WidgetKind::Switch,
        ui::types::WidgetKind::Slider => WidgetKind::Slider,
        ui::types::WidgetKind::Progress => WidgetKind::Progress,
        ui::types::WidgetKind::Text => WidgetKind::Text,
        ui::types::WidgetKind::TextField => WidgetKind::TextField,
        ui::types::WidgetKind::TextArea => WidgetKind::TextArea,
        ui::types::WidgetKind::ListView => WidgetKind::ListView,
        ui::types::WidgetKind::TreeView => WidgetKind::TreeView,
        ui::types::WidgetKind::Image => WidgetKind::Image,
        ui::types::WidgetKind::Canvas => WidgetKind::Canvas,
    }
}

fn widget_node_from_wit(node: ui::types::WidgetNode) -> Result<WidgetNode, ui::types::UiError> {
    let id = widget_id(node.id)?;
    let parent = node.parent.map(widget_id).transpose()?;
    let style = WidgetStyle {
        width: node.style.width,
        height: node.style.height,
        grow: node.style.grow,
        padding: node.style.padding,
    };
    if let Some(value) = node.value {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(ui::types::UiError::Unsupported(
                "widget value must be a finite number in 0..=1".to_string(),
            ));
        }
    }
    let kind = widget_kind_from_wit(node.kind);
    if node.selected.is_some() && !kind_is_selectable(kind) {
        return Err(ui::types::UiError::Unsupported(format!(
            "widget kind {kind:?} cannot carry a selected index"
        )));
    }

    Ok(WidgetNode {
        id,
        parent,
        kind,
        label: node.label,
        role: node.role,
        style,
        checked: node.checked,
        value: node.value,
        selected: node.selected,
    })
}

fn modifiers_to_wit(modifiers: Modifiers) -> ui::types::Modifiers {
    ui::types::Modifiers {
        shift: modifiers.shift,
        control: modifiers.control,
        alt: modifiers.alt,
        meta: modifiers.meta,
    }
}

fn pointer_button_to_wit(button: PointerButton) -> ui::types::PointerButton {
    match button {
        PointerButton::Primary => ui::types::PointerButton::Primary,
        PointerButton::Secondary => ui::types::PointerButton::Secondary,
        PointerButton::Middle => ui::types::PointerButton::Middle,
        PointerButton::Other => ui::types::PointerButton::Other,
    }
}

fn theme_to_wit(theme: Theme) -> ui::types::Theme {
    match theme {
        Theme::Light => ui::types::Theme::Light,
        Theme::Dark => ui::types::Theme::Dark,
        Theme::Unknown => ui::types::Theme::Unknown,
    }
}

/// Map one shared adapter event into the portable WIT event shape.
///
/// Events without a WIT variant yet (window created/shown, widget bookkeeping)
/// are host-side bookkeeping and are not delivered to apps.
fn event_to_wit(event: UiEvent) -> Option<ui::types::Event> {
    match event {
        UiEvent::WindowCloseRequested(id) => Some(ui::types::Event::CloseRequested(id.get())),
        UiEvent::Resized { size, .. } => Some(ui::types::Event::Resized(ui::types::WindowSize {
            width: size.width,
            height: size.height,
        })),
        UiEvent::RedrawRequested(id) => Some(ui::types::Event::RedrawRequested(id.get())),
        UiEvent::Pointer(pointer) => Some(ui::types::Event::Pointer(ui::types::PointerEvent {
            window: pointer.window.get(),
            widget: pointer.widget.map(|widget| widget.get()),
            x: pointer.x,
            y: pointer.y,
            button: pointer.button.map(pointer_button_to_wit),
            pressed: pointer.pressed,
            modifiers: modifiers_to_wit(pointer.modifiers),
        })),
        UiEvent::Key(key) => Some(ui::types::Event::Key(ui::types::KeyEvent {
            window: key.window.get(),
            widget: key.widget.map(|widget| widget.get()),
            key: key.key,
            pressed: key.pressed,
            modifiers: modifiers_to_wit(key.modifiers),
        })),
        UiEvent::TextInput(text) => Some(ui::types::Event::TextInput(text.text)),
        UiEvent::TextChanged(changed) => {
            Some(ui::types::Event::TextChanged(ui::types::TextChangedEvent {
                window: changed.window.get(),
                widget: changed.widget.get(),
                text: changed.text,
            }))
        }
        UiEvent::FocusChanged { widget, .. } => {
            Some(ui::types::Event::FocusChanged(Some(widget.get())))
        }
        UiEvent::ThemeChanged { theme } => {
            Some(ui::types::Event::ThemeChanged(theme_to_wit(theme)))
        }
        _ => None,
    }
}

impl ui::types::Host for Phase3GuiHost {}

impl ui::window::Host for Phase3GuiHost {
    fn create(
        &mut self,
        title: String,
        size: ui::types::WindowSize,
    ) -> wasmtime::Result<Result<u64, ui::types::UiError>> {
        let size = match WindowSize::new(size.width, size.height) {
            Ok(size) => size,
            Err(err) => return Ok(Err(ui::types::UiError::Platform(err.to_string()))),
        };
        let options = match WindowOptions::new(title, size) {
            Ok(options) => options,
            Err(err) => return Ok(Err(ui::types::UiError::Platform(err.to_string()))),
        };

        match self.dispatcher().create_window(options) {
            Ok(id) => {
                self.windows.push(id);
                Ok(Ok(id.get()))
            }
            Err(err) => Ok(Err(dispatch_error_to_ui_error(err))),
        }
    }

    fn show(&mut self, window: u64) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        let id = match self.window_id(window) {
            Ok(id) => id,
            Err(err) => return Ok(Err(err)),
        };
        Ok(self
            .dispatcher()
            .show_window(id)
            .map_err(dispatch_error_to_ui_error))
    }

    fn close(&mut self, window: u64) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        let id = match self.window_id(window) {
            Ok(id) => id,
            Err(err) => return Ok(Err(err)),
        };
        let result = self
            .dispatcher()
            .close_window(id)
            .map_err(dispatch_error_to_ui_error);
        self.windows.retain(|tracked| *tracked != id);
        Ok(result)
    }

    fn set_title(
        &mut self,
        window: u64,
        title: String,
    ) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        let id = match self.window_id(window) {
            Ok(id) => id,
            Err(err) => return Ok(Err(err)),
        };
        Ok(self
            .dispatcher()
            .set_title(id, title)
            .map_err(dispatch_error_to_ui_error))
    }

    fn set_size(
        &mut self,
        window: u64,
        size: ui::types::WindowSize,
    ) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        let id = match self.window_id(window) {
            Ok(id) => id,
            Err(err) => return Ok(Err(err)),
        };
        let size = match WindowSize::new(size.width, size.height) {
            Ok(size) => size,
            Err(err) => return Ok(Err(ui::types::UiError::Platform(err.to_string()))),
        };
        Ok(self
            .dispatcher()
            .set_size(id, size)
            .map_err(dispatch_error_to_ui_error))
    }

    fn set_state(
        &mut self,
        _window: u64,
        _state: ui::types::WindowState,
    ) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        Ok(Err(ui::types::UiError::Unsupported(
            "window state changes are not implemented yet".to_string(),
        )))
    }

    fn request_redraw(&mut self, window: u64) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        let id = match self.window_id(window) {
            Ok(id) => id,
            Err(err) => return Ok(Err(err)),
        };
        Ok(self
            .dispatcher()
            .request_redraw(id)
            .map_err(dispatch_error_to_ui_error))
    }
}

impl ui::tree::Host for Phase3GuiHost {
    fn set_root(
        &mut self,
        window: u64,
        root: ui::types::WidgetNode,
    ) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        let id = match self.window_id(window) {
            Ok(id) => id,
            Err(err) => return Ok(Err(err)),
        };
        let root = match widget_node_from_wit(root) {
            Ok(root) => root,
            Err(err) => return Ok(Err(err)),
        };
        if let Err(err) = self.dispatcher().set_root(id, root) {
            return Ok(Err(dispatch_error_to_ui_error(err)));
        }
        Ok(self
            .sync_native_widgets(id)
            .map_err(dispatch_error_to_ui_error))
    }

    fn upsert_node(
        &mut self,
        window: u64,
        node: ui::types::WidgetNode,
    ) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        let id = match self.window_id(window) {
            Ok(id) => id,
            Err(err) => return Ok(Err(err)),
        };
        let node = match widget_node_from_wit(node) {
            Ok(node) => node,
            Err(err) => return Ok(Err(err)),
        };
        if let Err(err) = self.dispatcher().upsert_node(id, node) {
            return Ok(Err(dispatch_error_to_ui_error(err)));
        }
        Ok(self
            .sync_native_widgets(id)
            .map_err(dispatch_error_to_ui_error))
    }

    fn remove_node(
        &mut self,
        window: u64,
        widget: u64,
    ) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        let id = match self.window_id(window) {
            Ok(id) => id,
            Err(err) => return Ok(Err(err)),
        };
        let Ok(widget) = widget_id(widget) else {
            return Ok(Err(ui::types::UiError::InvalidWidget));
        };
        if let Err(err) = self.dispatcher().remove_node(id, widget) {
            return Ok(Err(dispatch_error_to_ui_error(err)));
        }
        Ok(self
            .sync_native_widgets(id)
            .map_err(dispatch_error_to_ui_error))
    }

    fn focus_node(
        &mut self,
        window: u64,
        widget: u64,
    ) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        let id = match self.window_id(window) {
            Ok(id) => id,
            Err(err) => return Ok(Err(err)),
        };
        let Ok(widget) = widget_id(widget) else {
            return Ok(Err(ui::types::UiError::InvalidWidget));
        };
        Ok(self
            .dispatcher()
            .focus_node(id, widget)
            .map_err(dispatch_error_to_ui_error))
    }

    fn set_enabled(
        &mut self,
        _window: u64,
        _widget: u64,
        _enabled: bool,
    ) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        Ok(Err(ui::types::UiError::Unsupported(
            "widget enable state is not implemented yet".to_string(),
        )))
    }
}

impl ui::events::Host for Phase3GuiHost {
    fn poll(&mut self) -> wasmtime::Result<Option<ui::types::Event>> {
        self.poll_one_event()
            .map_err(|err| wasmtime::Error::msg(err.to_string()))
    }

    fn wait(&mut self, timeout_millis: Option<u32>) -> wasmtime::Result<Option<ui::types::Event>> {
        let deadline = timeout_millis
            .map(|ms| std::time::Instant::now() + std::time::Duration::from_millis(u64::from(ms)));

        loop {
            let event = self
                .poll_one_event()
                .map_err(|err| wasmtime::Error::msg(err.to_string()))?;
            if event.is_some() {
                return Ok(event);
            }
            if let Some(deadline) = deadline {
                if std::time::Instant::now() >= deadline {
                    return Ok(None);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(WAIT_POLL_INTERVAL_MILLIS));
        }
    }
}

impl ui::dialog::Host for Phase3GuiHost {
    fn message(
        &mut self,
        _window: u64,
        _title: String,
        _body: String,
    ) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        Ok(Err(ui::types::UiError::Unsupported(
            "system dialogs are not implemented yet".to_string(),
        )))
    }

    fn confirm(
        &mut self,
        _window: u64,
        _title: String,
        _body: String,
    ) -> wasmtime::Result<Result<bool, ui::types::UiError>> {
        Ok(Err(ui::types::UiError::Unsupported(
            "system dialogs are not implemented yet".to_string(),
        )))
    }
}

impl ui::clipboard::Host for Phase3GuiHost {
    fn read_text(&mut self) -> wasmtime::Result<Result<String, ui::types::UiError>> {
        Ok(self
            .dispatcher()
            .read_clipboard_text()
            .map_err(dispatch_error_to_ui_error))
    }

    fn write_text(&mut self, text: String) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        Ok(self
            .dispatcher()
            .write_clipboard_text(&text)
            .map_err(dispatch_error_to_ui_error))
    }
}

impl ui::menu::Host for Phase3GuiHost {
    fn set_items(
        &mut self,
        _window: u64,
        _items: Vec<ui::types::MenuItem>,
    ) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        Ok(Err(ui::types::UiError::Unsupported(
            "menus are not implemented yet".to_string(),
        )))
    }
}

fn gfx_unsupported() -> gfx::types::GfxError {
    gfx::types::GfxError::Unsupported("graphics are not implemented yet".to_string())
}

impl gfx::types::Host for Phase3GuiHost {}

impl gfx::canvas2d::Host for Phase3GuiHost {
    fn bind(
        &mut self,
        _window: u64,
        _widget: u64,
    ) -> wasmtime::Result<Result<u64, gfx::types::GfxError>> {
        Ok(Err(gfx_unsupported()))
    }

    fn clear(
        &mut self,
        _canvas: u64,
        _color: gfx::types::Color,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        Ok(Err(gfx_unsupported()))
    }

    fn submit(
        &mut self,
        _canvas: u64,
        _commands: Vec<gfx::types::DrawCommand>,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        Ok(Err(gfx_unsupported()))
    }
}

impl gfx::gpu3d::Host for Phase3GuiHost {
    fn create_surface(
        &mut self,
        _window: u64,
        _widget: u64,
        _options: gfx::types::SurfaceOptions,
    ) -> wasmtime::Result<Result<u64, gfx::types::GfxError>> {
        Ok(Err(gfx_unsupported()))
    }

    fn present(&mut self, _surface: u64) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        Ok(Err(gfx_unsupported()))
    }
}

fn audio_unsupported() -> audio::types::AudioError {
    audio::types::AudioError::Unsupported("audio is not implemented yet".to_string())
}

impl audio::types::Host for Phase3GuiHost {}

impl audio::playback::Host for Phase3GuiHost {
    fn open(
        &mut self,
        _config: audio::types::StreamConfig,
    ) -> wasmtime::Result<Result<u64, audio::types::AudioError>> {
        Ok(Err(audio_unsupported()))
    }

    fn start(&mut self, _stream_id: u64) -> wasmtime::Result<Result<(), audio::types::AudioError>> {
        Ok(Err(audio_unsupported()))
    }

    fn stop(&mut self, _stream_id: u64) -> wasmtime::Result<Result<(), audio::types::AudioError>> {
        Ok(Err(audio_unsupported()))
    }

    fn write(
        &mut self,
        _stream_id: u64,
        _bytes: Vec<u8>,
    ) -> wasmtime::Result<Result<u32, audio::types::AudioError>> {
        Ok(Err(audio_unsupported()))
    }
}

impl audio::capture::Host for Phase3GuiHost {
    fn open(
        &mut self,
        _config: audio::types::StreamConfig,
    ) -> wasmtime::Result<Result<u64, audio::types::AudioError>> {
        Ok(Err(audio_unsupported()))
    }

    fn start(&mut self, _stream_id: u64) -> wasmtime::Result<Result<(), audio::types::AudioError>> {
        Ok(Err(audio_unsupported()))
    }

    fn stop(&mut self, _stream_id: u64) -> wasmtime::Result<Result<(), audio::types::AudioError>> {
        Ok(Err(audio_unsupported()))
    }

    fn read(
        &mut self,
        _stream_id: u64,
        _max_bytes: u32,
    ) -> wasmtime::Result<Result<Vec<u8>, audio::types::AudioError>> {
        Ok(Err(audio_unsupported()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_offset_clamps_to_content_extent() {
        // Content 192 tall in a 120 viewport: max offset 72.
        assert_eq!(clamped_scroll_offset(0.0, 30.0, 192.0, 120.0), 30.0);
        assert_eq!(clamped_scroll_offset(60.0, 30.0, 192.0, 120.0), 72.0);
        assert_eq!(clamped_scroll_offset(10.0, -30.0, 192.0, 120.0), 0.0);
        // Content shorter than the viewport never scrolls.
        assert_eq!(clamped_scroll_offset(0.0, 30.0, 80.0, 120.0), 0.0);
    }

    #[test]
    fn presses_focus_text_entry_widgets_only() {
        assert!(press_focuses(WidgetKind::TextField));
        assert!(press_focuses(WidgetKind::TextArea));
        assert!(!press_focuses(WidgetKind::Button));
        assert!(!press_focuses(WidgetKind::Text));
        assert!(!press_focuses(WidgetKind::Stack));
    }
}
