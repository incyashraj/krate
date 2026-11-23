//! Video playback behind `krate:gfx/video` (IC-922).
//!
//! A player decodes and plays the sound; the app only asks for the current
//! frame by handle when it draws. Each player supplies `open`, `control`,
//! `state`, `frame` and `close`; this module hands every call to the one
//! that holds the video, under handles of its own, so a Mac can move a
//! video AVFoundation cannot open to the portable player (K-1348) without
//! the app noticing. A system with neither (a browser tab's runtime, which
//! plays video in the page instead) answers every call with `UNSUPPORTED`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use krate_adapter_common::ui::ImagePixels;

/// What to play.
#[derive(Debug, Clone)]
pub(crate) enum Source {
    /// A file, already resolved and checked (a bundled asset, a picked
    /// folder's file, a file in the app's own folder).
    File(PathBuf),
    /// An `https://` address the app was granted `net.connect` to.
    Url(String),
}

/// One thing to tell a player.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Control {
    Play,
    Pause,
    Seek(u64),
    Looping(bool),
    Muted(bool),
    /// How loud, 0.0 (silent) to 1.0 (as recorded) (K-1351).
    Volume(f32),
}

/// Where a video is and what it is doing; the WIT `video-state`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct State {
    pub position_millis: u64,
    pub duration_millis: u64,
    pub width: u32,
    pub height: u32,
    pub ready: bool,
    pub playing: bool,
    pub ended: bool,
    pub problem: String,
}

// The system's own player: AVPlayer on a Mac and an iPhone (IC-922),
// MediaPlayer into an ImageReader on Android.
#[cfg(target_os = "android")]
use crate::video_android as native;
#[cfg(any(target_os = "macos", target_os = "ios"))]
use crate::video_apple as native;

// The portable player (K-1348): every computer. Windows and Linux have no
// native player behind this; a Mac falls back to it for a file AVFoundation
// cannot open (an Ogg sound, Vorbis in WebM), or uses it outright with
// KRATE_VIDEO_PORTABLE=1.
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
use crate::video_portable as portable;

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
const UNSUPPORTED: &str = "video playback is not on this system yet";

/// Which player holds a video.
#[derive(Debug, Clone, Copy)]
enum Player {
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
    Native(u64),
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    Portable(u64),
}

/// What the app last asked of a video, so a player taking over (the
/// fallback) starts where the first one was.
#[derive(Debug, Clone, Copy, Default)]
struct Asked {
    playing: bool,
    looping: bool,
    muted: bool,
    volume: Option<f32>,
    seek: Option<u64>,
}

#[derive(Debug, Clone)]
// The source is read only where a fallback exists.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
struct Entry {
    player: Player,
    source: Source,
    asked: Asked,
}

fn entries() -> &'static Mutex<BTreeMap<u64, Entry>> {
    static ENTRIES: std::sync::OnceLock<Mutex<BTreeMap<u64, Entry>>> = std::sync::OnceLock::new();
    ENTRIES.get_or_init(Default::default)
}

static NEXT_VIDEO: AtomicU64 = AtomicU64::new(1);

fn entry(video: u64) -> Result<Entry, String> {
    entries()
        .lock()
        .map_err(|_| "the video table is unavailable".to_string())?
        .get(&video)
        .cloned()
        .ok_or_else(|| "that video is not open".to_string())
}

/// True when the portable player is asked for outright.
#[cfg(target_os = "macos")]
fn portable_asked() -> bool {
    std::env::var("KRATE_VIDEO_PORTABLE").is_ok_and(|value| value == "1")
}

#[allow(unreachable_code, unused_variables)]
fn open_player(source: &Source) -> Result<Player, String> {
    #[cfg(target_os = "macos")]
    if portable_asked() {
        return portable::open(source.clone()).map(Player::Portable);
    }
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
    return native::open(source.clone()).map(Player::Native);
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    return portable::open(source.clone()).map(Player::Portable);
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows",
        target_os = "linux"
    )))]
    Err(UNSUPPORTED.to_string())
}

pub(crate) fn open(source: Source) -> Result<u64, String> {
    let player = open_player(&source)?;
    let video = NEXT_VIDEO.fetch_add(1, Ordering::Relaxed);
    entries()
        .lock()
        .map_err(|_| "the video table is unavailable".to_string())?
        .insert(
            video,
            Entry {
                player,
                source,
                asked: Asked::default(),
            },
        );
    Ok(video)
}

fn control_player(player: Player, control: Control) -> Result<(), String> {
    match player {
        #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
        Player::Native(id) => native::control(id, control),
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        Player::Portable(id) => portable::control(id, control),
    }
}

pub(crate) fn control(video: u64, control: Control) -> Result<(), String> {
    let player = {
        let mut table = entries()
            .lock()
            .map_err(|_| "the video table is unavailable".to_string())?;
        let entry = table
            .get_mut(&video)
            .ok_or_else(|| "that video is not open".to_string())?;
        match control {
            Control::Play => entry.asked.playing = true,
            Control::Pause => entry.asked.playing = false,
            Control::Seek(millis) => entry.asked.seek = Some(millis),
            Control::Looping(looping) => entry.asked.looping = looping,
            Control::Muted(muted) => entry.asked.muted = muted,
            Control::Volume(volume) => entry.asked.volume = Some(volume),
        }
        entry.player
    };
    // Outside the table's lock: a phone's player answers from its main thread.
    control_player(player, control)
}

/// A Mac's AVPlayer could not open the file: hand it to the portable
/// player, told what the app had asked so far, and answer for that one.
#[cfg(target_os = "macos")]
fn fall_back(video: u64, native_id: u64, entry: &Entry) -> Option<State> {
    let portable_id = portable::open(entry.source.clone()).ok()?;
    let asked = entry.asked;
    let mut replay = vec![Control::Looping(asked.looping), Control::Muted(asked.muted)];
    replay.extend(asked.volume.map(Control::Volume));
    replay.extend(asked.seek.map(Control::Seek));
    if asked.playing {
        replay.push(Control::Play);
    }
    for control in replay {
        let _ = portable::control(portable_id, control);
    }
    if let Ok(mut table) = entries().lock() {
        if let Some(entry) = table.get_mut(&video) {
            entry.player = Player::Portable(portable_id);
        }
    }
    native::close(native_id);
    portable::state(portable_id).ok()
}

pub(crate) fn state(video: u64) -> Result<State, String> {
    let entry = entry(video)?;
    match entry.player {
        #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
        Player::Native(id) => {
            let state = native::state(id)?;
            #[cfg(target_os = "macos")]
            if !state.problem.is_empty() {
                if let Some(state) = fall_back(video, id, &entry) {
                    return Ok(state);
                }
            }
            Ok(state)
        }
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        Player::Portable(id) => portable::state(id),
    }
}

/// The frame to show now, or `None` when the one shown last is still right.
pub(crate) fn frame(video: u64) -> Result<Option<ImagePixels>, String> {
    match entry(video)?.player {
        #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
        Player::Native(id) => native::frame(id),
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        Player::Portable(id) => portable::frame(id),
    }
}

pub(crate) fn close(video: u64) {
    let Some(entry) = entries()
        .lock()
        .ok()
        .and_then(|mut table| table.remove(&video))
    else {
        return;
    };
    match entry.player {
        #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
        Player::Native(id) => native::close(id),
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        Player::Portable(id) => portable::close(id),
    }
}

/// The videos one run opened. A run controls only its own, and every one it
/// left open stops when the run ends, so a closed app is never heard.
#[derive(Debug, Default)]
pub(crate) struct Videos {
    open: BTreeSet<u64>,
}

impl Videos {
    pub(crate) fn add(&mut self, video: u64) {
        self.open.insert(video);
    }

    pub(crate) fn owns(&self, video: u64) -> bool {
        self.open.contains(&video)
    }

    pub(crate) fn remove(&mut self, video: u64) -> bool {
        self.open.remove(&video)
    }
}

impl Drop for Videos {
    fn drop(&mut self) {
        for video in std::mem::take(&mut self.open) {
            close(video);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_owns_only_the_videos_it_opened() {
        let mut videos = Videos::default();
        videos.add(7);
        assert!(videos.owns(7));
        assert!(!videos.owns(8), "another run's video is not this run's");
        assert!(videos.remove(7));
        assert!(!videos.owns(7), "a closed video is gone");
        assert!(!videos.remove(7));
    }
}
