//! The microphone for the Audio recorder (src/recorder.rs): opened when a
//! recording starts, read while it runs, written out when it stops.

use super::Ui;
use crate::recorder::Want;
use alloc::vec::Vec;
use krate::audio::capture;
use krate::audio::types::{AudioError, SampleFormat, StreamConfig};
#[allow(unused_imports)]
use krate::prelude::*;

pub const RATE: u32 = 48_000;

/// A recording in progress: its stream and what it has heard.
pub struct Mic {
    pub stream: u64,
    pub pcm: Vec<u8>,
}

fn why(e: &AudioError) -> &'static str {
    match e {
        // The notes app's own words.
        // Krate offers an app the microphone only when it opens, and only
        // when the app cannot open without it; Vault opens without it, so
        // the recorder works where it was granted (K-948).
        AudioError::PermissionDenied => {
            "Microphone access was denied: Krate has not given Vault the microphone (K-948)."
        }
        AudioError::DeviceUnavailable => "No microphone is connected.",
        _ => "Krate could not open the microphone here.",
    }
}

impl Ui {
    /// Start, read or stop the microphone as the recorder asks. True when
    /// something changed on screen.
    pub(super) fn pump_mic(&mut self) -> bool {
        match self.app.record_want.take() {
            Some(Want::Start) if self.mic.is_none() => {
                let config = StreamConfig {
                    sample_rate: RATE,
                    channels: 1,
                    format: SampleFormat::PcmS16,
                    buffer_frames: 1024,
                };
                match capture::open(config).and_then(|id| capture::start(id).map(|()| id)) {
                    Ok(stream) => {
                        self.mic = Some(Mic {
                            stream,
                            pcm: Vec::new(),
                        });
                        self.app.mic_on = true;
                    }
                    Err(e) => self.app.record_failed(why(&e)),
                }
                return true;
            }
            Some(Want::Stop) => {
                if let Some(mut m) = self.mic.take() {
                    let _ = capture::stop(m.stream);
                    while let Ok(chunk) = capture::read(m.stream, 256 * 1024) {
                        if chunk.is_empty() {
                            break;
                        }
                        m.pcm.extend_from_slice(&chunk);
                    }
                    let file = crate::recorder::wav(&m.pcm, RATE, 1);
                    self.app.save_recording(&file, ".wav");
                }
                self.app.mic_on = false;
                return true;
            }
            _ => {}
        }
        if let Some(m) = self.mic.as_mut() {
            if let Ok(chunk) = capture::read(m.stream, 256 * 1024) {
                m.pcm.extend_from_slice(&chunk);
            }
        }
        false
    }
}
