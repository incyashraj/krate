//! The browser adapter: a Krate app painting into a canvas in a tab.
//!
//! # Why this can work at all
//!
//! Two properties of the existing runtime, neither obvious until measured:
//!
//! 1. **The painter is a CPU framebuffer.** `paint_placements` writes
//!    `0xAARRGGBB` into a row-major `&mut [u32]`. That is byte-compatible
//!    with what `ImageData` wants, so a frame reaches the screen through
//!    `putImageData` with no WebGPU, no WebGL, and no shader path.
//! 2. **The JIT problem is already solved.** wasmtime selects pulley, its
//!    portable interpreter, automatically on wasm32 -- the same answer iOS
//!    needed because it forbids executable pages. Measured on the rate card,
//!    the interpreter costs nothing worth naming: the guest is a few
//!    kilobytes of decisions and the painting is host code either way.
//!
//! # What this file is today
//!
//! The spike, and only the spike: proof that the shared painter's pixels
//! reach a canvas. It deliberately does NOT implement `UiAdapter` yet,
//! because the honest blocker is the event loop, not the drawing -- the
//! guest's `run()` never returns, and the host blocks inside it on
//! `thread::sleep`, which traps on a browser's main thread. That needs a
//! Web Worker and `Atomics.wait`, and it is worth settling on top of a
//! proven pixel path rather than underneath an unproven one.
//!
//! Everything here is the real shared code. Nothing about the frame is
//! mocked: same painter, same placements, same layout crate the native
//! hosts use, so this cannot drift from what a Mac draws.

// The symbols wasmtime asks a host it does not recognise to supply.
// Declared before anything else uses the runtime, since without them the
// module loads and then fails to find them.
#[cfg(target_arch = "wasm32")]
mod platform;

use krate_adapter_common::painter::{self, PaintInteraction};
use krate_adapter_common::ui::{WidgetPlacement, WidgetTree};
use krate_layout::{absolute_rect, compute_layout, LayoutViewport};

/// A frame of pixels, painted by the shared painter.
///
/// Kept separate from anything browser-specific so the same call can be
/// exercised by a native test -- which is how the spike is verified
/// without a browser in the loop.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// 0xAARRGGBB, row-major from the top.
    pub buffer: Vec<u32>,
}

impl Frame {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            buffer: vec![painter::COLOR_BACKGROUND; (width as usize) * (height as usize)],
        }
    }

    /// Paint one frame of widgets through the shared CPU painter.
    pub fn paint(&mut self, placements: &[WidgetPlacement], scale: f32) {
        painter::paint_placements(
            &mut self.buffer,
            self.width,
            self.height,
            scale,
            placements,
            PaintInteraction::default(),
        );
    }

    /// The frame as RGBA bytes, which is what `ImageData` takes.
    ///
    /// The painter stores `0xAARRGGBB` in a `u32`; a canvas wants
    /// `[R, G, B, A]` bytes in memory order. This is the one conversion
    /// between the two worlds, and it is a byte shuffle, not a re-render.
    pub fn to_rgba(&self) -> Vec<u8> {
        let mut rgba = vec![0u8; self.buffer.len() * 4];
        for (chunk, word) in rgba.chunks_exact_mut(4).zip(self.buffer.iter()) {
            chunk[0] = (word >> 16) as u8;
            chunk[1] = (word >> 8) as u8;
            chunk[2] = *word as u8;
            chunk[3] = (word >> 24) as u8;
        }
        rgba
    }
}

/// Lay a widget tree out at a given size and lower it to placements the
/// painter can draw.
///
/// This is the second half of the chain an app actually travels: the guest
/// describes a tree, the layout crate computes rectangles, and the painter
/// fills pixels. Both crates already compile to wasm32 untouched, so the
/// whole chain runs in a tab today.
///
/// The runtime owns a richer version of this (scroll clipping, list-row
/// selection, native-control hints in `phase3_gui_host.rs`). That code is
/// not reachable here yet because `krate-runtime` does not build for
/// wasm32 -- rusqlite, ureq and tungstenite are unconditional dependencies
/// that a browser build must gate out first. Until then this covers the
/// plain case honestly, and it must NOT grow into a second, drifting
/// implementation: when the runtime compiles for the browser, this goes.
pub fn lay_out(tree: &WidgetTree, width: f32, height: f32) -> Vec<WidgetPlacement> {
    let Ok(viewport) = LayoutViewport::new(width, height) else {
        return Vec::new();
    };
    let Ok(layout) = compute_layout(tree, viewport) else {
        return Vec::new();
    };
    let mut placements = Vec::new();
    for (id, node) in tree.nodes() {
        let Some(rect) = absolute_rect(tree, &layout, *id) else {
            continue;
        };
        placements.push(WidgetPlacement {
            widget: *id,
            kind: node.kind,
            label: node.label.clone(),
            checked: node.checked,
            value: node.value,
            selection: None,
            text_cursor: node.text_cursor,
            clip: None,
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
            clickable: false,
            role: node.role.clone(),
            pixels: None,
        });
    }
    placements
}

#[cfg(target_arch = "wasm32")]
mod web {
    use super::Frame;
    use krate_adapter_common::ui::{
        WidgetId, WidgetKind, WidgetNode, WidgetPlacement, WidgetStyle, WidgetTree,
    };
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::Clamped;

    /// Paint a demonstration frame into the canvas with the given id.
    ///
    /// This is the spike's whole public surface: it proves that pixels
    /// produced by the shared painter land on a canvas. Real placements
    /// will come from the runtime's layout pass; here they are built by
    /// hand so the path can be tested before the runtime compiles for
    /// this target.
    #[wasm_bindgen]
    pub fn paint_demo(canvas_id: &str, width: u32, height: u32) -> Result<(), JsValue> {
        let window = web_sys::window().ok_or("no window")?;
        let document = window.document().ok_or("no document")?;
        let canvas = document
            .get_element_by_id(canvas_id)
            .ok_or("no canvas with that id")?
            .dyn_into::<web_sys::HtmlCanvasElement>()?;
        canvas.set_width(width);
        canvas.set_height(height);
        let ctx = canvas
            .get_context("2d")?
            .ok_or("no 2d context")?
            .dyn_into::<web_sys::CanvasRenderingContext2d>()?;

        // The real chain: a widget tree, laid out by the layout crate,
        // painted by the shared painter. Not hand-placed rectangles.
        let mut frame = Frame::new(width, height);
        let placements = match demo_tree() {
            Some(tree) => super::lay_out(&tree, width as f32, height as f32),
            None => demo_placements(width as f32, height as f32),
        };
        frame.paint(&placements, 1.0);

        let mut rgba = frame.to_rgba();
        let data = web_sys::ImageData::new_with_u8_clamped_array_and_sh(
            Clamped(&mut rgba),
            width,
            height,
        )?;
        ctx.put_image_data(&data, 0.0, 0.0)?;
        Ok(())
    }

    /// A small app's widget tree, the way a guest would describe one:
    /// a padded stack with a heading, a line of body text, a button, a
    /// text field and a progress bar. The layout crate decides where
    /// they all go.
    fn demo_tree() -> Option<WidgetTree> {
        let id = |n: u64| WidgetId::new(n).ok();
        let mut root = WidgetNode::new(id(1)?, WidgetKind::Stack);
        root.style = WidgetStyle {
            width: None,
            height: None,
            grow: 1.0,
            padding: 24.0,
        };
        let mut tree = WidgetTree::new(root).ok()?;

        let mut add = |n: u64, kind: WidgetKind, label: Option<&str>, height: f32, value: Option<f32>| {
            let mut node = WidgetNode::new(id(n).expect("non-zero"), kind);
            node.parent = Some(id(1).expect("non-zero"));
            node.label = label.map(|s| s.to_string());
            node.value = value;
            node.style = WidgetStyle {
                width: None,
                height: Some(height),
                grow: 0.0,
                padding: 0.0,
            };
            let _ = tree.upsert(node);
        };
        add(2, WidgetKind::Text, Some("Krate, running in a browser tab"), 30.0, None);
        add(3, WidgetKind::Text, Some("Laid out and painted by the same code a Mac runs."), 24.0, None);
        add(4, WidgetKind::Button, Some("A button"), 38.0, None);
        add(5, WidgetKind::TextField, Some("A text field"), 34.0, None);
        add(6, WidgetKind::Progress, None, 16.0, Some(0.6));
        Some(tree)
    }

    /// A handful of widgets, laid out by hand. The fallback for when the
    /// tree cannot be built, kept so the canvas is never blank.
    fn demo_placements(width: f32, _height: f32) -> Vec<WidgetPlacement> {
        let pad = 24.0;
        let w = width - pad * 2.0;
        vec![
            placement(1, WidgetKind::Text, "Krate, running in a browser tab", pad, pad, w, 28.0),
            placement(2, WidgetKind::Text, "Painted by the same shared painter a Mac uses.", pad, pad + 36.0, w, 22.0),
            placement(3, WidgetKind::Button, "A button", pad, pad + 76.0, 160.0, 36.0),
            placement(4, WidgetKind::TextField, "A text field", pad, pad + 124.0, w, 34.0),
            placement(5, WidgetKind::Progress, "", pad, pad + 174.0, w, 14.0),
        ]
    }

    fn placement(
        id: u32,
        kind: WidgetKind,
        label: &str,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> WidgetPlacement {
        WidgetPlacement {
            widget: WidgetId::new(id.into()).expect("demo ids are non-zero"),
            kind,
            label: if label.is_empty() { None } else { Some(label.to_string()) },
            checked: None,
            value: if matches!(kind, WidgetKind::Progress) { Some(0.6) } else { None },
            selection: None,
            text_cursor: None,
            clip: None,
            x,
            y,
            width,
            height,
            clickable: matches!(kind, WidgetKind::Button),
            role: None,
            pixels: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use krate_adapter_common::ui::{WidgetId, WidgetKind};

    fn text(id: u32, label: &str, x: f32, y: f32, w: f32, h: f32) -> WidgetPlacement {
        WidgetPlacement {
            widget: WidgetId::new(id.into()).expect("demo ids are non-zero"),
            kind: WidgetKind::Button,
            label: Some(label.to_string()),
            checked: None,
            value: None,
            selection: None,
            text_cursor: None,
            clip: None,
            x,
            y,
            width: w,
            height: h,
            clickable: true,
            role: None,
            pixels: None,
        }
    }

    /// The spike's real assertion: the shared painter puts pixels in the
    /// buffer, and they survive the conversion a canvas needs.
    ///
    /// Run natively, so the pixel path is proven without a browser. If
    /// this passes and `putImageData` shows nothing, the bug is in the
    /// glue, not in the rendering -- which is exactly the split worth
    /// having before the adapter work starts.
    #[test]
    fn the_shared_painter_fills_a_frame_a_canvas_could_show() {
        let mut frame = Frame::new(200, 80);
        let blank = frame.buffer.clone();
        frame.paint(&[text(1, "Hello", 10.0, 10.0, 120.0, 30.0)], 1.0);

        assert_ne!(frame.buffer, blank, "painting should change the frame");

        let rgba = frame.to_rgba();
        assert_eq!(rgba.len(), 200 * 80 * 4, "RGBA is four bytes a pixel");
        assert!(
            rgba.chunks_exact(4).all(|px| px[3] == 0xFF),
            "every pixel must be opaque, or the canvas shows the page through it",
        );

        // The button is drawn somewhere inside its own rect, and the frame
        // outside it is untouched background.
        let at = |x: usize, y: usize| frame.buffer[y * 200 + x];
        assert_ne!(
            at(60, 25),
            painter::COLOR_BACKGROUND,
            "the button's middle should be painted",
        );
        assert_eq!(
            at(190, 75),
            painter::COLOR_BACKGROUND,
            "the far corner should be untouched",
        );
    }

    /// The whole chain an app travels: a widget tree, laid out by the
    /// real layout engine, lowered to placements, painted. This is what
    /// separates "we can draw rectangles" from "an app can appear".
    #[test]
    fn a_widget_tree_lays_out_and_paints() {
        use krate_adapter_common::ui::{WidgetNode, WidgetStyle};

        let id = |n: u64| WidgetId::new(n).expect("non-zero");
        let mut root = WidgetNode::new(id(1), WidgetKind::Stack);
        root.style = WidgetStyle { width: None, height: None, grow: 1.0, padding: 12.0 };
        let mut tree = WidgetTree::new(root).expect("root");

        let mut child = WidgetNode::new(id(2), WidgetKind::Button);
        child.parent = Some(id(1));
        child.label = Some("Press".to_string());
        child.style = WidgetStyle { width: None, height: Some(30.0), grow: 0.0, padding: 0.0 };
        tree.upsert(child).expect("child");

        let placements = lay_out(&tree, 300.0, 150.0);
        assert_eq!(placements.len(), 2, "the stack and its button");

        let button = placements
            .iter()
            .find(|p| p.kind == WidgetKind::Button)
            .expect("the button should be placed");
        // The layout engine, not us, decided this: the padding pushed it in
        // from the edge and the height came from the style.
        assert_eq!(button.x, 12.0, "padding should offset the child");
        assert_eq!(button.height, 30.0, "the styled height should survive");
        assert!(button.width > 0.0, "the button should have real width");

        let mut frame = Frame::new(300, 150);
        let blank = frame.buffer.clone();
        frame.paint(&placements, 1.0);
        assert_ne!(frame.buffer, blank, "a laid-out tree should paint");
    }

    /// The byte order the canvas expects, pinned. A red pixel must come
    /// out as R=255, and not as B=255 -- the classic way this path fails
    /// silently, with everything on screen looking almost right.
    #[test]
    fn pixels_convert_to_canvas_byte_order() {
        let mut frame = Frame::new(1, 1);
        frame.buffer[0] = 0xFF_FF_00_00; // opaque red
        assert_eq!(frame.to_rgba(), vec![0xFF, 0x00, 0x00, 0xFF]);
    }
}

/// Does wasmtime actually run in a browser tab?
///
/// Compiling and linking are not the same as running, and the answer
/// should come from a browser rather than a build log. This starts a real
/// engine and then asks it to load a real app's component -- the two steps
/// that decide whether an in-tab preview is possible at all.
///
/// Verified 2026-09-01: the engine STARTS. Loading a component then panics
/// with "time not implemented on this platform", because bare
/// wasm32-unknown-unknown has no clock and `std::time::Instant` is
/// unimplemented there. That is the next piece of work, and it is
/// plumbing -- the browser has `performance.now()`.
#[cfg(target_arch = "wasm32")]
mod runtime_probe {
    use wasm_bindgen::prelude::*;

    /// A real app's compiled component, baked in so the probe answers its
    /// question without needing a fetch.
    ///
    /// Not committed -- it is build input. A browser build has no
    /// compiler, so this is a component ALREADY compiled for pulley, the
    /// interpreter a tab runs:
    ///
    /// ```text
    /// unzip -o evidence/demo/ratecard.krate code.wasm
    /// cargo run -p krate-runtime --example precompile_for_web -- \
    ///   code.wasm crates/adapter-web/probe-component.wasm
    /// ```
    const BUNDLED_COMPONENT: &[u8] = include_bytes!("../probe-component.wasm");

    #[wasm_bindgen]
    pub fn probe_engine() -> String {
        // wasm panics abort rather than unwind, so `catch_unwind` never
        // sees them and a browser reports only "RuntimeError:
        // unreachable". The hook is what turned that shrug into the real
        // sentence naming the clock.
        std::panic::set_hook(Box::new(|info| {
            web_sys::console::error_1(&format!("krate panic: {info}").into());
        }));
        // The engine's settings must match the ones the component was
        // compiled with, or wasmtime answers "compilation settings are not
        // compatible with the native host". A previewed app opens a
        // window, so this is a windowed mode -- the same one
        // `precompile_for_web` uses.
        let config = krate_runtime::Config {
            phase3_ui_mode: krate_runtime::phase3_ui::Phase3HostUiMode::NativePrototype,
            ..krate_runtime::Config::default()
        };
        let runtime = match krate_runtime::Runtime::new(&config) {
            Ok(runtime) => runtime,
            Err(err) => return format!("engine refused: {err}"),
        };
        match runtime.load_component(BUNDLED_COMPONENT) {
            Ok(_) => format!(
                "engine started and loaded a component ({} bytes)",
                BUNDLED_COMPONENT.len()
            ),
            Err(err) => format!("engine started, load refused: {err}"),
        }
    }
}
