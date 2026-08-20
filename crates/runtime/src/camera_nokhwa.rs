//! The Windows and Linux camera backend: nokhwa behind `CameraBackend`.
//!
//! macOS has its own AVFoundation backend (`camera_macos`) because it needs the
//! platform's permission dance and a delegate on a private dispatch queue.
//! Windows and Linux need neither, and their native APIs -- Media Foundation
//! and V4L2 -- are different enough from each other that writing both by hand
//! would be two more bodies of unsafe code to maintain. nokhwa already wraps
//! both behind one interface, so one file closes both gaps.
//!
//! Everything that decides BEHAVIOUR still lives in `camera_capture`: the
//! newest-frame slot, the stream cap, the capability check, the rule that
//! stopping drops the frame captured while running. This file only does the
//! part that is genuinely per-platform -- open a device, pull frames, convert
//! to the RGBA the canvas already draws.
//!
//! One difference from macOS worth stating: nokhwa is a PULL API. There is no
//! callback, so a worker thread does the pulling and drops each frame into the
//! shared slot, which is the same shape the AVFoundation delegate produces.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use nokhwa::pixel_format::RgbAFormat;
use nokhwa::utils::{
    ApiBackend, CameraFormat, CameraIndex, RequestedFormat, RequestedFormatType, Resolution,
};
use nokhwa::Camera;

use crate::camera_capture::{
    CameraBackend, CameraConfig, CameraError, DeviceInfo, Frame, FrameFormat, FrameInfo, FrameSlot,
    PlatformStream,
};

#[derive(Default)]
pub struct NokhwaCameraBackend;

/// One opened camera plus the thread that pulls frames from it.
struct NokhwaStream {
    /// Held by the worker; `stop` clears it and the worker parks.
    running: Arc<AtomicBool>,
    /// Cleared on drop so the worker thread ends with the stream.
    alive: Arc<AtomicBool>,
    observed: Arc<Mutex<Option<(u32, u32)>>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl PlatformStream for NokhwaStream {
    fn start(&mut self) -> Result<(), CameraError> {
        self.running.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), CameraError> {
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn observed_size(&self) -> Option<(u32, u32)> {
        self.observed.lock().ok().and_then(|size| *size)
    }
}

impl Drop for NokhwaStream {
    fn drop(&mut self) {
        // Tell the worker to end, then wait for it. Joining matters: the
        // worker owns the device handle, and returning from `close` while it
        // still holds the camera means the next `open` finds the device busy.
        self.alive.store(false, Ordering::SeqCst);
        self.running.store(false, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn map_err(err: impl std::fmt::Display) -> CameraError {
    let text = err.to_string();
    // A refused device reads as a permission problem on both systems: Windows
    // has a per-app camera privacy setting, and Linux is usually a missing
    // `video` group. Either way the person has to change a system setting, so
    // it is `SystemDenied` -- the same wall macOS reports -- rather than a
    // generic platform error they cannot act on.
    let lower = text.to_lowercase();
    if lower.contains("denied") || lower.contains("permission") || lower.contains("access") {
        CameraError::SystemDenied
    } else {
        CameraError::Platform(text)
    }
}

impl CameraBackend for NokhwaCameraBackend {
    fn devices(&self) -> Result<Vec<DeviceInfo>, CameraError> {
        let backend = if cfg!(target_os = "windows") {
            ApiBackend::MediaFoundation
        } else {
            ApiBackend::Video4Linux
        };
        // An empty list is a normal answer, not an error: plenty of desktops
        // have no camera, and an app should say so plainly.
        let found = nokhwa::query(backend).map_err(map_err)?;
        Ok(found
            .into_iter()
            .map(|info| DeviceInfo {
                id: info.index().to_string(),
                label: info.human_name(),
            })
            .collect())
    }

    fn open(
        &mut self,
        device: &str,
        config: CameraConfig,
        slot: Arc<FrameSlot>,
    ) -> Result<(FrameInfo, Box<dyn PlatformStream>), CameraError> {
        // An empty device means "the default camera", matching the WIT.
        let index = if device.is_empty() {
            CameraIndex::Index(0)
        } else {
            match device.parse::<u32>() {
                Ok(n) => CameraIndex::Index(n),
                Err(_) => CameraIndex::String(device.to_string()),
            }
        };

        let running = Arc::new(AtomicBool::new(false));
        let alive = Arc::new(AtomicBool::new(true));
        let observed: Arc<Mutex<Option<(u32, u32)>>> = Arc::new(Mutex::new(None));
        // How the worker reports back what it managed to open. `open` must not
        // return until this arrives, because the caller needs a FrameInfo and
        // an honest error rather than a handle to a camera that never opened.
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<FrameInfo, CameraError>>();

        let worker_running = running.clone();
        let worker_alive = alive.clone();
        let worker_observed = observed.clone();
        // `Builder::spawn`, not `thread::spawn`: the plain one panics when the
        // OS is out of threads, and a panic here would take the runtime down
        // rather than failing one camera (the same rule as K-137).
        let worker = std::thread::Builder::new()
            .name("krate-camera".to_string())
            .spawn(move || {
                // The camera is BUILT HERE, on the worker, and never crosses a
                // thread boundary. Media Foundation hands back COM objects with
                // thread affinity, so nokhwa's `Camera` is not `Send` on
                // Windows -- opening it on the caller's thread and moving it in
                // does not compile there, and would be wrong even if it did.
                // Everything the worker needs to report goes back through the
                // channel instead.
                let started = std::time::Instant::now();
                let wanted = RequestedFormat::new::<RgbAFormat>(RequestedFormatType::Closest(
                    CameraFormat::new(
                        Resolution::new(config.width, config.height),
                        nokhwa::utils::FrameFormat::NV12,
                        config.fps,
                    ),
                ));
                let mut camera = match Camera::new(index, wanted) {
                    Ok(camera) => camera,
                    Err(err) => {
                        let _ = ready_tx.send(Err(map_err(err)));
                        return;
                    }
                };
                let opened = camera.resolution();
                let info = FrameInfo {
                    width: opened.width(),
                    height: opened.height(),
                    fps: camera.frame_rate(),
                    format: FrameFormat::Rgba8,
                };
                if ready_tx.send(Ok(info)).is_err() {
                    // Nobody is waiting any more: `open` gave up. Release the
                    // device rather than holding it for a caller that is gone.
                    return;
                }

                let mut opened_stream = false;
                while worker_alive.load(Ordering::SeqCst) {
                    if !worker_running.load(Ordering::SeqCst) {
                        // Not capturing: close the device so the indicator
                        // light goes out, the way `stop` promises.
                        if opened_stream {
                            let _ = camera.stop_stream();
                            opened_stream = false;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(30));
                        continue;
                    }
                    if !opened_stream {
                        if camera.open_stream().is_err() {
                            // The device went away or was refused. Park rather
                            // than spin: `read` keeps returning None, which the
                            // app already handles as "no frame yet".
                            std::thread::sleep(std::time::Duration::from_millis(200));
                            continue;
                        }
                        opened_stream = true;
                    }
                    match camera.frame() {
                        Ok(buffer) => match buffer.decode_image::<RgbAFormat>() {
                            Ok(image) => {
                                let (width, height) = (image.width(), image.height());
                                if let Ok(mut size) = worker_observed.lock() {
                                    *size = Some((width, height));
                                }
                                slot.put(Frame {
                                    bytes: image.into_raw(),
                                    width,
                                    height,
                                    elapsed_millis: started.elapsed().as_millis() as u64,
                                });
                            }
                            Err(_) => {
                                // One undecodable frame is not a dead camera.
                                // Skip it; the next one usually decodes.
                            }
                        },
                        Err(_) => {
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                    }
                }
                if opened_stream {
                    let _ = camera.stop_stream();
                }
            })
            .map_err(|err| {
                CameraError::Platform(format!("could not start a camera worker: {err}"))
            })?;

        // Wait for the worker to say whether the device opened. Bounded: a
        // camera that never answers must fail rather than hang the app that
        // asked for it.
        let info = match ready_rx.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(Ok(info)) => info,
            Ok(Err(err)) => {
                alive.store(false, Ordering::SeqCst);
                let _ = worker.join();
                return Err(err);
            }
            Err(_) => {
                alive.store(false, Ordering::SeqCst);
                return Err(CameraError::Platform(
                    "the camera did not answer within ten seconds".to_string(),
                ));
            }
        };

        Ok((
            info,
            Box::new(NokhwaStream {
                running,
                alive,
                observed,
                worker: Some(worker),
            }),
        ))
    }
}
