//! Video and sound on a computer with no native player for them (K-1348):
//! Windows and Linux always, and a Mac when AVFoundation cannot open a file
//! (an Ogg or a WebM sound) or when `KRATE_VIDEO_PORTABLE=1` asks for it.
//!
//! Everything is Rust or built from source with the runtime: symphonia
//! demuxes and decodes the sound, `mp4` demuxes the picture and OpenH264
//! decodes H.264, so a fresh machine needs no codec pack, no GStreamer and
//! no Media Foundation extension. Each video runs on its own thread; the
//! app's thread only posts a control and reads the state and the newest
//! frame, so drawing never waits on a decoder.
//!
//! Time is kept by the sound (clock.rs): a frame is shown once the speakers
//! have played up to it, a frame that is already late when decoded is never
//! converted, and of several frames due at once only the newest is shown.

mod clock;
mod picture;
mod sound;
mod source;

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use krate_adapter_common::ui::ImagePixels;

use crate::video::{Control, Source, State};
use clock::Clock;
use picture::{Found, Frame, Picture};
use sound::{Resampler, Sound, Speaker, Stretch};

/// Most portable videos open at once, across every app: each holds a
/// thread, a decoder and a sound output.
const MAX_OPEN: usize = 16;
/// Sound kept queued ahead of the speakers.
const SOUND_AHEAD_MILLIS: f64 = 300.0;
/// A frame later than this when decoded is dropped, never converted.
const LATE_MILLIS: f64 = 40.0;

/// Whether the speakers are used: tests run without them, so they make no
/// noise and keep the wall's time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Output {
    Speakers,
    #[cfg_attr(not(test), allow(dead_code))]
    Silent,
}

#[derive(Default)]
struct Inner {
    commands: Vec<Control>,
    closed: bool,
    state: State,
    /// The newest frame not yet drawn.
    frame: Option<ImagePixels>,
}

#[derive(Default)]
struct Shared {
    inner: Mutex<Inner>,
    wake: Condvar,
}

fn players() -> &'static Mutex<BTreeMap<u64, Arc<Shared>>> {
    static PLAYERS: std::sync::OnceLock<Mutex<BTreeMap<u64, Arc<Shared>>>> =
        std::sync::OnceLock::new();
    PLAYERS.get_or_init(Default::default)
}

static NEXT_VIDEO: AtomicU64 = AtomicU64::new(1);

fn shared(video: u64) -> Result<Arc<Shared>, String> {
    players()
        .lock()
        .map_err(|_| "the video player stopped".to_string())?
        .get(&video)
        .cloned()
        .ok_or_else(|| "that video is not open".to_string())
}

pub(crate) fn open(source: Source) -> Result<u64, String> {
    open_with(source, Output::Speakers)
}

pub(crate) fn open_with(source: Source, output: Output) -> Result<u64, String> {
    let mut table = players()
        .lock()
        .map_err(|_| "the video player stopped".to_string())?;
    if table.len() >= MAX_OPEN {
        return Err(format!(
            "{MAX_OPEN} videos are already open; close one before opening another"
        ));
    }
    let video = NEXT_VIDEO.fetch_add(1, Ordering::Relaxed);
    let shared = Arc::new(Shared::default());
    let worker = shared.clone();
    std::thread::Builder::new()
        .name(format!("krate-video-{video}"))
        .spawn(move || {
            let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                play(&worker, &source, output)
            }));
            if run.is_err() {
                if let Ok(mut inner) = worker.inner.lock() {
                    inner.state.playing = false;
                    inner.state.problem = "the video player stopped on this file".into();
                }
            }
        })
        .map_err(|error| format!("the video player could not start: {error}"))?;
    table.insert(video, shared);
    Ok(video)
}

pub(crate) fn control(video: u64, control: Control) -> Result<(), String> {
    let shared = shared(video)?;
    let mut inner = shared
        .inner
        .lock()
        .map_err(|_| "the video player stopped".to_string())?;
    // Read back at once as the app expects: a paused video says paused on
    // the next state read, not a frame later.
    match control {
        Control::Play => {
            inner.state.playing = inner.state.problem.is_empty() || inner.state.ready;
            inner.state.ended = false;
        }
        Control::Pause => inner.state.playing = false,
        Control::Seek(millis) => {
            inner.state.position_millis = if inner.state.duration_millis > 0 {
                millis.min(inner.state.duration_millis)
            } else {
                millis
            };
            inner.state.ended = false;
        }
        _ => {}
    }
    inner.commands.push(control);
    shared.wake.notify_all();
    Ok(())
}

pub(crate) fn state(video: u64) -> Result<State, String> {
    let shared = shared(video)?;
    let inner = shared
        .inner
        .lock()
        .map_err(|_| "the video player stopped".to_string())?;
    Ok(inner.state.clone())
}

/// The frame to show now, or `None` when the one shown last is still right.
pub(crate) fn frame(video: u64) -> Result<Option<ImagePixels>, String> {
    let shared = shared(video)?;
    let mut inner = shared
        .inner
        .lock()
        .map_err(|_| "the video player stopped".to_string())?;
    Ok(inner.frame.take())
}

pub(crate) fn close(video: u64) {
    let Some(shared) = players()
        .lock()
        .ok()
        .and_then(|mut table| table.remove(&video))
    else {
        return;
    };
    // The thread sees this on its next turn and ends, its sound with it.
    // Not joined: a read from a slow server must not hold the app up.
    if let Ok(mut inner) = shared.inner.lock() {
        inner.closed = true;
    }
    shared.wake.notify_all();
}

/// Which decoded frame to show now: the newest one due. `next` is the
/// frame decoded but not yet due; `decode` gives the one after it. Returns
/// the frame to show and how many due frames were passed over for a newer
/// one (late frames, dropped).
fn take_due(
    next: &mut Option<Frame>,
    now: f64,
    mut decode: impl FnMut() -> Result<Option<Frame>, String>,
) -> Result<(Option<Frame>, usize), String> {
    let mut shown: Option<Frame> = None;
    let mut dropped = 0usize;
    // Bounded: after a long stall, catch up a few frames a turn, not the
    // whole video in one.
    for _ in 0..12 {
        if next.is_none() {
            *next = decode()?;
        }
        match next.take() {
            Some(frame) if frame.millis <= now => {
                if shown.is_some() {
                    dropped += 1;
                }
                if frame.pixels.is_some() || shown.is_none() {
                    shown = Some(frame);
                } else {
                    dropped += 1;
                }
            }
            other => {
                *next = other;
                break;
            }
        }
    }
    Ok((shown.filter(|frame| frame.pixels.is_some()), dropped))
}

type Reader = Box<dyn symphonia::core::io::MediaSource>;

struct Player {
    picture: Option<Picture<Reader>>,
    /// The next frame decoded, waiting for its time.
    next: Option<Frame>,
    picture_done: bool,
    sound: Option<Sound>,
    resampler: Option<Resampler>,
    /// The sound at the playback rate, pitch kept (K-1357).
    stretch: Option<Stretch>,
    /// The playback rate, 0.25 to 4.0.
    speed: f64,
    speaker: Option<Speaker>,
    sound_done: bool,
    clock: Clock,
    duration: f64,
    looping: bool,
    muted: bool,
    volume: f32,
    ended: bool,
    shown_once: bool,
    dropped: usize,
}

/// Open the media: its picture (from an MP4-family file) and its sound.
fn load(source: &Source, output: Output) -> Result<(Player, String), String> {
    let mut problem = String::new();
    let mut head = [0u8; 12];
    let mut reader = source::open(source)?;
    let size = source::length(reader.as_mut())?;
    let read = reader.read(&mut head).unwrap_or(0);
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    let picture = if picture::looks_like_mp4(&head[..read]) {
        match Picture::find(reader, size)? {
            Found::Picture(picture) => Some(*picture),
            Found::Unplayable(words) => {
                problem = words;
                None
            }
            Found::None => None,
        }
    } else {
        None
    };
    let sound = Sound::open(source::open(source)?, source::extension(source).as_deref());
    let sound = match (sound, &picture) {
        (Ok(sound), _) => sound,
        // A picture whose sound cannot be read still plays, silent.
        (Err(_), Some(_)) => None,
        (Err(error), None) if problem.is_empty() => return Err(error),
        (Err(_), None) => None,
    };
    if picture.is_none() && sound.is_none() && problem.is_empty() {
        return Err("this file holds no sound or picture Krate can play".into());
    }
    let speaker = match (output, &sound) {
        (Output::Speakers, Some(_)) => Speaker::open(),
        _ => None,
    };
    let clock = match &speaker {
        Some(speaker) => Clock::sound(speaker.rate, speaker.played()),
        None => Clock::wall(),
    };
    tracing::debug!(
        picture = picture
            .as_ref()
            .map(|p| format!("H.264 {}x{}", p.width, p.height)),
        sound = sound
            .as_ref()
            .map(|s| format!("{} Hz x{}", s.rate, s.channels)),
        clock = if speaker.is_some() {
            "speakers"
        } else {
            "wall"
        },
        "portable video opened"
    );
    let duration = picture
        .as_ref()
        .map_or(0.0, |picture| picture.duration_millis)
        .max(sound.as_ref().map_or(0.0, |sound| sound.duration_millis));
    Ok((
        Player {
            picture,
            next: None,
            picture_done: false,
            sound,
            resampler: None,
            speaker,
            sound_done: false,
            clock,
            duration,
            looping: false,
            muted: false,
            volume: 1.0,
            stretch: None,
            speed: 1.0,
            ended: false,
            shown_once: false,
            dropped: 0,
        },
        problem,
    ))
}

impl Player {
    fn played(&self) -> u64 {
        self.speaker.as_ref().map_or(0, Speaker::played)
    }

    fn now(&self) -> f64 {
        self.clock.millis(self.played(), Instant::now())
    }

    fn set_running(&mut self, running: bool) {
        let (played, now) = (self.played(), Instant::now());
        if running {
            self.clock.start(played, now);
        } else {
            self.clock.pause(played, now);
        }
        if let Some(speaker) = &self.speaker {
            speaker.set_running(running);
        }
    }

    fn apply_gain(&self) {
        if let Some(speaker) = &self.speaker {
            speaker.set_gain(if self.muted { 0.0 } else { self.volume });
        }
    }

    /// Jump to `millis` exactly; the frame there is decoded at once, so a
    /// paused video shows it.
    fn seek(&mut self, millis: f64) -> Result<Option<ImagePixels>, String> {
        let millis = if self.duration > 0.0 {
            millis.clamp(0.0, self.duration)
        } else {
            millis.max(0.0)
        };
        self.restart_sound(millis)?;
        let (played, now) = (self.played(), Instant::now());
        if let (Some(speaker), Some(_), false) =
            (&self.speaker, &self.sound, self.clock.follows_sound())
        {
            self.clock.follow_sound(speaker.rate, played, now);
        }
        self.clock.set(millis, played, now);
        self.ended = false;
        let mut shown = None;
        if let Some(picture) = &mut self.picture {
            picture.seek(millis)?;
            self.picture_done = false;
            self.next = None;
            // Half a frame early still counts as the frame asked for.
            while let Some(frame) = picture.next(|at| at >= millis - 17.0)? {
                if frame.pixels.is_some() {
                    shown = frame.pixels;
                    break;
                }
            }
            if shown.is_none() {
                self.picture_done = true;
            }
        }
        if let Some(speaker) = &self.speaker {
            speaker.set_running(self.clock.running());
        }
        Ok(shown)
    }

    /// Start the sound again at `millis`: what is queued for the speakers
    /// was made for the old place (or the old rate) and is dropped.
    fn restart_sound(&mut self, millis: f64) -> Result<(), String> {
        if let Some(speaker) = &self.speaker {
            speaker.set_running(false);
            speaker.clear();
        }
        if let Some(sound) = &mut self.sound {
            sound.seek(millis)?;
            self.sound_done = false;
        }
        if let Some(resampler) = &mut self.resampler {
            resampler.reset();
        }
        if let Some(stretch) = &mut self.stretch {
            stretch.reset();
        }
        Ok(())
    }

    /// Play at `speed` from where it is now (K-1357).
    fn set_rate(&mut self, speed: f64) -> Result<(), String> {
        let speed = speed.clamp(0.25, 4.0);
        let (played, now) = (self.played(), Instant::now());
        let at = self.clock.millis(played, now);
        self.speed = speed;
        self.stretch = None;
        if self.speaker.is_some() && self.sound.is_some() {
            // The queued sound was stretched for the old rate: start the
            // sound again here, at the new one.
            self.restart_sound(at)?;
            let played = self.played();
            if let (Some(speaker), false) = (&self.speaker, self.clock.follows_sound()) {
                self.clock.follow_sound(speaker.rate, played, now);
            }
            self.clock.set(at, played, now);
            if let Some(speaker) = &self.speaker {
                speaker.set_running(self.clock.running());
            }
        }
        let played = self.played();
        self.clock.set_speed(speed, played, now);
        Ok(())
    }

    /// Keep the speakers fed.
    fn feed(&mut self) -> Result<(), String> {
        let (Some(sound), Some(speaker)) = (&mut self.sound, &self.speaker) else {
            return Ok(());
        };
        let mut out = Vec::new();
        let mut stretched = Vec::new();
        for _ in 0..32 {
            if self.sound_done || speaker.queued_millis() >= SOUND_AHEAD_MILLIS {
                break;
            }
            match sound.next()? {
                Some(chunk) => {
                    // Made from the sound's first real chunk, and again if
                    // the stream changes shape part way.
                    let resampler = match &mut self.resampler {
                        Some(resampler) if resampler.takes(chunk.rate, chunk.channels) => resampler,
                        slot => slot.insert(Resampler::new(
                            chunk.rate,
                            chunk.channels,
                            speaker.rate,
                            speaker.channels,
                        )),
                    };
                    out.clear();
                    resampler.push(&chunk.samples, &mut out);
                    let stretch = match &mut self.stretch {
                        Some(stretch) if stretch.speed() == self.speed => stretch,
                        slot => {
                            slot.insert(Stretch::new(speaker.rate, speaker.channels, self.speed))
                        }
                    };
                    stretched.clear();
                    stretch.push(&out, &mut stretched);
                    speaker.push(&stretched);
                }
                None => self.sound_done = true,
            }
        }
        // The sound ran out (it was shorter than the picture, or it ended):
        // the wall carries on from where the speakers stopped.
        if self.sound_done && speaker.queued_millis() <= 0.0 && self.clock.follows_sound() {
            let played = speaker.played();
            self.clock.follow_wall(played, Instant::now());
        }
        Ok(())
    }

    fn finished(&self, now: f64) -> bool {
        let picture = self.picture.is_none() || (self.picture_done && self.next.is_none());
        let sound = self.speaker.is_none()
            || self.sound.is_none()
            || (self.sound_done && !self.clock.follows_sound());
        picture && sound && (self.duration <= 0.0 || now >= self.duration - 1.0)
    }
}

/// The video's own thread: load, then answer controls and keep time until
/// it is closed.
fn play(shared: &Shared, source: &Source, output: Output) {
    let publish = |f: &mut dyn FnMut(&mut Inner)| {
        if let Ok(mut inner) = shared.inner.lock() {
            f(&mut inner);
        }
    };
    let (mut player, problem) = match load(source, output) {
        Ok(loaded) => loaded,
        Err(problem) => {
            publish(&mut |inner| {
                inner.state.problem = problem.clone();
                inner.state.playing = false;
            });
            return;
        }
    };
    // The first frame, shown while paused at the start.
    let first = player.seek(0.0).unwrap_or(None);
    let (width, height) = player
        .picture
        .as_ref()
        .map_or((0, 0), |picture| (picture.width, picture.height));
    publish(&mut |inner| {
        inner.state.duration_millis = player.duration.round() as u64;
        inner.state.width = first.as_ref().map_or(width, |frame| frame.width);
        inner.state.height = first.as_ref().map_or(height, |frame| frame.height);
        inner.state.ready = true;
        inner.state.problem = problem.clone();
        if first.is_some() {
            inner.frame = first.clone();
        }
    });
    player.shown_once = first.is_some();
    loop {
        let commands = {
            let Ok(mut inner) = shared.inner.lock() else {
                return;
            };
            if inner.closed {
                return;
            }
            std::mem::take(&mut inner.commands)
        };
        let mut fresh = None;
        for command in commands {
            match command {
                Control::Play => {
                    if player.ended || player.finished(player.now()) {
                        fresh = player.seek(0.0).unwrap_or(None).or(fresh);
                    }
                    player.ended = false;
                    player.set_running(true);
                }
                Control::Pause => player.set_running(false),
                Control::Seek(millis) => match player.seek(millis as f64) {
                    Ok(frame) => fresh = frame.or(fresh),
                    Err(problem) => publish(&mut |inner| inner.state.problem = problem.clone()),
                },
                Control::Looping(looping) => player.looping = looping,
                Control::Muted(muted) => {
                    player.muted = muted;
                    player.apply_gain();
                }
                Control::Rate(rate) => {
                    if let Err(problem) = player.set_rate(f64::from(rate)) {
                        publish(&mut |inner| inner.state.problem = problem.clone());
                    }
                }
                Control::Volume(volume) => {
                    player.volume = volume.clamp(0.0, 1.0);
                    player.apply_gain();
                }
            }
        }
        if let Err(problem) = player.feed() {
            // The sound failed mid-way; the picture carries on.
            player.sound_done = true;
            publish(&mut |inner| inner.state.problem = problem.clone());
        }
        let now = player.now();
        if player.clock.running() {
            let Player {
                picture,
                next,
                picture_done,
                ..
            } = &mut player;
            if let Some(picture) = picture.as_mut().filter(|_| !*picture_done) {
                let (due, dropped) =
                    take_due(next, now, || picture.next(|at| at >= now - LATE_MILLIS))
                        .unwrap_or((None, 0));
                if next.is_none() {
                    *picture_done = true;
                }
                player.dropped += dropped;
                if let Some(frame) = due {
                    fresh = frame.pixels;
                }
            }
        }
        let now = player.now();
        if player.clock.running() && player.finished(now) {
            if player.looping {
                fresh = player.seek(0.0).unwrap_or(None).or(fresh);
            } else {
                player.set_running(false);
                player.ended = true;
                tracing::debug!(dropped = player.dropped, "portable video ended");
                if player.duration <= 0.0 {
                    player.duration = now;
                }
            }
        }
        let now = player.now();
        let next_due = player.next.as_ref().map(|frame| frame.millis);
        publish(&mut |inner| {
            let position = if player.ended {
                player.duration
            } else if player.duration > 0.0 {
                now.clamp(0.0, player.duration)
            } else {
                now.max(0.0)
            };
            inner.state.position_millis = position.round() as u64;
            inner.state.duration_millis = player.duration.round() as u64;
            inner.state.playing = player.clock.running();
            inner.state.ended = player.ended;
            if let Some(frame) = fresh.take() {
                inner.state.width = frame.width;
                inner.state.height = frame.height;
                inner.frame = Some(frame);
            }
        });
        // Sleep until the next frame is due, a command comes, or the
        // speakers want more.
        let wait = if !player.clock.running() {
            Duration::from_millis(250)
        } else if let Some(due) = next_due {
            Duration::from_secs_f64(((due - now) / 1000.0).clamp(0.001, 0.010))
        } else {
            Duration::from_millis(10)
        };
        let Ok(inner) = shared.inner.lock() else {
            return;
        };
        if inner.commands.is_empty() && !inner.closed {
            let _ = shared.wake.wait_timeout(inner, wait);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decoded(millis: f64, pixels: bool) -> Frame {
        Frame {
            millis,
            pixels: pixels.then(|| ImagePixels::new(1, 1, vec![0; 4]).unwrap()),
        }
    }

    #[test]
    fn of_the_frames_due_only_the_newest_is_shown() {
        let mut queue: std::collections::VecDeque<Frame> =
            [0.0, 33.0, 66.0, 100.0].map(|at| decoded(at, true)).into();
        let mut next = None;
        let (shown, dropped) = take_due(&mut next, 70.0, || Ok(queue.pop_front())).unwrap();
        assert_eq!(shown.map(|f| f.millis), Some(66.0));
        assert_eq!(dropped, 2, "0 and 33 were late");
        assert_eq!(
            next.as_ref().map(|f| f.millis),
            Some(100.0),
            "kept for its time"
        );
        // Nothing due yet: nothing shown, nothing lost.
        let (shown, dropped) = take_due(&mut next, 80.0, || Ok(queue.pop_front())).unwrap();
        assert!(shown.is_none());
        assert_eq!(dropped, 0);
        assert!(next.is_some());
    }

    #[test]
    fn a_frame_too_late_to_convert_is_not_shown_over_a_good_one() {
        let mut queue: std::collections::VecDeque<Frame> = [
            decoded(0.0, false),
            decoded(33.0, true),
            decoded(66.0, false),
        ]
        .into();
        let mut next = None;
        let (shown, _) = take_due(&mut next, 70.0, || Ok(queue.pop_front())).unwrap();
        assert_eq!(shown.map(|f| f.millis), Some(33.0));
    }

    fn fixture(name: &str) -> Source {
        Source::File(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/video_portable/fixtures")
                .join(name),
        )
    }

    fn wait_for(video: u64, what: impl Fn(&State) -> bool) -> State {
        let started = Instant::now();
        loop {
            let state = state(video).unwrap();
            if what(&state) || started.elapsed() > Duration::from_secs(10) {
                return state;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_clip_plays_in_time_seeks_loops_and_ends() {
        let video = open_with(fixture("clip.mp4"), Output::Silent).unwrap();
        let ready = wait_for(video, |s| s.ready);
        assert!(ready.problem.is_empty(), "{}", ready.problem);
        assert_eq!((ready.width, ready.height), (64, 64));
        // The longer of the picture (1000 ms) and the AAC sound, which
        // ends on a whole packet.
        let length = ready.duration_millis;
        assert!((1000..1030).contains(&length), "{length}");
        assert!(!ready.playing && ready.position_millis == 0);
        assert!(frame(video).unwrap().is_some(), "the first frame, paused");

        control(video, Control::Play).unwrap();
        let started = Instant::now();
        let mut frames = 0;
        let mut last = 0;
        while started.elapsed() < Duration::from_millis(400) {
            frames += usize::from(super::frame(video).unwrap().is_some());
            let at = state(video).unwrap().position_millis;
            assert!(at >= last, "the clock never runs back: {at} after {last}");
            last = at;
            std::thread::sleep(Duration::from_millis(5));
        }
        let at = state(video).unwrap().position_millis as i64;
        let wall = started.elapsed().as_millis() as i64;
        assert!(
            (at - wall).abs() < 80,
            "kept to the clock: {at} ms at {wall} ms"
        );
        // About 12 are due. A busy machine decodes fewer and shows only the
        // newest of those due (the test below), which is right; the clock
        // check above is what holds the pace. A CI runner under the whole
        // suite showed 4 (K-1383), so this asks only that pictures move on.
        assert!(frames >= 3, "frames advance: {frames} in 400 ms at 30 fps");

        // Twice as fast: the clock moves twice as far (K-1357).
        control(video, Control::Rate(2.0)).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        let (from, wall) = (state(video).unwrap().position_millis, Instant::now());
        std::thread::sleep(Duration::from_millis(150));
        let moved = state(video).unwrap().position_millis.saturating_sub(from) as f64;
        let real = wall.elapsed().as_millis() as f64;
        assert!(
            (moved / real - 2.0).abs() < 0.35,
            "{moved} ms in {real} ms at 2x"
        );
        control(video, Control::Rate(1.0)).unwrap();

        control(video, Control::Pause).unwrap();
        assert!(!state(video).unwrap().playing, "paused at once");
        // The player's own turn applies it within a few milliseconds.
        std::thread::sleep(Duration::from_millis(50));
        let held = state(video).unwrap().position_millis;
        std::thread::sleep(Duration::from_millis(150));
        assert!(
            state(video).unwrap().position_millis.abs_diff(held) <= 1,
            "paused holds"
        );

        control(video, Control::Seek(800)).unwrap();
        let seeked = wait_for(video, |s| s.position_millis == 800);
        assert_eq!(seeked.position_millis, 800);
        std::thread::sleep(Duration::from_millis(30));
        assert!(
            super::frame(video).unwrap().is_some(),
            "the frame there, paused"
        );

        control(video, Control::Play).unwrap();
        let ended = wait_for(video, |s| s.ended);
        assert!(
            ended.ended && !ended.playing && ended.position_millis == length,
            "{ended:?}"
        );

        control(video, Control::Looping(true)).unwrap();
        control(video, Control::Play).unwrap();
        std::thread::sleep(Duration::from_millis(1300));
        let looped = state(video).unwrap();
        assert!(
            looped.playing && !looped.ended,
            "a loop goes round: {looped:?}"
        );
        close(video);
        assert!(state(video).is_err(), "a closed video is gone");
    }

    #[test]
    fn a_sound_file_plays_with_no_picture() {
        let video = open_with(fixture("tone.mp3"), Output::Silent).unwrap();
        let ready = wait_for(video, |s| s.ready);
        assert!(ready.problem.is_empty(), "{}", ready.problem);
        assert_eq!((ready.width, ready.height), (0, 0));
        assert!(ready.duration_millis.abs_diff(1000) < 60, "{ready:?}");
        control(video, Control::Volume(0.5)).unwrap();
        control(video, Control::Play).unwrap();
        let ended = wait_for(video, |s| s.ended);
        assert!(ended.ended, "{ended:?}");
        close(video);
    }

    #[test]
    fn what_cannot_play_says_why() {
        let text = std::env::temp_dir().join("krate-video-portable-not-media.mp4");
        std::fs::write(&text, b"this is not a video at all").unwrap();
        let video = open_with(Source::File(text), Output::Silent).unwrap();
        let state = wait_for(video, |s| !s.problem.is_empty());
        assert!(state.problem.contains("not a sound or video"), "{state:?}");
        close(video);
        let video = open_with(fixture("hevc.mp4"), Output::Silent).unwrap();
        let state = wait_for(video, |s| !s.problem.is_empty());
        assert!(state.problem.contains("HEVC"), "{state:?}");
        close(video);
    }

    #[test]
    fn a_clip_streams_from_an_address() {
        let (url, _) = source::tests::serve(include_bytes!("fixtures/clip.mp4").to_vec(), true);
        let video = open_with(Source::Url(url), Output::Silent).unwrap();
        let ready = wait_for(video, |s| s.ready || !s.problem.is_empty());
        assert!(ready.problem.is_empty() && ready.ready, "{ready:?}");
        assert_eq!((ready.width, ready.height), (64, 64));
        assert!(frame(video).unwrap().is_some());
        close(video);
    }
}
