//! S2's live proof: open a real window and present the parity corpus through
//! GpuPresent -- the exact struct the Windows draw path uses -- for 90
//! frames. Exits 0 only if every frame presented on the GPU.
//!
//! Runs on the dev Mac via `--features dev-anyos` (Metal) and on Windows
//! as-is (DX12/Vulkan); wgpu is the point.

use std::sync::Arc;

use krate_adapter_common::painter::PaintInteraction;
use krate_adapter_common::ui::{WidgetId, WidgetKind, WidgetPlacement};
use krate_adapter_windows::winit_native::GpuPresent;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::Window;

fn place(kind: WidgetKind, id: u64, x: f32, y: f32, w: f32, h: f32) -> WidgetPlacement {
    WidgetPlacement {
        widget: WidgetId::new(id).unwrap(),
        kind,
        label: None,
        checked: None,
        value: None,
        selection: None,
        text_cursor: None,
        clip: None,
        role: None,
        pixels: None,
        clickable: false,
        x,
        y,
        width: w,
        height: h,
    }
}

#[derive(Default)]
struct App {
    window: Option<Arc<Window>>,
    gpu: Option<GpuPresent>,
    frames: u32,
    corpus: Vec<WidgetPlacement>,
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
                        .with_title("krate gpu proof")
                        .with_inner_size(winit::dpi::LogicalSize::new(320.0, 240.0)),
                )
                .expect("window"),
        );
        let gpu = GpuPresent::new(window.clone()).expect("GPU presenter must initialize");
        let mut button = place(WidgetKind::Button, 1, 20.0, 20.0, 180.0, 32.0);
        button.label = Some("GPU frames".into());
        let mut slider = place(WidgetKind::Slider, 2, 20.0, 70.0, 200.0, 20.0);
        slider.value = Some(0.4);
        self.corpus = vec![
            button,
            place(WidgetKind::TextField, 3, 20.0, 110.0, 200.0, 26.0),
            slider,
        ];
        window.request_redraw();
        self.window = Some(window);
        self.gpu = Some(gpu);
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
                let mut slid = self.corpus.clone();
                slid[2].value = Some((self.frames % 240) as f32 / 240.0);
                gpu.render(
                    window,
                    &slid,
                    PaintInteraction {
                        hovered: None,
                        pressed: None,
                    },
                    Some(std::time::Instant::now()),
                    None,
                )
                .expect("every frame must present on the GPU");
                self.frames += 1;
                if self.frames >= 240 {
                    println!("gpu-proof: {} frames presented", self.frames);
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
    assert!(app.frames >= 240, "exited after only {} frames", app.frames);
}
