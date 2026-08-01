//! Host implementation for the Phase 3 `gui` world's new imports.
//!
//! `Phase3GuiHost` backs the `krate:ui` interfaces with the UCap-gated
//! Phase 3 UI dispatcher. Window, widget-tree, and event calls are real;
//! after every tree change the host recomputes layout and re-lowers the
//! supported widgets to native controls when the selected adapter can (the
//! opt-in macOS AppKit prototype today — headless adapters lower nothing and
//! that is a valid state). Audio capture and playback drive real CPAL streams.
//! The `gfx` and `menu` surfaces return honest `unsupported` errors until
//! their runtimes exist.

use krate_adapter_common::painter::drawn_kind;
use krate_adapter_common::ui::{
    kind_is_selectable, ImagePixels, Modifiers, PointerButton, Theme, UiAdapterError, UiEvent,
    WidgetId, WidgetKind, WidgetNode, WidgetPlacement, WidgetStyle, WindowId, WindowOptions,
    WindowSize,
};
use krate_layout::{absolute_rect, LayoutViewport};
use std::sync::Arc;

use crate::{
    audio_capture::{AudioCaptureRuntime, CaptureConfig, CaptureError, CaptureSampleFormat},
    audio_playback::{AudioPlaybackRuntime, PlaybackConfig, PlaybackError, PlaybackSampleFormat},
    canvas_raster::{pack_color, CanvasSurface},
    phase3_gui_bindings::krate::{audio, gfx, speech, ui},
    phase3_ui::{Phase3HostUiMode, Phase3UiDispatcher, Phase3UiRuntime, UiDispatchError},
    speech_transcription::{LocalSpeechRuntime, SpeechError},
    uapi::{AudioCall, UapiCall, UapiGuard, UiCall},
};

/// How long `events.wait` sleeps between polls.
const WAIT_POLL_INTERVAL_MILLIS: u64 = 10;

/// How many idle `events.wait` calls a headless run tolerates before the host
/// reports the window closed. Small enough that `krate run` on a GUI app
/// finishes in about a second instead of spinning out the app's wait budget,
/// large enough that an app doing a few no-event waits during start-up still
/// reaches its event loop. Only ever consulted on the headless path.
const HEADLESS_IDLE_WAIT_LIMIT: u32 = 8;

/// Host state for the Phase 3 `gui` world imports.
pub struct Phase3GuiHost {
    runtime: Phase3UiRuntime,
    windows: Vec<WindowId>,
    /// Files the person chose in a dialog this run.
    ///
    /// The picker writes here and `fs.open-chosen` reads, so the two halves of
    /// one grant share a store that lives and dies with the run. An app cannot
    /// carry a token across runs because this is gone when the run ends.
    chosen_files: std::rc::Rc<std::cell::RefCell<crate::chosen_files::ChosenFiles>>,
    /// Host-side vertical scroll offsets per (window, Scroll widget).
    /// Scrolling never involves the guest: wheel input adjusts these and
    /// re-lowers placements, matching native platform feel.
    scroll_offsets: std::cell::RefCell<std::collections::BTreeMap<(WindowId, WidgetId), f32>>,
    /// Last text the host observed in each natively lowered editable control.
    /// AppKit keeps typed characters inside the control, so the guest only
    /// learns about them by the host reading the control back and comparing.
    native_text: std::cell::RefCell<std::collections::BTreeMap<(WindowId, WidgetId), String>>,
    /// True when no window on this host can ever receive human input, i.e. the
    /// headless draft path. A GUI app's normal shape is "loop until the person
    /// closes the window", so with no such window the loop has nothing to end
    /// it and the app spins out its whole wait budget. See [`idle_waits`].
    headless: bool,
    /// Consecutive `wait` calls that timed out with no event, used only when
    /// [`headless`] is set. Once this passes [`HEADLESS_IDLE_WAIT_LIMIT`] the
    /// host synthesises a close request so the guest exits the way it would if
    /// a person had closed the window.
    idle_waits: std::cell::Cell<u32>,
    /// Pictures for image widgets, keyed by the widget they belong to.
    ///
    /// Held here rather than on the widget node because a picture arrives
    /// through its own interface: adding a field to `widget-node` would change
    /// that record's type and stop every GUI app already built from
    /// instantiating at all.
    images: std::cell::RefCell<std::collections::BTreeMap<(WindowId, WidgetId), Arc<ImagePixels>>>,
    /// Bound 2D canvases, keyed by the id handed to the guest. Each remembers
    /// which widget it publishes to; the pixels land in [`Self::images`] and
    /// travel the image widget's proven path to all three systems.
    canvases:
        std::cell::RefCell<std::collections::BTreeMap<u64, (WindowId, WidgetId, CanvasSurface)>>,
    /// The next canvas id to hand out; ids are never reused within a run.
    next_canvas_id: std::cell::Cell<u64>,
    /// Native microphone streams owned by this one sandboxed app session.
    audio_capture: AudioCaptureRuntime,
    /// Native speaker streams owned by this one sandboxed app session.
    audio_playback: AudioPlaybackRuntime,
    /// Local speech model contexts, scoped to this one sandboxed app session.
    speech: LocalSpeechRuntime,
}

impl Phase3GuiHost {
    /// Create the GUI host with the requested host UI mode.
    pub fn new(guard: UapiGuard, mode: Phase3HostUiMode) -> Result<Self, UiDispatchError> {
        let runtime = Phase3UiRuntime::try_with_host_adapter_mode(guard, mode)?;
        Ok(Self {
            runtime,
            chosen_files: Default::default(),
            windows: Vec::new(),
            scroll_offsets: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            native_text: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            headless: matches!(mode, Phase3HostUiMode::HeadlessDraft),
            idle_waits: std::cell::Cell::new(0),
            images: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            canvases: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            next_canvas_id: std::cell::Cell::new(1),
            audio_capture: AudioCaptureRuntime::default(),
            audio_playback: AudioPlaybackRuntime::default(),
            speech: LocalSpeechRuntime::default(),
        })
    }

    pub fn with_asset_root(mut self, root: Option<std::path::PathBuf>) -> Self {
        self.speech = LocalSpeechRuntime::default().with_asset_root(root);
        self
    }

    fn dispatcher(&self) -> Phase3UiDispatcher<'_> {
        self.runtime.dispatcher()
    }

    /// On a headless host, report a window close once the guest has waited
    /// [`HEADLESS_IDLE_WAIT_LIMIT`] times with nothing to show for it.
    ///
    /// A GUI app is written as "keep going until the window closes", which on a
    /// real desktop ends when the person closes it. Headless has no window and
    /// no person, so an app that waits with no timeout can never be released,
    /// and the run hangs with nothing on screen to explain why.
    ///
    /// This only ever fires for an unbounded wait, where the alternative is
    /// waiting forever. A wait that carries a timeout is left alone to time out
    /// on its own: the guest asked to be given back control after a set time
    /// and it will be, so there is no need to tell it anything happened. That
    /// distinction matters because "the window closed" means a person closed
    /// it, and apps report it as such — `krate-hello-gui` exits 2 for "user
    /// closed the window" versus 1 for "finished without one". Synthesising the
    /// close on every idle timeout made those runs claim a person acted when
    /// nobody had.
    fn headless_close_request(&self) -> Option<ui::types::Event> {
        if !self.headless {
            return None;
        }
        let waits = self.idle_waits.get().saturating_add(1);
        self.idle_waits.set(waits);
        if waits < HEADLESS_IDLE_WAIT_LIMIT {
            return None;
        }
        let window = self.windows.first().map(|id| id.get()).unwrap_or(0);
        Some(ui::types::Event::CloseRequested(window))
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
                let selected_child = *tree.children(parent_id).get(index as usize)?;
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
                // Shared, not copied: this runs once per widget per frame, and
                // a photograph is a quarter-gigabyte of pixels.
                pixels: self.images.borrow().get(&(window, *id)).cloned(),
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
    /// The laid-out size of a canvas widget, in logical pixels.
    fn canvas_widget_rect(
        &self,
        window: WindowId,
        widget: WidgetId,
    ) -> Result<(f32, f32), gfx::types::GfxError> {
        let dispatcher = self.dispatcher();
        let tree = match dispatcher.widget_tree(window) {
            Ok(Some(tree)) => tree,
            _ => return Err(gfx::types::GfxError::InvalidTarget),
        };
        match tree.nodes().iter().find(|(id, _)| **id == widget) {
            Some((_, node)) if node.kind == WidgetKind::Canvas => {}
            Some(_) => {
                return Err(gfx::types::GfxError::Unsupported(
                    "canvas2d binds to a widget of kind canvas".to_string(),
                ))
            }
            None => return Err(gfx::types::GfxError::InvalidTarget),
        }
        let record = match dispatcher.window(window) {
            Ok(Some(record)) => record,
            _ => return Err(gfx::types::GfxError::InvalidTarget),
        };
        let viewport =
            match LayoutViewport::new(record.size.width as f32, record.size.height as f32) {
                Ok(viewport) => viewport,
                Err(error) => return Err(gfx::types::GfxError::Platform(error.to_string())),
            };
        let layout = match dispatcher.compute_layout(window, viewport) {
            Ok(layout) => layout,
            Err(error) => return Err(gfx::types::GfxError::Platform(error.to_string())),
        };
        match absolute_rect(&tree, &layout, widget) {
            Some(rect) => Ok((rect.width, rect.height)),
            None => Err(gfx::types::GfxError::InvalidTarget),
        }
    }

    /// Push a canvas's pixels through the image path and re-lower.
    fn publish_canvas(&self, canvas: u64) -> Result<(), gfx::types::GfxError> {
        let (window, widget, image) = {
            let canvases = self.canvases.borrow();
            let Some((window, widget, surface)) = canvases.get(&canvas) else {
                return Err(gfx::types::GfxError::InvalidTarget);
            };
            let image = surface
                .to_image()
                .map_err(|error| gfx::types::GfxError::Platform(error.to_string()))?;
            (*window, *widget, image)
        };
        self.images
            .borrow_mut()
            .insert((window, widget), Arc::new(image));
        self.sync_native_widgets(window)
            .map_err(|error| gfx::types::GfxError::Platform(error.to_string()))
    }

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
    // A caret only means something on an editable text widget. Rejecting it
    // elsewhere keeps a stray value from silently riding on, say, a button.
    if node.text_cursor.is_some() && !matches!(kind, WidgetKind::TextArea | WidgetKind::TextField) {
        return Err(ui::types::UiError::Unsupported(format!(
            "widget kind {kind:?} cannot carry a text caret"
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
        text_cursor: node.text_cursor.map(|tc| (tc.cursor, tc.anchor)),
        // A picture arrives through `krate:ui/image`, keyed by widget id, not
        // as a field here. The node the app sends must stay the exact record
        // it was compiled against.
        pixels: None,
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
                self.idle_waits.set(0);
                return Ok(event);
            }
            if let Some(deadline) = deadline {
                if std::time::Instant::now() >= deadline {
                    // The guest asked for a timeout and it has arrived. Hand
                    // control back exactly as asked; it is the guest's own loop
                    // bound that ends the run, and it stays free to treat the
                    // quiet round however it likes.
                    return Ok(None);
                }
            } else if let Some(close) = self.headless_close_request() {
                // An unbounded wait on a headless host can never return on its
                // own: nothing exists that could deliver an event. Reporting
                // the close is the only honest way out, and every GUI app
                // already handles it, so the app shuts down through its normal
                // path and still saves on the way out.
                return Ok(Some(close));
            }
            std::thread::sleep(std::time::Duration::from_millis(WAIT_POLL_INTERVAL_MILLIS));
        }
    }
}

impl ui::launcher::Host for Phase3GuiHost {
    fn open_url(&mut self, url: String) -> wasmtime::Result<Result<(), ui::launcher::LaunchError>> {
        // Checked before the URL is even looked at, so a denied app cannot use
        // the difference between error messages to probe what would be allowed.
        let granted = self
            .runtime
            .guard()
            .check(&UapiCall::Ui(UiCall::OpenUrl))
            .is_ok();
        Ok(
            crate::desktop_host::open_url(&url, granted).map_err(|err| match err {
                crate::desktop_host::LaunchError::Denied => ui::launcher::LaunchError::Denied,
                crate::desktop_host::LaunchError::InvalidUrl(m) => {
                    ui::launcher::LaunchError::InvalidUrl(m)
                }
                crate::desktop_host::LaunchError::Unavailable(m) => {
                    ui::launcher::LaunchError::Unavailable(m)
                }
            }),
        )
    }
}

impl ui::notify::Host for Phase3GuiHost {
    fn show(
        &mut self,
        title: String,
        body: String,
    ) -> wasmtime::Result<Result<(), ui::notify::NotifyError>> {
        let granted = self
            .runtime
            .guard()
            .check(&UapiCall::Ui(UiCall::Notify))
            .is_ok();
        // The app's own title is used for attribution, so a notification cannot
        // be made to look like it came from somewhere else.
        Ok(
            crate::desktop_host::notify(&title, &body, &title, granted).map_err(|err| match err {
                crate::desktop_host::NotifyError::Denied => ui::notify::NotifyError::Denied,
                crate::desktop_host::NotifyError::InvalidContent(m) => {
                    ui::notify::NotifyError::InvalidContent(m)
                }
                crate::desktop_host::NotifyError::Unavailable(m) => {
                    ui::notify::NotifyError::Unavailable(m)
                }
            }),
        )
    }
}

impl ui::dialog::Host for Phase3GuiHost {
    /// Show the system's open-file dialog and remember what was chosen.
    ///
    /// The app gets a name and a token, never a path. That is what makes the
    /// click a grant rather than a hole: it can open the one file the person
    /// picked, and cannot read its siblings, walk to its folder, or store the
    /// location for a later run.
    fn open_file(
        &mut self,
        _window: u64,
        title: String,
        filter: String,
    ) -> wasmtime::Result<Result<Option<ui::dialog::ChosenFile>, ui::types::UiError>> {
        let chosen = match choose_file_on_host(&title, &filter) {
            Ok(Some(path)) => path,
            // Cancelling is a normal answer, not a failure.
            Ok(None) => return Ok(Ok(None)),
            Err(err) => return Ok(Err(ui::types::UiError::Unsupported(err))),
        };

        let name = crate::chosen_files::ChosenFiles::display_name(&chosen);
        let Some(token) = self.chosen_files.borrow_mut().remember(chosen) else {
            return Ok(Err(ui::types::UiError::Unsupported(
                "too many files chosen in one run".to_string(),
            )));
        };
        Ok(Ok(Some(ui::dialog::ChosenFile { name, token })))
    }

    /// Show a message and wait for the person to dismiss it.
    fn message(
        &mut self,
        _window: u64,
        title: String,
        body: String,
    ) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        rfd::MessageDialog::new()
            .set_title(&title)
            .set_description(&body)
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
        Ok(Ok(()))
    }

    /// Ask a yes/no question and return what the person chose.
    ///
    /// A dismissed dialog counts as "no": an app that treats silence as consent
    /// is doing something the person did not agree to.
    fn confirm(
        &mut self,
        _window: u64,
        title: String,
        body: String,
    ) -> wasmtime::Result<Result<bool, ui::types::UiError>> {
        let answer = rfd::MessageDialog::new()
            .set_title(&title)
            .set_description(&body)
            .set_buttons(rfd::MessageButtons::YesNo)
            .show();
        Ok(Ok(answer == rfd::MessageDialogResult::Yes))
    }
}

impl ui::image::Host for Phase3GuiHost {
    fn set_pixels(
        &mut self,
        window: u64,
        widget: u64,
        pixels: ui::image::ImagePixels,
    ) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        let window_id = match self.window_id(window) {
            Ok(id) => id,
            Err(err) => return Ok(Err(err)),
        };
        let widget_id = match WidgetId::new(widget) {
            Ok(id) => id,
            Err(err) => return Ok(Err(ui::types::UiError::Unsupported(err.to_string()))),
        };

        // A picture only means something on an image widget. Accepting one for
        // a button would store a buffer nothing ever draws and leave the app
        // believing it had shown something.
        match self.dispatcher().widget_tree(window_id) {
            Ok(Some(tree)) => match tree.nodes().iter().find(|(id, _)| **id == widget_id) {
                Some((_, node)) if node.kind == WidgetKind::Image => {}
                Some((_, node)) => {
                    return Ok(Err(ui::types::UiError::Unsupported(format!(
                        "widget kind {:?} cannot show a picture",
                        node.kind
                    ))))
                }
                None => {
                    return Ok(Err(ui::types::UiError::Unsupported(format!(
                        "window {window} has no widget {widget}"
                    ))))
                }
            },
            Ok(None) => {
                return Ok(Err(ui::types::UiError::Unsupported(format!(
                    "window {window} has no widgets yet"
                ))))
            }
            Err(err) => return Ok(Err(dispatch_error_to_ui_error(err))),
        }

        let image = match ImagePixels::new(pixels.width, pixels.height, pixels.rgba) {
            Ok(image) => image,
            Err(err) => return Ok(Err(ui::types::UiError::Unsupported(err.to_string()))),
        };
        self.images
            .borrow_mut()
            .insert((window_id, widget_id), Arc::new(image));
        Ok(self
            .sync_native_widgets(window_id)
            .map_err(dispatch_error_to_ui_error))
    }

    fn clear(
        &mut self,
        window: u64,
        widget: u64,
    ) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        let window_id = match self.window_id(window) {
            Ok(id) => id,
            Err(err) => return Ok(Err(err)),
        };
        let widget_id = match WidgetId::new(widget) {
            Ok(id) => id,
            Err(err) => return Ok(Err(ui::types::UiError::Unsupported(err.to_string()))),
        };
        // Clearing a widget that has no picture is not an error: an app
        // resetting its view should not have to remember whether it ever set
        // one.
        self.images.borrow_mut().remove(&(window_id, widget_id));
        Ok(self
            .sync_native_widgets(window_id)
            .map_err(dispatch_error_to_ui_error))
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
        window: u64,
        widget: u64,
    ) -> wasmtime::Result<Result<u64, gfx::types::GfxError>> {
        let window_id = match self.window_id(window) {
            Ok(id) => id,
            Err(_) => return Ok(Err(gfx::types::GfxError::InvalidTarget)),
        };
        let Ok(widget_id) = WidgetId::new(widget) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };

        // The canvas takes the widget's laid-out size, so what the app draws
        // is what the layout gave it -- the same rect every host will show.
        let rect = match self.canvas_widget_rect(window_id, widget_id) {
            Ok(rect) => rect,
            Err(error) => return Ok(Err(error)),
        };
        let surface = match CanvasSurface::new(rect.0.max(1.0) as u32, rect.1.max(1.0) as u32) {
            Ok(surface) => surface,
            Err(error) => return Ok(Err(gfx::types::GfxError::Unsupported(error.to_string()))),
        };

        let canvas_id = self.next_canvas_id.get();
        self.next_canvas_id.set(canvas_id.saturating_add(1));
        self.canvases
            .borrow_mut()
            .insert(canvas_id, (window_id, widget_id, surface));
        Ok(Ok(canvas_id))
    }

    fn clear(
        &mut self,
        canvas: u64,
        fill: gfx::types::Color,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        let mut canvases = self.canvases.borrow_mut();
        let Some((_, _, surface)) = canvases.get_mut(&canvas) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.clear(pack_color(fill.r, fill.g, fill.b, fill.a));
        Ok(Ok(()))
    }

    fn fill_rect(
        &mut self,
        canvas: u64,
        area: gfx::types::Rect,
        fill: gfx::types::Color,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        let mut canvases = self.canvases.borrow_mut();
        let Some((_, _, surface)) = canvases.get_mut(&canvas) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.fill_rect(
            area.x,
            area.y,
            area.width,
            area.height,
            pack_color(fill.r, fill.g, fill.b, fill.a),
        );
        Ok(Ok(()))
    }

    fn stroke_rect(
        &mut self,
        canvas: u64,
        area: gfx::types::Rect,
        stroke: gfx::types::Color,
        width: f32,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        let mut canvases = self.canvases.borrow_mut();
        let Some((_, _, surface)) = canvases.get_mut(&canvas) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.stroke_rect(
            area.x,
            area.y,
            area.width,
            area.height,
            width,
            pack_color(stroke.r, stroke.g, stroke.b, stroke.a),
        );
        Ok(Ok(()))
    }

    fn draw_text(
        &mut self,
        canvas: u64,
        text: String,
        origin: gfx::types::Point,
        font_size: f32,
        ink: gfx::types::Color,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        let mut canvases = self.canvases.borrow_mut();
        let Some((_, _, surface)) = canvases.get_mut(&canvas) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.text(
            &text,
            origin.x,
            origin.y,
            font_size,
            pack_color(ink.r, ink.g, ink.b, ink.a),
        );
        Ok(Ok(()))
    }

    fn draw_pixels(
        &mut self,
        canvas: u64,
        area: gfx::types::Rect,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        // The guest is the untrusted side: a buffer shorter than its stated
        // size would read past the end on the last row. ImagePixels checks
        // that once, here, rather than in the sampling loop.
        let image = match ImagePixels::new(width, height, rgba) {
            Ok(image) => image,
            Err(error) => return Ok(Err(gfx::types::GfxError::Unsupported(error.to_string()))),
        };
        let mut canvases = self.canvases.borrow_mut();
        let Some((_, _, surface)) = canvases.get_mut(&canvas) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.draw_pixels(area.x, area.y, area.width, area.height, &image);
        Ok(Ok(()))
    }

    fn present(&mut self, canvas: u64) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        // The one call that reaches the widget. Draw calls mutate the raster;
        // this publishes it, so a hundred fills cost one render.
        Ok(self.publish_canvas(canvas))
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

fn audio_permission_denied() -> audio::types::AudioError {
    audio::types::AudioError::PermissionDenied
}

fn capture_config(config: audio::types::StreamConfig) -> CaptureConfig {
    CaptureConfig {
        sample_rate: config.sample_rate,
        channels: config.channels,
        format: match config.format {
            audio::types::SampleFormat::PcmS16 => CaptureSampleFormat::PcmS16,
            audio::types::SampleFormat::Float32 => CaptureSampleFormat::Float32,
        },
        buffer_frames: config.buffer_frames,
    }
}

fn capture_error(error: CaptureError) -> audio::types::AudioError {
    match error {
        CaptureError::InvalidStream => audio::types::AudioError::InvalidStream,
        CaptureError::DeviceUnavailable => audio::types::AudioError::DeviceUnavailable,
        CaptureError::InvalidConfig(message) => audio::types::AudioError::Unsupported(message),
        CaptureError::Platform(message) => audio::types::AudioError::Platform(message),
    }
}

fn playback_config(config: audio::types::StreamConfig) -> PlaybackConfig {
    PlaybackConfig {
        sample_rate: config.sample_rate,
        channels: config.channels,
        format: match config.format {
            audio::types::SampleFormat::PcmS16 => PlaybackSampleFormat::PcmS16,
            audio::types::SampleFormat::Float32 => PlaybackSampleFormat::Float32,
        },
        buffer_frames: config.buffer_frames,
    }
}

fn playback_error(error: PlaybackError) -> audio::types::AudioError {
    match error {
        PlaybackError::InvalidStream => audio::types::AudioError::InvalidStream,
        PlaybackError::DeviceUnavailable => audio::types::AudioError::DeviceUnavailable,
        PlaybackError::InvalidConfig(message) => audio::types::AudioError::Unsupported(message),
        PlaybackError::Platform(message) => audio::types::AudioError::Platform(message),
    }
}

impl audio::types::Host for Phase3GuiHost {}

impl audio::playback::Host for Phase3GuiHost {
    fn open(
        &mut self,
        config: audio::types::StreamConfig,
    ) -> wasmtime::Result<Result<u64, audio::types::AudioError>> {
        if self
            .runtime
            .guard()
            .check(&UapiCall::Audio(AudioCall::Playback))
            .is_err()
        {
            return Ok(Err(audio_permission_denied()));
        }
        Ok(self
            .audio_playback
            .open(playback_config(config))
            .map_err(playback_error))
    }

    fn start(&mut self, stream_id: u64) -> wasmtime::Result<Result<(), audio::types::AudioError>> {
        if self
            .runtime
            .guard()
            .check(&UapiCall::Audio(AudioCall::Playback))
            .is_err()
        {
            return Ok(Err(audio_permission_denied()));
        }
        Ok(self.audio_playback.start(stream_id).map_err(playback_error))
    }

    fn stop(&mut self, stream_id: u64) -> wasmtime::Result<Result<(), audio::types::AudioError>> {
        if self
            .runtime
            .guard()
            .check(&UapiCall::Audio(AudioCall::Playback))
            .is_err()
        {
            return Ok(Err(audio_permission_denied()));
        }
        Ok(self.audio_playback.stop(stream_id).map_err(playback_error))
    }

    fn write(
        &mut self,
        stream_id: u64,
        bytes: Vec<u8>,
    ) -> wasmtime::Result<Result<u32, audio::types::AudioError>> {
        if self
            .runtime
            .guard()
            .check(&UapiCall::Audio(AudioCall::Playback))
            .is_err()
        {
            return Ok(Err(audio_permission_denied()));
        }
        Ok(self
            .audio_playback
            .write(stream_id, &bytes)
            .map_err(playback_error))
    }
}

impl audio::capture::Host for Phase3GuiHost {
    fn open(
        &mut self,
        config: audio::types::StreamConfig,
    ) -> wasmtime::Result<Result<u64, audio::types::AudioError>> {
        if self
            .runtime
            .guard()
            .check(&UapiCall::Audio(AudioCall::Capture))
            .is_err()
        {
            return Ok(Err(audio_permission_denied()));
        }
        Ok(self
            .audio_capture
            .open(capture_config(config))
            .map_err(capture_error))
    }

    fn start(&mut self, stream_id: u64) -> wasmtime::Result<Result<(), audio::types::AudioError>> {
        if self
            .runtime
            .guard()
            .check(&UapiCall::Audio(AudioCall::Capture))
            .is_err()
        {
            return Ok(Err(audio_permission_denied()));
        }
        Ok(self.audio_capture.start(stream_id).map_err(capture_error))
    }

    fn stop(&mut self, stream_id: u64) -> wasmtime::Result<Result<(), audio::types::AudioError>> {
        if self
            .runtime
            .guard()
            .check(&UapiCall::Audio(AudioCall::Capture))
            .is_err()
        {
            return Ok(Err(audio_permission_denied()));
        }
        Ok(self.audio_capture.stop(stream_id).map_err(capture_error))
    }

    fn read(
        &mut self,
        stream_id: u64,
        max_bytes: u32,
    ) -> wasmtime::Result<Result<Vec<u8>, audio::types::AudioError>> {
        if self
            .runtime
            .guard()
            .check(&UapiCall::Audio(AudioCall::Capture))
            .is_err()
        {
            return Ok(Err(audio_permission_denied()));
        }
        Ok(self
            .audio_capture
            .read(stream_id, max_bytes)
            .map_err(capture_error))
    }
}

impl speech::transcription::Host for Phase3GuiHost {
    fn transcribe(
        &mut self,
        model_asset: String,
        pcm_s16_le: Vec<u8>,
        sample_rate: u32,
        language: Option<String>,
    ) -> wasmtime::Result<
        Result<speech::transcription::Transcript, speech::transcription::SpeechError>,
    > {
        Ok(self
            .speech
            .transcribe(&model_asset, &pcm_s16_le, sample_rate, language.as_deref())
            .map(|text| speech::transcription::Transcript { text })
            .map_err(speech_error))
    }

    fn match_line(
        &mut self,
        model_asset: String,
        pcm_s16_le: Vec<u8>,
        sample_rate: u32,
        language: Option<String>,
        expected: String,
    ) -> wasmtime::Result<Result<u8, speech::transcription::MatchError>> {
        Ok(self
            .speech
            .match_line(
                &model_asset,
                &pcm_s16_le,
                sample_rate,
                language.as_deref(),
                &expected,
            )
            .map_err(match_error))
    }

    fn match_line_stream(
        &mut self,
        model_asset: String,
        pcm_s16_le: Vec<u8>,
        sample_rate: u32,
        language: Option<String>,
        expected: String,
        finish: bool,
    ) -> wasmtime::Result<Result<Option<u8>, speech::transcription::MatchError>> {
        Ok(self
            .speech
            .match_line_stream(
                &model_asset,
                &pcm_s16_le,
                sample_rate,
                language.as_deref(),
                &expected,
                finish,
            )
            .map_err(match_error))
    }
}

fn speech_error(error: SpeechError) -> speech::transcription::SpeechError {
    match error {
        SpeechError::InvalidRequest(message) => {
            speech::transcription::SpeechError::InvalidRequest(message)
        }
        SpeechError::ModelNotFound => speech::transcription::SpeechError::ModelNotFound,
        SpeechError::ModelInvalid(message) => {
            speech::transcription::SpeechError::ModelInvalid(message)
        }
        SpeechError::Unsupported(message) => {
            speech::transcription::SpeechError::Unsupported(message)
        }
        SpeechError::Inference(message) => speech::transcription::SpeechError::Inference(message),
    }
}

fn match_error(error: SpeechError) -> speech::transcription::MatchError {
    match error {
        SpeechError::InvalidRequest(_) => speech::transcription::MatchError::InvalidRequest,
        SpeechError::ModelNotFound => speech::transcription::MatchError::ModelNotFound,
        SpeechError::ModelInvalid(_) => speech::transcription::MatchError::ModelInvalid,
        SpeechError::Unsupported(_) => speech::transcription::MatchError::Unsupported,
        SpeechError::Inference(_) => speech::transcription::MatchError::Inference,
    }
}

/// Ask the operating system to show its open-file dialog.
///
/// One implementation for all three systems, through `rfd`, which uses the
/// native dialog on each: NSOpenPanel on macOS, the common item dialog on
/// Windows, and the XDG desktop portal on Linux. A picker that worked on the
/// machine an app was built on and failed when it was shared would be the exact
/// failure Krate exists to remove, so there is deliberately no per-platform
/// branch here to drift.
///
/// `filter` is a comma-separated extension list. It narrows what the dialog
/// offers and is not a rule the runtime enforces -- whatever the person picks
/// is what the app gets, because the click is the grant.
fn choose_file_on_host(title: &str, filter: &str) -> Result<Option<std::path::PathBuf>, String> {
    let mut dialog = rfd::FileDialog::new();
    if !title.is_empty() {
        dialog = dialog.set_title(title);
    }

    let extensions: Vec<&str> = filter
        .split(',')
        .map(str::trim)
        .filter(|ext| !ext.is_empty())
        .collect();
    if !extensions.is_empty() {
        dialog = dialog.add_filter("Supported files", &extensions);
    }

    // `None` is a cancelled dialog, which is a normal answer rather than a
    // failure, and the caller reports it as such.
    Ok(dialog.pick_file())
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

    fn headless_host() -> Phase3GuiHost {
        Phase3GuiHost::new(
            UapiGuard::new(Default::default()),
            Phase3HostUiMode::HeadlessDraft,
        )
        .expect("headless host")
    }

    fn capture_config() -> audio::types::StreamConfig {
        audio::types::StreamConfig {
            sample_rate: 16_000,
            channels: 1,
            format: audio::types::SampleFormat::PcmS16,
            buffer_frames: 1_600,
        }
    }

    #[test]
    fn microphone_open_denies_before_reaching_the_audio_adapter() {
        let mut host = headless_host();
        let result = <Phase3GuiHost as audio::capture::Host>::open(&mut host, capture_config())
            .expect("host call");

        assert!(matches!(
            result,
            Err(audio::types::AudioError::PermissionDenied)
        ));
    }

    #[test]
    fn granted_microphone_open_reaches_capture_validation() {
        let policy = krate_policy::SessionPolicy::from_cli_grants(&["audio.capture".to_string()])
            .expect("capture policy");
        let mut host = Phase3GuiHost::new(UapiGuard::new(policy), Phase3HostUiMode::HeadlessDraft)
            .expect("headless host");
        let result = <Phase3GuiHost as audio::capture::Host>::open(
            &mut host,
            audio::types::StreamConfig {
                sample_rate: 0,
                ..capture_config()
            },
        )
        .expect("host call");

        assert!(matches!(
            result,
            Err(audio::types::AudioError::Unsupported(_))
        ));
    }

    #[test]
    fn headless_waits_report_a_close_once_idle() {
        let host = headless_host();
        // The app gets a grace period to reach its event loop...
        for _ in 1..HEADLESS_IDLE_WAIT_LIMIT {
            assert!(host.headless_close_request().is_none());
        }
        // ...and then is told the window closed, so its loop can end instead of
        // spinning out a wait budget that nothing will ever interrupt.
        assert!(matches!(
            host.headless_close_request(),
            Some(ui::types::Event::CloseRequested(_))
        ));
    }

    #[test]
    fn a_real_event_resets_the_headless_idle_count() {
        let host = headless_host();
        for _ in 0..(HEADLESS_IDLE_WAIT_LIMIT - 1) {
            assert!(host.headless_close_request().is_none());
        }
        // An app that is doing work keeps its window: the counter only counts
        // *consecutive* empty waits, and `wait` clears it on every real event.
        host.idle_waits.set(0);
        assert!(host.headless_close_request().is_none());
    }

    #[test]
    fn a_wait_with_a_timeout_is_left_to_time_out_on_its_own() {
        // Only an unbounded wait gets the synthetic close. A guest that asked
        // for a timeout is handed control back with no event, because "the
        // window closed" states that a person closed it -- krate-hello-gui
        // reports exactly that as exit 2 -- and on a quiet round nobody did.
        let mut host = headless_host();
        for _ in 0..(HEADLESS_IDLE_WAIT_LIMIT * 3) {
            let event = ui::events::Host::wait(&mut host, Some(0)).expect("bounded wait");
            assert!(
                event.is_none(),
                "a bounded wait must report no event, never a close nobody asked for"
            );
        }
    }

    #[test]
    fn a_windowed_host_is_never_closed_by_the_idle_rule() {
        let mut host = headless_host();
        host.headless = false;
        for _ in 0..(HEADLESS_IDLE_WAIT_LIMIT * 4) {
            assert!(
                host.headless_close_request().is_none(),
                "a host with a real window must stay open until the person closes it"
            );
        }
    }

    #[test]
    fn presses_focus_text_entry_widgets_only() {
        assert!(press_focuses(WidgetKind::TextField));
        assert!(press_focuses(WidgetKind::TextArea));
        assert!(!press_focuses(WidgetKind::Button));
        assert!(!press_focuses(WidgetKind::Text));
        assert!(!press_focuses(WidgetKind::Stack));
    }

    fn wit_node(kind: ui::types::WidgetKind, cursor: Option<(u32, u32)>) -> ui::types::WidgetNode {
        ui::types::WidgetNode {
            id: 1,
            parent: None,
            kind,
            label: Some("hello".to_string()),
            role: None,
            style: ui::types::Style {
                width: Some(100.0),
                height: Some(30.0),
                grow: 0.0,
                padding: 0.0,
            },
            checked: None,
            value: None,
            selected: None,
            text_cursor: cursor.map(|(c, a)| ui::types::TextCursor {
                cursor: c,
                anchor: a,
            }),
        }
    }

    #[test]
    fn a_text_caret_lowers_onto_a_text_widget() {
        let node = widget_node_from_wit(wit_node(ui::types::WidgetKind::TextArea, Some((2, 0))))
            .expect("a text area may carry a caret");
        assert_eq!(node.text_cursor, Some((2, 0)));
    }

    /// A window with one image widget as its root, ready to be given a picture.
    fn host_with_image_widget() -> (Phase3GuiHost, u64, u64) {
        use ui::window::Host as _;
        let mut host = headless_host();
        let window = host
            .create(
                "viewer".to_string(),
                ui::types::WindowSize {
                    width: 200,
                    height: 200,
                },
            )
            .expect("create call")
            .expect("a window");

        let mut node = wit_node(ui::types::WidgetKind::Image, None);
        node.label = None;
        let widget = node.id;
        ui::tree::Host::set_root(&mut host, window, node)
            .expect("set_root call")
            .expect("an image widget may be the root");
        (host, window, widget)
    }

    /// A window with one canvas widget as its root.
    fn host_with_canvas_widget() -> (Phase3GuiHost, u64, u64) {
        use ui::window::Host as _;
        let mut host = headless_host();
        let window = host
            .create(
                "sketch".to_string(),
                ui::types::WindowSize {
                    width: 200,
                    height: 200,
                },
            )
            .expect("create call")
            .expect("a window");
        let node = wit_node(ui::types::WidgetKind::Canvas, None);
        let widget = node.id;
        ui::tree::Host::set_root(&mut host, window, node)
            .expect("set_root call")
            .expect("a canvas may be the root");
        (host, window, widget)
    }

    #[test]
    fn canvas_drawing_reaches_the_widget_only_at_present() {
        // The whole gfx.canvas2d path: bind, draw, present -- and the raster
        // lands in the same per-widget image store every host already reads.
        // Present is the only publisher: a hundred fills must not re-render
        // the window a hundred times, so before it the store stays empty.
        let (mut host, window, widget) = host_with_canvas_widget();
        let canvas = gfx::canvas2d::Host::bind(&mut host, window, widget)
            .expect("bind call")
            .expect("a canvas widget binds");

        gfx::canvas2d::Host::clear(
            &mut host,
            canvas,
            gfx::types::Color {
                r: 0.0,
                g: 0.0,
                b: 1.0,
                a: 1.0,
            },
        )
        .expect("clear call")
        .expect("clear succeeds");
        gfx::canvas2d::Host::fill_rect(
            &mut host,
            canvas,
            gfx::types::Rect {
                x: 0.0,
                y: 0.0,
                width: 10.0,
                height: 10.0,
            },
            gfx::types::Color {
                r: 1.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
        )
        .expect("fill call")
        .expect("fill succeeds");

        let id = host.window_id(window).expect("window");
        let widget_id = WidgetId::new(widget).expect("widget");
        assert!(
            host.images.borrow().get(&(id, widget_id)).is_none(),
            "nothing may reach the widget before present"
        );

        gfx::canvas2d::Host::present(&mut host, canvas)
            .expect("present call")
            .expect("present succeeds");

        let images = host.images.borrow();
        let image = images
            .get(&(id, widget_id))
            .expect("present publishes the raster");
        assert_eq!(&image.rgba[0..4], &[255, 0, 0, 255], "the red fill");
        let last = image.rgba.len() - 4;
        assert_eq!(&image.rgba[last..], &[0, 0, 255, 255], "the blue clear");
    }

    #[test]
    fn a_canvas_refuses_to_bind_to_a_button() {
        use ui::window::Host as _;
        let mut host = headless_host();
        let window = host
            .create(
                "app".to_string(),
                ui::types::WindowSize {
                    width: 100,
                    height: 100,
                },
            )
            .expect("create call")
            .expect("a window");
        let node = wit_node(ui::types::WidgetKind::Button, None);
        let widget = node.id;
        ui::tree::Host::set_root(&mut host, window, node)
            .expect("set_root call")
            .expect("a button root");

        let err = gfx::canvas2d::Host::bind(&mut host, window, widget)
            .expect("bind call")
            .expect_err("a button is not a canvas");
        assert!(matches!(err, gfx::types::GfxError::Unsupported(_)));
    }

    #[test]
    fn a_picture_reaches_the_widget_it_was_sent_for() {
        // The whole path: an app sets pixels through krate:ui/image, and they
        // arrive on the placement the painters draw from. Without this, a
        // picture could be accepted and stored and never reach a window.
        let (mut host, window, widget) = host_with_image_widget();
        ui::image::Host::set_pixels(
            &mut host,
            window,
            widget,
            ui::image::ImagePixels {
                width: 2,
                height: 2,
                rgba: vec![255u8; 16],
            },
        )
        .expect("set_pixels call")
        .expect("an image widget accepts a picture");

        let id = host.window_id(window).expect("the window exists");
        let widget_id = WidgetId::new(widget).expect("a real widget id");
        let stored = host.images.borrow();
        let picture = stored
            .get(&(id, widget_id))
            .expect("the picture must be held for this widget");
        assert_eq!((picture.width, picture.height), (2, 2));
        drop(stored);

        // And clearing takes it away, leaving the empty frame a viewer shows
        // before anybody has chosen a file.
        ui::image::Host::clear(&mut host, window, widget)
            .expect("clear call")
            .expect("clearing succeeds");
        assert!(host.images.borrow().is_empty());
    }

    #[test]
    fn a_picture_is_refused_for_a_widget_that_cannot_show_one() {
        // Storing pixels for a button would leave the app believing it had
        // shown something while nothing ever drew.
        use ui::window::Host as _;
        let mut host = headless_host();
        let window = host
            .create(
                "app".to_string(),
                ui::types::WindowSize {
                    width: 100,
                    height: 100,
                },
            )
            .expect("create call")
            .expect("a window");
        let node = wit_node(ui::types::WidgetKind::Button, None);
        let widget = node.id;
        ui::tree::Host::set_root(&mut host, window, node)
            .expect("set_root call")
            .expect("a button root");

        let err = ui::image::Host::set_pixels(
            &mut host,
            window,
            widget,
            ui::image::ImagePixels {
                width: 1,
                height: 1,
                rgba: vec![0u8; 4],
            },
        )
        .expect("set_pixels call")
        .expect_err("a button cannot show a picture");
        assert!(matches!(err, ui::types::UiError::Unsupported(_)));
    }

    #[test]
    fn a_picture_whose_bytes_do_not_match_its_size_is_refused() {
        // Every host indexes this buffer by row and column. A buffer shorter
        // than its stated size would read past the end on the last row, and
        // the guest is the untrusted side of this boundary.
        let (mut host, window, widget) = host_with_image_widget();
        let err = ui::image::Host::set_pixels(
            &mut host,
            window,
            widget,
            ui::image::ImagePixels {
                width: 4,
                height: 4,
                // A 4x4 image needs 64 bytes, not 8.
                rgba: vec![0u8; 8],
            },
        )
        .expect("set_pixels call")
        .expect_err("the byte count must match the size");
        assert!(matches!(err, ui::types::UiError::Unsupported(_)));
        assert!(host.images.borrow().is_empty(), "nothing may be stored");
    }

    #[test]
    fn a_text_caret_on_a_non_text_widget_is_rejected() {
        // A caret only means something on editable text; carrying one on, say,
        // a button is a guest bug and must not silently pass through.
        let err = widget_node_from_wit(wit_node(ui::types::WidgetKind::Button, Some((1, 1))))
            .expect_err("a button cannot carry a text caret");
        assert!(matches!(err, ui::types::UiError::Unsupported(_)));
    }
}
