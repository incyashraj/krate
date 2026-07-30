//! Native microphone capture behind the `audio.capture` capability.
//!
//! CPAL owns the operating-system stream. The Wasm guest sees only bounded PCM
//! chunks after the runtime policy has granted microphone access.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

const MAX_CAPTURE_BUFFER_BYTES: usize = 8 * 1024 * 1024;
const MAX_READ_BYTES: usize = 1024 * 1024;
const MIN_SAMPLE_RATE: u32 = 8_000;
const MAX_SAMPLE_RATE: u32 = 192_000;
const MAX_CHANNELS: u16 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureSampleFormat {
    PcmS16,
    Float32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub format: CaptureSampleFormat,
    pub buffer_frames: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureError {
    InvalidStream,
    DeviceUnavailable,
    InvalidConfig(String),
    Platform(String),
}

struct CaptureStream {
    stream: cpal::Stream,
    bytes: Arc<Mutex<VecDeque<u8>>>,
    stream_error: Arc<Mutex<Option<String>>>,
    started: bool,
}

pub struct AudioCaptureRuntime {
    next_stream_id: u64,
    streams: BTreeMap<u64, CaptureStream>,
}

impl Default for AudioCaptureRuntime {
    fn default() -> Self {
        Self {
            next_stream_id: 1,
            streams: BTreeMap::new(),
        }
    }
}

impl AudioCaptureRuntime {
    pub fn open(&mut self, config: CaptureConfig) -> Result<u64, CaptureError> {
        validate_config(config)?;
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or(CaptureError::DeviceUnavailable)?;
        let device_config = device
            .default_input_config()
            .map_err(|error| CaptureError::Platform(error.to_string()))?;
        let stream_config = device_config.config();
        let input_rate = stream_config.sample_rate;
        let input_channels = stream_config.channels;
        let bytes = Arc::new(Mutex::new(VecDeque::new()));
        let stream_error = Arc::new(Mutex::new(None));
        let callback_error = stream_error.clone();
        let error_callback = move |error: cpal::Error| {
            if let Ok(mut current) = callback_error.lock() {
                *current = Some(error.to_string());
            }
        };

        let stream = match device_config.sample_format() {
            cpal::SampleFormat::I16 => {
                let callback_bytes = bytes.clone();
                let mut converter = CaptureConverter::new(input_rate, input_channels, config);
                device.build_input_stream(
                    stream_config,
                    move |samples: &[i16], _| {
                        converter.process_i16(&callback_bytes, samples);
                    },
                    error_callback,
                    None,
                )
            }
            cpal::SampleFormat::U16 => {
                let callback_bytes = bytes.clone();
                let mut converter = CaptureConverter::new(input_rate, input_channels, config);
                device.build_input_stream(
                    stream_config,
                    move |samples: &[u16], _| {
                        converter.process_u16(&callback_bytes, samples);
                    },
                    error_callback,
                    None,
                )
            }
            cpal::SampleFormat::F32 => {
                let callback_bytes = bytes.clone();
                let mut converter = CaptureConverter::new(input_rate, input_channels, config);
                device.build_input_stream(
                    stream_config,
                    move |samples: &[f32], _| {
                        converter.process_f32(&callback_bytes, samples);
                    },
                    error_callback,
                    None,
                )
            }
            format => {
                return Err(CaptureError::Platform(format!(
                    "the default microphone uses unsupported sample format {format:?}"
                )))
            }
        }
        .map_err(|error| CaptureError::Platform(error.to_string()))?;

        let stream_id = self.next_stream_id;
        self.next_stream_id = self
            .next_stream_id
            .checked_add(1)
            .ok_or_else(|| CaptureError::Platform("audio stream id space exhausted".to_string()))?;
        self.streams.insert(
            stream_id,
            CaptureStream {
                stream,
                bytes,
                stream_error,
                started: false,
            },
        );
        Ok(stream_id)
    }

    pub fn start(&mut self, stream_id: u64) -> Result<(), CaptureError> {
        let stream = self
            .streams
            .get_mut(&stream_id)
            .ok_or(CaptureError::InvalidStream)?;
        stream
            .stream
            .play()
            .map_err(|error| CaptureError::Platform(error.to_string()))?;
        stream.started = true;
        Ok(())
    }

    pub fn stop(&mut self, stream_id: u64) -> Result<(), CaptureError> {
        let stream = self
            .streams
            .get_mut(&stream_id)
            .ok_or(CaptureError::InvalidStream)?;
        stream
            .stream
            .pause()
            .map_err(|error| CaptureError::Platform(error.to_string()))?;
        stream.started = false;
        Ok(())
    }

    pub fn read(&mut self, stream_id: u64, max_bytes: u32) -> Result<Vec<u8>, CaptureError> {
        let stream = self
            .streams
            .get_mut(&stream_id)
            .ok_or(CaptureError::InvalidStream)?;
        if let Ok(mut error) = stream.stream_error.lock() {
            if let Some(error) = error.take() {
                return Err(CaptureError::Platform(error));
            }
        }
        if !stream.started {
            return Ok(Vec::new());
        }
        let requested = usize::try_from(max_bytes)
            .unwrap_or(MAX_READ_BYTES)
            .min(MAX_READ_BYTES);
        let mut result = Vec::with_capacity(requested);
        let mut bytes = stream
            .bytes
            .lock()
            .map_err(|_| CaptureError::Platform("microphone buffer lock poisoned".to_string()))?;
        for _ in 0..requested {
            let Some(byte) = bytes.pop_front() else {
                break;
            };
            result.push(byte);
        }
        Ok(result)
    }
}

/// Normalizes the host's default microphone format to the exact bounded PCM
/// contract requested by the guest. A phone headset, laptop microphone, and
/// USB interface can expose different rates and channel counts; that is a host
/// detail and must not make the same `.krate` file behave differently.
struct CaptureConverter {
    input_rate: u32,
    input_channels: usize,
    output_rate: u32,
    output_channels: usize,
    output_format: CaptureSampleFormat,
    rate_accumulator: u64,
}

impl CaptureConverter {
    fn new(input_rate: u32, input_channels: u16, output: CaptureConfig) -> Self {
        Self {
            input_rate,
            input_channels: usize::from(input_channels),
            output_rate: output.sample_rate,
            output_channels: usize::from(output.channels),
            output_format: output.format,
            rate_accumulator: 0,
        }
    }

    fn process_i16(&mut self, buffer: &Arc<Mutex<VecDeque<u8>>>, samples: &[i16]) {
        self.process(
            buffer,
            samples
                .iter()
                .map(|sample| f32::from(*sample) / f32::from(i16::MAX)),
        );
    }

    fn process_u16(&mut self, buffer: &Arc<Mutex<VecDeque<u8>>>, samples: &[u16]) {
        self.process(
            buffer,
            samples
                .iter()
                .map(|sample| (f32::from(*sample) / 32_767.5) - 1.0),
        );
    }

    fn process_f32(&mut self, buffer: &Arc<Mutex<VecDeque<u8>>>, samples: &[f32]) {
        self.process(buffer, samples.iter().copied());
    }

    fn process(&mut self, buffer: &Arc<Mutex<VecDeque<u8>>>, samples: impl Iterator<Item = f32>) {
        let samples = samples.collect::<Vec<_>>();
        let mut output = Vec::new();
        for frame in samples.chunks_exact(self.input_channels) {
            self.rate_accumulator += u64::from(self.output_rate);
            while self.rate_accumulator >= u64::from(self.input_rate) {
                self.rate_accumulator -= u64::from(self.input_rate);
                self.push_frame(&mut output, frame);
            }
        }
        append_bytes(buffer, output);
    }

    fn push_frame(&self, output: &mut Vec<u8>, frame: &[f32]) {
        if self.output_channels == 1 {
            let sum = frame.iter().copied().sum::<f32>();
            self.push_sample(output, sum / frame.len() as f32);
            return;
        }
        for channel in 0..self.output_channels {
            let sample = if frame.len() == 1 {
                frame[0]
            } else {
                frame[channel.min(frame.len() - 1)]
            };
            self.push_sample(output, sample);
        }
    }

    fn push_sample(&self, output: &mut Vec<u8>, sample: f32) {
        let sample = sample.clamp(-1.0, 1.0);
        match self.output_format {
            CaptureSampleFormat::PcmS16 => {
                output.extend_from_slice(&((sample * f32::from(i16::MAX)) as i16).to_le_bytes());
            }
            CaptureSampleFormat::Float32 => output.extend_from_slice(&sample.to_le_bytes()),
        }
    }
}

fn validate_config(config: CaptureConfig) -> Result<(), CaptureError> {
    if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&config.sample_rate) {
        return Err(CaptureError::InvalidConfig(format!(
            "sample rate must be between {MIN_SAMPLE_RATE} and {MAX_SAMPLE_RATE} Hz"
        )));
    }
    if config.channels == 0 || config.channels > MAX_CHANNELS {
        return Err(CaptureError::InvalidConfig(format!(
            "channels must be between 1 and {MAX_CHANNELS}"
        )));
    }
    if config.buffer_frames == 0 {
        return Err(CaptureError::InvalidConfig(
            "buffer-frames must be greater than zero".to_string(),
        ));
    }
    Ok(())
}

fn append_bytes(buffer: &Arc<Mutex<VecDeque<u8>>>, incoming: Vec<u8>) {
    let Ok(mut buffer) = buffer.lock() else {
        return;
    };
    let overflow = buffer
        .len()
        .saturating_add(incoming.len())
        .saturating_sub(MAX_CAPTURE_BUFFER_BYTES);
    for _ in 0..overflow.min(buffer.len()) {
        buffer.pop_front();
    }
    let remaining = MAX_CAPTURE_BUFFER_BYTES.saturating_sub(buffer.len());
    buffer.extend(incoming.into_iter().take(remaining));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_capture_configs_fail_before_opening_a_device() {
        let mut runtime = AudioCaptureRuntime::default();
        let error = runtime
            .open(CaptureConfig {
                sample_rate: 0,
                channels: 1,
                format: CaptureSampleFormat::PcmS16,
                buffer_frames: 1_600,
            })
            .expect_err("invalid sample rate");

        assert!(matches!(error, CaptureError::InvalidConfig(_)));
    }

    #[test]
    fn capture_buffers_are_bounded_and_keep_the_newest_audio() {
        let buffer = Arc::new(Mutex::new(VecDeque::from(vec![
            1_u8;
            MAX_CAPTURE_BUFFER_BYTES - 2
        ])));
        append_bytes(&buffer, vec![2, 3, 4, 5]);
        let buffer = buffer.lock().expect("buffer");

        assert_eq!(buffer.len(), MAX_CAPTURE_BUFFER_BYTES);
        assert_eq!(buffer[buffer.len() - 4], 2);
        assert_eq!(buffer[buffer.len() - 1], 5);
    }

    #[test]
    fn converter_remixes_stereo_and_resamples_to_the_guest_contract() {
        let buffer = Arc::new(Mutex::new(VecDeque::new()));
        let mut converter = CaptureConverter::new(
            48_000,
            2,
            CaptureConfig {
                sample_rate: 16_000,
                channels: 1,
                format: CaptureSampleFormat::PcmS16,
                buffer_frames: 1_600,
            },
        );
        // Three stereo input frames produce one mono output frame at 16 kHz.
        converter.process_f32(&buffer, &[0.5, 0.5, 0.25, 0.25, 1.0, 0.0]);
        let buffer = buffer.lock().expect("buffer");
        assert_eq!(buffer.len(), 2);
        let sample = i16::from_le_bytes([buffer[0], buffer[1]]);
        assert!((16_000..=16_500).contains(&sample));
    }
}
