//! The canvas display list: what an app drew, as data.
//!
//! The CPU raster path consumes draw calls immediately and publishes
//! pixels. A GPU adapter wants the opposite: the calls themselves, late,
//! so it can turn them into GPU work at the display's own rhythm. This
//! list is that second consumer's food -- recorded beside the CPU raster
//! only when the adapter asks for it (`supports_canvas_lists`), so the
//! desktop path pays nothing.
//!
//! Ops mirror the WIT canvas surface one-to-one, in logical pixels, in
//! draw order. The consumer owns scaling to physical.

use std::sync::Arc;

use crate::ui::ImagePixels;

/// One recorded draw call, colors packed 0xAARRGGBB like the raster path.
#[derive(Clone, Debug)]
pub enum CanvasOp {
    Clear(u32),
    SetClip(Option<(f32, f32, f32, f32)>),
    FillRect {
        rect: (f32, f32, f32, f32),
        color: u32,
    },
    StrokeRect {
        rect: (f32, f32, f32, f32),
        width: f32,
        color: u32,
    },
    FillRoundRect {
        rect: (f32, f32, f32, f32),
        radii: (f32, f32, f32, f32),
        color: u32,
    },
    StrokeRoundRect {
        rect: (f32, f32, f32, f32),
        radii: (f32, f32, f32, f32),
        width: f32,
        color: u32,
    },
    DropShadowRoundRect {
        rect: (f32, f32, f32, f32),
        radii: (f32, f32, f32, f32),
        blur: f32,
        color: u32,
    },
    LinearGradient {
        rect: (f32, f32, f32, f32),
        top: u32,
        bottom: u32,
    },
    LinearGradientStops {
        rect: (f32, f32, f32, f32),
        angle_degrees: f32,
        stops: Vec<(f32, u32)>,
    },
    RadialGradient {
        center: (f32, f32),
        radius: f32,
        inner: u32,
        outer: u32,
    },
    FillCircle {
        center: (f32, f32),
        radius: f32,
        color: u32,
    },
    StrokeCircle {
        center: (f32, f32),
        radius: f32,
        width: f32,
        color: u32,
    },
    StrokeArc {
        center: (f32, f32),
        radius: f32,
        start_degrees: f32,
        sweep_degrees: f32,
        width: f32,
        color: u32,
    },
    Text {
        origin: (f32, f32),
        font_size: f32,
        color: u32,
        weight: u16,
        italic: bool,
        letter_spacing: f32,
        /// 0 sans, 1 serif, 2 mono -- the WIT family enum's order.
        family: u8,
        text: String,
    },
    Pixels {
        rect: (f32, f32, f32, f32),
        image: Arc<ImagePixels>,
    },
    PixelsRound {
        rect: (f32, f32, f32, f32),
        radii: (f32, f32, f32, f32),
        image: Arc<ImagePixels>,
    },
    Sprite {
        center: (f32, f32),
        dst: (f32, f32),
        angle: f32,
        image: Arc<ImagePixels>,
    },
}

/// One frame's recording for one canvas, in logical pixels.
#[derive(Clone, Debug, Default)]
pub struct CanvasList {
    pub logical_width: u32,
    pub logical_height: u32,
    pub ops: Vec<CanvasOp>,
}

impl CanvasList {
    pub fn clear_for_frame(&mut self, width: u32, height: u32) {
        self.logical_width = width;
        self.logical_height = height;
        self.ops.clear();
    }
}
