//! The Android video backend (IC-922): MediaPlayer into an ImageReader
//! through the Android player's activity. The platform decodes and plays
//! the sound; the app draws the newest frame by handle, as on Apple systems.

use krate_adapter_android::video as android;
use krate_adapter_common::ui::ImagePixels;

use crate::video::{Control, Source, State};

pub(crate) fn open(source: Source) -> Result<u64, String> {
    let source = match source {
        Source::File(path) => path.to_string_lossy().into_owned(),
        Source::Url(url) => url,
    };
    android::open(&source)
}

pub(crate) fn control(video: u64, control: Control) -> Result<(), String> {
    let (kind, value) = match control {
        Control::Play => (0, 0),
        Control::Pause => (1, 0),
        Control::Seek(millis) => (2, millis.min(i64::MAX as u64) as i64),
        Control::Looping(on) => (3, i64::from(on)),
        Control::Muted(on) => (4, i64::from(on)),
        // Thousandths: the Java side takes a whole number (K-1351).
        Control::Volume(volume) => (5, (volume.clamp(0.0, 1.0) * 1000.0).round() as i64),
    };
    android::control(video, kind, value)
}

pub(crate) fn state(video: u64) -> Result<State, String> {
    let (s, problem) = android::state(video)?;
    let millis = |v: i64| v.max(0) as u64;
    Ok(State {
        position_millis: millis(s[0]),
        duration_millis: millis(s[1]),
        width: s[2].max(0) as u32,
        height: s[3].max(0) as u32,
        ready: s[4] != 0,
        playing: s[5] != 0,
        ended: s[6] != 0,
        problem,
    })
}

/// The frame to show now, or `None` when the one shown last is still right.
pub(crate) fn frame(video: u64) -> Result<Option<ImagePixels>, String> {
    Ok(android::frame(video)
        .and_then(|(rgba, width, height)| ImagePixels::new(width, height, rgba).ok()))
}

pub(crate) fn close(video: u64) {
    android::close(video)
}
