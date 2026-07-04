//! Host implementation for the Phase 3 `gui` world's new imports.
//!
//! `Phase3GuiHost` backs the `krate:ui` interfaces with the UCap-gated
//! Phase 3 UI dispatcher. Window, widget-tree, and event calls are real;
//! after every tree change the host recomputes layout and re-lowers the
//! supported widgets to native controls when the selected adapter can (the
//! opt-in macOS AppKit prototype today — headless adapters lower nothing and
//! that is a valid state). The `gfx`, `audio`, `dialog`, and `menu` surfaces
//! return honest `unsupported` errors until their runtimes exist.

use krate_adapter_common::ui::{
    Modifiers, PointerButton, Theme, UiAdapterError, UiEvent, WidgetId, WidgetKind, WidgetNode,
    WidgetPlacement, WidgetStyle, WindowId, WindowOptions, WindowSize,
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
}

impl Phase3GuiHost {
    /// Create the GUI host with the requested host UI mode.
    pub fn new(guard: UapiGuard, mode: Phase3HostUiMode) -> Result<Self, UiDispatchError> {
        let runtime = Phase3UiRuntime::try_with_host_adapter_mode(guard, mode)?;
        Ok(Self {
            runtime,
            windows: Vec::new(),
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

        let mut placements = Vec::new();
        for (id, node) in tree.nodes() {
            if !matches!(
                node.kind,
                WidgetKind::Button | WidgetKind::TextField | WidgetKind::Text
            ) {
                continue;
            }
            let Some(rect) = absolute_rect(&tree, &layout, *id) else {
                continue;
            };
            placements.push(WidgetPlacement {
                widget: *id,
                kind: node.kind,
                label: node.label.clone(),
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
            });
        }

        dispatcher.lower_widget_placements(window, &placements)?;
        Ok(())
    }

    fn poll_one_event(&self) -> Result<Option<ui::types::Event>, UiDispatchError> {
        let dispatcher = self.dispatcher();
        for window in &self.windows {
            // Native pumps refresh window state and drain delegate callbacks;
            // headless adapters return no tick. Ignore per-window pump errors
            // so one closed window cannot wedge event delivery.
            let _ = dispatcher.pump_event_loop_once(*window);
        }

        // Route raw native pointer input through layout hit testing so the
        // app-facing event carries a widget id. Raw samples never reach the
        // queue directly, so this cannot loop.
        for sample in dispatcher.drain_raw_pointer_input() {
            if let Some(record) = dispatcher.window(sample.window)? {
                if let Ok(viewport) =
                    LayoutViewport::new(record.size.width as f32, record.size.height as f32)
                {
                    let _ = dispatcher.route_pointer_event(crate::phase3_ui::PointerRouteRequest {
                        window: sample.window,
                        viewport,
                        x: sample.x,
                        y: sample.y,
                        button: Some(PointerButton::Primary),
                        pressed: sample.pressed,
                        modifiers: Modifiers::default(),
                    });
                }
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

    Ok(WidgetNode {
        id,
        parent,
        kind: widget_kind_from_wit(node.kind),
        label: node.label,
        role: node.role,
        style,
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
