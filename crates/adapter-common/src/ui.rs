//! Shared Phase 3 UI adapter draft types.
//!
//! This module is intentionally host-neutral. It does not create real native
//! windows yet. It gives the Linux, macOS, and Windows adapter work a shared
//! shape for ids, size validation, lifecycle state, and early event routing.

use std::sync::Arc;
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

/// Wheel or trackpad scroll event after runtime routing.
///
/// Deltas are logical pixels, positive-down and positive-right, already
/// normalized by the backend from whatever the platform reported.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WheelEvent {
    pub window: WindowId,
    pub widget: Option<WidgetId>,
    pub x: f32,
    pub y: f32,
    pub dx: f32,
    pub dy: f32,
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

static KEPT_FROM_CAPTURE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Keep the windows this process makes from now on out of screenshots,
/// recordings and screen sharing, where the system allows it: what an
/// organisation's `[apps] screen_capture = false` asks (G17). Set by the
/// runtime before each window it creates.
pub fn keep_windows_from_capture(kept: bool) {
    KEPT_FROM_CAPTURE.store(kept, std::sync::atomic::Ordering::SeqCst);
}

static CLIPBOARD_KEPT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// The organisation's clipboard rules for this process (G17), for what a
/// native text control does on its own: a Mac's Edit menu then offers no
/// Paste when reading is blocked and no Cut or Copy when writing is.
pub fn keep_clipboard(read_blocked: bool, write_blocked: bool) {
    let bits = u8::from(read_blocked) | (u8::from(write_blocked) << 1);
    CLIPBOARD_KEPT.store(bits, std::sync::atomic::Ordering::SeqCst);
}

/// (reading blocked, writing blocked).
pub fn clipboard_kept() -> (bool, bool) {
    let bits = CLIPBOARD_KEPT.load(std::sync::atomic::Ordering::SeqCst);
    (bits & 1 != 0, bits & 2 != 0)
}

/// Whether a window made now is kept out of captures.
pub fn windows_kept_from_capture() -> bool {
    KEPT_FROM_CAPTURE.load(std::sync::atomic::Ordering::SeqCst)
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
    /// An image view, carrying decoded pixels in `WidgetNode::pixels`.
    Image,
    Canvas,
    /// A container whose children all fill it and paint back to front.
    ///
    /// Every other container divides its space between its children. This one
    /// gives each of them all of it, so a HUD sits over a game and a caption
    /// sits over a photo. Without it a 3D app could not have a HUD at all: a
    /// canvas holding the scene and a canvas holding the overlay were laid out
    /// one above the other, each getting half the window.
    Overlay,
}

/// The largest picture one image widget may carry.
///
/// 64 megapixels is past any photograph a person would open and still a bound:
/// without one, a width and height multiplied together overflow into an
/// allocation that takes the host down, and the app that sent them is the one
/// we do not trust.
pub const MAX_IMAGE_PIXELS: u64 = 64 * 1024 * 1024;

/// Decoded picture data for an image widget: RGBA, four bytes per pixel,
/// top row first.
///
/// Not PNG or JPEG. The app decodes its own file inside the sandbox and sends
/// the result, so no image parser sits in the host where a malformed download
/// would reach it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImagePixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl ImagePixels {
    /// Build a picture, checking that the bytes match the size claimed.
    ///
    /// Every host indexes this buffer by row and column. A buffer shorter than
    /// its stated width and height would read past the end on the last row, so
    /// the check happens once here rather than three times in three languages.
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self, UiAdapterError> {
        if width == 0 || height == 0 {
            return Err(UiAdapterError::Unsupported(format!(
                "an image must have a non-zero width and height, got {width}x{height}"
            )));
        }
        let count = u64::from(width) * u64::from(height);
        if count > MAX_IMAGE_PIXELS {
            return Err(UiAdapterError::Unsupported(format!(
                "an image may be at most {MAX_IMAGE_PIXELS} pixels, got {width}x{height}"
            )));
        }
        let expected = count * 4;
        if rgba.len() as u64 != expected {
            return Err(UiAdapterError::Unsupported(format!(
                "a {width}x{height} image needs exactly {expected} RGBA bytes, got {}",
                rgba.len()
            )));
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }
}

/// Minimal layout hints attached to a widget node.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WidgetStyle {
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub grow: f32,
    pub padding: f32,
    /// How this widget's label is inked, when the app asked for something
    /// other than the host's ordinary label style.
    pub text: Option<TextStyle>,
    /// Where this widget sits inside its parent.
    pub place: Placement,
    /// How this widget's box is painted. `None` means the host's own look.
    pub r#box: Option<BoxStyle>,
}

/// Where a widget sits inside the space its parent gives it.
///
/// Every child of an `overlay` shares one grid cell, so without this each
/// starts at its top-left and padding is the only lever -- and padding
/// changes the PARENT'S size, which in an overlay resizes everything else in
/// that cell (K-410).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Placement {
    /// Wherever the container would have put it.
    #[default]
    Default,
    TopLeft,
    TopCentre,
    TopRight,
    CentreLeft,
    Centre,
    CentreRight,
    BottomLeft,
    BottomCentre,
    BottomRight,
}

impl Placement {
    /// How this sits down the cross axis and across the main one, as a pair
    /// of (row, column) alignments.
    ///
    /// Returned as plain directions rather than as a layout engine's type:
    /// this crate is host-neutral and depends on no layout engine. The layout
    /// crate maps these onto taffy, which already places a grid child in its
    /// cell with `align_self` and `justify_self` -- so an overlay, being a
    /// grid already, needs no new concept (K-410).
    pub fn axes(self) -> Option<(Align, Align)> {
        let pair = match self {
            Self::Default => return None,
            Self::TopLeft => (Align::Start, Align::Start),
            Self::TopCentre => (Align::Start, Align::Centre),
            Self::TopRight => (Align::Start, Align::End),
            Self::CentreLeft => (Align::Centre, Align::Start),
            Self::Centre => (Align::Centre, Align::Centre),
            Self::CentreRight => (Align::Centre, Align::End),
            Self::BottomLeft => (Align::End, Align::Start),
            Self::BottomCentre => (Align::End, Align::Centre),
            Self::BottomRight => (Align::End, Align::End),
        };
        Some(pair)
    }
}

/// One axis of a placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Start,
    Centre,
    End,
}

/// An 8-bit colour, as a widget names one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    /// The colour as the painters want it: `0xAARRGGBB`.
    pub fn argb(self) -> u32 {
        (u32::from(self.a) << 24)
            | (u32::from(self.r) << 16)
            | (u32::from(self.g) << 8)
            | u32::from(self.b)
    }
}

/// How a label is inked, for text drawn over something the app does not
/// control.
///
/// The `outline` is the part that matters for a HUD. A light glyph with a
/// dark outline reads at about 19:1 against its own outline whatever is
/// behind it; where the outline vanishes (dark asphalt, shadow) the light
/// glyph is already high-contrast against that background by itself. One
/// fixed colour cannot do that, which is what K-402 was.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyle {
    /// Ink colour, or the host's ordinary label colour when `None`.
    pub color: Option<Color>,
    /// Outline colour, or no outline when `None`.
    pub outline: Option<Color>,
    /// Outline thickness in logical pixels.
    pub outline_width: f32,
    /// Font size in logical pixels, or the host's default when `None`.
    pub size: Option<f32>,
    /// Draw the label bold.
    pub bold: bool,
}

impl TextStyle {
    /// The widest outline a host will draw.
    ///
    /// A guest names this number, so it is bounded here rather than trusted:
    /// the painter strokes the glyph once per offset in a ring of this
    /// radius, so an unbounded width is an unbounded amount of host work per
    /// label per frame. Three logical pixels is already a heavier outline
    /// than any HUD wants.
    pub const MAX_OUTLINE_WIDTH: f32 = 3.0;

    /// The range of font sizes a host will honour.
    ///
    /// Below the floor a label is not readable and is usually a unit mistake;
    /// above the ceiling one label can cost a whole frame to lay out.
    pub const MIN_SIZE: f32 = 6.0;
    pub const MAX_SIZE: f32 = 96.0;

    /// Clamp everything a guest can name into what a host will draw.
    ///
    /// Non-finite values fall back to the default rather than being clamped:
    /// a NaN width is not a big width, it is an app bug, and passing it on
    /// would put a NaN into a layout.
    pub fn sanitized(self) -> Self {
        let outline_width = if self.outline_width.is_finite() {
            self.outline_width.clamp(0.0, Self::MAX_OUTLINE_WIDTH)
        } else {
            0.0
        };
        let size = self.size.and_then(|size| {
            size.is_finite()
                .then(|| size.clamp(Self::MIN_SIZE, Self::MAX_SIZE))
        });
        Self {
            color: self.color,
            // An outline of zero width is no outline, however it is coloured.
            outline: (outline_width > 0.0).then_some(self.outline).flatten(),
            outline_width,
            size,
            bold: self.bold,
        }
    }
}

/// How a widget's box is painted: background, border and corner radius.
///
/// Containers were not painted at all and painted kinds took hard-coded
/// colours, so an app could not give a card a background or a panel a border.
/// The only way to draw one was to abandon the widget set for a canvas and
/// hand-draw the interface -- giving up real controls, native text editing,
/// host scrolling and accessibility to get a rounded corner.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BoxStyle {
    /// Fill behind the content. `None` means the host's own colour for that
    /// kind, so a button stays the host's button colour.
    pub background: Option<Color>,
    /// Border colour. `None` means no border however `border_width` is set.
    pub border: Option<Color>,
    /// Border thickness in logical pixels.
    pub border_width: f32,
    /// Corner radius in logical pixels, the same on all four corners.
    pub corner_radius: f32,
}

impl BoxStyle {
    /// The widest border a host will draw.
    ///
    /// A guest names this and the painter insets the fill by it on every side,
    /// so an unbounded width is an unbounded inset -- past half the widget it
    /// would swallow the content entirely. Sixteen logical pixels is already
    /// heavier than any real border.
    pub const MAX_BORDER_WIDTH: f32 = 16.0;

    /// The largest corner radius a host will honour.
    ///
    /// Bounded for the same reason, and because a radius past half the
    /// shorter side is not a rounder rectangle, it is a lozenge -- the
    /// painter clamps to that anyway, so naming a ceiling here keeps the
    /// number an app is told it gets close to the pixels it gets.
    pub const MAX_CORNER_RADIUS: f32 = 64.0;

    /// Clamp everything a guest can name into what a host will draw.
    ///
    /// Non-finite values fall back to zero rather than being clamped, the same
    /// way `TextStyle::sanitized` treats them: a NaN radius is not a big
    /// radius, it is an app bug, and passing it into the painter would put a
    /// NaN into a path.
    pub fn sanitized(self) -> Self {
        let border_width = if self.border_width.is_finite() {
            self.border_width.clamp(0.0, Self::MAX_BORDER_WIDTH)
        } else {
            0.0
        };
        let corner_radius = if self.corner_radius.is_finite() {
            self.corner_radius.clamp(0.0, Self::MAX_CORNER_RADIUS)
        } else {
            0.0
        };
        Self {
            background: self.background,
            // A border of zero width is no border, however it is coloured --
            // the same rule an outline of zero width follows.
            border: (border_width > 0.0).then_some(self.border).flatten(),
            border_width,
            corner_radius,
        }
    }

    /// Whether this style asks for anything at all.
    ///
    /// A style naming no background and no border paints nothing, so the
    /// painter can skip it rather than filling a transparent rectangle over
    /// whatever is behind.
    pub fn paints_anything(self) -> bool {
        self.background.is_some() || (self.border.is_some() && self.border_width > 0.0)
    }
}

impl Default for WidgetStyle {
    fn default() -> Self {
        Self {
            width: None,
            height: None,
            text: None,
            place: Placement::Default,
            grow: 0.0,
            padding: 0.0,
            r#box: None,
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
    // Tabs belongs here for the same reason as a list: a tab strip without a
    // selected tab is not a tab strip. It is what decides which panel is laid
    // out and which takes no space.
    matches!(
        kind,
        WidgetKind::ListView | WidgetKind::TreeView | WidgetKind::Tabs
    )
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
    /// Caret and selection (byte offsets into `label`) for a text widget the
    /// app edits itself. Painting hosts draw the caret and selection wash;
    /// native-control hosts ignore it. `None` leaves the text a passive label.
    pub text_cursor: Option<(u32, u32)>,
    /// Decoded picture for an image widget. Every other kind rejects one.
    pub pixels: Option<ImagePixels>,
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
            text_cursor: None,
            pixels: None,
        }
    }

    /// Attach a text caret and selection (cursor, anchor) for a text widget
    /// the app edits itself. Only text kinds may carry one.
    pub fn with_text_cursor(mut self, cursor: u32, anchor: u32) -> Result<Self, UiAdapterError> {
        if !matches!(self.kind, WidgetKind::TextArea | WidgetKind::TextField) {
            return Err(UiAdapterError::Unsupported(format!(
                "widget kind {:?} cannot carry a text caret",
                self.kind
            )));
        }
        self.text_cursor = Some((cursor, anchor));
        Ok(self)
    }

    /// Attach a decoded picture. Only an image widget may carry one.
    pub fn with_pixels(mut self, pixels: ImagePixels) -> Result<Self, UiAdapterError> {
        if self.kind != WidgetKind::Image {
            return Err(UiAdapterError::Unsupported(format!(
                "widget kind {:?} cannot carry an image",
                self.kind
            )));
        }
        self.pixels = Some(pixels);
        Ok(self)
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

    /// Insert or replace a widget.
    ///
    /// The root may be updated in place -- its label, style or kind -- and
    /// keeps its children (K-392). It used to be refused as a duplicate, so
    /// an app restyling its own root had to call `set_root`, which throws the
    /// whole tree away.
    pub fn upsert(&mut self, node: WidgetNode) -> Result<(), UiAdapterError> {
        if node.id == self.root {
            if node.parent.is_some() {
                // The root cannot be moved under another node.
                return Err(UiAdapterError::DuplicateWidget { id: node.id.get() });
            }
            let mut node = node;
            node.style = node.style.validate()?;
            self.nodes.insert(node.id, node);
            return Ok(());
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
    Wheel(WheelEvent),
    Key(KeyEvent),
    TextInput(TextInputEvent),
    TextChanged(TextChangedEvent),
    /// The person dropped a file onto a window.
    ///
    /// Carries the PATH, because this is the host side of the boundary. The
    /// runtime turns it into a name and a token before the guest sees it, so
    /// an app never learns where the file lives (K-175).
    FileDropped {
        window: WindowId,
        path: std::path::PathBuf,
    },
    /// A dragged file entered or left a window.
    ///
    /// Nothing about the file is carried: a hover is not consent. The app
    /// learns only that something is being dragged, which is enough to show
    /// a drop target and nothing more.
    FileHovering {
        window: WindowId,
        over: bool,
    },
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
    /// A file was dropped on this window. Carries the path; the runtime turns
    /// it into a name and a token before any guest sees it (K-175).
    FileDropped(std::path::PathBuf),
    /// A dragged file entered (`true`) or left (`false`) this window.
    FileHovering(bool),
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
            WinitWindowNativeEvent::FileDropped(path) => {
                adapter.queue_file_dropped(self.id, path)?;
                Ok(None)
            }
            WinitWindowNativeEvent::FileHovering(over) => {
                adapter.queue_file_hovering(self.id, over)?;
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

    /// Extend the window's content into the title-bar area, keeping the
    /// host's own window controls overlaid. Default is honest refusal: an
    /// adapter that has not wired the native call reports unsupported and
    /// the window keeps its standard chrome (K-117).
    fn set_full_bleed(&self, id: WindowId, enabled: bool) -> Result<(), UiAdapterError> {
        let _ = (id, enabled);
        Err(UiAdapterError::Unsupported(
            "full-bleed windows are not available on this host yet".to_string(),
        ))
    }

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

    /// Queue a dropped file for a window.
    fn queue_file_dropped(
        &self,
        id: WindowId,
        path: std::path::PathBuf,
    ) -> Result<(), UiAdapterError>;

    /// Queue a drag entering or leaving a window.
    fn queue_file_hovering(&self, id: WindowId, over: bool) -> Result<(), UiAdapterError>;

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
    /// Caret and selection (cursor, anchor) as byte offsets into `label`, for
    /// a text widget the app edits itself. Painting hosts draw a caret bar at
    /// `cursor` and a wash over `[min, max)`; `None` paints a passive label.
    pub text_cursor: Option<(u32, u32)>,
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
    /// Semantic role carried from the widget node ("heading", "status",
    /// "textbox", "option", "button"). Styling stays out of the guest
    /// contract; hosts may use the role to pick native typography.
    pub role: Option<String>,
    /// Decoded picture for an image widget, shared rather than copied.
    ///
    /// Placements are rebuilt every frame. A full-size photograph is a
    /// quarter of a gigabyte of pixels, so cloning one per frame would burn
    /// more time and memory than drawing it; the `Arc` makes a frame cost a
    /// refcount instead.
    pub pixels: Option<Arc<ImagePixels>>,
    /// How this placement's label is inked, when the app asked for something
    /// other than the host's ordinary label style. Already sanitized.
    pub text: Option<TextStyle>,
    /// How this placement's box is painted, when the app asked for one.
    /// Already sanitized.
    pub r#box: Option<BoxStyle>,
}

impl Default for WidgetPlacement {
    /// An empty placement, for tests to build on with `..Default::default()`.
    ///
    /// Added because every new field on this struct broke six test fixtures
    /// that spell out all of them. A fixture should name the fields it cares
    /// about and inherit the rest.
    fn default() -> Self {
        Self {
            widget: WidgetId::new(1).expect("1 is a valid widget id"),
            kind: WidgetKind::Text,
            label: None,
            checked: None,
            value: None,
            selection: None,
            text_cursor: None,
            clip: None,
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
            clickable: false,
            role: None,
            pixels: None,
            text: None,
            r#box: None,
        }
    }
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
/// containers and adjusts host-side scroll offsets, and also queues a
/// portable `Wheel` event so an app that draws its own content can scroll
/// itself. Raw samples never enter the UI event queue directly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawWheelSample {
    /// Window the sample belongs to.
    pub window: WindowId,
    /// Cursor X in logical pixels at wheel time.
    pub x: f32,
    /// Cursor Y in logical pixels at wheel time.
    pub y: f32,
    /// Horizontal scroll delta in logical pixels (positive scrolls right).
    pub dx: f32,
    /// Vertical scroll delta in logical pixels (positive scrolls down).
    pub dy: f32,
    /// Modifier state while the wheel moved.
    ///
    /// Carried rather than dropped because ctrl-scroll is zoom on every
    /// desktop, and an app cannot tell a zoom from a scroll without it.
    pub modifiers: Modifiers,
}

/// The mouse as a game asks about it (IC-909): where it is, which buttons
/// are down, how far it moved, and whether the app holds it.
///
/// Every native adapter keeps one per window and feeds it from its own
/// events; the rules live here once, so a captured pointer releases the same
/// way on every system.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PointerTracker {
    /// Last position over the window, logical pixels; `None` once it leaves.
    pub position: Option<(f32, f32)>,
    /// Held buttons: bit 0 primary, bit 1 secondary, bit 2 middle.
    pub buttons: u8,
    /// Motion the app has not taken yet, logical pixels.
    pub motion: (f32, f32),
    /// The app holds the pointer.
    pub captured: bool,
}

impl PointerTracker {
    /// The bit for a button in `buttons`, or `None` for one with no bit.
    pub fn button_bit(button: PointerButton) -> Option<u8> {
        match button {
            PointerButton::Primary => Some(1),
            PointerButton::Secondary => Some(2),
            PointerButton::Middle => Some(4),
            PointerButton::Other => None,
        }
    }

    /// The pointer is now at `(x, y)`. Outside capture the step from the last
    /// position is motion; while captured the cursor is pinned and only the
    /// device's own motion ([`Self::moved_by`]) counts.
    pub fn moved_to(&mut self, x: f32, y: f32) {
        if self.captured {
            return;
        }
        if let Some((px, py)) = self.position {
            self.motion.0 += x - px;
            self.motion.1 += y - py;
        }
        self.position = Some((x, y));
    }

    /// The mouse itself moved by `(dx, dy)`. Counted only while captured,
    /// where nothing else reports motion; outside capture the position does.
    pub fn moved_by(&mut self, dx: f32, dy: f32) {
        if self.captured {
            self.motion.0 += dx;
            self.motion.1 += dy;
        }
    }

    /// The pointer left the window.
    pub fn left(&mut self) {
        if !self.captured {
            self.position = None;
        }
    }

    /// A button went down or up.
    pub fn button(&mut self, button: PointerButton, pressed: bool) {
        if let Some(bit) = Self::button_bit(button) {
            if pressed {
                self.buttons |= bit;
            } else {
                self.buttons &= !bit;
            }
        }
    }

    /// Whether a button is down.
    pub fn held(&self, button: PointerButton) -> bool {
        Self::button_bit(button).is_some_and(|bit| self.buttons & bit != 0)
    }

    /// Hand over the motion so far and start again from zero.
    pub fn take_motion(&mut self) -> (f32, f32) {
        std::mem::take(&mut self.motion)
    }

    /// Focus went elsewhere: let go of the pointer and every button, so
    /// nothing sticks while the person is in another app.
    pub fn focus_lost(&mut self) {
        self.captured = false;
        self.buttons = 0;
        self.motion = (0.0, 0.0);
    }
}

/// A canvas frame handed to a GPU-capable adapter as draw calls instead of
/// pixels. Opaque here: the runtime defines the op list, the adapter's
/// renderer consumes it, and this crate only carries it between them.
pub type CanvasListHandle = std::sync::Arc<dyn std::any::Any + Send + Sync>;

/// Why a Bluetooth call did not work (IC-917).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BtError {
    NotAllowed,
    Unavailable,
    NotFound,
    Disconnected,
    Failed(String),
}

/// A Bluetooth device the person picked, by the handle the host gave it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BtDevice {
    pub handle: u64,
    pub name: String,
}

/// A value a device sent on its own: service, characteristic, data.
pub type BtNotification = (String, String, Vec<u8>);

/// One fix of where the phone is (IC-917).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Position {
    pub latitude: f64,
    pub longitude: f64,
    /// Metres the fix may be off by.
    pub accuracy: f32,
    pub altitude: Option<f32>,
    pub speed: Option<f32>,
    pub course: Option<f32>,
    /// How old the fix is.
    pub age_ms: u64,
}

/// What asking where the phone is answers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LocationAnswer {
    Here(Position),
    /// The phone's own prompt was refused, or location is restricted.
    NotAllowed,
    /// No location on this device, or it is switched off.
    Unavailable,
    /// Still looking.
    Finding,
}

/// One reading of a phone's motion sensors (IC-917), in the screen's frame:
/// x right, y down, z toward the person.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct MotionReading {
    /// Which way is down, in g.
    pub gravity: (f32, f32, f32),
    /// The push given the phone, gravity removed, in g.
    pub acceleration: (f32, f32, f32),
    /// Turning about each axis, in degrees a second.
    pub rotation: (f32, f32, f32),
    /// Degrees clockwise from magnetic north the top of the screen faces.
    pub heading: Option<f32>,
}

/// How the interface is turned, as UIKit and Android both number the four
/// ways up (IC-917).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenTurn {
    /// Held upright.
    Upright,
    /// Upside down.
    UpsideDown,
    /// Turned so the phone's top points left (iOS: home button on the right).
    TopLeft,
    /// Turned so the phone's top points right (iOS: home button on the left).
    TopRight,
}

/// A vector in a phone's own frame (x right and y up the phone held
/// upright, z out of the glass) in the screen's frame for how the interface
/// is turned: x right, y down, z out.
pub fn device_to_screen(turn: ScreenTurn, (dx, dy, dz): (f64, f64, f64)) -> (f32, f32, f32) {
    let (sx, sy) = match turn {
        ScreenTurn::Upright => (dx, -dy),
        ScreenTurn::UpsideDown => (-dx, dy),
        ScreenTurn::TopLeft => (-dy, -dx),
        ScreenTurn::TopRight => (dy, dx),
    };
    (sx as f32, sy as f32, dz as f32)
}

/// A compass heading of the phone's top as the heading of the screen's top.
pub fn heading_to_screen(turn: ScreenTurn, heading: f64) -> f32 {
    let offset = match turn {
        ScreenTurn::Upright => 0.0,
        ScreenTurn::UpsideDown => 180.0,
        ScreenTurn::TopLeft => 90.0,
        ScreenTurn::TopRight => 270.0,
    };
    ((heading + offset).rem_euclid(360.0)) as f32
}

#[cfg(test)]
mod motion_tests {
    use super::*;

    #[test]
    fn down_is_down_the_screen_however_the_phone_is_turned() {
        // Held upright in each way, gravity points to the phone's bottom
        // edge as the person sees it: down the screen, (0, 1).
        let cases = [
            (ScreenTurn::Upright, (0.0, -1.0, 0.0)),
            (ScreenTurn::UpsideDown, (0.0, 1.0, 0.0)),
            (ScreenTurn::TopLeft, (-1.0, 0.0, 0.0)),
            (ScreenTurn::TopRight, (1.0, 0.0, 0.0)),
        ];
        for (turn, device) in cases {
            assert_eq!(device_to_screen(turn, device), (0.0, 1.0, 0.0), "{turn:?}");
        }
        // Lying face up: down is into the table, through the back.
        assert_eq!(
            device_to_screen(ScreenTurn::TopLeft, (0.0, 0.0, -1.0)),
            (0.0, 0.0, -1.0)
        );
        // Tilting the phone's right edge down, upright: down leans right.
        assert_eq!(
            device_to_screen(ScreenTurn::Upright, (0.5, -0.8, 0.0)).0,
            0.5
        );
    }

    #[test]
    fn the_heading_is_where_the_top_of_the_screen_faces() {
        // The phone's top faces north (0). Turned so its top points left,
        // the screen's top is the phone's right side: 90 degrees on.
        assert_eq!(heading_to_screen(ScreenTurn::Upright, 0.0), 0.0);
        assert_eq!(heading_to_screen(ScreenTurn::TopLeft, 0.0), 90.0);
        assert_eq!(heading_to_screen(ScreenTurn::TopRight, 0.0), 270.0);
        assert_eq!(heading_to_screen(ScreenTurn::UpsideDown, 270.0), 90.0);
    }
}

/// Where a web page inside an app is and what it is doing (IC-927).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WebPageState {
    pub address: String,
    pub title: String,
    pub loading: bool,
    pub problem: String,
}

/// What kind of thing a described element is (IC-926).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessRole {
    Button,
    Link,
    Heading,
    Text,
    Image,
    Toggle,
    Adjustable,
    Field,
}

/// One thing on screen, as a screen reader is told it (IC-926).
#[derive(Clone, Debug, PartialEq)]
pub struct AccessElement {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub role: AccessRole,
    pub label: String,
    pub value: String,
    pub checked: Option<bool>,
    pub enabled: bool,
}

impl AccessElement {
    /// What a screen reader says for a widget Krate draws, so widget apps
    /// are described without doing anything; None for what it skips
    /// (containers, an unlabelled spacer).
    pub fn for_placement(placement: &WidgetPlacement) -> Option<Self> {
        let role = match placement.kind {
            WidgetKind::Button => AccessRole::Button,
            WidgetKind::Text => AccessRole::Text,
            WidgetKind::Checkbox | WidgetKind::Switch | WidgetKind::Radio => AccessRole::Toggle,
            WidgetKind::Slider | WidgetKind::Progress => AccessRole::Adjustable,
            WidgetKind::TextField | WidgetKind::TextArea => AccessRole::Field,
            WidgetKind::Image => AccessRole::Image,
            _ => return None,
        };
        let label = placement.label.clone().unwrap_or_default();
        if label.trim().is_empty() && !matches!(role, AccessRole::Adjustable | AccessRole::Field) {
            return None;
        }
        Some(Self {
            x: placement.x,
            y: placement.y,
            width: placement.width,
            height: placement.height,
            role,
            label,
            value: placement
                .value
                .map(|v| format!("{:.0}%", (v.clamp(0.0, 1.0)) * 100.0))
                .unwrap_or_default(),
            checked: placement.checked,
            enabled: true,
        })
    }
}

/// How the person asked things to look (IC-925).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Appearance {
    /// The system is in dark mode.
    pub dark: bool,
    /// The person asked for more contrast.
    pub high_contrast: bool,
    /// The person's text size against the default: 1.0 by default.
    pub text_scale: f32,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            dark: false,
            high_contrast: false,
            text_scale: 1.0,
        }
    }
}

/// Krate's own Close or Restart was not answered (IC-934): the engine ends
/// the app at its next safepoint, whatever it is doing. Process-wide, as the
/// one running app is.
static END_APP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// End the running app at its next safepoint: an app that stopped calling
/// the host cannot be asked to close, so Krate's controls end it instead.
pub fn end_running_app() {
    END_APP.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Whether Krate has asked the engine to end the running app.
pub fn app_ending() -> bool {
    END_APP.load(std::sync::atomic::Ordering::Relaxed)
}

/// A new app begins: an end asked for the last one is not meant for it.
pub fn clear_app_ending() {
    END_APP.store(false, std::sync::atomic::Ordering::Relaxed);
}

/// The look Krate's own widgets are drawn in, for every window of this
/// process: set by the runtime each frame from the person's settings, read
/// by layout (text sizes) and the painters (colours and text sizes).
static LOOK_DARK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static LOOK_CONTRAST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static LOOK_TEXT_SCALE: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0x3F80_0000);

/// Draw Krate's widgets in this look from now on (IC-925).
pub fn set_widget_look(look: Appearance) {
    use std::sync::atomic::Ordering::Relaxed;
    LOOK_DARK.store(look.dark, Relaxed);
    LOOK_CONTRAST.store(look.high_contrast, Relaxed);
    LOOK_TEXT_SCALE.store(look.text_scale.clamp(0.5, 4.0).to_bits(), Relaxed);
}

/// The look Krate's widgets are drawn in.
pub fn widget_look() -> Appearance {
    use std::sync::atomic::Ordering::Relaxed;
    Appearance {
        dark: LOOK_DARK.load(Relaxed),
        high_contrast: LOOK_CONTRAST.load(Relaxed),
        text_scale: f32::from_bits(LOOK_TEXT_SCALE.load(Relaxed)),
    }
}

impl Appearance {
    /// Read `KRATE_APPEARANCE` as a stand-in for the person's settings:
    /// comma-separated `dark`, `contrast` and `text=1.35`, in any order.
    pub fn parse(text: &str) -> Self {
        let mut look = Self::default();
        for part in text.split(',').map(str::trim) {
            match part {
                "dark" => look.dark = true,
                "light" => look.dark = false,
                "contrast" => look.high_contrast = true,
                _ => {
                    if let Some(scale) = part
                        .strip_prefix("text=")
                        .and_then(|v| v.parse::<f32>().ok())
                    {
                        look.text_scale = scale.clamp(0.5, 4.0);
                    }
                }
            }
        }
        look
    }
}

/// What a phone's tap back means (IC-917): the platform plays its own feel
/// for each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HapticFeel {
    Light,
    Medium,
    Heavy,
    Selection,
    Success,
    Warning,
    Error,
}

/// A screen beyond its size (IC-912): touched or pointed at, and the
/// space at each edge something else covers, in points (top, right,
/// bottom, left).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ScreenKind {
    /// A finger, not a mouse.
    pub touch: bool,
    /// Covered space at the top, right, bottom and left edges.
    pub safe: (f32, f32, f32, f32),
}

pub trait UiAdapter: WindowAdapter {
    /// Whether this adapter renders canvas display lists itself (a GPU
    /// backend). False everywhere the CPU raster remains the renderer.
    fn supports_canvas_lists(&self) -> bool {
        false
    }

    /// Present one canvas frame as a display list. Returns true when the
    /// adapter rendered it -- the runtime then skips the CPU raster
    /// publish for that frame entirely.
    fn present_canvas_list(
        &self,
        _window: WindowId,
        _widget: WidgetId,
        _list: CanvasListHandle,
    ) -> Result<bool, UiAdapterError> {
        Ok(false)
    }

    /// Park until the display says the next frame is due, or `max`
    /// elapses. Returns true if this adapter actually waited.
    ///
    /// Deliberately separate from `park_for_events`: pacing must wake on
    /// the panel's refresh, never on touches. An earlier build paced on
    /// input and a fast finger outran the drawable pool, stalling the next
    /// frame acquire for a full second (K-090).
    fn park_for_frame(&self, _max: std::time::Duration) -> bool {
        false
    }

    /// Park the calling thread until a native event arrives or `max`
    /// elapses, whichever is first. Returns true if this adapter actually
    /// waited natively -- the host then skips its own blind sleep, so a
    /// touch wakes the app immediately instead of landing mid-nap. The
    /// default does not wait and the host sleeps as it always has.
    fn park_for_events(&self, _max: std::time::Duration) -> bool {
        false
    }

    /// The display scale of a window, physical pixels per logical pixel.
    ///
    /// 1.0 everywhere a host has not wired it. Adapters that know better --
    /// a phone at 2.6x, a HiDPI desktop -- override it, and the canvas
    /// rasterizes at physical resolution while apps keep logical units.
    fn window_scale(&self, _id: WindowId) -> f32 {
        1.0
    }

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

    /// Queue a routed wheel event.
    ///
    /// Required rather than defaulted: a default would let an adapter compile
    /// while silently swallowing every scroll, which is the exact failure this
    /// event exists to end.
    fn queue_wheel_event(&self, event: WheelEvent) -> Result<(), UiAdapterError>;

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

    /// The pointer of a window, as its tracker stands now (IC-909). Hosts
    /// without native input answer an empty tracker: nowhere, nothing held.
    fn pointer(&self, _window: WindowId) -> PointerTracker {
        PointerTracker::default()
    }

    /// Hand over a window's pointer motion since the last call.
    fn take_pointer_motion(&self, _window: WindowId) -> (f32, f32) {
        (0.0, 0.0)
    }

    /// Every finger on a window now, as (id, x, y) in logical pixels. Hosts
    /// without a touch screen have none.
    fn touches(&self, _window: WindowId) -> Vec<(u64, f32, f32)> {
        Vec::new()
    }

    /// The app about to run lays itself out for any screen (it imports
    /// `krate:ui/screen`, IC-912), so a phone may follow the way it is held
    /// rather than turning to the shape of the window the app asked for.
    /// Said once per run, before the app's first window.
    fn set_adapts(&self, _adapts: bool) {}

    /// Show the platform's own picker for a file (`folder` false) or a folder
    /// and return the path the person picked, or None when they cancelled
    /// or the host has no picker. Hosts whose dialogs the runtime opens
    /// itself (the desktops, through rfd) leave this as None. `filter` is the
    /// app's filter string; a host may narrow the picker by it.
    fn choose_path(
        &self,
        _window: WindowId,
        _folder: bool,
        _filter: &str,
    ) -> Option<std::path::PathBuf> {
        None
    }

    /// Space at the top, right, bottom and left edges of `window` that a
    /// widget-tree app's layout keeps clear on this host: a phone's status
    /// bar, gesture bar and camera cutout (K-1060). Apps that draw their
    /// own canvas read the covered edges from krate:ui/screen instead. None
    /// on hosts whose window already excludes them.
    fn widget_insets(&self, _window: WindowId) -> (f32, f32, f32, f32) {
        (0.0, 0.0, 0.0, 0.0)
    }

    /// Whether this device can tap back (IC-917). Desktops cannot.
    fn haptics_available(&self) -> bool {
        false
    }

    /// Tap back with `feel`, or an impact at `strength` (0 to 1) when
    /// `feel` is None. False when there is nothing to tap with.
    fn haptic(&self, _feel: Option<HapticFeel>, _strength: f32) -> bool {
        false
    }

    /// Schedule a reminder for the running app (IC-921): the platform
    /// delivers it with the app closed. `sender` is the app's consented
    /// name for the sender line.
    fn reminder_schedule(
        &self,
        _id: &str,
        _sender: &str,
        _title: &str,
        _body: &str,
        _after_seconds: u64,
        _daily: bool,
    ) -> Result<(), String> {
        Err("reminders that arrive with the app closed are on phones only, for now".to_string())
    }

    /// Cancel one of the running app's reminders.
    fn reminder_cancel(&self, _id: &str) {}

    /// The running app's reminders still to come.
    fn reminder_pending(&self) -> Vec<String> {
        Vec::new()
    }

    /// Show the platform's photo picker for up to `limit` photos; the files
    /// picked, as JPEGs on disk (IC-920). Empty when cancelled. None when
    /// the host has no picker of its own (desktops use the file dialog).
    fn pick_photos(
        &self,
        _window: WindowId,
        _limit: u32,
    ) -> Option<Result<Vec<std::path::PathBuf>, String>> {
        None
    }

    /// Add a PNG or JPEG to the photo library; Err(None) when the person
    /// said no, Err(Some(why)) when it could not.
    fn save_photo(&self, _data: &[u8]) -> Result<(), Option<String>> {
        Err(Some(
            "this device has no photo library Krate can add to".to_string(),
        ))
    }

    /// Open the platform's share sheet with a text, a link and files (name,
    /// bytes); true when sent, false when cancelled (IC-919).
    fn share(
        &self,
        _window: WindowId,
        _text: Option<&str>,
        _url: Option<&str>,
        _files: &[(String, Vec<u8>)],
    ) -> Result<bool, String> {
        Err("this device has no share sheet yet".to_string())
    }

    /// Show a web page in a box of the window (IC-927), going only to the
    /// granted hosts (`net::web_page_allowed`); its handle.
    fn open_web_page(
        &self,
        _window: WindowId,
        _address: &str,
        _frame: (f32, f32, f32, f32),
        _granted: Vec<(String, String)>,
    ) -> Result<u64, String> {
        Err("this device cannot show a web page inside an app yet".to_string())
    }

    /// Move or resize a web page's box.
    fn place_web_page(&self, _page: u64, _frame: (f32, f32, f32, f32)) {}

    /// Send a web page to another address (already judged).
    fn web_page_go(&self, _page: u64, _address: &str) {}

    /// A web page goes back one page.
    fn web_page_back(&self, _page: u64) {}

    /// Where a web page is and what it is doing.
    fn web_page_state(&self, _page: u64) -> Option<WebPageState> {
        None
    }

    /// Take a web page away.
    fn close_web_page(&self, _page: u64) {}

    /// Tell the platform's screen reader what the window shows (IC-926).
    fn describe_accessibility(
        &self,
        _window: WindowId,
        _elements: &[AccessElement],
    ) -> Result<(), UiAdapterError> {
        Ok(())
    }

    /// Whether a screen reader is on (IC-926).
    fn screen_reader_on(&self) -> bool {
        false
    }

    /// How the person asked things to look: dark mode, contrast, text size
    /// (IC-925).
    fn appearance(&self) -> Appearance {
        Appearance::default()
    }

    /// Open the platform's sign-in sheet at `authorize_url` and wait for the
    /// service to send the person back to an address starting `scheme:`;
    /// that address, or None when they closed the sheet (IC-923).
    fn sign_in(
        &self,
        _window: WindowId,
        _authorize_url: &str,
        _scheme: &str,
    ) -> Result<Option<String>, String> {
        Err("this device has no sign-in sheet yet".to_string())
    }

    /// Whether this device has Bluetooth (IC-917).
    fn bluetooth_available(&self) -> bool {
        false
    }

    /// Show the platform's list of nearby devices offering `service` and
    /// connect to the one the person picks; None when they cancel.
    fn bluetooth_choose(
        &self,
        _window: WindowId,
        _service: &str,
        _title: &str,
    ) -> Result<Option<BtDevice>, BtError> {
        Err(BtError::Unavailable)
    }

    /// Read a characteristic of a picked device.
    fn bluetooth_read(
        &self,
        _device: u64,
        _service: &str,
        _characteristic: &str,
    ) -> Result<Vec<u8>, BtError> {
        Err(BtError::Unavailable)
    }

    /// Write a characteristic of a picked device.
    fn bluetooth_write(
        &self,
        _device: u64,
        _service: &str,
        _characteristic: &str,
        _data: &[u8],
        _with_response: bool,
    ) -> Result<(), BtError> {
        Err(BtError::Unavailable)
    }

    /// Be sent a characteristic's changes.
    fn bluetooth_subscribe(
        &self,
        _device: u64,
        _service: &str,
        _characteristic: &str,
    ) -> Result<(), BtError> {
        Err(BtError::Unavailable)
    }

    /// Values sent since the last ask.
    fn bluetooth_notifications(&self, _device: u64) -> Vec<BtNotification> {
        Vec::new()
    }

    /// Whether a picked device is still connected.
    fn bluetooth_connected(&self, _device: u64) -> bool {
        false
    }

    /// Let a picked device go.
    fn bluetooth_disconnect(&self, _device: u64) {}

    /// Whether this device can tell where it is (IC-917).
    fn location_available(&self) -> bool {
        false
    }

    /// Where the device is, starting the search (and the platform's own
    /// prompt) on the first ask. Desktops have none.
    fn location(&self) -> LocationAnswer {
        LocationAnswer::Unavailable
    }

    /// Whether this device can tell how it is held (IC-917).
    fn motion_available(&self) -> bool {
        false
    }

    /// The latest motion reading, starting the sensors on the first ask.
    fn motion(&self) -> Option<MotionReading> {
        None
    }

    /// Hand a file the app has written to the platform's own save sheet,
    /// where the person picks where it goes (phones, K-1032). The file is
    /// moved there, or left behind if they cancel. False when the host has
    /// no such sheet (the desktops save straight to the path picked).
    fn export_file(&self, _window: WindowId, _path: &std::path::Path) -> bool {
        false
    }

    /// What the screen a window is on is like, beyond its size: a finger
    /// or a mouse, and the edges something else covers (IC-912). Desktop
    /// hosts are pointer screens with nothing covering an edge.
    fn screen_kind(&self, _window: WindowId) -> ScreenKind {
        ScreenKind::default()
    }

    /// Show (`true`) or put away (`false`) the on-screen keyboard for a
    /// window. True when a keyboard is now showing. Hosts whose keyboard is
    /// a physical one have nothing to show.
    fn set_text_entry(&self, _window: WindowId, _on: bool) -> bool {
        false
    }

    /// Hold the pointer in a window (`true`) or let it go (`false`).
    ///
    /// Default is honest refusal, so a host that has not wired capture says so
    /// and a game falls back to its menu instead of turning with a cursor that
    /// hits the window's edge.
    fn set_pointer_capture(&self, _window: WindowId, capture: bool) -> Result<(), UiAdapterError> {
        if capture {
            Err(UiAdapterError::Unsupported(
                "holding the pointer is not available on this host yet".to_string(),
            ))
        } else {
            Ok(())
        }
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
    /// In-process clipboard buffer.
    ///
    /// Headless runs and CI have no OS pasteboard, so the draft adapter keeps a
    /// plain string that `write_clipboard_text` sets and `read_clipboard_text`
    /// returns. This lets a clipboard round-trip succeed without any real
    /// system clipboard.
    clipboard: Mutex<String>,
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
    /// The draft host has no real chrome to remove; accepting keeps a
    /// headless verification from failing an app that asks (K-117).
    fn set_full_bleed(&self, id: WindowId, _enabled: bool) -> Result<(), UiAdapterError> {
        if self.registry()?.window(id).is_none() {
            return Err(UiAdapterError::InvalidWindow { id: id.get() });
        }
        Ok(())
    }

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

    fn queue_file_dropped(
        &self,
        id: WindowId,
        path: std::path::PathBuf,
    ) -> Result<(), UiAdapterError> {
        self.registry()?.queue_file_dropped(id, path)
    }

    fn queue_file_hovering(&self, id: WindowId, over: bool) -> Result<(), UiAdapterError> {
        self.registry()?.queue_file_hovering(id, over)
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

    fn queue_wheel_event(&self, event: WheelEvent) -> Result<(), UiAdapterError> {
        self.registry()?.queue_wheel_event(event)
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

    /// Read the in-process clipboard buffer.
    ///
    /// There is no OS pasteboard headless, so this returns whatever the last
    /// `write_clipboard_text` stored (empty until then).
    fn read_clipboard_text(&self) -> Result<String, UiAdapterError> {
        self.clipboard
            .lock()
            .map(|buffer| buffer.clone())
            .map_err(|_| UiAdapterError::Internal("draft clipboard lock is poisoned".to_string()))
    }

    /// Write to the in-process clipboard buffer.
    fn write_clipboard_text(&self, text: &str) -> Result<(), UiAdapterError> {
        self.clipboard
            .lock()
            .map(|mut buffer| {
                text.clone_into(&mut buffer);
            })
            .map_err(|_| UiAdapterError::Internal("draft clipboard lock is poisoned".to_string()))
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
        // The first node of a window, with no parent, IS the root (K-392).
        // Building the tree with upsert_node alone used to fail on its first
        // call -- a bare error and an app that exited 1 with nothing said --
        // because only set_root could create a tree. There is one sensible
        // reading of "add this parentless node to an empty window".
        if node.parent.is_none() && !self.widget_trees.contains_key(&window) {
            return self.set_root(window, node);
        }
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

    /// Queue a dropped file. The path stops at the runtime, which mints a
    /// token before any guest sees it (K-175).
    pub fn queue_file_dropped(
        &mut self,
        id: WindowId,
        path: std::path::PathBuf,
    ) -> Result<(), UiAdapterError> {
        self.open_window(id)?;
        self.events
            .push_back(UiEvent::FileDropped { window: id, path });
        Ok(())
    }

    /// Queue a drag entering or leaving a window.
    pub fn queue_file_hovering(&mut self, id: WindowId, over: bool) -> Result<(), UiAdapterError> {
        self.open_window(id)?;
        self.events
            .push_back(UiEvent::FileHovering { window: id, over });
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

    /// Queue a wheel event after runtime hit testing has assigned a target.
    pub fn queue_wheel_event(&mut self, event: WheelEvent) -> Result<(), UiAdapterError> {
        self.validate_event_target(event.window, event.widget)?;
        self.events.push_back(UiEvent::Wheel(event));
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

/// The fingers on one window, for apps that ask for every finger
/// (krate:ui/touch).
///
/// A finger that lands and lifts between two reads is still reported once:
/// apps read fingers once a frame, and a quick tap -- or one a test injects,
/// which lands and lifts in the same instant -- would otherwise never be
/// seen at all.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FingerTracker {
    down: BTreeMap<u64, Finger>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Finger {
    x: f32,
    y: f32,
    /// An app has read this finger at least once.
    read: bool,
    /// It lifted before any app read it; the next read reports it and drops it.
    lifted: bool,
}

impl FingerTracker {
    /// A finger landed or moved, in logical pixels.
    pub fn touch(&mut self, id: u64, x: f32, y: f32) {
        let finger = self.down.entry(id).or_insert(Finger {
            x,
            y,
            read: false,
            lifted: false,
        });
        finger.x = x;
        finger.y = y;
        finger.lifted = false;
    }

    /// A finger lifted (or the system cancelled it).
    pub fn lift(&mut self, id: u64) {
        match self.down.get_mut(&id) {
            Some(finger) if !finger.read => finger.lifted = true,
            Some(_) => {
                self.down.remove(&id);
            }
            None => {}
        }
    }

    /// The window lost focus: lifted fingers never arrive after that.
    pub fn clear(&mut self) {
        self.down.clear();
    }

    /// Every finger as an app sees it now: id, x, y.
    pub fn read(&mut self) -> Vec<(u64, f32, f32)> {
        let now = self.down.iter().map(|(id, f)| (*id, f.x, f.y)).collect();
        self.down.retain(|_, f| !f.lifted);
        for finger in self.down.values_mut() {
            finger.read = true;
        }
        now
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tap that lands and lifts between two reads is reported once, then
    /// gone; a finger that was read lifts at once.
    #[test]
    fn a_quick_tap_is_still_seen_once() {
        let mut f = FingerTracker::default();
        f.touch(7, 10.0, 20.0);
        f.lift(7);
        assert_eq!(f.read(), vec![(7, 10.0, 20.0)]);
        assert_eq!(f.read(), vec![]);

        f.touch(8, 1.0, 2.0);
        assert_eq!(f.read(), vec![(8, 1.0, 2.0)]);
        f.touch(8, 3.0, 4.0);
        f.lift(8);
        assert_eq!(f.read(), vec![]);
    }

    /// K-392: a tree built with upsert_node alone works. The first
    /// parentless node of an empty window is its root; the root can be
    /// updated in place and keeps its children; a SECOND parentless node is
    /// still refused, because it has nowhere to go.
    #[test]
    fn a_tree_can_be_built_with_upsert_alone_and_the_root_restyled() {
        let mut registry = DraftWindowRegistry::default();
        let size = WindowSize::new(800, 600).expect("size");
        let window =
            registry.create_window(WindowOptions::new("Relay", size).expect("window options"));
        let id = |n| WidgetId::new(n).expect("id");

        let root = WidgetNode::new(id(1), WidgetKind::Stack);
        registry
            .upsert_node(window, root)
            .expect("the first parentless node of an empty window becomes its root");
        let child = WidgetNode::new(id(2), WidgetKind::Button)
            .with_parent(id(1))
            .with_label("Go")
            .expect("label");
        registry
            .upsert_node(window, child)
            .expect("child of the root");

        let restyled = WidgetNode::new(id(1), WidgetKind::Stack)
            .with_label("Main")
            .expect("label");
        registry
            .upsert_node(window, restyled)
            .expect("the root can be updated in place");
        let tree = registry.widget_tree(window).expect("tree");
        assert_eq!(tree.root(), id(1));
        assert_eq!(
            tree.node(id(1)).and_then(|n| n.label.clone()).as_deref(),
            Some("Main")
        );
        assert!(tree.node(id(2)).is_some(), "and keeps its children");

        let second = WidgetNode::new(id(3), WidgetKind::Stack);
        assert!(
            matches!(
                registry.upsert_node(window, second),
                Err(UiAdapterError::MissingWidgetParent { parent: 0, .. })
            ),
            "a second parentless node is still a mistake"
        );

        let moved = WidgetNode::new(id(1), WidgetKind::Stack).with_parent(id(2));
        assert!(
            matches!(
                registry.upsert_node(window, moved),
                Err(UiAdapterError::DuplicateWidget { .. })
            ),
            "the root cannot be moved under another node"
        );
    }

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
    fn draft_adapter_clipboard_round_trips_in_process() {
        let adapter = DraftUiAdapter::new();

        // Empty until the first write.
        assert_eq!(adapter.read_clipboard_text().expect("read empty"), "");

        adapter.write_clipboard_text("hello").expect("write");
        assert_eq!(adapter.read_clipboard_text().expect("read"), "hello");

        // A second write replaces the buffer.
        adapter.write_clipboard_text("world").expect("write again");
        assert_eq!(adapter.read_clipboard_text().expect("read again"), "world");
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
    fn draft_adapter_clipboard_is_isolated_per_instance() {
        // Each draft adapter owns its own in-process buffer; writing to one
        // does not leak into another.
        let first = DraftUiAdapter::new();
        let second = DraftUiAdapter::new();

        first.write_clipboard_text("first").expect("write first");
        assert_eq!(first.read_clipboard_text().expect("read first"), "first");
        assert_eq!(second.read_clipboard_text().expect("read second"), "");
    }

    /// The pointer rules every adapter shares (IC-909): motion from
    /// positions outside capture, only the device's own motion inside it,
    /// and focus loss letting go of everything.
    #[test]
    fn a_pointer_reports_motion_buttons_and_lets_go_on_focus_loss() {
        let mut p = PointerTracker::default();
        p.moved_to(10.0, 10.0);
        assert_eq!(
            p.take_motion(),
            (0.0, 0.0),
            "the first position is not a move"
        );
        p.moved_to(15.0, 7.0);
        p.moved_by(100.0, 100.0);
        assert_eq!(
            p.take_motion(),
            (5.0, -3.0),
            "outside capture the cursor's path is the motion, not raw deltas"
        );
        assert_eq!(p.take_motion(), (0.0, 0.0), "taking motion resets it");

        p.button(PointerButton::Secondary, true);
        assert!(p.held(PointerButton::Secondary));
        assert!(!p.held(PointerButton::Primary));

        p.captured = true;
        p.moved_to(400.0, 400.0);
        p.moved_by(3.0, -2.0);
        p.moved_by(3.0, -2.0);
        assert_eq!(
            p.take_motion(),
            (6.0, -4.0),
            "held, only the mouse's own motion counts: the cursor is pinned"
        );
        assert_eq!(p.position, Some((15.0, 7.0)), "a held pointer stays put");
        p.left();
        assert!(p.position.is_some(), "a held pointer never leaves");

        p.moved_by(9.0, 9.0);
        p.focus_lost();
        assert!(!p.captured, "focus loss gives the pointer back");
        assert!(!p.held(PointerButton::Secondary), "and no button sticks");
        assert_eq!(p.take_motion(), (0.0, 0.0), "and stale motion is dropped");
        assert!(
            !p.held(PointerButton::Other),
            "a button with no bit is never held"
        );
    }
}
