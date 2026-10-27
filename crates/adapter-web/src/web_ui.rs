//! The `krate:ui` host for a browser tab (IC-956 Phase 1).
//!
//! The app runs on the browser's own WebAssembly engine (its component
//! transpiled by jco), inside a Web Worker. Every `window.*`, `tree.*` and
//! input record reaches this module, which keeps the same state a desktop
//! host keeps and draws the same frame:
//!
//! - windows, trees, focus and the event queue live in
//!   [`DraftUiAdapter`], the adapter the desktop runtime runs headless;
//! - layout, placements, scroll offsets, which presses move focus, and
//!   Enter/Space activating a focused control are `krate-lowering`, the
//!   step the desktop runtime itself calls;
//! - the pixels are `krate-adapter-common`'s painter with vector text
//!   (parley + vello_cpu) and a bundled font, at the tab's pixel density.
//!
//! Nothing here knows about JavaScript: the wasm-bindgen face is in
//! `crate::bindings`, and every method is exercised by native tests.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use krate_adapter_common::painter::{self, PaintInteraction};
use krate_adapter_common::ui::DraftUiAdapter;
use krate_adapter_common::ui::{
    Appearance, BoxStyle, Color, ImagePixels, KeyEvent, Modifiers, Placement, PointerButton,
    PointerEvent, TextInputEvent, TextStyle, Theme, UiAdapter, UiAdapterError, UiEvent, WheelEvent,
    WidgetId, WidgetKind, WidgetNode, WidgetStyle, WindowAdapter, WindowId, WindowOptions,
    WindowSize,
};
use krate_layout::{hit_test, LayoutPoint, LayoutSnapshot, LayoutViewport};
use serde::Deserialize;
use serde_json::{json, Value};

/// A refusal in the WIT `ui-error` shape jco expects: `{tag}` or
/// `{tag, val}`.
pub fn ui_error(tag: &str, words: Option<String>) -> Value {
    match words {
        Some(words) => json!({ "tag": tag, "val": words }),
        None => json!({ "tag": tag }),
    }
}

fn adapter_error(err: UiAdapterError) -> Value {
    match err {
        UiAdapterError::InvalidWindow { .. } => ui_error("invalid-window", None),
        UiAdapterError::InvalidWidgetId { .. } => ui_error("invalid-widget", None),
        UiAdapterError::Unsupported(words) => ui_error("unsupported", Some(words)),
        other => ui_error("platform", Some(other.to_string())),
    }
}

/// A u64 as jco hands it over: the page's JSON carries BigInts as strings.
fn de_u64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Text(String),
        Number(u64),
    }
    match Raw::deserialize(d)? {
        Raw::Text(text) => text.parse().map_err(serde::de::Error::custom),
        Raw::Number(n) => Ok(n),
    }
}

fn de_opt_u64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    #[derive(Deserialize)]
    struct Wrap(#[serde(deserialize_with = "de_u64")] u64);
    Ok(Option::<Wrap>::deserialize(d)?.map(|w| w.0))
}

/// The WIT `widget-node` record as jco's JavaScript spells it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsNode {
    #[serde(deserialize_with = "de_u64")]
    id: u64,
    #[serde(default, deserialize_with = "de_opt_u64")]
    parent: Option<u64>,
    kind: String,
    label: Option<String>,
    role: Option<String>,
    style: JsStyle,
    checked: Option<bool>,
    value: Option<f32>,
    selected: Option<u32>,
    text_cursor: Option<JsCursor>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsStyle {
    width: Option<f32>,
    height: Option<f32>,
    #[serde(default)]
    grow: f32,
    #[serde(default)]
    padding: f32,
    text: Option<JsTextStyle>,
    place: Option<String>,
    #[serde(rename = "box")]
    box_: Option<JsBoxStyle>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsTextStyle {
    color: Option<JsColor>,
    outline: Option<JsColor>,
    #[serde(default)]
    outline_width: f32,
    size: Option<f32>,
    #[serde(default)]
    bold: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsBoxStyle {
    background: Option<JsColor>,
    border: Option<JsColor>,
    #[serde(default)]
    border_width: f32,
    #[serde(default)]
    corner_radius: f32,
}

#[derive(Debug, Deserialize, Clone, Copy)]
struct JsColor {
    r: u8,
    g: u8,
    b: u8,
    a: u8,
}

#[derive(Debug, Deserialize)]
struct JsCursor {
    cursor: u32,
    anchor: u32,
}

fn color(c: JsColor) -> Color {
    Color {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    }
}

/// The WIT `widget-kind` case names, phase 3 and 4 alike.
fn kind_named(name: &str) -> Option<WidgetKind> {
    Some(match name {
        "stack" => WidgetKind::Stack,
        "grid" => WidgetKind::Grid,
        "scroll" => WidgetKind::Scroll,
        "tabs" => WidgetKind::Tabs,
        "button" => WidgetKind::Button,
        "checkbox" => WidgetKind::Checkbox,
        "radio" => WidgetKind::Radio,
        "switch" => WidgetKind::Switch,
        "slider" => WidgetKind::Slider,
        "progress" => WidgetKind::Progress,
        "text" => WidgetKind::Text,
        "text-field" => WidgetKind::TextField,
        "text-area" => WidgetKind::TextArea,
        "list-view" => WidgetKind::ListView,
        "tree-view" => WidgetKind::TreeView,
        "image" => WidgetKind::Image,
        "canvas" => WidgetKind::Canvas,
        "overlay" => WidgetKind::Overlay,
        _ => return None,
    })
}

fn placement_named(name: Option<&str>) -> Placement {
    match name {
        Some("top-left") => Placement::TopLeft,
        Some("top-centre") => Placement::TopCentre,
        Some("top-right") => Placement::TopRight,
        Some("centre-left") => Placement::CentreLeft,
        Some("centre") => Placement::Centre,
        Some("centre-right") => Placement::CentreRight,
        Some("bottom-left") => Placement::BottomLeft,
        Some("bottom-centre") => Placement::BottomCentre,
        Some("bottom-right") => Placement::BottomRight,
        _ => Placement::Default,
    }
}

/// Read one node the app sent and hold it to the shared rules
/// (`krate_lowering::node_from_parts`, the same function the desktop runs).
pub fn node_from_json(text: &str) -> Result<WidgetNode, Value> {
    let node: JsNode = serde_json::from_str(text)
        .map_err(|err| ui_error("platform", Some(format!("not a widget node: {err}"))))?;
    let kind = kind_named(&node.kind).ok_or_else(|| {
        ui_error(
            "unsupported",
            Some(format!("unknown widget kind {:?}", node.kind)),
        )
    })?;
    let style = WidgetStyle {
        width: node.style.width,
        height: node.style.height,
        grow: node.style.grow,
        padding: node.style.padding,
        text: node.style.text.map(|t| TextStyle {
            color: t.color.map(color),
            outline: t.outline.map(color),
            outline_width: t.outline_width,
            size: t.size,
            bold: t.bold,
        }),
        place: placement_named(node.style.place.as_deref()),
        r#box: node.style.box_.map(|b| BoxStyle {
            background: b.background.map(color),
            border: b.border.map(color),
            border_width: b.border_width,
            corner_radius: b.corner_radius,
        }),
    };
    krate_lowering::node_from_parts(
        node.id,
        node.parent,
        kind,
        node.label,
        node.role,
        style,
        node.checked,
        node.value,
        node.selected,
        node.text_cursor.map(|c| (c.cursor, c.anchor)),
    )
    .map_err(|err| match err {
        krate_lowering::NodeError::InvalidWidget => ui_error("invalid-widget", None),
        krate_lowering::NodeError::Unsupported(words) => ui_error("unsupported", Some(words)),
    })
}

/// A DOM `KeyboardEvent.key` as Krate's portable key name, the names every
/// desktop adapter reports ("Enter", "Space", "ArrowLeft", "a"). Keys
/// outside that set (media keys, dead keys, IME) are dropped, as there.
pub fn key_name(dom: &str) -> Option<String> {
    let named = [
        "Enter",
        "Backspace",
        "Delete",
        "Tab",
        "Escape",
        "ArrowLeft",
        "ArrowRight",
        "ArrowUp",
        "ArrowDown",
        "Home",
        "End",
        "PageUp",
        "PageDown",
        "Shift",
        "Control",
        "Alt",
        "Meta",
        "F1",
        "F2",
        "F3",
        "F4",
        "F5",
        "F6",
        "F7",
        "F8",
        "F9",
        "F10",
        "F11",
        "F12",
    ];
    match dom {
        " " | "Spacebar" => Some("Space".to_string()),
        "OS" => Some("Meta".to_string()),
        _ if named.contains(&dom) => Some(dom.to_string()),
        _ if dom.chars().count() == 1 && !dom.chars().any(char::is_control) => {
            Some(dom.to_string())
        }
        _ => None,
    }
}

fn button_named(button: u8) -> PointerButton {
    match button {
        0 => PointerButton::Primary,
        1 => PointerButton::Middle,
        2 => PointerButton::Secondary,
        _ => PointerButton::Other,
    }
}

fn modifiers(bits: u8) -> Modifiers {
    Modifiers {
        shift: bits & 1 != 0,
        control: bits & 2 != 0,
        alt: bits & 4 != 0,
        meta: bits & 8 != 0,
    }
}

fn mods_json(m: Modifiers) -> Value {
    json!({ "shift": m.shift, "control": m.control, "alt": m.alt, "meta": m.meta })
}

fn button_json(b: PointerButton) -> &'static str {
    match b {
        PointerButton::Primary => "primary",
        PointerButton::Secondary => "secondary",
        PointerButton::Middle => "middle",
        PointerButton::Other => "other",
    }
}

/// One adapter event in the WIT `event` shape, u64s as strings; the
/// bookkeeping events with no WIT case are not delivered, as on desktop.
fn event_json(event: UiEvent) -> Option<Value> {
    let id = |w: u64| Value::String(w.to_string());
    let opt = |w: Option<WidgetId>| w.map(|w| id(w.get())).unwrap_or(Value::Null);
    Some(match event {
        UiEvent::WindowCloseRequested(w) => json!({ "tag": "close-requested", "val": id(w.get()) }),
        UiEvent::Resized { size, .. } => json!({ "tag": "resized",
            "val": { "width": size.width, "height": size.height } }),
        UiEvent::RedrawRequested(w) => json!({ "tag": "redraw-requested", "val": id(w.get()) }),
        UiEvent::Pointer(p) => json!({ "tag": "pointer", "val": {
            "window": id(p.window.get()), "widget": opt(p.widget), "x": p.x, "y": p.y,
            "button": p.button.map(button_json), "pressed": p.pressed,
            "modifiers": mods_json(p.modifiers) } }),
        UiEvent::Wheel(w) => json!({ "tag": "wheel", "val": {
            "window": id(w.window.get()), "widget": opt(w.widget), "x": w.x, "y": w.y,
            "dx": w.dx, "dy": w.dy, "modifiers": mods_json(w.modifiers) } }),
        UiEvent::Key(k) => json!({ "tag": "key", "val": {
            "window": id(k.window.get()), "widget": opt(k.widget), "key": k.key,
            "pressed": k.pressed, "modifiers": mods_json(k.modifiers) } }),
        UiEvent::TextInput(t) => json!({ "tag": "text-input", "val": t.text }),
        UiEvent::TextChanged(c) => json!({ "tag": "text-changed", "val": {
            "window": id(c.window.get()), "widget": id(c.widget.get()), "text": c.text } }),
        UiEvent::FocusChanged { widget, .. } => {
            json!({ "tag": "focus-changed", "val": id(widget.get()) })
        }
        UiEvent::ThemeChanged { theme } => json!({ "tag": "theme-changed", "val": match theme {
            Theme::Light => "light", Theme::Dark => "dark", Theme::Unknown => "unknown" } }),
        _ => return None,
    })
}

/// One window's frame, in physical pixels.
struct Frame {
    width: u32,
    height: u32,
    argb: Vec<u32>,
    rgba: Vec<u8>,
    dirty: bool,
}

impl Frame {
    fn new(width: u32, height: u32) -> Self {
        let len = width as usize * height as usize;
        Self {
            width,
            height,
            argb: vec![0; len],
            rgba: vec![0; len * 4],
            dirty: true,
        }
    }
}

/// Everything the `krate:ui` host holds for one app in a tab.
pub struct WebUi {
    adapter: DraftUiAdapter,
    windows: Vec<WindowId>,
    frames: BTreeMap<WindowId, Frame>,
    offsets: BTreeMap<(WindowId, WidgetId), f32>,
    images: BTreeMap<(WindowId, WidgetId), Arc<ImagePixels>>,
    scale: f32,
    held: BTreeSet<String>,
    buttons: BTreeSet<u8>,
    position: BTreeMap<WindowId, (f32, f32)>,
    hovered: Option<WidgetId>,
    pressed: Option<WidgetId>,
    /// The size each window asked for, before the tab fitted it.
    requested: BTreeMap<WindowId, (f32, f32)>,
    pub(crate) canvas: crate::web_canvas::Canvases,
    /// The finger being followed on a touch screen, if any.
    touch: Option<TouchGesture>,
}

/// Movement past this many logical pixels turns a touch from a tap into a
/// scroll: the slop every phone toolkit uses, and the Android adapter's.
pub const TOUCH_SLOP: f32 = 8.0;

/// One finger on the screen, as the phone adapters follow it.
#[derive(Debug, Clone, Copy)]
struct TouchGesture {
    window: u64,
    finger: u64,
    start: (f32, f32),
    last: (f32, f32),
    scrolling: bool,
    /// It landed on something that follows the finger (a canvas, a
    /// slider): pressed at once and held until it lifts.
    drags: bool,
}

impl WebUi {
    /// A host for one app, painting at `scale` physical pixels per logical
    /// pixel (the tab's `devicePixelRatio`), in the browser's light or dark.
    pub fn new(scale: f32, dark: bool) -> Self {
        krate_adapter_common::ui::set_widget_look(Appearance {
            dark,
            ..Appearance::default()
        });
        Self {
            adapter: DraftUiAdapter::new(),
            windows: Vec::new(),
            frames: BTreeMap::new(),
            offsets: BTreeMap::new(),
            images: BTreeMap::new(),
            scale: if scale.is_finite() {
                scale.clamp(1.0, 4.0)
            } else {
                1.0
            },
            held: BTreeSet::new(),
            buttons: BTreeSet::new(),
            position: BTreeMap::new(),
            hovered: None,
            pressed: None,
            requested: BTreeMap::new(),
            canvas: crate::web_canvas::Canvases::new(),
            touch: None,
        }
    }

    pub(crate) fn window_id_of(&self, raw: u64) -> Result<WindowId, Value> {
        self.window_id(raw)
    }

    pub(crate) fn tree_of(&self, window: WindowId) -> Option<krate_adapter_common::ui::WidgetTree> {
        self.adapter.widget_tree(window).ok()?
    }

    pub(crate) fn layout_of(
        &self,
        window: WindowId,
    ) -> Option<(krate_adapter_common::ui::WidgetTree, LayoutSnapshot)> {
        self.layout(window)
    }

    pub(crate) fn requested_size(&self, window: WindowId) -> Option<(f32, f32)> {
        self.requested.get(&window).copied()
    }

    /// A picture for a widget, from `image.set-pixels` or a canvas's frame.
    pub(crate) fn publish_image(
        &mut self,
        window: WindowId,
        widget: WidgetId,
        image: Arc<ImagePixels>,
    ) {
        self.images.insert((window, widget), image);
        self.mark(window);
    }

    fn window_id(&self, raw: u64) -> Result<WindowId, Value> {
        self.windows
            .iter()
            .copied()
            .find(|w| w.get() == raw)
            .ok_or_else(|| ui_error("invalid-window", None))
    }

    fn physical(&self, logical: u32) -> u32 {
        ((logical as f32) * self.scale).round().max(1.0) as u32
    }

    fn mark(&mut self, window: WindowId) {
        if let Some(frame) = self.frames.get_mut(&window) {
            frame.dirty = true;
        }
    }

    fn mark_all(&mut self) {
        for frame in self.frames.values_mut() {
            frame.dirty = true;
        }
    }

    /// `window.create`: a window of `width` x `height` logical pixels (what
    /// the tab had room for), for an app that asked for `asked`.
    pub fn create_window(
        &mut self,
        title: &str,
        width: u32,
        height: u32,
        asked: (u32, u32),
    ) -> Result<u64, Value> {
        let size = WindowSize::new(width, height)
            .map_err(|e| ui_error("platform", Some(e.to_string())))?;
        let options = WindowOptions::new(title, size)
            .map_err(|e| ui_error("platform", Some(e.to_string())))?;
        // Build the font stack before the app can draw anything (K-414).
        krate_adapter_common::vector_text::warm_text_engine();
        let id = self.adapter.create_window(options).map_err(adapter_error)?;
        self.windows.push(id);
        self.requested.insert(id, (asked.0 as f32, asked.1 as f32));
        let frame = Frame::new(self.physical(width), self.physical(height));
        self.frames.insert(id, frame);
        Ok(id.get())
    }

    pub fn show_window(&mut self, window: u64) -> Result<(), Value> {
        let id = self.window_id(window)?;
        self.adapter.show_window(id).map_err(adapter_error)
    }

    pub fn close_window(&mut self, window: u64) -> Result<(), Value> {
        let id = self.window_id(window)?;
        self.adapter.close_window(id).map_err(adapter_error)?;
        self.windows.retain(|w| *w != id);
        self.frames.remove(&id);
        self.requested.remove(&id);
        self.canvas.surfaces.retain(|_, (w, _, _)| *w != id);
        Ok(())
    }

    pub fn set_title(&mut self, window: u64, title: &str) -> Result<(), Value> {
        let id = self.window_id(window)?;
        self.adapter
            .set_title(id, title.to_string())
            .map_err(adapter_error)
    }

    /// `window.set-size`, and a resize the tab itself made: the frame
    /// follows, and the app hears `resized` when `host` is true.
    pub fn set_size(
        &mut self,
        window: u64,
        width: u32,
        height: u32,
        host: bool,
    ) -> Result<(), Value> {
        let id = self.window_id(window)?;
        let size = WindowSize::new(width, height)
            .map_err(|e| ui_error("platform", Some(e.to_string())))?;
        if host {
            self.adapter.queue_host_resize(id, size)
        } else {
            self.adapter.set_size(id, size)
        }
        .map_err(adapter_error)?;
        let frame = Frame::new(self.physical(width), self.physical(height));
        self.frames.insert(id, frame);
        Ok(())
    }

    pub fn request_redraw(&mut self, window: u64) -> Result<(), Value> {
        let id = self.window_id(window)?;
        self.mark(id);
        self.adapter.request_redraw(id).map_err(adapter_error)
    }

    pub fn set_root(&mut self, window: u64, node: &str) -> Result<(), Value> {
        let id = self.window_id(window)?;
        let node = node_from_json(node)?;
        self.adapter.set_root(id, node).map_err(adapter_error)?;
        self.mark(id);
        Ok(())
    }

    pub fn upsert(&mut self, window: u64, node: &str) -> Result<(), Value> {
        let id = self.window_id(window)?;
        let node = node_from_json(node)?;
        self.adapter.upsert_node(id, node).map_err(adapter_error)?;
        self.mark(id);
        Ok(())
    }

    pub fn remove(&mut self, window: u64, widget: u64) -> Result<(), Value> {
        let id = self.window_id(window)?;
        let widget = WidgetId::new(widget).map_err(|_| ui_error("invalid-widget", None))?;
        self.adapter
            .remove_node(id, widget)
            .map_err(adapter_error)?;
        self.images.remove(&(id, widget));
        self.mark(id);
        Ok(())
    }

    pub fn focus(&mut self, window: u64, widget: u64) -> Result<(), Value> {
        let id = self.window_id(window)?;
        let widget = WidgetId::new(widget).map_err(|_| ui_error("invalid-widget", None))?;
        self.adapter.focus_node(id, widget).map_err(adapter_error)?;
        self.mark(id);
        Ok(())
    }

    /// `image.set-pixels`: the picture an image widget shows.
    pub fn set_image(
        &mut self,
        window: u64,
        widget: u64,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> Result<(), Value> {
        let id = self.window_id(window)?;
        let widget = WidgetId::new(widget).map_err(|_| ui_error("invalid-widget", None))?;
        let pixels = ImagePixels::new(width, height, rgba)
            .map_err(|e| ui_error("unsupported", Some(e.to_string())))?;
        self.images.insert((id, widget), Arc::new(pixels));
        self.mark(id);
        Ok(())
    }

    pub fn clear_image(&mut self, window: u64, widget: u64) -> Result<(), Value> {
        let id = self.window_id(window)?;
        let widget = WidgetId::new(widget).map_err(|_| ui_error("invalid-widget", None))?;
        self.images.remove(&(id, widget));
        self.mark(id);
        Ok(())
    }

    fn layout(
        &self,
        window: WindowId,
    ) -> Option<(krate_adapter_common::ui::WidgetTree, LayoutSnapshot)> {
        let tree = self.adapter.widget_tree(window).ok()??;
        let record = self.adapter.window(window).ok()??;
        let viewport =
            LayoutViewport::new(record.size.width as f32, record.size.height as f32).ok()?;
        let layout = krate_lowering::layout_inside(&tree, viewport, (0.0, 0.0, 0.0, 0.0)).ok()?;
        Some((tree, layout))
    }

    fn widget_at(&self, window: WindowId, x: f32, y: f32) -> Option<WidgetId> {
        let (tree, layout) = self.layout(window)?;
        let point = LayoutPoint::new(x, y).ok()?;
        hit_test(&tree, &layout, point).map(|hit| hit.widget)
    }

    /// A press or release at (`x`, `y`) in logical pixels, routed the way
    /// the desktop routes one: hit-tested so `pointer.widget` names what was
    /// pressed, and a press on a text field moves keyboard focus there.
    pub fn pointer(&mut self, window: u64, x: f32, y: f32, button: u8, pressed: bool, mods: u8) {
        let Ok(id) = self.window_id(window) else {
            return;
        };
        self.position.insert(id, (x, y));
        if pressed {
            self.buttons.insert(button);
        } else {
            self.buttons.remove(&button);
        }
        let widget = self.widget_at(id, x, y);
        let _ = self.adapter.queue_pointer_event(PointerEvent {
            window: id,
            widget,
            x,
            y,
            button: Some(button_named(button)),
            pressed,
            modifiers: modifiers(mods),
        });
        self.pressed = if pressed { widget } else { None };
        if pressed {
            if let (Some(widget), Ok(Some(tree))) = (widget, self.adapter.widget_tree(id)) {
                let focusable = tree
                    .node(widget)
                    .is_some_and(|node| krate_lowering::press_focuses(node.kind));
                if focusable && self.adapter.focused_widget(id).ok().flatten() != Some(widget) {
                    let _ = self.adapter.focus_node(id, widget);
                }
            }
        }
        self.mark(id);
    }

    /// The pointer moved: where it is, and what it hovers, for the painter.
    /// Movement is not an app event, as on desktop.
    pub fn pointer_moved(&mut self, window: u64, x: f32, y: f32) {
        let Ok(id) = self.window_id(window) else {
            return;
        };
        self.position.insert(id, (x, y));
        let hovered = self.widget_at(id, x, y);
        if hovered != self.hovered {
            self.hovered = hovered;
            self.mark(id);
        }
    }

    pub fn pointer_left(&mut self, window: u64) {
        let Ok(id) = self.window_id(window) else {
            return;
        };
        self.position.remove(&id);
        if self.hovered.take().is_some() {
            self.mark(id);
        }
    }

    /// A wheel turn: delivered to the app always, and a scroll container
    /// under it scrolls host-side (`krate_lowering::scroll_by`).
    pub fn wheel(&mut self, window: u64, x: f32, y: f32, dx: f32, dy: f32, mods: u8) {
        let Ok(id) = self.window_id(window) else {
            return;
        };
        let widget = self.widget_at(id, x, y);
        let _ = self.adapter.queue_wheel_event(WheelEvent {
            window: id,
            widget,
            x,
            y,
            dx,
            dy,
            modifiers: modifiers(mods),
        });
        let Some((tree, layout)) = self.layout(id) else {
            return;
        };
        let offsets = &self.offsets;
        if let Some((scroll, offset)) = krate_lowering::scroll_by(&tree, &layout, x, y, dy, |s| {
            offsets.get(&(id, s)).copied().unwrap_or(0.0)
        }) {
            self.offsets.insert((id, scroll), offset);
            self.mark(id);
        }
    }

    /// A finger on a touch screen, in logical pixels, mapped the way the
    /// phone adapters map one (crates/adapter-android): a tap is one press
    /// and release where it lifted, exactly what a click delivers; a finger
    /// that moves past [`TOUCH_SLOP`] scrolls what is under it, content
    /// following the finger; one that lands on a canvas or a slider presses
    /// it at once and the press follows the finger until it lifts. Only the
    /// first finger is followed. `phase` is start, move, end or cancel.
    pub fn touch(&mut self, window: u64, phase: &str, finger: u64, x: f32, y: f32) {
        let Ok(id) = self.window_id(window) else {
            return;
        };
        match phase {
            "start" => {
                if self.touch.is_some() {
                    return;
                }
                let drags = self.widget_at(id, x, y).is_some_and(|w| {
                    self.adapter
                        .widget_tree(id)
                        .ok()
                        .flatten()
                        .and_then(|tree| tree.node(w).map(|n| n.kind))
                        .is_some_and(|kind| matches!(kind, WidgetKind::Canvas | WidgetKind::Slider))
                });
                self.touch = Some(TouchGesture {
                    window,
                    finger,
                    start: (x, y),
                    last: (x, y),
                    scrolling: false,
                    drags,
                });
                // The finger is the pointer: hit tests and hover read from here.
                self.pointer_moved(window, x, y);
                if drags {
                    self.pointer(window, x, y, 0, true, 0);
                }
            }
            "move" => {
                let Some(mut g) = self
                    .touch
                    .filter(|g| g.finger == finger && g.window == window)
                else {
                    return;
                };
                if g.drags {
                    g.last = (x, y);
                    self.touch = Some(g);
                    self.pointer_moved(window, x, y);
                    return;
                }
                if !g.scrolling {
                    let (sx, sy) = (x - g.start.0, y - g.start.1);
                    g.scrolling = sx * sx + sy * sy > TOUCH_SLOP * TOUCH_SLOP;
                }
                let (dx, dy) = (x - g.last.0, y - g.last.1);
                g.last = (x, y);
                self.touch = Some(g);
                if g.scrolling && (dx.abs() > f32::EPSILON || dy.abs() > f32::EPSILON) {
                    // Content follows the finger: moving up scrolls deeper.
                    self.wheel(window, g.start.0, g.start.1, -dx, -dy, 0);
                }
            }
            "end" | "cancel" => {
                let Some(g) = self
                    .touch
                    .filter(|g| g.finger == finger && g.window == window)
                else {
                    return;
                };
                self.touch = None;
                if g.drags {
                    self.pointer(window, x, y, 0, false, 0);
                } else if phase == "end" && !g.scrolling {
                    self.pointer(window, x, y, 0, true, 0);
                    self.pointer(window, x, y, 0, false, 0);
                }
                // No hover is left behind a finger.
                self.pointer_left(window);
                self.mark(id);
            }
            _ => {}
        }
    }

    /// Where the text fields and text areas are on screen in `window`, as
    /// x, y, width, height in logical pixels, four numbers each: a tap on
    /// one raises the phone's keyboard, which a browser only does inside
    /// the tap itself.
    pub fn text_rects(&self, window: u64) -> Vec<f32> {
        let Ok(id) = self.window_id(window) else {
            return Vec::new();
        };
        let Some((tree, layout)) = self.layout(id) else {
            return Vec::new();
        };
        let offsets = &self.offsets;
        krate_lowering::placements(
            &tree,
            &layout,
            |s| offsets.get(&(id, s)).copied().unwrap_or(0.0),
            |_| None,
        )
        .iter()
        .filter(|p| matches!(p.kind, WidgetKind::TextField | WidgetKind::TextArea))
        .flat_map(|p| match p.clip {
            Some((cx, cy, cw, ch)) => {
                let (x0, y0) = (p.x.max(cx), p.y.max(cy));
                let (x1, y1) = ((p.x + p.width).min(cx + cw), (p.y + p.height).min(cy + ch));
                [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)]
            }
            None => [p.x, p.y, p.width, p.height],
        })
        .collect()
    }

    /// A key, with the DOM's name for it. `text` is what it typed, when it
    /// typed something; it arrives as `text-input` after the key, as on
    /// desktop.
    pub fn key(&mut self, window: u64, dom_key: &str, pressed: bool, mods: u8, text: Option<&str>) {
        let Ok(id) = self.window_id(window) else {
            return;
        };
        let focused = self.adapter.focused_widget(id).ok().flatten();
        if let Some(name) = key_name(dom_key) {
            if pressed {
                self.held.insert(name.clone());
            } else {
                self.held.remove(&name);
            }
            if let Ok(event) = KeyEvent::new(id, focused, name, pressed, modifiers(mods)) {
                let _ = self.adapter.queue_key_event(event);
            }
        }
        if pressed {
            if let Some(text) = text.filter(|t| !t.is_empty()) {
                if let Ok(event) = TextInputEvent::new(id, focused, text) {
                    let _ = self.adapter.queue_text_input(event);
                }
            }
        }
    }

    /// Text the person typed or pasted that no single key carries.
    pub fn text(&mut self, window: u64, text: &str) {
        let Ok(id) = self.window_id(window) else {
            return;
        };
        let focused = self.adapter.focused_widget(id).ok().flatten();
        if let Ok(event) = TextInputEvent::new(id, focused, text) {
            let _ = self.adapter.queue_text_input(event);
        }
    }

    /// The tab lost focus: every key and button held is let go, or an app
    /// asking `key-held` sees a key that never comes back up.
    pub fn blurred(&mut self) {
        self.held.clear();
        self.buttons.clear();
        self.pressed = None;
    }

    pub fn close_requested(&mut self, window: u64) {
        if let Ok(id) = self.window_id(window) {
            let _ = self.adapter.queue_close_requested(id);
        }
    }

    /// The browser switched light and dark: the widgets follow, and the app
    /// hears `theme-changed`.
    pub fn theme(&mut self, dark: bool) {
        krate_adapter_common::ui::set_widget_look(Appearance {
            dark,
            ..krate_adapter_common::ui::widget_look()
        });
        let _ = self
            .adapter
            .queue_theme_changed(if dark { Theme::Dark } else { Theme::Light });
        self.mark_all();
    }

    pub fn dark(&self) -> bool {
        krate_adapter_common::ui::widget_look().dark
    }

    pub fn key_held(&self, name: &str) -> bool {
        self.held.contains(name)
    }

    pub fn button_held(&self, button: u8) -> bool {
        self.buttons.contains(&button)
    }

    pub fn position(&self, window: u64) -> Option<(f32, f32)> {
        let id = self.window_id(window).ok()?;
        self.position.get(&id).copied()
    }

    pub fn focused(&self, window: u64) -> Option<u64> {
        let id = self.window_id(window).ok()?;
        self.adapter
            .focused_widget(id)
            .ok()
            .flatten()
            .map(|w| w.get())
    }

    /// Whether the focused widget takes typing: the page raises the phone's
    /// keyboard for it.
    pub fn text_focused(&self, window: u64) -> bool {
        let Ok(id) = self.window_id(window) else {
            return false;
        };
        let Some(widget) = self.adapter.focused_widget(id).ok().flatten() else {
            return false;
        };
        self.adapter
            .widget_tree(id)
            .ok()
            .flatten()
            .and_then(|tree| {
                tree.node(widget)
                    .map(|n| krate_lowering::press_focuses(n.kind))
            })
            .unwrap_or(false)
    }

    /// The next event for the app, in the WIT shape, or `None`.
    ///
    /// Enter or Space on a focused control IS a press of it: the key comes
    /// first, then a press and release at the control's centre, exactly as
    /// the desktop delivers them (`krate_lowering::activation_point`).
    pub fn next_event(&mut self) -> Option<String> {
        while let Ok(Some(event)) = self.adapter.poll_event() {
            if let UiEvent::Key(key) = &event {
                if key.pressed {
                    if let Some(widget) = key.widget {
                        if let Some((tree, layout)) = self.layout(key.window) {
                            if let Some((x, y)) =
                                krate_lowering::activation_point(&tree, &layout, widget, &key.key)
                            {
                                for pressed in [true, false] {
                                    let hit = self.widget_at(key.window, x, y);
                                    let _ = self.adapter.queue_pointer_event(PointerEvent {
                                        window: key.window,
                                        widget: hit,
                                        x,
                                        y,
                                        button: Some(PointerButton::Primary),
                                        pressed,
                                        modifiers: key.modifiers,
                                    });
                                }
                            }
                        }
                    }
                }
            }
            if let UiEvent::FocusChanged { window, .. } = &event {
                self.mark(*window);
            }
            // An app that fixed its canvas's coordinate system hears the
            // pointer in that system, as on desktop.
            let event = match event {
                UiEvent::Pointer(mut p) => {
                    if let Some((x, y, _)) = self.to_design(p.x, p.y) {
                        p.x = x;
                        p.y = y;
                    }
                    UiEvent::Pointer(p)
                }
                UiEvent::Wheel(mut w) => {
                    if let Some((x, y, k)) = self.to_design(w.x, w.y) {
                        w.x = x;
                        w.y = y;
                        // Deltas are lengths, not positions: scale, do not offset.
                        w.dx /= k;
                        w.dy /= k;
                    }
                    UiEvent::Wheel(w)
                }
                other => other,
            };
            if let Some(json) = event_json(event) {
                return Some(json.to_string());
            }
        }
        None
    }

    /// Lay out and paint `window` if anything changed since its last frame.
    /// Returns whether it painted.
    pub fn render(&mut self, window: u64) -> bool {
        let Ok(id) = self.window_id(window) else {
            return false;
        };
        if !self.frames.get(&id).is_some_and(|f| f.dirty) {
            return false;
        }
        let placements = match self.layout(id) {
            Some((tree, layout)) => {
                let offsets = &self.offsets;
                let images = &self.images;
                krate_lowering::placements(
                    &tree,
                    &layout,
                    |s| offsets.get(&(id, s)).copied().unwrap_or(0.0),
                    |w| images.get(&(id, w)).cloned(),
                )
            }
            None => Vec::new(),
        };
        let interaction = PaintInteraction {
            hovered: self.hovered,
            pressed: self.pressed,
        };
        let scale = self.scale;
        let Some(frame) = self.frames.get_mut(&id) else {
            return false;
        };
        frame.dirty = false;
        painter::paint_placements(
            &mut frame.argb,
            frame.width,
            frame.height,
            scale,
            &placements,
            interaction,
        );
        for (chunk, word) in frame.rgba.chunks_exact_mut(4).zip(frame.argb.iter()) {
            chunk[0] = (word >> 16) as u8;
            chunk[1] = (word >> 8) as u8;
            chunk[2] = *word as u8;
            chunk[3] = (word >> 24) as u8;
        }
        true
    }

    /// The last painted frame of `window`, RGBA, `frame_size` pixels.
    pub fn frame(&self, window: u64) -> Option<&[u8]> {
        let id = self.window_id(window).ok()?;
        self.frames.get(&id).map(|f| f.rgba.as_slice())
    }

    pub fn frame_size(&self, window: u64) -> Option<(u32, u32)> {
        let id = self.window_id(window).ok()?;
        self.frames.get(&id).map(|f| (f.width, f.height))
    }

    /// The window's size in logical pixels, as the app sees it.
    pub fn logical_size(&self, window: u64) -> Option<(u32, u32)> {
        let id = self.window_id(window).ok()?;
        let record = self.adapter.window(id).ok()??;
        Some((record.size.width, record.size.height))
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// What is on screen in `window`, as the painter was handed it: each
    /// placement's widget, kind, words, state and rectangle (logical
    /// pixels, scroll applied). The proofs click by what a person sees, and
    /// it is what a screen reader in a tab will read (Phase 2).
    pub fn on_screen(&self, window: u64) -> Value {
        let Ok(id) = self.window_id(window) else {
            return json!([]);
        };
        let Some((tree, layout)) = self.layout(id) else {
            return json!([]);
        };
        let offsets = &self.offsets;
        let placed = krate_lowering::placements(
            &tree,
            &layout,
            |s| offsets.get(&(id, s)).copied().unwrap_or(0.0),
            |_| None,
        );
        Value::Array(
            placed
                .iter()
                .map(|p| {
                    json!({
                        "id": p.widget.get().to_string(),
                        "kind": format!("{:?}", p.kind),
                        "label": p.label,
                        "checked": p.checked,
                        "value": p.value,
                        "selected": p.selection.is_some(),
                        "x": p.x, "y": p.y, "w": p.width, "h": p.height,
                        "clip": p.clip.map(|(x, y, w, h)| json!([x, y, w, h])),
                    })
                })
                .collect(),
        )
    }

    /// A widget's rectangle on screen, in logical pixels (for tests and for
    /// the page placing the phone's keyboard).
    pub fn widget_rect(&self, window: u64, widget: u64) -> Option<(f32, f32, f32, f32)> {
        let id = self.window_id(window).ok()?;
        let (tree, layout) = self.layout(id)?;
        let widget = WidgetId::new(widget).ok()?;
        let rect = krate_layout::absolute_rect(&tree, &layout, widget)?;
        let shift = krate_lowering::nearest_scroll_ancestor(&tree, widget)
            .and_then(|s| self.offsets.get(&(id, s)).copied())
            .unwrap_or(0.0);
        Some((rect.x, rect.y - shift, rect.width, rect.height))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: u64, parent: Option<u64>, kind: &str, extra: Value) -> String {
        let mut v = json!({ "id": id.to_string(), "kind": kind,
            "style": { "grow": 0.0, "padding": 0.0 } });
        if let Some(p) = parent {
            v["parent"] = Value::String(p.to_string());
        }
        if let Value::Object(map) = extra {
            for (k, val) in map {
                v[k] = val;
            }
        }
        v.to_string()
    }

    fn drain(ui: &mut WebUi) -> Vec<Value> {
        let mut out = Vec::new();
        while let Some(e) = ui.next_event() {
            out.push(serde_json::from_str(&e).unwrap());
        }
        out
    }

    fn form(ui: &mut WebUi) -> u64 {
        let win = ui.create_window("Form", 320, 240, (320, 240)).unwrap();
        ui.show_window(win).unwrap();
        ui.set_root(
            win,
            &node(
                1,
                None,
                "stack",
                json!({ "style": { "grow": 0.0, "padding": 12.0 } }),
            ),
        )
        .unwrap();
        ui.upsert(
            win,
            &node(
                2,
                Some(1),
                "text-field",
                json!({ "label": "", "style": { "height": 32.0, "grow": 0.0, "padding": 0.0 } }),
            ),
        )
        .unwrap();
        ui.upsert(
            win,
            &node(
                3,
                Some(1),
                "button",
                json!({ "label": "Add", "style": { "height": 32.0, "grow": 0.0, "padding": 0.0 } }),
            ),
        )
        .unwrap();
        drain(ui);
        win
    }

    /// A press on the text field focuses it; typing goes to it as a key and
    /// then text; the frame paints at the tab's density.
    #[test]
    fn a_press_focuses_the_field_and_typing_reaches_it() {
        let mut ui = WebUi::new(2.0, false);
        let win = form(&mut ui);
        let (x, y, w, h) = ui.widget_rect(win, 2).expect("field");
        ui.pointer(win, x + w / 2.0, y + h / 2.0, 0, true, 0);
        ui.pointer(win, x + w / 2.0, y + h / 2.0, 0, false, 0);
        let events = drain(&mut ui);
        assert_eq!(events[0]["tag"], "pointer");
        assert_eq!(events[0]["val"]["widget"], "2");
        assert!(
            events
                .iter()
                .any(|e| e["tag"] == "focus-changed" && e["val"] == "2"),
            "{events:?}"
        );
        assert!(ui.text_focused(win));
        ui.key(win, "a", true, 0, Some("a"));
        let events = drain(&mut ui);
        assert_eq!(events[0]["tag"], "key");
        assert_eq!(events[0]["val"]["widget"], "2");
        assert_eq!(events[1], json!({ "tag": "text-input", "val": "a" }));
        assert!(ui.render(win));
        assert_eq!(ui.frame_size(win), Some((640, 480)));
        assert!(!ui.render(win), "a clean frame does not repaint");
    }

    /// A tap is a press and a release where the finger lifted; a finger
    /// that moves past the slop scrolls and presses nothing; the text
    /// field's rectangle is there for the page to raise the keyboard.
    #[test]
    fn a_tap_presses_and_a_drag_scrolls() {
        let mut ui = WebUi::new(1.0, false);
        let win = form(&mut ui);
        let (x, y, w, h) = ui.widget_rect(win, 3).expect("button");
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        ui.touch(win, "start", 7, cx, cy);
        assert!(drain(&mut ui).is_empty(), "nothing until it lifts");
        ui.touch(win, "move", 7, cx + 3.0, cy);
        ui.touch(win, "end", 7, cx + 3.0, cy);
        let events = drain(&mut ui);
        let pressed: Vec<_> = events
            .iter()
            .filter(|e| e["tag"] == "pointer")
            .map(|e| (e["val"]["widget"].clone(), e["val"]["pressed"].clone()))
            .collect();
        assert_eq!(
            pressed,
            [(json!("3"), json!(true)), (json!("3"), json!(false))],
            "{events:?}"
        );

        ui.touch(win, "start", 8, cx, cy);
        ui.touch(win, "move", 8, cx, cy - 20.0);
        ui.touch(win, "end", 8, cx, cy - 20.0);
        let events = drain(&mut ui);
        assert!(events.iter().all(|e| e["tag"] != "pointer"), "{events:?}");
        let wheel = events
            .iter()
            .find(|e| e["tag"] == "wheel")
            .expect("a scroll");
        assert_eq!(
            wheel["val"]["dy"],
            json!(20.0),
            "content follows the finger"
        );

        let rects = ui.text_rects(win);
        let (fx, fy, fw, fh) = ui.widget_rect(win, 2).unwrap();
        assert_eq!(rects, vec![fx, fy, fw, fh]);
    }

    /// Enter on a focused button presses it, after the key.
    #[test]
    fn enter_on_a_focused_button_presses_it() {
        let mut ui = WebUi::new(1.0, false);
        let win = form(&mut ui);
        ui.focus(win, 3).unwrap();
        drain(&mut ui);
        ui.key(win, "Enter", true, 0, None);
        let events = drain(&mut ui);
        let tags: Vec<_> = events
            .iter()
            .map(|e| e["tag"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(tags, ["key", "pointer", "pointer"], "{events:?}");
        assert_eq!(events[1]["val"]["widget"], "3");
        assert_eq!(events[1]["val"]["pressed"], true);
        assert_eq!(events[2]["val"]["pressed"], false);
    }

    /// A wheel over a long list scrolls it host-side, and the app still
    /// hears the wheel.
    #[test]
    fn a_wheel_scrolls_a_long_list() {
        let mut ui = WebUi::new(1.0, false);
        let win = ui.create_window("List", 200, 200, (200, 200)).unwrap();
        ui.set_root(win, &node(1, None, "stack", json!({})))
            .unwrap();
        ui.upsert(
            win,
            &node(
                2,
                Some(1),
                "scroll",
                json!({ "style": { "height": 150.0, "grow": 0.0, "padding": 0.0 } }),
            ),
        )
        .unwrap();
        for i in 0..40u64 {
            ui.upsert(win, &node(10 + i, Some(2), "text", json!({ "label": format!("Row {i}"), "style": { "height": 20.0, "grow": 0.0, "padding": 0.0 } })))
                .unwrap();
        }
        drain(&mut ui);
        let before = ui.widget_rect(win, 10).unwrap().1;
        ui.wheel(win, 50.0, 50.0, 0.0, 60.0, 0);
        let after = ui.widget_rect(win, 10).unwrap().1;
        assert_eq!(after, before - 60.0);
        let events = drain(&mut ui);
        assert_eq!(events[0]["tag"], "wheel");
        assert!(ui.render(win));
    }

    /// Light and dark follow the browser: the background changes and the app
    /// hears `theme-changed`.
    #[test]
    fn dark_mode_follows_the_browser() {
        let mut ui = WebUi::new(1.0, false);
        let win = form(&mut ui);
        ui.render(win);
        let light = ui.frame(win).unwrap()[..4].to_vec();
        ui.theme(true);
        assert_eq!(
            drain(&mut ui),
            vec![json!({ "tag": "theme-changed", "val": "dark" })]
        );
        assert!(ui.render(win));
        let dark = ui.frame(win).unwrap()[..4].to_vec();
        assert!(
            u32::from(dark[0]) + u32::from(dark[1]) < u32::from(light[0]) + u32::from(light[1])
        );
        ui.theme(false);
    }

    /// A node that breaks the shared rules is refused in the WIT's words.
    #[test]
    fn a_bad_node_is_refused_in_the_wit_shape() {
        let mut ui = WebUi::new(1.0, false);
        let win = ui.create_window("x", 10, 10, (10, 10)).unwrap();
        let err = ui
            .set_root(
                win,
                &node(
                    1,
                    None,
                    "button",
                    json!({ "textCursor": { "cursor": 0, "anchor": 0 } }),
                ),
            )
            .unwrap_err();
        assert_eq!(err["tag"], "unsupported");
        assert!(err["val"].as_str().unwrap().contains("caret"));
        assert_eq!(
            ui.upsert(99, &node(2, Some(1), "text", json!({})))
                .unwrap_err()["tag"],
            "invalid-window"
        );
    }

    #[test]
    fn dom_keys_become_krate_key_names() {
        assert_eq!(key_name(" ").as_deref(), Some("Space"));
        assert_eq!(key_name("Enter").as_deref(), Some("Enter"));
        assert_eq!(key_name("ArrowLeft").as_deref(), Some("ArrowLeft"));
        assert_eq!(key_name("q").as_deref(), Some("q"));
        assert_eq!(key_name("Dead"), None);
        assert_eq!(key_name("MediaPlayPause"), None);
    }
}
