//! A video's sound, or a sound file, on a system without a native player
//! (K-1348): demuxed and decoded by symphonia (MP3, AAC and ALAC in MP4 or
//! M4A, FLAC, WAV, Ogg Vorbis, and Vorbis in WebM or MKV), turned to the
//! speakers' rate and channels, and played through the system's audio
//! output with cpal, the same output `audio.playback` uses.
//!
//! The speakers count every frame they play. That count is the clock the
//! picture is kept to (clock.rs).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::{Time, TimeBase};

/// Decoded sound, interleaved, starting `millis` from the start.
pub(super) struct Chunk {
    #[cfg_attr(not(test), allow(dead_code))]
    pub millis: f64,
    pub samples: Vec<f32>,
    /// The chunk's own rate and channels: a container need not say them
    /// before the first packet (AAC in MP4 often does not).
    pub rate: u32,
    pub channels: usize,
}

pub(super) struct Sound {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track: u32,
    time_base: Option<TimeBase>,
    pub rate: u32,
    pub channels: usize,
    pub duration_millis: f64,
    /// After a seek: the sound before this is cut off the first chunk.
    trim_before: Option<f64>,
}

fn millis_of(time: Time) -> f64 {
    (time.seconds as f64 + time.frac) * 1000.0
}

impl Sound {
    /// The file's sound, `None` when it has none.
    pub(super) fn open(
        source: Box<dyn MediaSource>,
        extension: Option<&str>,
    ) -> Result<Option<Self>, String> {
        let mut hint = Hint::new();
        if let Some(extension) = extension {
            hint.with_extension(extension);
        }
        let stream = MediaSourceStream::new(source, Default::default());
        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                stream,
                &FormatOptions {
                    enable_gapless: true,
                    ..Default::default()
                },
                &MetadataOptions::default(),
            )
            .map_err(|_| "this file is not a sound or video Krate can play".to_string())?;
        let format = probed.format;
        let Some(track) = format
            .tracks()
            .iter()
            .find(|track| track.codec_params.codec != CODEC_TYPE_NULL)
        else {
            return Ok(None);
        };
        let params = track.codec_params.clone();
        let id = track.id;
        let decoder = symphonia::default::get_codecs()
            .make(&params, &DecoderOptions::default())
            .map_err(|_| "this file's sound is in a format Krate cannot play here".to_string())?;
        let rate = params.sample_rate.unwrap_or(0);
        let channels = params.channels.map_or(0, |channels| channels.count());
        let duration_millis = match (params.time_base, params.n_frames) {
            (Some(base), Some(frames)) => millis_of(base.calc_time(frames)),
            (None, Some(frames)) if rate > 0 => frames as f64 * 1000.0 / f64::from(rate),
            _ => 0.0,
        };
        Ok(Some(Self {
            format,
            decoder,
            track: id,
            time_base: params.time_base,
            rate,
            channels,
            duration_millis,
            trim_before: None,
        }))
    }

    /// The next stretch of sound, `None` at the end.
    pub(super) fn next(&mut self) -> Result<Option<Chunk>, String> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(Error::IoError(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Ok(None)
                }
                Err(Error::ResetRequired) => return Ok(None),
                Err(error) => return Err(format!("the sound stopped: {error}")),
            };
            if packet.track_id() != self.track {
                continue;
            }
            let millis = self
                .time_base
                .map(|base| millis_of(base.calc_time(packet.ts())))
                .unwrap_or(0.0);
            let decoded = match self.decoder.decode(&packet) {
                Ok(decoded) => decoded,
                // One damaged packet: skip it, as a player does.
                Err(Error::DecodeError(_)) => continue,
                Err(error) => return Err(format!("the sound stopped: {error}")),
            };
            let spec = *decoded.spec();
            self.rate = spec.rate;
            self.channels = spec.channels.count();
            let mut buffer = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
            buffer.copy_interleaved_ref(decoded);
            let mut samples = buffer.samples().to_vec();
            let mut millis = millis;
            if let Some(cut) = self.trim_before.take() {
                let channels = spec.channels.count().max(1);
                let frames = ((cut - millis) * f64::from(spec.rate) / 1000.0).max(0.0) as usize;
                let drop = (frames * channels).min(samples.len());
                samples.drain(..drop);
                millis = millis.max(cut);
            }
            if samples.is_empty() {
                continue;
            }
            return Ok(Some(Chunk {
                millis,
                samples,
                rate: spec.rate,
                channels: spec.channels.count(),
            }));
        }
    }

    /// Jump to `millis`, exactly: the next chunk starts there.
    pub(super) fn seek(&mut self, millis: f64) -> Result<(), String> {
        let seconds = (millis / 1000.0).max(0.0);
        let time = Time::new(seconds.trunc() as u64, seconds.fract());
        self.format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time,
                    track_id: Some(self.track),
                },
            )
            .map_err(|error| format!("the sound could not jump there: {error}"))?;
        self.decoder.reset();
        self.trim_before = Some(millis);
        Ok(())
    }
}

/// Sound from one rate and channel layout to another, by straight lines
/// between neighbouring frames. Keeps its place between chunks, so a stream
/// cut into chunks comes out the same as one played whole.
pub(super) struct Resampler {
    from_rate: u32,
    to_rate: u32,
    from_channels: usize,
    to_channels: usize,
    /// How far past `last` the next output frame falls, in input frames.
    phase: f64,
    last: Option<Vec<f32>>,
}

impl Resampler {
    pub(super) fn new(
        from_rate: u32,
        from_channels: usize,
        to_rate: u32,
        to_channels: usize,
    ) -> Self {
        Self {
            from_rate: from_rate.max(1),
            to_rate: to_rate.max(1),
            from_channels: from_channels.max(1),
            to_channels: to_channels.max(1),
            phase: 0.0,
            last: None,
        }
    }

    /// True when this resampler takes `rate` and `channels`.
    pub(super) fn takes(&self, rate: u32, channels: usize) -> bool {
        self.from_rate == rate.max(1) && self.from_channels == channels.max(1)
    }

    pub(super) fn reset(&mut self) {
        self.phase = 0.0;
        self.last = None;
    }

    fn map(&self, frame: &[f32], out: &mut Vec<f32>) {
        let from = self.from_channels;
        for channel in 0..self.to_channels {
            let sample = if self.to_channels == 1 && from > 1 {
                frame.iter().sum::<f32>() / from as f32
            } else if channel < from {
                frame[channel]
            } else if from == 1 {
                frame[0]
            } else {
                0.0
            };
            out.push(sample);
        }
    }

    /// Convert interleaved `input` and append it to `out`.
    pub(super) fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        let step = f64::from(self.from_rate) / f64::from(self.to_rate);
        let mut mixed = vec![0.0f32; self.from_channels];
        for frame in input.chunks_exact(self.from_channels) {
            let Some(last) = self.last.as_deref() else {
                self.last = Some(frame.to_vec());
                continue;
            };
            while self.phase < 1.0 {
                let t = self.phase as f32;
                for ((slot, a), b) in mixed.iter_mut().zip(last).zip(frame) {
                    *slot = a + (b - a) * t;
                }
                self.map(&mixed, out);
                self.phase += step;
            }
            self.phase -= 1.0;
            self.last = Some(frame.to_vec());
        }
    }
}

/// The system's sound output for one video: a ring of samples in the
/// device's own layout, drained by the device, counting what it plays.
pub(super) struct Speaker {
    // Dropped last: the device stops with it.
    _stream: cpal::Stream,
    ring: Arc<Mutex<VecDeque<f32>>>,
    played: Arc<AtomicU64>,
    running: Arc<AtomicBool>,
    /// The gain, as f32 bits: 0 when muted.
    gain: Arc<AtomicU32>,
    pub rate: u32,
    pub channels: usize,
}

impl Speaker {
    /// The default output device, or `None` when this computer has none
    /// (a server, a CI runner): the picture then keeps the wall's time.
    pub(super) fn open() -> Option<Self> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let device = cpal::default_host().default_output_device()?;
        let config = device.default_output_config().ok()?;
        let stream_config = config.config();
        let rate = stream_config.sample_rate;
        let channels = usize::from(stream_config.channels).max(1);
        let ring = Arc::new(Mutex::new(VecDeque::<f32>::new()));
        let played = Arc::new(AtomicU64::new(0));
        let running = Arc::new(AtomicBool::new(false));
        let gain = Arc::new(AtomicU32::new(1.0f32.to_bits()));
        macro_rules! stream {
            ($sample:ty, $convert:expr) => {{
                let (ring, played, running, gain) =
                    (ring.clone(), played.clone(), running.clone(), gain.clone());
                device.build_output_stream(
                    stream_config,
                    move |out: &mut [$sample], _| {
                        let level = f32::from_bits(gain.load(Ordering::Relaxed));
                        let mut taken = 0usize;
                        if running.load(Ordering::Relaxed) {
                            // try_lock: this is the real-time thread, and a
                            // missed callback beats a stalled device.
                            if let Ok(mut ring) = ring.try_lock() {
                                let whole = (ring.len().min(out.len()) / channels) * channels;
                                for (slot, sample) in out.iter_mut().zip(ring.drain(..whole)) {
                                    *slot = $convert((sample * level).clamp(-1.0, 1.0));
                                }
                                taken = whole;
                            }
                        }
                        for slot in out.iter_mut().skip(taken) {
                            *slot = $convert(0.0);
                        }
                        played.fetch_add((taken / channels) as u64, Ordering::Relaxed);
                    },
                    |_error| {},
                    None,
                )
            }};
        }
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => stream!(f32, |s: f32| s),
            cpal::SampleFormat::I16 => stream!(i16, |s: f32| (s * f32::from(i16::MAX)) as i16),
            cpal::SampleFormat::U16 => stream!(u16, |s: f32| ((s + 1.0) * 32_767.5) as u16),
            _ => return None,
        }
        .ok()?;
        stream.play().ok()?;
        Some(Self {
            _stream: stream,
            ring,
            played,
            running,
            gain,
            rate,
            channels,
        })
    }

    /// Frames played so far.
    pub(super) fn played(&self) -> u64 {
        self.played.load(Ordering::Relaxed)
    }

    pub(super) fn set_running(&self, running: bool) {
        self.running.store(running, Ordering::Relaxed);
    }

    pub(super) fn set_gain(&self, gain: f32) {
        self.gain
            .store(gain.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// Milliseconds of sound waiting to be played.
    pub(super) fn queued_millis(&self) -> f64 {
        let queued = self.ring.lock().map_or(0, |ring| ring.len());
        queued as f64 / self.channels as f64 * 1000.0 / f64::from(self.rate.max(1))
    }

    pub(super) fn push(&self, samples: &[f32]) {
        if let Ok(mut ring) = self.ring.lock() {
            ring.extend(samples.iter().copied());
        }
    }

    pub(super) fn clear(&self) {
        if let Ok(mut ring) = self.ring.lock() {
            ring.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn open(bytes: &'static [u8], extension: &str) -> Sound {
        Sound::open(Box::new(Cursor::new(bytes)), Some(extension))
            .expect("opens")
            .expect("has sound")
    }

    fn all(sound: &mut Sound) -> (f64, usize) {
        let mut first = None;
        let mut samples = 0;
        while let Some(chunk) = sound.next().expect("decodes") {
            first.get_or_insert(chunk.millis);
            samples += chunk.samples.len();
        }
        (first.unwrap_or(-1.0), samples)
    }

    #[test]
    fn every_sound_format_decodes_to_its_length() {
        for (bytes, extension, rate, channels) in [
            (&include_bytes!("fixtures/tone.wav")[..], "wav", 8_000, 1),
            (&include_bytes!("fixtures/tone.mp3")[..], "mp3", 22_050, 1),
            (&include_bytes!("fixtures/tone.ogg")[..], "ogg", 22_050, 2),
            (&include_bytes!("fixtures/clip.mp4")[..], "mp4", 44_100, 1),
        ] {
            let mut sound = open(bytes, extension);
            let first = sound.next().expect("decodes").expect("has sound");
            assert_eq!(
                (first.rate, first.channels),
                (rate, channels),
                "{extension}"
            );
            sound.seek(0.0).expect("back to the start");
            assert!(
                (sound.duration_millis - 1000.0).abs() < 60.0,
                "{extension}: {}",
                sound.duration_millis
            );
            let (_, samples) = all(&mut sound);
            let seconds = samples as f64 / f64::from(rate) / channels as f64;
            assert!((seconds - 1.0).abs() < 0.06, "{extension}: {seconds} s");
        }
    }

    #[test]
    fn a_seek_starts_the_sound_where_it_was_asked() {
        let mut sound = open(include_bytes!("fixtures/tone.wav"), "wav");
        sound.seek(600.0).unwrap();
        let (first, samples) = all(&mut sound);
        assert!((first - 600.0).abs() < 1.0, "{first}");
        let seconds = samples as f64 / 8_000.0;
        assert!((seconds - 0.4).abs() < 0.01, "{seconds}");
    }

    #[test]
    fn a_file_that_is_not_sound_says_so() {
        let error = match Sound::open(Box::new(Cursor::new(&b"just some text"[..])), Some("mp3")) {
            Ok(_) => panic!("text is not sound"),
            Err(error) => error,
        };
        assert!(error.contains("not a sound or video"), "{error}");
    }

    #[test]
    fn resampling_keeps_the_length_and_the_place_between_chunks() {
        let input: Vec<f32> = (0..4_410).map(|i| i as f32 / 4_410.0).collect();
        let mut whole = Vec::new();
        Resampler::new(44_100, 1, 48_000, 2).push(&input, &mut whole);
        let mut parts = Vec::new();
        let mut resampler = Resampler::new(44_100, 1, 48_000, 2);
        for chunk in input.chunks(777) {
            resampler.push(chunk, &mut parts);
        }
        assert_eq!(whole, parts, "chunks change nothing");
        // A tenth of a second at 48 kHz, stereo, give or take a frame.
        assert!(
            (whole.len() as i64 / 2 - 4_800).abs() <= 2,
            "{}",
            whole.len()
        );
        assert!(
            whole.chunks(2).all(|pair| pair[0] == pair[1]),
            "mono to both"
        );
        assert!(
            whole.windows(4).all(|w| w[2] >= w[0]),
            "a rising line stays one"
        );
        let mut down = Vec::new();
        Resampler::new(48_000, 2, 48_000, 1).push(&[0.2, 0.4, 0.6, 0.8, 1.0, 1.0], &mut down);
        assert_eq!(down.len(), 2);
        assert!(
            (down[0] - 0.3).abs() < 1e-6 && (down[1] - 0.7).abs() < 1e-6,
            "{down:?}"
        );
    }
}
