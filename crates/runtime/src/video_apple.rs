//! Video on a Mac and an iPhone: AVPlayer behind `krate:gfx/video` (IC-922).
//!
//! AVPlayer decodes in hardware and plays the sound; an
//! `AVPlayerItemVideoOutput` hands over the frame for the item's current
//! time as a 32BGRA pixel buffer, converted to RGBA by the camera's own
//! conversion. AVPlayer and AVPlayerItem belong to the main thread, so every
//! player lives in a main-thread table and every call runs there: inline
//! when the app already runs on the main thread (a Mac window, a headless
//! run), dispatched and waited for when it does not (an iPhone, where the
//! main thread runs UIKit).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::AllocAnyThread;
use objc2_av_foundation::{
    AVPlayer, AVPlayerActionAtItemEnd, AVPlayerItem, AVPlayerItemStatus, AVPlayerItemVideoOutput,
    AVPlayerTimeControlStatus,
};
use objc2_core_media::{kCMTimeZero, CMTime};
use objc2_core_video::kCVPixelFormatType_32BGRA;
use objc2_foundation::{MainThreadMarker, NSDictionary, NSNumber, NSString, NSURL};

use krate_adapter_common::ui::ImagePixels;

use crate::video::{Control, Source, State};

struct Player {
    player: Retained<AVPlayer>,
    item: Retained<AVPlayerItem>,
    output: Retained<AVPlayerItemVideoOutput>,
    looping: bool,
}

thread_local! {
    /// Every open player, on the main thread, by handle.
    static PLAYERS: RefCell<HashMap<u64, Player>> = RefCell::new(HashMap::new());
}

static NEXT_VIDEO: AtomicU64 = AtomicU64::new(1);

/// Run `f` on the main thread and hand back what it returned.
fn on_main<T: Send + 'static>(
    f: impl FnOnce(MainThreadMarker) -> T + Send + 'static,
) -> Result<T, String> {
    if let Some(mtm) = MainThreadMarker::new() {
        // AVFoundation reports loading and the clock on the main queue. A
        // window's event loop drains it; a headless run has none, so give
        // the run loop one turn first or the video never becomes ready.
        turn_run_loop();
        return Ok(f(mtm));
    }
    let (tx, rx) = std::sync::mpsc::channel();
    dispatch2::DispatchQueue::main().exec_async(move || {
        // SAFETY: the main queue runs on the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let _ = tx.send(f(mtm));
    });
    rx.recv_timeout(Duration::from_secs(5))
        .map_err(|_| "the video player did not answer".to_string())
}

#[allow(non_upper_case_globals)]
extern "C" {
    static kCFRunLoopDefaultMode: *const std::ffi::c_void;
    fn CFRunLoopRunInMode(
        mode: *const std::ffi::c_void,
        seconds: f64,
        return_after_source_handled: u8,
    ) -> i32;
}

/// Run what is waiting on the main run loop, without waiting for more.
fn turn_run_loop() {
    // SAFETY: called on the main thread; a zero-second run returns at once
    // once the pending sources and main-queue blocks have run.
    unsafe {
        CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.0, 0);
    }
}

fn with_player<T>(video: u64, f: impl FnOnce(&mut Player) -> T) -> Result<T, String> {
    PLAYERS.with(|players| {
        players
            .borrow_mut()
            .get_mut(&video)
            .map(f)
            .ok_or_else(|| "that video is not open".to_string())
    })
}

fn seconds(time: CMTime) -> f64 {
    // SAFETY: CMTimeGetSeconds reads a plain struct.
    let value = unsafe { time.seconds() };
    if value.is_finite() && value > 0.0 {
        value
    } else {
        0.0
    }
}

fn millis(time: CMTime) -> u64 {
    (seconds(time) * 1000.0).round() as u64
}

fn at(millis: u64) -> CMTime {
    // SAFETY: CMTimeMakeWithSeconds builds a plain struct.
    unsafe { CMTime::with_seconds(millis as f64 / 1000.0, 600) }
}

fn zero() -> CMTime {
    // SAFETY: a constant CoreMedia exports.
    unsafe { kCMTimeZero }
}

/// True when the item sits at its end (within a frame).
fn at_end(player: &Player) -> bool {
    // SAFETY: main-thread reads of a live item.
    let (position, duration) = unsafe {
        (
            seconds(player.item.currentTime()),
            seconds(player.item.duration()),
        )
    };
    duration > 0.0 && position >= duration - 0.04
}

/// Start a looping video again once it reaches the end.
fn keep_looping(player: &Player) {
    if player.looping && at_end(player) {
        // SAFETY: main-thread calls on a live player.
        unsafe {
            player
                .player
                .seekToTime_toleranceBefore_toleranceAfter(zero(), zero(), zero());
            player.player.play();
        }
    }
}

pub(crate) fn open(source: Source) -> Result<u64, String> {
    let video = NEXT_VIDEO.fetch_add(1, Ordering::Relaxed);
    on_main(move |mtm| {
        let url = match &source {
            Source::File(path) => {
                let path = path
                    .to_str()
                    .ok_or_else(|| "the video's path is not text".to_string())?;
                NSURL::fileURLWithPath(&NSString::from_str(path))
            }
            Source::Url(address) => NSURL::URLWithString(&NSString::from_str(address))
                .ok_or_else(|| format!("{address} is not an address"))?,
        };
        // 32BGRA, the layout the camera's conversion reads. The CoreVideo key
        // is a CFString toll-free bridged to this NSString.
        let key = NSString::from_str("PixelFormatType");
        let format = NSNumber::new_u32(kCVPixelFormatType_32BGRA);
        let value: &AnyObject = format.as_ref();
        let attributes = NSDictionary::from_slices(&[&*key], &[value]);
        // SAFETY: main-thread construction with documented attribute keys.
        let player = unsafe {
            let item = AVPlayerItem::playerItemWithURL(&url, mtm);
            let output = AVPlayerItemVideoOutput::initWithPixelBufferAttributes(
                AVPlayerItemVideoOutput::alloc(),
                Some(&attributes),
            );
            item.addOutput(&output);
            let player = AVPlayer::playerWithPlayerItem(Some(&item), mtm);
            player.setActionAtItemEnd(AVPlayerActionAtItemEnd::Pause);
            Player {
                player,
                item,
                output,
                looping: false,
            }
        };
        PLAYERS.with(|players| players.borrow_mut().insert(video, player));
        Ok(video)
    })?
}

pub(crate) fn control(video: u64, control: Control) -> Result<(), String> {
    on_main(move |_| {
        with_player(video, |player| {
            // SAFETY: main-thread calls on a live player.
            unsafe {
                match control {
                    Control::Play => {
                        // Play at the end starts again, as every player does.
                        if at_end(player) {
                            player.player.seekToTime_toleranceBefore_toleranceAfter(
                                zero(),
                                zero(),
                                zero(),
                            );
                        }
                        player.player.play();
                    }
                    Control::Pause => player.player.pause(),
                    Control::Seek(millis) => player
                        .player
                        .seekToTime_toleranceBefore_toleranceAfter(at(millis), zero(), zero()),
                    Control::Looping(looping) => player.looping = looping,
                    Control::Muted(muted) => player.player.setMuted(muted),
                    Control::Volume(volume) => player.player.setVolume(volume.clamp(0.0, 1.0)),
                }
            }
        })
    })?
}

pub(crate) fn state(video: u64) -> Result<State, String> {
    on_main(move |_| {
        with_player(video, |player| {
            keep_looping(player);
            // SAFETY: main-thread reads of a live player and item.
            unsafe {
                let status = player.item.status();
                let size = player.item.presentationSize();
                let playing =
                    player.player.timeControlStatus() != AVPlayerTimeControlStatus::Paused;
                let problem = if status == AVPlayerItemStatus::Failed {
                    player
                        .item
                        .error()
                        .map(|error| error.localizedDescription().to_string())
                        .unwrap_or_else(|| "this video cannot be played".to_string())
                } else {
                    String::new()
                };
                State {
                    position_millis: millis(player.item.currentTime()),
                    duration_millis: millis(player.item.duration()),
                    width: size.width.max(0.0).round() as u32,
                    height: size.height.max(0.0).round() as u32,
                    ready: status == AVPlayerItemStatus::ReadyToPlay,
                    playing,
                    ended: !playing && !player.looping && at_end(player),
                    problem,
                }
            }
        })
    })?
}

pub(crate) fn frame(video: u64) -> Result<Option<ImagePixels>, String> {
    on_main(move |_| {
        with_player(video, |player| {
            keep_looping(player);
            // SAFETY: main-thread reads of a live item and its output; the
            // pixel buffer is owned by the Retained for this scope.
            unsafe {
                let now = player.item.currentTime();
                if !player.output.hasNewPixelBufferForItemTime(now) {
                    return None;
                }
                let buffer = player
                    .output
                    .copyPixelBufferForItemTime_itemTimeForDisplay(now, std::ptr::null_mut())?;
                let (rgba, width, height) = crate::camera_macos::rgba_from_pixel_buffer(&buffer)?;
                ImagePixels::new(width, height, rgba).ok()
            }
        })
    })?
}

pub(crate) fn close(video: u64) {
    let _ = on_main(move |_| {
        if let Some(player) = PLAYERS.with(|players| players.borrow_mut().remove(&video)) {
            // SAFETY: main-thread calls on a live player.
            unsafe {
                player.player.pause();
                player.player.replaceCurrentItemWithPlayerItem(None);
            }
        }
    });
}
