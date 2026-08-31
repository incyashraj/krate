//! Native audio output behind the `audio.playback` capability.
//!
//! The mirror of `audio_capture`: the guest writes bounded PCM chunks, CPAL
//! owns the operating-system stream, and the device callback drains a shared
//! ring buffer. All format conversion happens at `write` time on the guest's
//! thread — the audio callback runs on a real-time thread where an allocation
//! or a lock held too long becomes an audible glitch, so it does nothing but
//! pop samples and fill silence when the ring runs dry.

//! ## In a browser tab
//!
//! There is no CPAL. A tab has WebAudio, but it is a different contract:
//! asynchronous, and silent until the person has clicked something, because a
//! browser will not let a page make noise on its own. Bridging it is later
//! work. This module keeps its shapes on wasm32 so the rest of the runtime
//! compiles unchanged, and every call answers `PlaybackError::Unsupported` --
//! which the permission wall turns into words a person can act on.
//!
//! Nothing here reports success. An app that believes it is playing a sound
//! and is not will look broken in a way nobody can trace, which is worse than
//! being told plainly there is no audio on this host.

#[cfg(not(target_arch = "wasm32"))]
use std::collections::{BTreeMap, VecDeque};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Mutex};

#[cfg(not(target_arch = "wasm32"))]
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Most samples the ring may hold: about 10 seconds of 48 kHz stereo.
///
/// A guest that writes faster than the device plays would otherwise grow the
/// ring without bound. Unlike capture — where the newest audio matters and the
/// oldest is dropped — playback must never drop, or the person hears a skip.
/// So a full ring accepts fewer bytes and the guest learns it from `write`'s
/// return value: backpressure, not loss.
#[cfg(not(target_arch = "wasm32"))]
const MAX_RING_SAMPLES: usize = 48_000 * 2 * 10;
/// Largest device callback this mixes in one pass.
///
/// A fixed scratch buffer because the callback runs on a real-time thread and
/// must not allocate. 8192 samples is far beyond any normal callback -- 1024
/// is typical -- and anything past it is filled with silence rather than
/// stale audio.
#[cfg(not(target_arch = "wasm32"))]
const MAX_CALLBACK_FRAMES: usize = 8_192;
/// Most sound effects that may sound at once.
///
/// Beyond this a mix is noise anyway, and the bound is what stops an app that
/// calls `play-sound` in a loop from growing the voice list until the
/// real-time callback cannot keep up.
#[cfg(not(target_arch = "wasm32"))]
const MAX_VOICES: usize = 64;
const MIN_SAMPLE_RATE: u32 = 8_000;
const MAX_SAMPLE_RATE: u32 = 192_000;
const MAX_CHANNELS: u16 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackSampleFormat {
    PcmS16,
    Float32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaybackConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub format: PlaybackSampleFormat,
    pub buffer_frames: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaybackError {
    InvalidStream,
    DeviceUnavailable,
    InvalidConfig(String),
    Platform(String),
    /// This host has no speakers at all -- a browser preview. Only added on
    /// wasm32: a native build can never produce it, and adding a variant
    /// there would change what the rest of the runtime has to match on for no
    /// gain.
    ///
    /// Distinct from the permission wall, which refuses before a call ever
    /// reaches this module. An app that was granted `audio.playback` and
    /// still cannot make a sound learns that from here, so the words a person
    /// reads can say which of the two it was.
    #[cfg(target_arch = "wasm32")]
    Unsupported,
}

#[cfg(not(target_arch = "wasm32"))]
struct PlaybackStream {
    stream: cpal::Stream,
    ring: Arc<Mutex<VecDeque<f32>>>,
    stream_error: Arc<Mutex<Option<String>>>,
    converter: WriteConverter,
    started: bool,
    /// Loaded sound effects, in the device's own sample layout so playing one
    /// costs no conversion.
    sounds: Arc<Mutex<Vec<Arc<Vec<f32>>>>>,
    /// Sounds currently playing, shared with the device callback.
    voices: Arc<Mutex<Vec<Voice>>>,
}

/// One sound effect in flight.
///
/// A copy per play rather than a cursor per sound, so firing the same shot
/// twice in quick succession sounds like two shots rather than one restarting.
#[cfg(not(target_arch = "wasm32"))]
struct Voice {
    /// Which loaded sound this is, so `stop-sound` can find every copy.
    sound: u64,
    samples: Arc<Vec<f32>>,
    /// How far through, in samples.
    at: usize,
    gain: f32,
}

#[cfg(not(target_arch = "wasm32"))]
pub struct AudioPlaybackRuntime {
    next_stream_id: u64,
    streams: BTreeMap<u64, PlaybackStream>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for AudioPlaybackRuntime {
    fn default() -> Self {
        Self {
            next_stream_id: 1,
            streams: BTreeMap::new(),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl AudioPlaybackRuntime {
    pub fn open(&mut self, config: PlaybackConfig) -> Result<u64, PlaybackError> {
        validate_config(config)?;
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or(PlaybackError::DeviceUnavailable)?;
        let device_config = device
            .default_output_config()
            .map_err(|error| PlaybackError::Platform(error.to_string()))?;
        let stream_config = device_config.config();
        let device_rate = stream_config.sample_rate;
        let device_channels = stream_config.channels;

        let ring: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
        let sounds: Arc<Mutex<Vec<Arc<Vec<f32>>>>> = Arc::new(Mutex::new(Vec::new()));
        let voices: Arc<Mutex<Vec<Voice>>> = Arc::new(Mutex::new(Vec::new()));
        let stream_error = Arc::new(Mutex::new(None));
        let callback_error = stream_error.clone();
        let error_callback = move |error: cpal::Error| {
            if let Ok(mut current) = callback_error.lock() {
                *current = Some(error.to_string());
            }
        };

        // The ring already holds samples in the device's rate and channel
        // layout, so every callback is the same shape: pop one f32 per slot,
        // silence when empty, convert to the device's sample type.
        let stream = match device_config.sample_format() {
            cpal::SampleFormat::I16 => {
                let ring = ring.clone();
                let voices = voices.clone();
                device.build_output_stream(
                    stream_config,
                    move |out: &mut [i16], _| {
                        drain_into(&ring, &voices, out, |sample| {
                            (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16
                        });
                    },
                    error_callback,
                    None,
                )
            }
            cpal::SampleFormat::U16 => {
                let ring = ring.clone();
                let voices = voices.clone();
                device.build_output_stream(
                    stream_config,
                    move |out: &mut [u16], _| {
                        drain_into(&ring, &voices, out, |sample| {
                            ((sample.clamp(-1.0, 1.0) + 1.0) * 32_767.5) as u16
                        });
                    },
                    error_callback,
                    None,
                )
            }
            cpal::SampleFormat::F32 => {
                let ring = ring.clone();
                let voices = voices.clone();
                device.build_output_stream(
                    stream_config,
                    move |out: &mut [f32], _| {
                        drain_into(&ring, &voices, out, |sample| sample.clamp(-1.0, 1.0));
                    },
                    error_callback,
                    None,
                )
            }
            format => {
                return Err(PlaybackError::Platform(format!(
                    "the default output device uses unsupported sample format {format:?}"
                )))
            }
        }
        .map_err(|error| PlaybackError::Platform(error.to_string()))?;

        let stream_id = self.next_stream_id;
        self.next_stream_id = self.next_stream_id.checked_add(1).ok_or_else(|| {
            PlaybackError::Platform("audio stream id space exhausted".to_string())
        })?;
        self.streams.insert(
            stream_id,
            PlaybackStream {
                stream,
                ring,
                stream_error,
                converter: WriteConverter::new(config, device_rate, device_channels),
                started: false,
                sounds: sounds.clone(),
                voices: voices.clone(),
            },
        );
        Ok(stream_id)
    }

    pub fn start(&mut self, stream_id: u64) -> Result<(), PlaybackError> {
        let stream = self
            .streams
            .get_mut(&stream_id)
            .ok_or(PlaybackError::InvalidStream)?;
        stream
            .stream
            .play()
            .map_err(|error| PlaybackError::Platform(error.to_string()))?;
        stream.started = true;
        Ok(())
    }

    pub fn stop(&mut self, stream_id: u64) -> Result<(), PlaybackError> {
        let stream = self
            .streams
            .get_mut(&stream_id)
            .ok_or(PlaybackError::InvalidStream)?;
        stream
            .stream
            .pause()
            .map_err(|error| PlaybackError::Platform(error.to_string()))?;
        stream.started = false;
        Ok(())
    }

    /// Decode a sound effect once and keep it ready to play.
    pub fn load_sound(&mut self, stream_id: u64, bytes: &[u8]) -> Result<u64, PlaybackError> {
        let stream = self
            .streams
            .get_mut(&stream_id)
            .ok_or(PlaybackError::InvalidStream)?;
        let samples = stream.converter.decode(bytes);
        if samples.is_empty() {
            return Err(PlaybackError::InvalidConfig(
                "a sound needs at least one whole frame of samples".to_string(),
            ));
        }
        let mut sounds = stream
            .sounds
            .lock()
            .map_err(|_| PlaybackError::Platform("sound table lock poisoned".to_string()))?;
        sounds.push(Arc::new(samples));
        // Handles start at one so zero stays an obviously invalid value.
        Ok(sounds.len() as u64)
    }

    /// Start a loaded sound, mixed with whatever else is playing.
    pub fn play_sound(
        &mut self,
        stream_id: u64,
        sound: u64,
        gain: f32,
    ) -> Result<(), PlaybackError> {
        let stream = self
            .streams
            .get_mut(&stream_id)
            .ok_or(PlaybackError::InvalidStream)?;
        let samples = {
            let sounds = stream
                .sounds
                .lock()
                .map_err(|_| PlaybackError::Platform("sound table lock poisoned".to_string()))?;
            let index = (sound as usize)
                .checked_sub(1)
                .ok_or(PlaybackError::InvalidStream)?;
            sounds
                .get(index)
                .cloned()
                .ok_or(PlaybackError::InvalidStream)?
        };
        let mut voices = stream
            .voices
            .lock()
            .map_err(|_| PlaybackError::Platform("voice list lock poisoned".to_string()))?;
        // A bound on simultaneous voices: without one, an app looping
        // `play-sound` would grow this list until memory ran out, and the
        // real-time callback would slow down with every added voice.
        if voices.len() >= MAX_VOICES {
            return Ok(());
        }
        voices.push(Voice {
            sound,
            samples,
            at: 0,
            gain: if gain.is_finite() {
                gain.clamp(0.0, 4.0)
            } else {
                1.0
            },
        });
        Ok(())
    }

    /// Stop every copy of one sound.
    pub fn stop_sound(&mut self, stream_id: u64, sound: u64) -> Result<(), PlaybackError> {
        let stream = self
            .streams
            .get_mut(&stream_id)
            .ok_or(PlaybackError::InvalidStream)?;
        let mut voices = stream
            .voices
            .lock()
            .map_err(|_| PlaybackError::Platform("voice list lock poisoned".to_string()))?;
        voices.retain(|voice| voice.sound != sound);
        Ok(())
    }

    /// Queue interleaved little-endian audio bytes; returns how many were
    /// accepted. Only whole frames are consumed, so the guest keeps the
    /// remainder and writes it again with the next chunk.
    pub fn write(&mut self, stream_id: u64, bytes: &[u8]) -> Result<u32, PlaybackError> {
        let stream = self
            .streams
            .get_mut(&stream_id)
            .ok_or(PlaybackError::InvalidStream)?;
        if let Ok(mut error) = stream.stream_error.lock() {
            if let Some(error) = error.take() {
                return Err(PlaybackError::Platform(error));
            }
        }
        let accepted = stream.converter.write(&stream.ring, bytes)?;
        Ok(accepted as u32)
    }
}

// The browser's answer: the same shapes, and an honest refusal.
//
// Kept beside the real one rather than behind a runtime branch so the wasm
// build never links CPAL at all, and so the two cannot drift into disagreeing
// about the type the rest of the runtime sees.
#[cfg(target_arch = "wasm32")]
#[derive(Default)]
pub struct AudioPlaybackRuntime;

#[cfg(target_arch = "wasm32")]
impl AudioPlaybackRuntime {
    /// A bad config is still a bad config here. Reporting it before the
    /// refusal means an app with a broken sample rate hears the same thing on
    /// both hosts, rather than having the mistake hidden until it ships
    /// somewhere with speakers.
    pub fn open(&mut self, config: PlaybackConfig) -> Result<u64, PlaybackError> {
        validate_config(config)?;
        Err(PlaybackError::Unsupported)
    }

    pub fn start(&mut self, _stream_id: u64) -> Result<(), PlaybackError> {
        Err(PlaybackError::Unsupported)
    }

    pub fn stop(&mut self, _stream_id: u64) -> Result<(), PlaybackError> {
        Err(PlaybackError::Unsupported)
    }

    pub fn load_sound(&mut self, _stream_id: u64, _bytes: &[u8]) -> Result<u64, PlaybackError> {
        Err(PlaybackError::Unsupported)
    }

    pub fn play_sound(
        &mut self,
        _stream_id: u64,
        _sound: u64,
        _gain: f32,
    ) -> Result<(), PlaybackError> {
        Err(PlaybackError::Unsupported)
    }

    /// Refuses rather than answering `Ok(())` the way the native path does
    /// when the voice list is full. There, the sound was genuinely handled and
    /// only the mix was crowded; here it never played at all.
    pub fn stop_sound(&mut self, _stream_id: u64, _sound: u64) -> Result<(), PlaybackError> {
        Err(PlaybackError::Unsupported)
    }

    /// Refuses rather than claiming the bytes were accepted. Reporting them
    /// consumed would let a guest stream a whole song into nothing and never
    /// learn the speakers were not there.
    pub fn write(&mut self, _stream_id: u64, _bytes: &[u8]) -> Result<u32, PlaybackError> {
        Err(PlaybackError::Unsupported)
    }
}

/// Fill a device buffer from the ring, silence on underrun.
///
/// Runs on the real-time audio thread: one try_lock, no allocation. If the
/// guest's thread holds the lock right now, this callback plays silence rather
/// than blocking — a missed millisecond of audio beats a stalled device.
#[cfg(not(target_arch = "wasm32"))]
fn drain_into<T>(
    ring: &Arc<Mutex<VecDeque<f32>>>,
    voices: &Arc<Mutex<Vec<Voice>>>,
    out: &mut [T],
    convert: impl Fn(f32) -> T,
) where
    T: Copy,
{
    // Streamed audio first, then sound effects mixed on top. Both use
    // try_lock: this is the real-time thread, and a missed millisecond of
    // audio beats a stalled device.
    let mut mixed = [0.0_f32; MAX_CALLBACK_FRAMES];
    let count = out.len().min(MAX_CALLBACK_FRAMES);

    if let Ok(mut ring) = ring.try_lock() {
        for slot in mixed.iter_mut().take(count) {
            *slot = ring.pop_front().unwrap_or(0.0);
        }
    }

    if let Ok(mut voices) = voices.try_lock() {
        for voice in voices.iter_mut() {
            let remaining = voice.samples.len().saturating_sub(voice.at);
            let take = remaining.min(count);
            for (slot, sample) in mixed
                .iter_mut()
                .take(take)
                .zip(voice.samples[voice.at..].iter())
            {
                // Additive, clamped at conversion: a dozen sounds at once
                // distort rather than wrapping around into noise, which is
                // what integer overflow would sound like.
                *slot += sample * voice.gain;
            }
            voice.at += take;
        }
        // Finished voices are dropped here rather than in the caller, so a
        // game that never stops anything does not accumulate them.
        voices.retain(|voice| voice.at < voice.samples.len());
    }

    for (slot, sample) in out.iter_mut().zip(mixed.iter()) {
        *slot = convert(*sample);
    }
    // Any tail beyond the scratch buffer is silence rather than stale audio.
    for slot in out.iter_mut().skip(count) {
        *slot = convert(0.0);
    }
}

/// Converts the guest's declared PCM into the device's rate and channel
/// layout at `write` time, so the audio callback never has to.
///
/// Same accumulator resampler as capture, pointed the other way: for every
/// guest frame, emit device frames while the accumulator carries. Upsampling
/// repeats frames, downsampling skips them — nearest-neighbour, matching the
/// fidelity promise capture already makes.
#[cfg(not(target_arch = "wasm32"))]
struct WriteConverter {
    guest_rate: u32,
    guest_channels: usize,
    guest_format: PlaybackSampleFormat,
    device_rate: u32,
    device_channels: usize,
    rate_accumulator: u64,
}

#[cfg(not(target_arch = "wasm32"))]
impl WriteConverter {
    fn new(guest: PlaybackConfig, device_rate: u32, device_channels: u16) -> Self {
        Self {
            guest_rate: guest.sample_rate,
            guest_channels: usize::from(guest.channels),
            guest_format: guest.format,
            device_rate,
            device_channels: usize::from(device_channels),
            rate_accumulator: 0,
        }
    }

    fn bytes_per_sample(&self) -> usize {
        match self.guest_format {
            PlaybackSampleFormat::PcmS16 => 2,
            PlaybackSampleFormat::Float32 => 4,
        }
    }

    /// Turn guest bytes into device-layout samples, for a stored sound.
    ///
    /// Reuses the same resampling and channel mapping as `write`, so a sound
    /// effect and a stream at the same rate come out identical -- a footstep
    /// should not sound different depending on which call played it.
    fn decode(&mut self, bytes: &[u8]) -> Vec<f32> {
        let frame_bytes = self.bytes_per_sample() * self.guest_channels;
        let whole = bytes.len() / frame_bytes.max(1);
        let mut out = Vec::with_capacity(whole * self.device_channels);
        let mut frame = vec![0.0_f32; self.guest_channels];
        // A private accumulator: decoding a sound must not disturb the phase
        // of the stream that is playing.
        let mut accumulator = 0_u64;

        for index in 0..whole {
            let start = index * frame_bytes;
            for (channel, slot) in frame.iter_mut().enumerate() {
                let at = start + channel * self.bytes_per_sample();
                *slot = match self.guest_format {
                    PlaybackSampleFormat::PcmS16 => {
                        let Some(pair) = bytes.get(at..at + 2) else {
                            continue;
                        };
                        f32::from(i16::from_le_bytes([pair[0], pair[1]])) / f32::from(i16::MAX)
                    }
                    PlaybackSampleFormat::Float32 => {
                        let Some(quad) = bytes.get(at..at + 4) else {
                            continue;
                        };
                        f32::from_le_bytes([quad[0], quad[1], quad[2], quad[3]])
                    }
                };
            }
            accumulator += u64::from(self.device_rate);
            while accumulator >= u64::from(self.guest_rate) {
                accumulator -= u64::from(self.guest_rate);
                push_device_frame(&mut out, &frame, self.device_channels);
            }
        }
        out
    }

    /// Convert and queue as many whole frames as fit; return bytes consumed.
    fn write(
        &mut self,
        ring: &Arc<Mutex<VecDeque<f32>>>,
        bytes: &[u8],
    ) -> Result<usize, PlaybackError> {
        let frame_bytes = self.bytes_per_sample() * self.guest_channels;
        let whole = bytes.len() / frame_bytes;
        if whole == 0 {
            return Ok(0);
        }

        // Convert outside the ring lock, then take it once.
        let mut converted: Vec<f32> = Vec::new();
        let mut frame: Vec<f32> = vec![0.0; self.guest_channels];
        let mut consumed_frames = 0_usize;

        let mut ring_guard = ring
            .lock()
            .map_err(|_| PlaybackError::Platform("playback ring lock poisoned".to_string()))?;
        let mut room = MAX_RING_SAMPLES.saturating_sub(ring_guard.len());

        for index in 0..whole {
            let start = index * frame_bytes;
            for (channel, slot) in frame.iter_mut().enumerate() {
                let at = start + channel * self.bytes_per_sample();
                *slot = match self.guest_format {
                    PlaybackSampleFormat::PcmS16 => {
                        f32::from(i16::from_le_bytes([bytes[at], bytes[at + 1]]))
                            / f32::from(i16::MAX)
                    }
                    PlaybackSampleFormat::Float32 => {
                        f32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
                    }
                };
            }

            // How many device frames this guest frame becomes.
            converted.clear();
            self.rate_accumulator += u64::from(self.device_rate);
            while self.rate_accumulator >= u64::from(self.guest_rate) {
                self.rate_accumulator -= u64::from(self.guest_rate);
                push_device_frame(&mut converted, &frame, self.device_channels);
            }

            if converted.len() > room {
                // The ring is full: stop before this frame so the guest can
                // retry it. The accumulator must give back what this frame
                // took, or the retry would emit a different number of frames.
                self.rate_accumulator = self
                    .rate_accumulator
                    .wrapping_sub(u64::from(self.device_rate))
                    .wrapping_add(
                        u64::from(self.guest_rate)
                            * (converted.len() / self.device_channels) as u64,
                    );
                break;
            }
            room -= converted.len();
            ring_guard.extend(converted.iter().copied());
            consumed_frames += 1;
        }

        Ok(consumed_frames * frame_bytes)
    }
}

/// Remix one guest frame into the device's channel count.
#[cfg(not(target_arch = "wasm32"))]
fn push_device_frame(out: &mut Vec<f32>, frame: &[f32], device_channels: usize) {
    if device_channels == 1 {
        let sum = frame.iter().copied().sum::<f32>();
        out.push(sum / frame.len() as f32);
        return;
    }
    for channel in 0..device_channels {
        let sample = if frame.len() == 1 {
            frame[0]
        } else {
            frame[channel.min(frame.len() - 1)]
        };
        out.push(sample);
    }
}

fn validate_config(config: PlaybackConfig) -> Result<(), PlaybackError> {
    if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&config.sample_rate) {
        return Err(PlaybackError::InvalidConfig(format!(
            "sample rate must be between {MIN_SAMPLE_RATE} and {MAX_SAMPLE_RATE} Hz"
        )));
    }
    if config.channels == 0 || config.channels > MAX_CHANNELS {
        return Err(PlaybackError::InvalidConfig(format!(
            "channels must be between 1 and {MAX_CHANNELS}"
        )));
    }
    if config.buffer_frames == 0 {
        return Err(PlaybackError::InvalidConfig(
            "buffer-frames must be greater than zero".to_string(),
        ));
    }
    Ok(())
}

// The tests reach for the converter and the ring, which only the native
// build has. They run where the device code they cover does.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    fn config(rate: u32, channels: u16) -> PlaybackConfig {
        PlaybackConfig {
            sample_rate: rate,
            channels,
            format: PlaybackSampleFormat::PcmS16,
            buffer_frames: 1_024,
        }
    }

    /// Ignored by default: CI runners have no audio device. Run it on a real
    /// machine with `cargo test -p krate-runtime plays_a_tone -- --ignored`;
    /// a short quiet tone from the speakers is the pass condition the other
    /// tests cannot give -- proof the samples reach actual hardware.
    #[test]
    #[ignore = "needs a real output device"]
    fn plays_a_tone_on_the_default_device() {
        let mut runtime = AudioPlaybackRuntime::default();
        let stream = runtime.open(config(48_000, 1)).expect("open the speakers");

        // 200 ms of a 440 Hz sine at low volume.
        let mut bytes = Vec::new();
        for n in 0..9_600_u32 {
            let t = n as f32 / 48_000.0;
            let sample = (t * 440.0 * core::f32::consts::TAU).sin() * 0.2;
            bytes.extend_from_slice(&((sample * f32::from(i16::MAX)) as i16).to_le_bytes());
        }
        let mut written = 0_usize;
        while written < bytes.len() {
            let accepted = runtime
                .write(stream, &bytes[written..])
                .expect("queue samples") as usize;
            assert!(accepted > 0, "the ring refused everything");
            written += accepted;
        }
        runtime.start(stream).expect("start");
        std::thread::sleep(std::time::Duration::from_millis(300));
        runtime.stop(stream).expect("stop");
    }

    #[test]
    fn invalid_playback_configs_fail_before_opening_a_device() {
        let mut runtime = AudioPlaybackRuntime::default();
        let error = runtime.open(config(0, 1)).expect_err("invalid sample rate");
        assert!(matches!(error, PlaybackError::InvalidConfig(_)));

        let error = runtime.open(config(48_000, 0)).expect_err("zero channels");
        assert!(matches!(error, PlaybackError::InvalidConfig(_)));
    }

    #[test]
    fn guest_mono_becomes_device_stereo_with_the_same_value_on_both_sides() {
        // A mono chime on stereo speakers must come from both, not the left.
        let ring = Arc::new(Mutex::new(VecDeque::new()));
        let mut converter = WriteConverter::new(config(48_000, 1), 48_000, 2);
        // One mono frame at full positive amplitude.
        let bytes = i16::MAX.to_le_bytes();
        let consumed = converter.write(&ring, &bytes).expect("write");
        assert_eq!(consumed, 2);
        let ring = ring.lock().expect("ring");
        assert_eq!(ring.len(), 2);
        assert!((ring[0] - 1.0).abs() < 1e-4);
        assert_eq!(ring[0], ring[1]);
    }

    #[test]
    fn resampling_up_repeats_frames_and_down_skips_them() {
        // 8 kHz guest audio on a 16 kHz device doubles each frame.
        let ring = Arc::new(Mutex::new(VecDeque::new()));
        let mut up = WriteConverter::new(config(8_000, 1), 16_000, 1);
        let mut bytes = Vec::new();
        for value in [1000_i16, 2000, 3000] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let consumed = up.write(&ring, &bytes).expect("write");
        assert_eq!(consumed, 6, "all three guest frames consumed");
        assert_eq!(ring.lock().expect("ring").len(), 6, "each frame doubled");

        // 48 kHz guest audio on a 16 kHz device keeps one frame in three.
        let ring = Arc::new(Mutex::new(VecDeque::new()));
        let mut down = WriteConverter::new(config(48_000, 1), 16_000, 1);
        let mut bytes = Vec::new();
        for value in 0..6_i16 {
            bytes.extend_from_slice(&(value * 1000).to_le_bytes());
        }
        let consumed = down.write(&ring, &bytes).expect("write");
        assert_eq!(consumed, 12, "all six guest frames consumed");
        assert_eq!(ring.lock().expect("ring").len(), 2, "one kept per three");
    }

    #[test]
    fn a_partial_frame_is_left_for_the_guest_to_resend() {
        // Three bytes of s16 stereo is less than one frame; nothing must be
        // consumed, or the stream would go out of alignment forever after.
        let ring = Arc::new(Mutex::new(VecDeque::new()));
        let mut converter = WriteConverter::new(config(48_000, 2), 48_000, 2);
        let consumed = converter.write(&ring, &[1, 2, 3]).expect("write");
        assert_eq!(consumed, 0);
        assert!(ring.lock().expect("ring").is_empty());
    }

    #[test]
    fn a_full_ring_accepts_fewer_bytes_instead_of_dropping_audio() {
        // Capture drops the oldest sample when full because the newest audio
        // is the one that matters. Playback is the opposite: dropping causes
        // an audible skip, so the ring refuses and the guest retries later.
        let ring = Arc::new(Mutex::new(VecDeque::new()));
        {
            let mut ring = ring.lock().expect("ring");
            for _ in 0..MAX_RING_SAMPLES - 1 {
                ring.push_back(0.0);
            }
        }
        let mut converter = WriteConverter::new(config(48_000, 1), 48_000, 1);
        let mut bytes = Vec::new();
        for _ in 0..4 {
            bytes.extend_from_slice(&1000_i16.to_le_bytes());
        }
        let consumed = converter.write(&ring, &bytes).expect("write");
        assert_eq!(consumed, 2, "exactly the one frame that fit");
        assert_eq!(ring.lock().expect("ring").len(), MAX_RING_SAMPLES);
    }

    #[test]
    fn the_device_callback_fills_silence_on_underrun() {
        let ring = Arc::new(Mutex::new(VecDeque::from(vec![0.5_f32, -0.5])));
        let mut out = [123_i16; 4];
        let voices = Arc::new(Mutex::new(Vec::new()));
        drain_into(&ring, &voices, &mut out, |sample| {
            (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16
        });
        assert_eq!(out[0], (0.5 * f32::from(i16::MAX)) as i16);
        assert_eq!(out[1], (-0.5 * f32::from(i16::MAX)) as i16);
        assert_eq!(out[2], 0, "past the ring is silence, not stale data");
        assert_eq!(out[3], 0);
    }
}
