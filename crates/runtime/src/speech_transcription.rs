//! Bounded, local speech transcription for Krate GUI applications.
//!
//! The guest supplies microphone audio it already received and names a model
//! carried under its own bundle `assets/` directory. The runtime validates both
//! before calling whisper.cpp. No subprocess, ambient filesystem path, or
//! network service is exposed to the guest.

#[cfg(feature = "speech")]
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

#[cfg(feature = "speech")]
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

const REQUIRED_SAMPLE_RATE: u32 = 16_000;
const MAX_AUDIO_BYTES: usize = REQUIRED_SAMPLE_RATE as usize * 2 * 60;
const MAX_MODEL_BYTES: u64 = 512 * 1024 * 1024;
#[cfg(feature = "speech")]
const MAX_TRANSCRIPT_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpeechError {
    InvalidRequest(String),
    ModelNotFound,
    ModelInvalid(String),
    Unsupported(String),
    /// Only produced when the engine is compiled in, but present in both
    /// builds: the error type must not change shape with a feature, or every
    /// match on it would need its own cfg.
    #[cfg_attr(not(feature = "speech"), allow(dead_code))]
    Inference(String),
}

#[derive(Default)]
pub struct LocalSpeechRuntime {
    asset_root: Option<PathBuf>,
    #[cfg(feature = "speech")]
    contexts: BTreeMap<PathBuf, WhisperContext>,
    pending_pcm_s16_le: Vec<u8>,
}

impl LocalSpeechRuntime {
    pub fn with_asset_root(mut self, root: Option<PathBuf>) -> Self {
        self.asset_root = root;
        self
    }

    pub fn transcribe(
        &mut self,
        model_asset: &str,
        pcm_s16_le: &[u8],
        sample_rate: u32,
        language: Option<&str>,
    ) -> Result<String, SpeechError> {
        // Validation runs either way. A guest that sends malformed audio should
        // get the same answer on a build without the engine as on one with it,
        // so the only difference between them is whether transcription itself
        // is available -- not what counts as a valid request.
        validate_audio(pcm_s16_le, sample_rate)?;
        let language = validate_language(language)?;
        let model_path = self.model_path(model_asset)?;

        #[cfg(not(feature = "speech"))]
        {
            let _ = (language, model_path);
            Err(SpeechError::Unsupported(
                "this build of Krate was compiled without speech-to-text".to_string(),
            ))
        }

        #[cfg(feature = "speech")]
        {
            if !self.contexts.contains_key(&model_path) {
                let context = WhisperContext::new_with_params(
                    &model_path,
                    WhisperContextParameters::default(),
                )
                .map_err(|error| SpeechError::ModelInvalid(error.to_string()))?;
                self.contexts.insert(model_path.clone(), context);
            }

            let context = self.contexts.get(&model_path).ok_or_else(|| {
                SpeechError::ModelInvalid("model cache was unavailable".to_string())
            })?;
            let mut state = context
                .create_state()
                .map_err(|error| SpeechError::Inference(error.to_string()))?;
            let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
            params.set_n_threads(
                std::thread::available_parallelism()
                    .map(|count| count.get().min(4) as i32)
                    .unwrap_or(1),
            );
            params.set_language(language);
            params.set_translate(false);
            params.set_no_context(true);
            params.set_single_segment(false);
            params.set_print_special(false);
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_timestamps(false);

            let samples = pcm_s16_le
                .chunks_exact(2)
                .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / i16::MAX as f32)
                .collect::<Vec<_>>();
            state
                .full(params, &samples)
                .map_err(|error| SpeechError::Inference(error.to_string()))?;

            let mut transcript = String::new();
            for segment in state.as_iter() {
                let text = segment
                    .to_str_lossy()
                    .map_err(|error| SpeechError::Inference(error.to_string()))?;
                let text = text.trim();
                if text.is_empty() {
                    continue;
                }
                if !transcript.is_empty() {
                    transcript.push(' ');
                }
                if transcript.len().saturating_add(text.len()) > MAX_TRANSCRIPT_BYTES {
                    return Err(SpeechError::Inference(
                        "transcript exceeded the runtime limit".to_string(),
                    ));
                }
                transcript.push_str(text);
            }
            Ok(transcript)
        }
    }

    pub fn match_line(
        &mut self,
        model_asset: &str,
        pcm_s16_le: &[u8],
        sample_rate: u32,
        language: Option<&str>,
        expected: &str,
    ) -> Result<u8, SpeechError> {
        if expected.trim().is_empty() || expected.len() > 16 * 1024 {
            return Err(SpeechError::InvalidRequest(
                "expected script line must be non-empty and bounded".to_string(),
            ));
        }
        let transcript = self.transcribe(model_asset, pcm_s16_le, sample_rate, language)?;
        Ok(word_overlap_score(&transcript, expected))
    }

    pub fn match_line_stream(
        &mut self,
        model_asset: &str,
        pcm_s16_le: &[u8],
        sample_rate: u32,
        language: Option<&str>,
        expected: &str,
        finish: bool,
    ) -> Result<Option<u8>, SpeechError> {
        validate_audio_chunk(pcm_s16_le, sample_rate)?;
        if self
            .pending_pcm_s16_le
            .len()
            .saturating_add(pcm_s16_le.len())
            > MAX_AUDIO_BYTES
        {
            self.pending_pcm_s16_le.clear();
            return Err(SpeechError::InvalidRequest(
                "speech input exceeds the 60 second runtime limit".to_string(),
            ));
        }
        self.pending_pcm_s16_le.extend_from_slice(pcm_s16_le);
        if !finish {
            return Ok(None);
        }

        let utterance = std::mem::take(&mut self.pending_pcm_s16_le);
        self.match_line(model_asset, &utterance, sample_rate, language, expected)
            .map(Some)
    }

    fn model_path(&self, model_asset: &str) -> Result<PathBuf, SpeechError> {
        let root = self.asset_root.as_ref().ok_or_else(|| {
            SpeechError::Unsupported("the app does not carry bundled assets".to_string())
        })?;
        let relative = safe_relative_path(model_asset)?;
        let path = root.join(relative);
        let metadata = std::fs::metadata(&path).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => SpeechError::ModelNotFound,
            _ => SpeechError::ModelInvalid(error.to_string()),
        })?;
        if !metadata.is_file() {
            return Err(SpeechError::ModelNotFound);
        }
        if metadata.len() == 0 || metadata.len() > MAX_MODEL_BYTES {
            return Err(SpeechError::ModelInvalid(
                "model is empty or exceeds 512 MiB".to_string(),
            ));
        }
        let canonical_root = std::fs::canonicalize(root)
            .map_err(|error| SpeechError::ModelInvalid(error.to_string()))?;
        let canonical_path = std::fs::canonicalize(&path)
            .map_err(|error| SpeechError::ModelInvalid(error.to_string()))?;
        if !canonical_path.starts_with(&canonical_root) {
            return Err(SpeechError::InvalidRequest(
                "model path escapes the bundled asset root".to_string(),
            ));
        }
        Ok(canonical_path)
    }
}

fn word_overlap_score(transcript: &str, expected: &str) -> u8 {
    let mut expected_count = 0u32;
    let mut matched = 0u32;
    for expected_word in expected
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|word| word.len() >= 3)
    {
        expected_count = expected_count.saturating_add(1);
        if transcript
            .split(|ch: char| !ch.is_ascii_alphanumeric())
            .any(|spoken| spoken.eq_ignore_ascii_case(expected_word))
        {
            matched = matched.saturating_add(1);
        }
    }
    if expected_count == 0 {
        return 0;
    }
    ((matched.saturating_mul(100) / expected_count).min(100)) as u8
}

fn validate_audio(pcm_s16_le: &[u8], sample_rate: u32) -> Result<(), SpeechError> {
    validate_audio_chunk(pcm_s16_le, sample_rate)?;
    if pcm_s16_le.is_empty() {
        return Err(SpeechError::InvalidRequest(
            "speech input must not be empty".to_string(),
        ));
    }
    Ok(())
}

fn validate_audio_chunk(pcm_s16_le: &[u8], sample_rate: u32) -> Result<(), SpeechError> {
    if sample_rate != REQUIRED_SAMPLE_RATE {
        return Err(SpeechError::InvalidRequest(format!(
            "speech input must be {REQUIRED_SAMPLE_RATE} Hz mono PCM"
        )));
    }
    if !pcm_s16_le.len().is_multiple_of(2) {
        return Err(SpeechError::InvalidRequest(
            "speech input must contain complete signed 16-bit samples".to_string(),
        ));
    }
    if pcm_s16_le.len() > MAX_AUDIO_BYTES {
        return Err(SpeechError::InvalidRequest(
            "speech input exceeds the 60 second runtime limit".to_string(),
        ));
    }
    Ok(())
}

fn validate_language(language: Option<&str>) -> Result<Option<&str>, SpeechError> {
    let Some(language) = language else {
        return Ok(None);
    };
    if language.is_empty()
        || language.len() > 16
        || !language
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(SpeechError::InvalidRequest(
            "language must be a short BCP 47 style tag".to_string(),
        ));
    }
    Ok(Some(language))
}

fn safe_relative_path(path: &str) -> Result<&Path, SpeechError> {
    if path.is_empty()
        || path.contains('\\')
        || path.chars().any(char::is_control)
        || Path::new(path).is_absolute()
    {
        return Err(SpeechError::InvalidRequest(
            "model asset must be a safe relative path".to_string(),
        ));
    }
    let path = Path::new(path);
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(SpeechError::InvalidRequest(
            "model asset must not contain traversal".to_string(),
        ));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_audio_before_loading_a_model() {
        let mut runtime = LocalSpeechRuntime::default();
        let error = runtime
            .transcribe("models/tiny.bin", &[0, 0], 48_000, Some("en"))
            .expect_err("sample rate must be checked");
        assert!(matches!(error, SpeechError::InvalidRequest(_)));
    }

    #[test]
    fn rejects_model_traversal_before_host_io() {
        let root = tempfile::tempdir().expect("asset root");
        let mut runtime =
            LocalSpeechRuntime::default().with_asset_root(Some(root.path().to_path_buf()));
        let error = runtime
            .transcribe("../model.bin", &[0, 0], 16_000, Some("en"))
            .expect_err("traversal must fail");
        assert!(matches!(error, SpeechError::InvalidRequest(_)));
    }

    #[test]
    fn missing_bundled_model_is_reported() {
        let root = tempfile::tempdir().expect("asset root");
        let mut runtime =
            LocalSpeechRuntime::default().with_asset_root(Some(root.path().to_path_buf()));
        let error = runtime
            .transcribe("models/tiny.bin", &[0, 0], 16_000, None)
            .expect_err("missing model must fail");
        assert_eq!(error, SpeechError::ModelNotFound);
    }

    #[test]
    fn word_overlap_is_bounded_and_case_insensitive() {
        assert_eq!(
            word_overlap_score("Krate made this application", "Krate made this app"),
            75
        );
        assert_eq!(word_overlap_score("", "Krate made this app"), 0);
    }

    #[test]
    fn streaming_match_buffers_without_loading_model_until_finish() {
        let mut runtime = LocalSpeechRuntime::default();
        assert_eq!(
            runtime
                .match_line_stream(
                    "models/tiny.bin",
                    &[0, 0, 1, 0],
                    16_000,
                    Some("en"),
                    "hello world",
                    false,
                )
                .expect("buffer chunk"),
            None
        );
        assert_eq!(runtime.pending_pcm_s16_le, [0, 0, 1, 0]);
    }

    #[test]
    fn streaming_match_clears_an_oversized_utterance() {
        let mut runtime = LocalSpeechRuntime {
            pending_pcm_s16_le: vec![0; MAX_AUDIO_BYTES],
            ..LocalSpeechRuntime::default()
        };
        let error = runtime
            .match_line_stream(
                "models/tiny.bin",
                &[0, 0],
                16_000,
                Some("en"),
                "hello world",
                false,
            )
            .expect_err("oversized stream");
        assert!(matches!(error, SpeechError::InvalidRequest(_)));
        assert!(runtime.pending_pcm_s16_le.is_empty());
    }
}
