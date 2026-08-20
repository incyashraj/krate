//! The macOS camera backend: AVFoundation behind `CameraBackend`.
//!
//! AVFoundation delivers frames on a dispatch queue of our choosing, as
//! `CMSampleBuffer`s wrapping a `CVPixelBuffer`. This converts each one to the
//! straight-alpha RGBA the canvas already draws and drops it in the shared
//! newest-frame slot. Nothing here decides policy -- the capability check and
//! the newest-frame rule live in `camera_capture`, so a second platform
//! inherits both.
//!
//! Two macOS-specific things worth stating, because getting either wrong is
//! the difference between a working camera and a mystery:
//!
//! - **macOS asks the person too.** Krate's grant is not the system's grant.
//!   `AVCaptureDevice` reports its own status, and a refusal there surfaces as
//!   `SystemDenied` so an app can say "your system settings are blocking the
//!   camera" rather than blaming the Krate wall the person already cleared.
//!
//! - **The app bundle must declare `NSCameraUsageDescription`.** Without that
//!   key macOS terminates the process the instant capture starts, with no
//!   catchable error. It is checked here so the failure is a sentence instead
//!   of a crash.

use std::sync::{Arc, Mutex};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_av_foundation::{
    AVAuthorizationStatus, AVCaptureConnection, AVCaptureDevice, AVCaptureDeviceInput,
    AVCaptureOutput, AVCaptureSession, AVCaptureSessionPreset, AVCaptureVideoDataOutput,
    AVCaptureVideoDataOutputSampleBufferDelegate, AVMediaTypeVideo,
};
use objc2_core_media::CMSampleBuffer;
use objc2_core_video::{
    kCVPixelFormatType_32BGRA, CVPixelBufferGetBaseAddress,
    CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight, CVPixelBufferGetWidth,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
};
use objc2_foundation::{NSDictionary, NSNumber, NSObject, NSObjectProtocol, NSString};

use crate::camera_capture::{
    CameraBackend, CameraConfig, CameraError, DeviceInfo, Frame, FrameFormat, FrameInfo,
    FrameSlot, PlatformStream,
};

/// State the delegate needs to turn a sample buffer into a frame.
struct DelegateState {
    slot: Arc<FrameSlot>,
    started: std::time::Instant,
    /// What the session actually produced, learned from the first frame.
    /// AVFoundation honours a preset, not an exact request, so the true size
    /// is only knowable once a frame arrives.
    observed: Arc<Mutex<Option<(u32, u32)>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "KrateCameraDelegate"]
    #[ivars = DelegateState]
    struct CameraDelegate;

    unsafe impl NSObjectProtocol for CameraDelegate {}

    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for CameraDelegate {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        fn did_output_sample_buffer(
            &self,
            _output: &AVCaptureOutput,
            sample_buffer: &CMSampleBuffer,
            _connection: &AVCaptureConnection,
        ) {
            let state = self.ivars();
            let Some((bytes, width, height)) = rgba_from_sample_buffer(sample_buffer) else {
                return;
            };
            if let Ok(mut observed) = state.observed.lock() {
                *observed = Some((width, height));
            }
            state.slot.put(Frame {
                bytes,
                width,
                height,
                elapsed_millis: state.started.elapsed().as_millis() as u64,
            });
        }
    }
);

/// Convert one BGRA sample buffer to straight-alpha RGBA.
///
/// The session is configured for 32BGRA because that is the format every Mac
/// camera produces without a conversion pass of its own; the swap to RGBA is
/// two byte moves per pixel and keeps the guest side in the one layout the
/// canvas draws.
fn rgba_from_sample_buffer(sample: &CMSampleBuffer) -> Option<(Vec<u8>, u32, u32)> {
    // SAFETY: called on AVFoundation's delivery queue with a live sample
    // buffer, which owns the image buffer for the duration of this call.
    let image = unsafe { sample.image_buffer() }?;

    // SAFETY: the lock/unlock pair brackets every read below, which is what
    // makes the base address valid to dereference.
    unsafe {
        if CVPixelBufferLockBaseAddress(&image, CVPixelBufferLockFlags::ReadOnly) != 0 {
            return None;
        }
    }
    let result = (|| {
        let width = CVPixelBufferGetWidth(&image) as u32;
        let height = CVPixelBufferGetHeight(&image) as u32;
        if width == 0 || height == 0 {
            return None;
        }
        let stride = CVPixelBufferGetBytesPerRow(&image);
        let base = CVPixelBufferGetBaseAddress(&image) as *const u8;
        if base.is_null() {
            return None;
        }

        let mut rgba = vec![0u8; width as usize * height as usize * 4];
        for y in 0..height as usize {
            // SAFETY: row y starts at y*stride and CoreVideo guarantees
            // `height` rows of at least `width * 4` bytes each.
            let row = unsafe { base.add(y * stride) };
            let out = y * width as usize * 4;
            for x in 0..width as usize {
                // SAFETY: x < width, and the row holds width*4 bytes.
                let px = unsafe { row.add(x * 4) };
                // BGRA on the wire, RGBA in the buffer.
                // SAFETY: four bytes within the row, bounds checked above.
                unsafe {
                    rgba[out + x * 4] = *px.add(2);
                    rgba[out + x * 4 + 1] = *px.add(1);
                    rgba[out + x * 4 + 2] = *px;
                    rgba[out + x * 4 + 3] = *px.add(3);
                }
            }
        }
        Some((rgba, width, height))
    })();
    // SAFETY: pairs with the lock above; runs on every path out.
    unsafe {
        CVPixelBufferUnlockBaseAddress(&image, CVPixelBufferLockFlags::ReadOnly);
    }
    result
}

/// Diagnostic trace. Writes to a file as well as stderr, because the path
/// that matters most -- a double-clicked app -- has no terminal attached.
fn camera_trace(line: &str) {
    if std::env::var_os("KRATE_CAMERA_TRACE").is_none() {
        return;
    }
    eprintln!("krate-camera: {line}");
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/krate-camera.log")
    {
        let _ = writeln!(f, "{line}");
    }
}

/// The system's camera authorization for this process, as a word.
pub(crate) fn system_status() -> &'static str {
    // SAFETY: class method taking a constant media type.
    let status =
        unsafe { AVCaptureDevice::authorizationStatusForMediaType(AVMediaTypeVideo.unwrap()) };
    match status {
        AVAuthorizationStatus::NotDetermined => "not-determined",
        AVAuthorizationStatus::Restricted => "restricted",
        AVAuthorizationStatus::Denied => "denied",
        AVAuthorizationStatus::Authorized => "authorized",
        _ => "unknown",
    }
}

#[derive(Default)]
pub struct MacosCameraBackend;

/// One running AVCaptureSession, stopped and released when dropped.
struct MacosStream {
    session: Retained<AVCaptureSession>,
    // Held so the delegate outlives the session that calls it. Dropping this
    // while frames are in flight would be a use-after-free.
    _delegate: Retained<CameraDelegate>,
    _queue: dispatch2::DispatchRetained<dispatch2::DispatchQueue>,
    observed: Arc<Mutex<Option<(u32, u32)>>>,
}

// SAFETY: the session and delegate are only touched through this struct, which
// the runtime owns behind its own borrow rules; AVCaptureSession's start/stop
// are documented as callable from any thread.
unsafe impl Send for MacosStream {}

impl PlatformStream for MacosStream {
    fn start(&mut self) -> Result<(), CameraError> {
        // SAFETY: the session is live for as long as this struct is.
        unsafe { self.session.startRunning() };
        Ok(())
    }

    fn stop(&mut self) -> Result<(), CameraError> {
        // SAFETY: as above; stopping a stopped session is a no-op in AVF.
        unsafe { self.session.stopRunning() };
        Ok(())
    }

    /// The size the camera actually delivered, learned from the first frame.
    fn observed_size(&self) -> Option<(u32, u32)> {
        self.observed.lock().ok().and_then(|size| *size)
    }
}

impl Drop for MacosStream {
    fn drop(&mut self) {
        // The camera light stays on until the session stops, so an app that
        // forgets to close still releases the device when the run ends.
        // SAFETY: the session is live until this struct is gone.
        unsafe { self.session.stopRunning() };
    }
}

impl MacosCameraBackend {
    /// Whether macOS itself will allow this process to use the camera.
    ///
    /// `NotDetermined` is treated as allowed: creating the device input is
    /// what makes macOS show its own prompt, and refusing here would mean the
    /// person is never asked at all.
    fn system_permission() -> Result<(), CameraError> {
        // SAFETY: a class method taking a constant media type.
        let status =
            unsafe { AVCaptureDevice::authorizationStatusForMediaType(AVMediaTypeVideo.unwrap()) };
        match status {
            AVAuthorizationStatus::Denied | AVAuthorizationStatus::Restricted => {
                Err(CameraError::SystemDenied)
            }
            AVAuthorizationStatus::Authorized => Ok(()),
            // Never asked. AVFoundation only prompts on its own when an input
            // is created on the main thread with a run loop turning, which a
            // Krate app is not doing at this point -- so relying on that left
            // the status at not-determined forever, the session running, and
            // not one frame ever delivered. Measured: ten seconds of polling,
            // no prompt, no frames, no error. Ask explicitly instead.
            _ => Self::request_system_permission(),
        }
    }

    /// Ask macOS for camera access, and do NOT wait for the answer.
    ///
    /// The prompt is a modal dialog. Blocking `open` until it is answered
    /// freezes the app that called it -- no frames, no repaint, no reaction to
    /// clicks -- for as long as the person takes to read it. That is the same
    /// "the app looks frozen" failure a blocking network fetch causes, and it
    /// is worse here because the thing the person must click is on top of the
    /// window that is frozen behind it.
    ///
    /// So the request is fired and the call returns straight away. The stream
    /// opens, `start` runs, and AVFoundation simply delivers nothing until
    /// access is granted -- at which point frames begin, with no further call
    /// from the app. The app's own loop keeps turning throughout, which is why
    /// the pack tells it to hold the last frame and keep drawing: "waiting for
    /// the camera" is a state it can paint, not a stall it has to survive.
    ///
    /// Called only when the status is `NotDetermined`. A real refusal is
    /// already `Denied` and reported as `SystemDenied` before reaching here.
    fn request_system_permission() -> Result<(), CameraError> {
        // The block must outlive this call: macOS invokes it whenever the
        // person answers, which may be a minute later. `RcBlock` is
        // reference-counted and AVFoundation copies it, so handing it over and
        // dropping our handle is correct -- the callback keeps it alive.
        let handler = block2::RcBlock::new(|_granted: objc2::runtime::Bool| {
            // Nothing to do. The status the next call reads is the answer, and
            // frames start on their own once access is granted.
        });
        // SAFETY: a class method taking a constant media type and a block with
        // the documented signature.
        unsafe {
            AVCaptureDevice::requestAccessForMediaType_completionHandler(
                AVMediaTypeVideo.unwrap(),
                &handler,
            );
        }
        Ok(())
    }

    /// macOS kills a process that starts capture without this Info.plist key,
    /// with no error the app can catch. Say so instead.
    fn usage_description_declared() -> Result<(), CameraError> {
        let key = NSString::from_str("NSCameraUsageDescription");
        // SAFETY: main bundle lookup with a valid key; nil when absent.
        let value: Option<Retained<NSObject>> = unsafe {
            let bundle: *mut NSObject = msg_send![objc2::class!(NSBundle), mainBundle];
            let object: *mut NSObject = msg_send![bundle, objectForInfoDictionaryKey: &*key];
            Retained::retain(object)
        };
        if value.is_none() {
            return Err(CameraError::Unsupported(
                "this app bundle does not declare NSCameraUsageDescription, which macOS \
                 requires before any process may open a camera"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

impl CameraBackend for MacosCameraBackend {
    fn devices(&self) -> Result<Vec<DeviceInfo>, CameraError> {
        Self::system_permission()?;
        // SAFETY: class method with a constant media type; None when the Mac
        // has no camera, which is an empty list rather than an error.
        let device =
            unsafe { AVCaptureDevice::defaultDeviceWithMediaType(AVMediaTypeVideo.unwrap()) };
        let Some(device) = device else {
            return Ok(Vec::new());
        };
        // SAFETY: reading two string properties off a live device.
        let (id, label) = unsafe { (device.uniqueID(), device.localizedName()) };
        Ok(vec![DeviceInfo {
            id: id.to_string(),
            label: label.to_string(),
        }])
    }

    fn open(
        &mut self,
        device: &str,
        config: CameraConfig,
        slot: Arc<FrameSlot>,
    ) -> Result<(FrameInfo, Box<dyn PlatformStream>), CameraError> {
        camera_trace(&format!("open, status={}", system_status()));
        if let Err(err) = Self::usage_description_declared() {
            camera_trace(&format!("usage description missing: {err:?}"));
            return Err(err);
        }
        if let Err(err) = Self::system_permission() {
            camera_trace(&format!("system permission: {err:?}"));
            return Err(err);
        }
        camera_trace(&format!("past permission, status={}", system_status()));

        // SAFETY: both are class methods; a missing device is None, and an
        // unknown id is a device that is gone rather than a fault.
        let device = unsafe {
            if device.is_empty() {
                AVCaptureDevice::defaultDeviceWithMediaType(AVMediaTypeVideo.unwrap())
            } else {
                AVCaptureDevice::deviceWithUniqueID(&NSString::from_str(device))
            }
        }
        .ok_or(CameraError::DeviceUnavailable)?;

        // SAFETY: constructing a session and an input for a live device. The
        // input initialiser reports a failure as an Objective-C error, which
        // objc2 surfaces as Err.
        let session = unsafe { AVCaptureSession::new() };
        let input = unsafe { AVCaptureDeviceInput::initWithDevice_error(
            AVCaptureDeviceInput::alloc(),
            &device,
        ) }
        .map_err(|err| CameraError::Platform(err.localizedDescription().to_string()))?;

        // SAFETY: adding an input the session accepts; asked first so a
        // refusal is an error rather than an AVFoundation exception.
        unsafe {
            if !session.canAddInput(&input) {
                return Err(CameraError::DeviceUnavailable);
            }
            session.addInput(&input);
        }

        // A preset near what the app asked for. AVFoundation does not take an
        // arbitrary size, so this picks the closest standard one and the true
        // dimensions are read back from the first frame.
        let preset = preset_for(config.width, config.height);
        // SAFETY: setting a supported preset on a configured session.
        unsafe {
            if session.canSetSessionPreset(preset) {
                session.setSessionPreset(preset);
            }
        }

        let output = unsafe { AVCaptureVideoDataOutput::new() };
        // 32BGRA is what every Mac camera produces natively, so asking for it
        // avoids a conversion pass inside AVFoundation before ours.
        let format = NSNumber::new_u32(kCVPixelFormatType_32BGRA);
        // The CoreVideo constant is a `CFString`; `NSDictionary` wants an
        // `NSString` key. The two are toll-free bridged and this key's name is
        // part of the published CoreVideo API, so naming it directly is
        // equivalent and avoids a bridging cast for one string.
        let key = NSString::from_str("PixelFormatType");
        let settings = NSDictionary::from_slices(&[&*key], &[format.as_ref()]);
        // SAFETY: settings dictionary matches the documented key and type.
        unsafe {
            output.setVideoSettings(Some(&settings));
            // The newest-frame rule, enforced by AVFoundation as well as by
            // the slot: a late frame is dropped rather than queued.
            output.setAlwaysDiscardsLateVideoFrames(true);
        }

        let observed = Arc::new(Mutex::new(None));
        let delegate = CameraDelegate::alloc().set_ivars(DelegateState {
            slot,
            started: std::time::Instant::now(),
            observed: observed.clone(),
        });
        let delegate: Retained<CameraDelegate> = unsafe { msg_send![super(delegate), init] };

        // A serial queue of our own, so frame delivery never lands on the main
        // thread and cannot compete with the window's event loop.
        let queue = dispatch2::DispatchQueue::new("tech.krate.camera", None);
        // SAFETY: delegate conforms to the protocol; queue outlives the output
        // because both are held in the returned stream.
        unsafe {
            output.setSampleBufferDelegate_queue(
                Some(ProtocolObject::from_ref(&*delegate)),
                Some(&queue),
            );
            if !session.canAddOutput(&output) {
                return Err(CameraError::Platform(
                    "this camera cannot deliver video frames to Krate".to_string(),
                ));
            }
            session.addOutput(&output);
        }

        // What was asked for, until a frame proves otherwise. `info` is read
        // again by the app after the first frame, which is why the WIT tells
        // apps to draw from `info` rather than from their own request.
        let info = FrameInfo {
            width: config.width,
            height: config.height,
            fps: config.fps,
            format: FrameFormat::Rgba8,
        };

        Ok((
            info,
            Box::new(MacosStream {
                session,
                _delegate: delegate,
                _queue: queue,
                observed,
            }),
        ))
    }
}

/// The standard preset closest to a requested size.
///
/// AVFoundation takes presets, not arbitrary dimensions. Picking the nearest
/// one and reporting the truth back beats refusing a request a camera could
/// nearly satisfy.
fn preset_for(width: u32, height: u32) -> &'static AVCaptureSessionPreset {
    let pixels = width.saturating_mul(height);
    unsafe {
        if pixels >= 1920 * 1080 {
            objc2_av_foundation::AVCaptureSessionPreset1920x1080
        } else if pixels >= 1280 * 720 {
            objc2_av_foundation::AVCaptureSessionPreset1280x720
        } else if pixels >= 640 * 480 {
            objc2_av_foundation::AVCaptureSessionPreset640x480
        } else {
            objc2_av_foundation::AVCaptureSessionPreset352x288
        }
    }
}


