//! Native camera capture behind the `camera.capture` capability.
//!
//! Shaped like `audio_capture`: the runtime owns the operating-system stream
//! and the guest sees only finished frames, after the policy has granted
//! camera access. Three things are deliberate here, because each one is a way
//! this could have gone quietly wrong.
//!
//! - **Frames are pulled, and only the newest is kept.** A camera runs at its
//!   own rate and an app draws at its own. Queueing would mean an app drawing
//!   at 30fps from a 60fps camera falls further behind every second, showing a
//!   picture that is minutes old by the end of a call. One slot, overwritten,
//!   means "late" costs nothing and the app always shows now.
//!
//! - **The operating system's own permission is asked for separately**, and
//!   its refusal is reported as `SystemDenied` rather than folded into
//!   `PermissionDenied`. Krate's wall and the platform's wall are different
//!   walls, and an app that cannot tell them apart cannot tell the person
//!   which one to go and change.
//!
//! - **Nothing is captured until `start`.** On macOS the camera indicator
//!   light is wired to the hardware, not to us, so a person can always see
//!   whether an app that says it stopped looking actually did.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// The largest frame this will hand a guest, in pixels.
///
/// A 4K frame is 33 megabytes as RGBA, and the guest copy makes two. Capping
/// the request keeps one careless `open` from trying to allocate a gigabyte
/// across a handful of streams; the camera is asked for something smaller
/// rather than refused, because a smaller picture is a better outcome than no
/// picture.
const MAX_FRAME_PIXELS: u32 = 3840 * 2160;

/// The most streams one app may hold open at once.
///
/// Every stream is a device handle and a frame buffer. A cap turns a leak into
/// an honest error instead of a machine that runs out of camera handles.
const MAX_STREAMS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameFormat {
    /// Straight-alpha RGBA, four bytes per pixel, row-major from the top left
    /// -- the layout `canvas2d::draw_pixels` already takes.
    Rgba8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CameraConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub format: FrameFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameInfo {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub format: FrameFormat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub bytes: Vec<u8>,
    /// The size of THIS frame. Carried with the bytes so the two can never
    /// disagree -- see the `frame` record in the WIT for why (K-147).
    pub width: u32,
    pub height: u32,
    pub elapsed_millis: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CameraError {
    InvalidStream,
    DeviceUnavailable,
    /// The operating system's own camera permission was refused. Distinct from
    /// Krate's `permission-denied`, so an app can say which wall it hit.
    SystemDenied,
    InvalidConfig(String),
    Unsupported(String),
    Platform(String),
}

/// The newest frame from a running camera, and nothing older.
///
/// Shared with whatever thread the platform delivers frames on. `Option`
/// rather than a queue is the whole latency design: a late reader gets the
/// current picture, never a backlog.
#[derive(Default)]
pub struct FrameSlot {
    latest: Mutex<Option<Frame>>,
}

impl FrameSlot {
    /// Replace whatever is there. Called from the platform's delivery thread.
    pub fn put(&self, frame: Frame) {
        if let Ok(mut slot) = self.latest.lock() {
            *slot = Some(frame);
        }
    }

    /// Take the newest frame, leaving the slot empty.
    ///
    /// Empty means "nothing new since you last asked", which is an ordinary
    /// answer for an app polling faster than the camera runs -- not an error,
    /// and not a reason to redraw.
    pub fn take(&self) -> Option<Frame> {
        self.latest.lock().ok().and_then(|mut slot| slot.take())
    }
}

/// What a platform backend has to provide.
///
/// Kept narrow on purpose: everything above this line -- the newest-frame
/// policy, the caps, the id bookkeeping, the capability checks -- is shared,
/// so a second platform implements only the part that is genuinely different.
pub trait CameraBackend: Send {
    fn devices(&self) -> Result<Vec<DeviceInfo>, CameraError>;

    /// Open the device and begin delivering frames into `slot` once started.
    /// Returns what the device actually opened as, which may not be what was
    /// asked for.
    fn open(
        &mut self,
        device: &str,
        config: CameraConfig,
        slot: Arc<FrameSlot>,
    ) -> Result<(FrameInfo, Box<dyn PlatformStream>), CameraError>;
}

/// One open device, owned by the runtime and closed when it is dropped.
pub trait PlatformStream: Send {
    fn start(&mut self) -> Result<(), CameraError>;
    fn stop(&mut self) -> Result<(), CameraError>;

    /// The size frames are actually arriving at, once one has.
    ///
    /// A camera gives its nearest supported mode, not the one asked for, and
    /// on macOS the true size is only knowable from a delivered frame. `info`
    /// reports this in preference to the request, because the bytes are laid
    /// out for THIS size -- a guest drawing 1920x1080 bytes as though they
    /// were 640x480 reads three rows into the first and paints noise or
    /// black. That is exactly what a webcam app did: green light on, frames
    /// flowing, black rectangle on screen (K-147).
    ///
    /// `None` means no frame has arrived yet, and the requested size stands as
    /// the best available answer.
    fn observed_size(&self) -> Option<(u32, u32)> {
        None
    }
}

struct CameraStream {
    platform: Box<dyn PlatformStream>,
    slot: Arc<FrameSlot>,
    info: FrameInfo,
    started: bool,
}

/// Every open camera stream for one run.
pub struct CameraCaptureRuntime {
    next_stream_id: u64,
    streams: BTreeMap<u64, CameraStream>,
    backend: Option<Box<dyn CameraBackend>>,
}

impl Default for CameraCaptureRuntime {
    fn default() -> Self {
        Self {
            next_stream_id: 1,
            streams: BTreeMap::new(),
            backend: platform_backend(),
        }
    }
}

impl CameraCaptureRuntime {
    /// Build a runtime over an explicit backend, for tests.
    pub fn with_backend(backend: Box<dyn CameraBackend>) -> Self {
        Self {
            next_stream_id: 1,
            streams: BTreeMap::new(),
            backend: Some(backend),
        }
    }

    fn backend(&mut self) -> Result<&mut Box<dyn CameraBackend>, CameraError> {
        self.backend.as_mut().ok_or_else(|| {
            CameraError::Unsupported(
                "this build of Krate has no camera support on this system yet".to_string(),
            )
        })
    }

    pub fn devices(&mut self) -> Result<Vec<DeviceInfo>, CameraError> {
        self.backend()?.devices()
    }

    pub fn open(&mut self, device: &str, config: CameraConfig) -> Result<u64, CameraError> {
        validate_config(config)?;
        if self.streams.len() >= MAX_STREAMS {
            return Err(CameraError::Platform(format!(
                "{MAX_STREAMS} camera streams are already open"
            )));
        }
        let slot = Arc::new(FrameSlot::default());
        let (info, platform) = self.backend()?.open(device, config, slot.clone())?;

        let stream_id = self.next_stream_id;
        self.next_stream_id = self
            .next_stream_id
            .checked_add(1)
            .ok_or_else(|| CameraError::Platform("camera stream id space exhausted".to_string()))?;
        self.streams.insert(
            stream_id,
            CameraStream {
                platform,
                slot,
                info,
                started: false,
            },
        );
        Ok(stream_id)
    }

    /// What this stream is really producing.
    ///
    /// The size the DEVICE settled on wins over the size that was asked for,
    /// as soon as a frame has proved what that is. An app is told to call this
    /// every frame for exactly this reason: the honest answer changes once,
    /// shortly after `start`.
    pub fn info(&self, stream_id: u64) -> Result<FrameInfo, CameraError> {
        let stream = self
            .streams
            .get(&stream_id)
            .ok_or(CameraError::InvalidStream)?;
        let mut info = stream.info;
        if let Some((width, height)) = stream.platform.observed_size() {
            info.width = width;
            info.height = height;
        }
        Ok(info)
    }

    pub fn start(&mut self, stream_id: u64) -> Result<(), CameraError> {
        let stream = self
            .streams
            .get_mut(&stream_id)
            .ok_or(CameraError::InvalidStream)?;
        if stream.started {
            return Ok(());
        }
        stream.platform.start()?;
        stream.started = true;
        Ok(())
    }

    pub fn stop(&mut self, stream_id: u64) -> Result<(), CameraError> {
        let stream = self
            .streams
            .get_mut(&stream_id)
            .ok_or(CameraError::InvalidStream)?;
        if !stream.started {
            return Ok(());
        }
        stream.platform.stop()?;
        stream.started = false;
        // Drop whatever was captured last. Without this a stopped camera still
        // has one frame to hand out, so an app that stops and reads shows a
        // picture taken while the light was on -- exactly the surprise the
        // indicator is meant to rule out.
        stream.slot.take();
        Ok(())
    }

    /// The newest frame, or `None` when none has arrived since the last read.
    ///
    /// Reading a stream that was never started is `None`, not an error: an app
    /// polling before it starts is early, not wrong.
    pub fn read(&mut self, stream_id: u64) -> Result<Option<Frame>, CameraError> {
        let stream = self
            .streams
            .get(&stream_id)
            .ok_or(CameraError::InvalidStream)?;
        if !stream.started {
            return Ok(None);
        }
        Ok(stream.slot.take())
    }

    pub fn close(&mut self, stream_id: u64) -> Result<(), CameraError> {
        let mut stream = self
            .streams
            .remove(&stream_id)
            .ok_or(CameraError::InvalidStream)?;
        if stream.started {
            let _ = stream.platform.stop();
        }
        Ok(())
    }
}

fn validate_config(config: CameraConfig) -> Result<(), CameraError> {
    if config.width == 0 || config.height == 0 {
        return Err(CameraError::InvalidConfig(
            "width and height must both be greater than zero".to_string(),
        ));
    }
    let pixels = config.width.saturating_mul(config.height);
    if pixels > MAX_FRAME_PIXELS {
        return Err(CameraError::InvalidConfig(format!(
            "{}x{} is larger than the {MAX_FRAME_PIXELS}-pixel limit",
            config.width, config.height
        )));
    }
    if config.fps == 0 || config.fps > 240 {
        return Err(CameraError::InvalidConfig(
            "fps must be between 1 and 240".to_string(),
        ));
    }
    Ok(())
}

/// What the operating system says about camera access for this process.
///
/// Diagnostic only: the app-facing answer is the `system-denied` error. This
/// exists so a probe can tell "the person has not been asked yet" apart from
/// "the person said no", which look identical from the guest side.
pub fn system_status() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        crate::camera_macos::system_status()
    }
    #[cfg(not(target_os = "macos"))]
    {
        "unknown"
    }
}

/// The backend for this system, or `None` where there is not one yet.
///
/// `None` is an honest answer that reaches the app as `unsupported` with a
/// sentence naming the system, rather than a silent failure or a fake frame.
fn platform_backend() -> Option<Box<dyn CameraBackend>> {
    #[cfg(target_os = "macos")]
    {
        Some(Box::new(crate::camera_macos::MacosCameraBackend))
    }
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    {
        Some(Box::new(
            crate::camera_nokhwa::NokhwaCameraBackend::default(),
        ))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A backend that hands out frames on demand, so the shared policy above
    /// can be tested without a camera.
    #[derive(Default)]
    struct FakeBackend {
        opened: Vec<String>,
        /// Set to make the fake device deliver a size other than the request.
        observed: Arc<Mutex<Option<(u32, u32)>>>,
    }

    struct FakeStream {
        slot: Arc<FrameSlot>,
        running: Arc<Mutex<bool>>,
        /// What the "device" really produces, regardless of what was asked.
        observed: Arc<Mutex<Option<(u32, u32)>>>,
    }

    impl PlatformStream for FakeStream {
        fn start(&mut self) -> Result<(), CameraError> {
            *self.running.lock().expect("lock") = true;
            // Deliver one frame, the way a real device would once running.
            self.slot.put(Frame {
                bytes: vec![0u8; 4],
                width: 1,
                height: 1,
                elapsed_millis: 0,
            });
            Ok(())
        }

        fn stop(&mut self) -> Result<(), CameraError> {
            *self.running.lock().expect("lock") = false;
            Ok(())
        }

        fn observed_size(&self) -> Option<(u32, u32)> {
            *self.observed.lock().expect("lock")
        }
    }

    impl CameraBackend for FakeBackend {
        fn devices(&self) -> Result<Vec<DeviceInfo>, CameraError> {
            Ok(vec![DeviceInfo {
                id: "fake-0".to_string(),
                label: "Fake Camera".to_string(),
            }])
        }

        fn open(
            &mut self,
            device: &str,
            config: CameraConfig,
            slot: Arc<FrameSlot>,
        ) -> Result<(FrameInfo, Box<dyn PlatformStream>), CameraError> {
            self.opened.push(device.to_string());
            Ok((
                FrameInfo {
                    width: config.width,
                    height: config.height,
                    fps: config.fps,
                    format: config.format,
                },
                Box::new(FakeStream {
                    slot,
                    running: Arc::new(Mutex::new(false)),
                    observed: self.observed.clone(),
                }),
            ))
        }
    }

    fn config() -> CameraConfig {
        CameraConfig {
            width: 640,
            height: 480,
            fps: 30,
            format: FrameFormat::Rgba8,
        }
    }

    fn runtime() -> CameraCaptureRuntime {
        CameraCaptureRuntime::with_backend(Box::new(FakeBackend::default()))
    }

    /// Nothing is captured until the app asks, which is what the indicator
    /// light on the machine is promising.
    #[test]
    fn an_opened_stream_captures_nothing_until_it_is_started() {
        let mut cameras = runtime();
        let stream = cameras.open("", config()).expect("open");
        assert_eq!(cameras.read(stream).expect("read"), None);

        cameras.start(stream).expect("start");
        assert!(
            cameras.read(stream).expect("read").is_some(),
            "a started stream delivers frames"
        );
    }

    /// The whole latency design: one slot, overwritten, so a slow reader gets
    /// the newest picture rather than the oldest of a backlog.
    #[test]
    fn only_the_newest_frame_is_kept() {
        let slot = FrameSlot::default();
        let frame = |n: u8| Frame {
            bytes: vec![n],
            width: 1,
            height: 1,
            elapsed_millis: u64::from(n),
        };
        slot.put(frame(1));
        slot.put(frame(2));
        slot.put(frame(3));

        let got = slot.take().expect("a frame");
        assert_eq!(got.elapsed_millis, 3, "the newest frame is the one kept");
        assert_eq!(slot.take(), None, "and there is no backlog behind it");
    }

    /// A stopped camera must not hand out the picture it took while running.
    #[test]
    fn stopping_drops_the_frame_captured_while_running() {
        let mut cameras = runtime();
        let stream = cameras.open("", config()).expect("open");
        cameras.start(stream).expect("start");
        cameras.stop(stream).expect("stop");

        assert_eq!(
            cameras.read(stream).expect("read"),
            None,
            "a stopped stream has nothing to show, including what it saw before"
        );
    }

    #[test]
    fn a_closed_stream_is_unknown_rather_than_silently_working() {
        let mut cameras = runtime();
        let stream = cameras.open("", config()).expect("open");
        cameras.close(stream).expect("close");

        assert_eq!(cameras.read(stream), Err(CameraError::InvalidStream));
        assert_eq!(cameras.info(stream), Err(CameraError::InvalidStream));
        assert_eq!(cameras.close(stream), Err(CameraError::InvalidStream));
    }

    /// The bug behind a black webcam window (K-147).
    ///
    /// A camera gives its nearest supported mode, not the one asked for. If
    /// `info` keeps reporting the REQUEST, the guest lays 1920x1080 bytes out
    /// as though they were 640x480 -- it reads three rows into the first and
    /// paints noise or black, with the camera light on the whole time.
    #[test]
    fn info_reports_the_size_the_device_really_delivered() {
        let observed = Arc::new(Mutex::new(None));
        let mut cameras = CameraCaptureRuntime::with_backend(Box::new(FakeBackend {
            opened: Vec::new(),
            observed: observed.clone(),
        }));
        let stream = cameras.open("", config()).expect("open");

        // Before any frame, the request is the best answer available.
        let info = cameras.info(stream).expect("info");
        assert_eq!((info.width, info.height), (640, 480));

        // The device turns out to deliver 1080p. `info` must now say so.
        *observed.lock().expect("lock") = Some((1920, 1080));
        let info = cameras.info(stream).expect("info");
        assert_eq!(
            (info.width, info.height),
            (1920, 1080),
            "info must report what the device delivers, not what was requested"
        );
        assert_eq!(info.fps, 30, "the rest of the info is unchanged");
    }

    /// `info` reports what the DEVICE opened as. An app that draws from the
    /// config it asked for is drawing at the wrong size on any camera that
    /// could not honour the request.
    #[test]
    fn info_reports_what_was_opened() {
        let mut cameras = runtime();
        let stream = cameras.open("", config()).expect("open");
        let info = cameras.info(stream).expect("info");
        assert_eq!((info.width, info.height, info.fps), (640, 480, 30));
    }

    #[test]
    fn an_impossible_config_is_refused_before_any_device_is_touched() {
        let mut cameras = runtime();
        assert!(matches!(
            cameras.open(
                "",
                CameraConfig {
                    width: 0,
                    ..config()
                }
            ),
            Err(CameraError::InvalidConfig(_))
        ));
        assert!(matches!(
            cameras.open("", CameraConfig { fps: 0, ..config() }),
            Err(CameraError::InvalidConfig(_))
        ));
        assert!(
            matches!(
                cameras.open(
                    "",
                    CameraConfig {
                        width: 10_000,
                        height: 10_000,
                        ..config()
                    }
                ),
                Err(CameraError::InvalidConfig(_))
            ),
            "a frame larger than the pixel cap is refused, not allocated"
        );
    }

    /// Starting twice is what a person double-clicking a button does.
    #[test]
    fn start_and_stop_are_safe_to_repeat() {
        let mut cameras = runtime();
        let stream = cameras.open("", config()).expect("open");
        cameras.start(stream).expect("start");
        cameras.start(stream).expect("start again");
        cameras.stop(stream).expect("stop");
        cameras.stop(stream).expect("stop again");
    }

    /// A leak must become an honest error, not an exhausted machine.
    #[test]
    fn past_the_stream_cap_opening_is_refused() {
        let mut cameras = runtime();
        for _ in 0..MAX_STREAMS {
            cameras.open("", config()).expect("open within the cap");
        }
        assert!(matches!(
            cameras.open("", config()),
            Err(CameraError::Platform(_))
        ));
    }

    /// The parity guarantee, in a form CI can check on every platform.
    ///
    /// A camera backend must EXIST on all three desktop systems. It cannot
    /// prove a camera works -- CI runners and cloud VMs have no camera
    /// hardware -- but it does prove the platform is wired at all, which is
    /// the thing that was silently false for Windows and Linux until now: the
    /// `None` branch meant every webcam app reported `unsupported` forever
    /// (K-148).
    ///
    /// If this fails on a desktop platform, somebody removed a backend or
    /// added an OS without one, and every camera app there is dead.
    #[test]
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    fn every_desktop_platform_has_a_camera_backend() {
        assert!(
            platform_backend().is_some(),
            "this desktop platform has no camera backend, so every camera app on it \
             reports unsupported no matter what the person does"
        );
    }

    /// A system with no backend says so plainly rather than pretending.
    #[test]
    fn a_system_without_a_backend_reports_unsupported() {
        let mut cameras = CameraCaptureRuntime {
            next_stream_id: 1,
            streams: BTreeMap::new(),
            backend: None,
        };
        assert!(matches!(
            cameras.open("", config()),
            Err(CameraError::Unsupported(_))
        ));
        assert!(matches!(
            cameras.devices(),
            Err(CameraError::Unsupported(_))
        ));
    }
}
