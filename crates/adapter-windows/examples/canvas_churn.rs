//! The game workload against the GPU presenter: a full-window canvas whose
//! pixels are NEW every frame, exactly what publish_canvas hands the winit
//! draw path 60 times a second. Prints per-frame cost so the "re-upload a
//! brand-new image each frame" suspicion is measured, not guessed.
//!
//! Runs on the dev Mac via `--features dev-anyos` (Metal) and on Windows
//! as-is.

use std::sync::Arc;
use std::time::Instant;

use krate_adapter_common::painter::PaintInteraction;
use krate_adapter_common::ui::{ImagePixels, WidgetId, WidgetKind, WidgetPlacement};
use krate_adapter_windows::winit_native::GpuPresent;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::Window;

const W: u32 = 1280;
const H: u32 = 800;
const FRAMES: u32 = 300;

fn frame_pixels(tick: u32) -> ImagePixels {
    // A moving gradient: cheap to compute, different every frame, honest
    // about the part that matters -- the buffer is a fresh allocation, so
    // any pointer-keyed cache misses just like the real publish path.
    let mut rgba = vec![0u8; (W * H * 4) as usize];
    let shift = (tick * 3) as u32;
    for y in 0..H {
        let row = (y * W * 4) as usize;
        let g = ((y + shift) % 256) as u8;
        for x in 0..W {
            let i = row + (x * 4) as usize;
            rgba[i] = ((x + shift) % 256) as u8;
            rgba[i + 1] = g;
            rgba[i + 2] = 96;
            rgba[i + 3] = 255;
        }
    }
    ImagePixels::new(W, H, rgba).expect("pixels")
}

#[derive(Default)]
struct App {
    window: Option<Arc<Window>>,
    gpu: Option<GpuPresent>,
    frames: u32,
    times: Vec<f32>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("krate canvas churn")
                        .with_inner_size(winit::dpi::LogicalSize::new(W as f64, H as f64)),
                )
                .expect("window"),
        );
        let gpu = GpuPresent::new(window.clone()).expect("GPU presenter must initialize");
        self.window = Some(window);
        self.gpu = Some(gpu);
        self.window.as_ref().unwrap().request_redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::RedrawRequested => {
                let (Some(window), Some(gpu)) = (self.window.as_ref(), self.gpu.as_mut()) else {
                    return;
                };
                let mut canvas = WidgetPlacement {
                    widget: WidgetId::new(1).unwrap(),
                    kind: WidgetKind::Canvas,
                    label: None,
                    checked: None,
                    value: None,
                    selection: None,
                    text_cursor: None,
                    clip: None,
                    role: None,
                    pixels: None,
                    clickable: false,
                    x: 0.0,
                    y: 0.0,
                    width: W as f32,
                    height: H as f32,
                };
                canvas.pixels = Some(Arc::new(frame_pixels(self.frames)));
                let started = Instant::now();
                gpu.render(
                    window,
                    &[canvas],
                    PaintInteraction {
                        hovered: None,
                        pressed: None,
                    },
                    None,
                )
                .expect("present");
                self.times.push(started.elapsed().as_secs_f32() * 1000.0);
                self.frames += 1;
                if self.frames >= FRAMES {
                    self.times.sort_by(|a, b| a.total_cmp(b));
                    let p = |q: f32| self.times[((self.times.len() - 1) as f32 * q) as usize];
                    println!(
                        "canvas-churn: render+present p50 {:.2}ms p99 {:.2}ms over {} frames at {}x{}",
                        p(0.5),
                        p(0.99),
                        FRAMES,
                        W,
                        H
                    );
                    event_loop.exit();
                } else {
                    window.request_redraw();
                }
            }
            WindowEvent::CloseRequested => event_loop.exit(),
            _ => {}
        }
    }
}

fn main() {
    let event_loop = EventLoop::new().expect("event loop");
    let mut app = App::default();
    event_loop.run_app(&mut app).expect("run");
}
