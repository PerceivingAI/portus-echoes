//! Bundled Silero VAD asset and stateful Local streaming VAD pipeline.
//!
//! Portus Echoes owns one pinned Silero model and one pinned whisper.cpp backend.
//! Production Local VAD consumes each 16 kHz PCM frame once through
//! `WhisperVadContext::detect_speech_no_reset`, preserving Silero's recurrent
//! state across adjacent frames and brief hesitations, then resetting it only
//! after a genuine minimum-silence utterance end. Forced inference splits do not
//! reset recurrent state. There is no bounded-tail batch re-analysis or Local
//! fallback path here.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tauri::path::BaseDirectory;
use tauri::{AppHandle, Manager};
use whisper_rs::{WhisperVadContext, WhisperVadContextParams};

const VAD_SAMPLE_RATE: usize = 16_000;
const VAD_FRAME_SAMPLES: usize = 512;
const VAD_THRESHOLD: f32 = 0.45;
const VAD_NEG_THRESHOLD: f32 = 0.30;
const VAD_MIN_SPEECH_MS: usize = 200;
const VAD_MIN_SILENCE_MS: usize = 500;
const VAD_MAX_SPEECH_SECONDS: usize = 8;
const VAD_SPEECH_PAD_MS: usize = 100;
const VAD_OVERLAP_MS: usize = 150;
const VAD_MAX_SPLIT_SILENCE_MS: usize = 98;

const VAD_MIN_SPEECH_SAMPLES: u64 = (VAD_SAMPLE_RATE * VAD_MIN_SPEECH_MS / 1000) as u64;
const VAD_MIN_SILENCE_SAMPLES: u64 = (VAD_SAMPLE_RATE * VAD_MIN_SILENCE_MS / 1000) as u64;
const VAD_SPEECH_PAD_SAMPLES: u64 = (VAD_SAMPLE_RATE * VAD_SPEECH_PAD_MS / 1000) as u64;
const VAD_OVERLAP_SAMPLES: u64 = (VAD_SAMPLE_RATE * VAD_OVERLAP_MS / 1000) as u64;
const VAD_MAX_SPLIT_SILENCE_SAMPLES: u64 =
    (VAD_SAMPLE_RATE * VAD_MAX_SPLIT_SILENCE_MS / 1000) as u64;
const VAD_MAX_SPEECH_SAMPLES: u64 = (VAD_SAMPLE_RATE * VAD_MAX_SPEECH_SECONDS) as u64
    - VAD_FRAME_SAMPLES as u64
    - 2 * VAD_SPEECH_PAD_SAMPLES;

#[allow(dead_code)]
pub const BUNDLED_VAD_VERSION: &str = "silero-v6.2.0";
pub const BUNDLED_VAD_RESOURCE_PATH: &str = "assets/ggml-silero-v6.2.0.bin";
#[allow(dead_code)]
pub const BUNDLED_VAD_LICENSE_RESOURCE_PATH: &str = "assets/silero-vad-LICENSE.txt";
pub const BUNDLED_VAD_SHA256: &str =
    "2aa269b785eeb53a82983a20501ddf7c1d9c48e33ab63a41391ac6c9f7fb6987";
pub const BUNDLED_VAD_SIZE_BYTES: u64 = 885_098;
#[allow(dead_code)]
pub const BUNDLED_VAD_UPSTREAM_COMMIT: &str = "9ffd54a1e1ee413ddf265af9913beaf518d1639b";
#[allow(dead_code)]
pub const BUNDLED_VAD_SOURCE: &str = "https://huggingface.co/ggml-org/whisper-vad/resolve/9ffd54a1e1ee413ddf265af9913beaf518d1639b/ggml-silero-v6.2.0.bin";

/// Structured technical failures for the app-owned VAD asset.
/// No user-facing message text is stored here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadAssetError {
    Resolve,
    Missing,
    Read,
    SizeMismatch,
    HashMismatch,
    InvalidPath,
    Load,
}

/// Structured VAD-processing failures. These are technical facts only and
/// never contain message text or user audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadProcessingError {
    Inference,
    InvalidProbability,
    InvalidSegment,
    InvalidState,
}

/// One finalized protected speech segment produced by the Local VAD pipeline.
/// The recording-owned Local Whisper worker consumes these samples in order.
#[derive(Debug)]
#[allow(dead_code)] // consumed by the Local segment-inference queue
pub(crate) struct VadSpeechSegment {
    pub(crate) samples: Vec<f32>,
    pub(crate) start_sample: u64,
    pub(crate) end_sample: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RawSpeechSegment {
    start_sample: u64,
    end_sample: u64,
    leading_overlap: bool,
    trailing_overlap: bool,
}

#[derive(Debug, Default)]
struct VadDecisionOutcome {
    completed: Vec<RawSpeechSegment>,
    utterance_ended: bool,
}

/// Pure segmentation state over one Silero probability every 512 samples.
/// This mirrors the relevant upstream threshold/minimum-silence/max-duration
/// semantics while allowing finalized boundaries to be emitted online.
#[derive(Debug)]
struct VadDecisionState {
    in_speech: bool,
    current_start: Option<u64>,
    current_leading_overlap: bool,
    temp_end: Option<u64>,
    previous_silence_end: Option<u64>,
    next_start_after_silence: Option<u64>,
}

impl VadDecisionState {
    fn new() -> Self {
        Self {
            in_speech: false,
            current_start: None,
            current_leading_overlap: false,
            temp_end: None,
            previous_silence_end: None,
            next_start_after_silence: None,
        }
    }

    fn consume_probability(
        &mut self,
        probability: f32,
        current_sample: u64,
    ) -> Result<VadDecisionOutcome, VadProcessingError> {
        if !probability.is_finite() || !(0.0..=1.0).contains(&probability) {
            return Err(VadProcessingError::InvalidProbability);
        }

        let mut completed = Vec::new();
        let mut utterance_ended = false;

        if probability >= VAD_THRESHOLD && self.temp_end.is_some() {
            self.temp_end = None;
            if let Some(previous_end) = self.previous_silence_end {
                if self
                    .next_start_after_silence
                    .map(|next| next < previous_end)
                    .unwrap_or(true)
                {
                    self.next_start_after_silence = Some(current_sample);
                }
            }
        }

        if probability >= VAD_THRESHOLD && !self.in_speech {
            self.in_speech = true;
            self.current_start = Some(current_sample);
            self.current_leading_overlap = false;
            return Ok(VadDecisionOutcome {
                completed,
                utterance_ended,
            });
        }

        if self.in_speech {
            let start = self.current_start.ok_or(VadProcessingError::InvalidState)?;
            if current_sample.saturating_sub(start) > VAD_MAX_SPEECH_SAMPLES {
                if let Some(split_at) = self.previous_silence_end {
                    let resume_at = self
                        .next_start_after_silence
                        .filter(|next| *next >= split_at);
                    let uses_internal_overlap = resume_at.is_some();
                    if split_at.saturating_sub(start) >= VAD_MIN_SPEECH_SAMPLES {
                        completed.push(RawSpeechSegment {
                            start_sample: start,
                            end_sample: split_at,
                            leading_overlap: self.current_leading_overlap,
                            trailing_overlap: uses_internal_overlap,
                        });
                    }

                    if let Some(resume_at) = resume_at {
                        self.in_speech = true;
                        self.current_start = Some(resume_at);
                        self.current_leading_overlap = true;
                    } else {
                        self.in_speech = false;
                        self.current_start = None;
                        self.current_leading_overlap = false;
                    }
                    self.temp_end = None;
                    self.previous_silence_end = None;
                    self.next_start_after_silence = None;
                } else {
                    if current_sample.saturating_sub(start) >= VAD_MIN_SPEECH_SAMPLES {
                        completed.push(RawSpeechSegment {
                            start_sample: start,
                            end_sample: current_sample,
                            leading_overlap: self.current_leading_overlap,
                            trailing_overlap: true,
                        });
                    }
                    // The current frame is already classified as part of the
                    // continuing speech, so make the forced boundary exactly
                    // its start and immediately begin the next segment there.
                    self.in_speech = true;
                    self.current_start = Some(current_sample);
                    self.current_leading_overlap = true;
                    self.temp_end = None;
                    self.previous_silence_end = None;
                    self.next_start_after_silence = None;

                    if probability >= VAD_THRESHOLD {
                        return Ok(VadDecisionOutcome {
                            completed,
                            utterance_ended,
                        });
                    }
                }
            }
        }

        if probability < VAD_NEG_THRESHOLD && self.in_speech {
            let start = self.current_start.ok_or(VadProcessingError::InvalidState)?;
            let temp_end = *self.temp_end.get_or_insert(current_sample);
            let silence = current_sample.saturating_sub(temp_end);

            if silence > VAD_MAX_SPLIT_SILENCE_SAMPLES {
                self.previous_silence_end = Some(temp_end);
            }

            if silence >= VAD_MIN_SILENCE_SAMPLES {
                if temp_end.saturating_sub(start) >= VAD_MIN_SPEECH_SAMPLES {
                    completed.push(RawSpeechSegment {
                        start_sample: start,
                        end_sample: temp_end,
                        leading_overlap: self.current_leading_overlap,
                        trailing_overlap: false,
                    });
                }
                self.in_speech = false;
                self.current_start = None;
                self.current_leading_overlap = false;
                self.temp_end = None;
                self.previous_silence_end = None;
                utterance_ended = true;
                self.next_start_after_silence = None;
            }
        }

        Ok(VadDecisionOutcome {
            completed,
            utterance_ended,
        })
    }

    fn finish(
        &mut self,
        audio_end_sample: u64,
    ) -> Result<Vec<RawSpeechSegment>, VadProcessingError> {
        let mut completed = Vec::new();
        if self.in_speech {
            let start = self.current_start.ok_or(VadProcessingError::InvalidState)?;
            if audio_end_sample.saturating_sub(start) >= VAD_MIN_SPEECH_SAMPLES {
                completed.push(RawSpeechSegment {
                    start_sample: start,
                    end_sample: audio_end_sample,
                    leading_overlap: self.current_leading_overlap,
                    trailing_overlap: false,
                });
            }
        }
        self.in_speech = false;
        self.current_start = None;
        self.current_leading_overlap = false;
        self.temp_end = None;
        self.previous_silence_end = None;
        self.next_start_after_silence = None;
        Ok(completed)
    }

    fn active_protected_start(&self) -> Option<u64> {
        self.current_start.map(|start| {
            let protection = if self.current_leading_overlap {
                VAD_OVERLAP_SAMPLES.max(VAD_SPEECH_PAD_SAMPLES)
            } else {
                VAD_SPEECH_PAD_SAMPLES
            };
            start.saturating_sub(protection)
        })
    }

    fn history_samples_to_keep(&self) -> u64 {
        VAD_SPEECH_PAD_SAMPLES
    }
}

/// Recording-owned stateful VAD processor.
///
/// Each complete 512-sample frame is passed through Silero exactly once. Only
/// an incomplete frame remainder and the PCM still needed for active/pending
/// segment boundary protection are retained.
pub(crate) struct VadSegmenter {
    context: WhisperVadContext,
    decision: VadDecisionState,
    frame_remainder: Vec<f32>,
    audio: Vec<f32>,
    audio_base_sample: u64,
    total_audio_samples: u64,
    next_frame_sample: u64,
    pending_segments: VecDeque<RawSpeechSegment>,
    #[cfg(test)]
    utterance_resets: u64,
    frames_processed: u64,
    finalized: bool,
}

impl VadSegmenter {
    pub(crate) fn new(mut context: WhisperVadContext) -> Self {
        context.reset_state();
        Self {
            context,
            decision: VadDecisionState::new(),
            frame_remainder: Vec::with_capacity(VAD_FRAME_SAMPLES),
            audio: Vec::new(),
            audio_base_sample: 0,
            total_audio_samples: 0,
            next_frame_sample: 0,
            pending_segments: VecDeque::new(),
            #[cfg(test)]
            utterance_resets: 0,
            frames_processed: 0,
            finalized: false,
        }
    }

    /// Consume sequential 16 kHz mono PCM and return speech segments that have
    /// become fully boundary-protected while recording continues.
    pub(crate) fn push(
        &mut self,
        samples: &[f32],
    ) -> Result<Vec<VadSpeechSegment>, VadProcessingError> {
        if self.finalized {
            return Err(VadProcessingError::InvalidState);
        }
        if samples.is_empty() {
            return Ok(Vec::new());
        }

        self.audio.extend_from_slice(samples);
        self.total_audio_samples = self
            .total_audio_samples
            .saturating_add(samples.len() as u64);
        self.frame_remainder.extend_from_slice(samples);

        let mut output = Vec::new();
        while self.frame_remainder.len() >= VAD_FRAME_SAMPLES {
            let frame: Vec<f32> = self.frame_remainder.drain(..VAD_FRAME_SAMPLES).collect();
            self.process_frame(&frame)?;
            output.extend(self.materialize_ready(false)?);
            self.drain_unneeded_audio();
        }
        Ok(output)
    }

    /// Finish the recording. A final partial 512-sample frame is submitted once;
    /// whisper.cpp performs its documented zero-padding internally.
    pub(crate) fn finish(&mut self) -> Result<Vec<VadSpeechSegment>, VadProcessingError> {
        if self.finalized {
            return Err(VadProcessingError::InvalidState);
        }

        if !self.frame_remainder.is_empty() {
            let final_frame = std::mem::take(&mut self.frame_remainder);
            self.process_frame(&final_frame)?;
        }

        let trailing = self.decision.finish(self.total_audio_samples)?;
        self.pending_segments.extend(trailing);
        let output = self.materialize_ready(true)?;
        self.audio.clear();
        self.audio_base_sample = self.total_audio_samples;
        self.finalized = true;
        Ok(output)
    }

    fn process_frame(&mut self, frame: &[f32]) -> Result<(), VadProcessingError> {
        if frame.is_empty() || frame.len() > VAD_FRAME_SAMPLES {
            return Err(VadProcessingError::InvalidState);
        }
        self.context
            .detect_speech_no_reset(frame)
            .map_err(|_| VadProcessingError::Inference)?;
        let probabilities = self.context.probabilities();
        if probabilities.len() != 1 {
            return Err(VadProcessingError::InvalidProbability);
        }
        let current_sample = self.next_frame_sample;
        self.next_frame_sample = self
            .next_frame_sample
            .saturating_add(VAD_FRAME_SAMPLES as u64);
        self.frames_processed = self.frames_processed.saturating_add(1);
        let outcome = self
            .decision
            .consume_probability(probabilities[0], current_sample)?;
        self.pending_segments.extend(outcome.completed);
        if outcome.utterance_ended {
            self.context.reset_state();
            #[cfg(test)]
            {
                self.utterance_resets = self.utterance_resets.saturating_add(1);
            }
        }
        Ok(())
    }

    fn protected_bounds(raw: RawSpeechSegment, audio_end_sample: u64) -> Option<(u64, u64)> {
        let start_protection = if raw.leading_overlap {
            VAD_OVERLAP_SAMPLES.max(VAD_SPEECH_PAD_SAMPLES)
        } else {
            VAD_SPEECH_PAD_SAMPLES
        };
        let end_protection = if raw.trailing_overlap {
            VAD_OVERLAP_SAMPLES.max(VAD_SPEECH_PAD_SAMPLES)
        } else {
            VAD_SPEECH_PAD_SAMPLES
        };
        let start = raw.start_sample.saturating_sub(start_protection);
        let end = raw
            .end_sample
            .saturating_add(end_protection)
            .min(audio_end_sample);
        (start < end).then_some((start, end))
    }

    fn materialize_ready(
        &mut self,
        end_of_stream: bool,
    ) -> Result<Vec<VadSpeechSegment>, VadProcessingError> {
        let mut output = Vec::new();
        loop {
            let Some(raw) = self.pending_segments.front().copied() else {
                break;
            };
            let required_end_protection = if raw.trailing_overlap {
                VAD_OVERLAP_SAMPLES.max(VAD_SPEECH_PAD_SAMPLES)
            } else {
                VAD_SPEECH_PAD_SAMPLES
            };
            let target_end = raw.end_sample.saturating_add(required_end_protection);
            if !end_of_stream && self.total_audio_samples < target_end {
                break;
            }
            let (protected_start, protected_end) =
                Self::protected_bounds(raw, self.total_audio_samples)
                    .ok_or(VadProcessingError::InvalidSegment)?;
            let protected_start = protected_start.max(self.audio_base_sample);
            let protected_end = protected_end.min(self.total_audio_samples);
            if protected_start >= protected_end {
                self.pending_segments.pop_front();
                continue;
            }
            let relative_start = (protected_start - self.audio_base_sample) as usize;
            let relative_end = (protected_end - self.audio_base_sample) as usize;
            let relative_end = relative_end.min(self.audio.len());
            if relative_start >= relative_end {
                self.pending_segments.pop_front();
                continue;
            }

            output.push(VadSpeechSegment {
                samples: self.audio[relative_start..relative_end].to_vec(),
                start_sample: protected_start,
                end_sample: protected_end,
            });
            self.pending_segments.pop_front();
        }
        Ok(output)
    }

    fn drain_unneeded_audio(&mut self) {
        let history_start = self
            .next_frame_sample
            .saturating_sub(self.decision.history_samples_to_keep());
        let mut retain_from = history_start;

        if let Some(active_start) = self.decision.active_protected_start() {
            retain_from = retain_from.min(active_start);
        }
        if let Some(raw) = self.pending_segments.front() {
            let protection = if raw.leading_overlap {
                VAD_OVERLAP_SAMPLES.max(VAD_SPEECH_PAD_SAMPLES)
            } else {
                VAD_SPEECH_PAD_SAMPLES
            };
            retain_from = retain_from.min(raw.start_sample.saturating_sub(protection));
        }

        retain_from = retain_from
            .max(self.audio_base_sample)
            .min(self.total_audio_samples);
        let drain = (retain_from - self.audio_base_sample) as usize;
        if drain > 0 {
            self.audio.drain(..drain.min(self.audio.len()));
            self.audio_base_sample = retain_from;
        }
    }

    #[cfg(test)]
    pub(crate) fn frames_processed(&self) -> u64 {
        self.frames_processed
    }

    #[cfg(test)]
    fn retained_audio_samples(&self) -> usize {
        self.audio.len()
    }

    #[cfg(test)]
    fn utterance_resets(&self) -> u64 {
        self.utterance_resets
    }
}

/// Resolve the exact packaged VAD resource through Tauri's resource directory,
/// then verify that the installed bytes are the pinned asset.
pub fn resolve_bundled_vad_model(app: &AppHandle) -> Result<PathBuf, VadAssetError> {
    let path = app
        .path()
        .resolve(BUNDLED_VAD_RESOURCE_PATH, BaseDirectory::Resource)
        .map_err(|_| VadAssetError::Resolve)?;
    validate_bundled_vad_model(&path)?;
    Ok(path)
}

/// Validate an explicit VAD model path against the pinned size and SHA-256.
pub fn validate_bundled_vad_model(path: &Path) -> Result<(), VadAssetError> {
    if !path.is_file() {
        return Err(VadAssetError::Missing);
    }
    let metadata = fs::metadata(path).map_err(|_| VadAssetError::Read)?;
    if metadata.len() != BUNDLED_VAD_SIZE_BYTES {
        return Err(VadAssetError::SizeMismatch);
    }
    let bytes = fs::read(path).map_err(|_| VadAssetError::Read)?;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    if digest != BUNDLED_VAD_SHA256 {
        return Err(VadAssetError::HashMismatch);
    }
    Ok(())
}

/// Load a verified standalone whisper.cpp VAD context from the pinned asset.
pub fn load_bundled_vad_context(path: &Path) -> Result<WhisperVadContext, VadAssetError> {
    validate_bundled_vad_model(path)?;
    let path = path.to_str().ok_or(VadAssetError::InvalidPath)?;
    super::super::configure_native_logging();
    let mut params = WhisperVadContextParams::new();
    params.set_use_gpu(false);
    WhisperVadContext::new(path, params).map_err(|_| VadAssetError::Load)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source_asset_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(BUNDLED_VAD_RESOURCE_PATH)
    }

    fn diagnostic_audio_16k() -> Vec<f32> {
        let wav_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/diagnostics_speech.wav");
        let mut reader = hound::WavReader::open(wav_path).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        let samples: Vec<f32> = reader
            .samples::<i16>()
            .map(|sample| sample.unwrap() as f32 / 32768.0)
            .collect();
        crate::audio::convert::resample_to_16k_mono(&samples, spec.sample_rate).unwrap()
    }

    fn speech_clip(audio: &[f32], start_ms: usize, end_ms: usize) -> Vec<f32> {
        let start = start_ms * VAD_SAMPLE_RATE / 1000;
        let end = end_ms * VAD_SAMPLE_RATE / 1000;
        audio[start..end].to_vec()
    }

    fn new_segmenter() -> VadSegmenter {
        VadSegmenter::new(load_bundled_vad_context(&source_asset_path()).unwrap())
    }

    fn run_stream(audio: &[f32], chunk_sizes: &[usize]) -> (Vec<VadSpeechSegment>, u64) {
        let mut segmenter = new_segmenter();
        let mut output = Vec::new();
        let mut offset = 0usize;
        let mut chunk_index = 0usize;
        while offset < audio.len() {
            let chunk = chunk_sizes[chunk_index % chunk_sizes.len()].max(1);
            let end = (offset + chunk).min(audio.len());
            output.extend(segmenter.push(&audio[offset..end]).unwrap());
            offset = end;
            chunk_index += 1;
        }
        output.extend(segmenter.finish().unwrap());
        (output, segmenter.frames_processed())
    }

    fn collect_streaming_probabilities(context: &mut WhisperVadContext, audio: &[f32]) -> Vec<f32> {
        context.reset_state();
        let mut output = Vec::new();
        for frame in audio.chunks(VAD_FRAME_SAMPLES) {
            context.detect_speech_no_reset(frame).unwrap();
            let probabilities = context.probabilities();
            assert_eq!(probabilities.len(), 1);
            output.push(probabilities[0]);
        }
        output
    }

    fn high_frames(count: usize) -> Vec<f32> {
        vec![0.95; count]
    }

    fn low_frames(count: usize) -> Vec<f32> {
        vec![0.0; count]
    }

    fn drive_decision(probabilities: &[f32]) -> Vec<RawSpeechSegment> {
        let mut state = VadDecisionState::new();
        let mut output = Vec::new();
        for (index, probability) in probabilities.iter().copied().enumerate() {
            output.extend(
                state
                    .consume_probability(probability, (index * VAD_FRAME_SAMPLES) as u64)
                    .unwrap()
                    .completed,
            );
        }
        output
    }

    fn drive_decision_with_utterance_ends(probabilities: &[f32]) -> (Vec<RawSpeechSegment>, usize) {
        let mut state = VadDecisionState::new();
        let mut output = Vec::new();
        let mut utterance_ends = 0usize;
        for (index, probability) in probabilities.iter().copied().enumerate() {
            let outcome = state
                .consume_probability(probability, (index * VAD_FRAME_SAMPLES) as u64)
                .unwrap();
            output.extend(outcome.completed);
            utterance_ends += usize::from(outcome.utterance_ended);
        }
        (output, utterance_ends)
    }

    #[test]
    fn stateful_probabilities_match_upstream_batch_detection() {
        let audio = speech_clip(&diagnostic_audio_16k(), 0, 1600);
        let mut batch = load_bundled_vad_context(&source_asset_path()).unwrap();
        batch.detect_speech(&audio).unwrap();
        let batch_probabilities = batch.probabilities().to_vec();

        let mut streaming = load_bundled_vad_context(&source_asset_path()).unwrap();
        let streaming_probabilities = collect_streaming_probabilities(&mut streaming, &audio);

        assert_eq!(streaming_probabilities.len(), batch_probabilities.len());
        let max_error = streaming_probabilities
            .iter()
            .zip(batch_probabilities.iter())
            .map(|(left, right)| (left - right).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_error <= 1e-7,
            "streaming probability drift: {max_error}"
        );
    }

    #[test]
    fn reset_state_restores_fresh_stream_behavior() {
        let audio = speech_clip(&diagnostic_audio_16k(), 0, 1600);
        let mut reference = load_bundled_vad_context(&source_asset_path()).unwrap();
        let expected = collect_streaming_probabilities(&mut reference, &audio);

        let mut reused = load_bundled_vad_context(&source_asset_path()).unwrap();
        reused
            .detect_speech_no_reset(&audio[..VAD_FRAME_SAMPLES])
            .unwrap();
        let actual = collect_streaming_probabilities(&mut reused, &audio);
        assert_eq!(actual, expected);
    }

    #[test]
    fn new_recording_segmenter_clears_preexisting_recurrent_vad_state() {
        let audio = diagnostic_audio_16k();
        let (expected, expected_frames) = run_stream(&audio, &[1600]);

        let mut contaminated = load_bundled_vad_context(&source_asset_path()).unwrap();
        for frame in audio[..VAD_FRAME_SAMPLES * 12].chunks(VAD_FRAME_SAMPLES) {
            contaminated.detect_speech_no_reset(frame).unwrap();
        }
        let mut segmenter = VadSegmenter::new(contaminated);
        let mut actual = Vec::new();
        for chunk in audio.chunks(1600) {
            actual.extend(segmenter.push(chunk).unwrap());
        }
        actual.extend(segmenter.finish().unwrap());

        assert_eq!(segmenter.frames_processed(), expected_frames);
        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(expected.iter()) {
            assert_eq!(actual.start_sample, expected.start_sample);
            assert_eq!(actual.end_sample, expected.end_sample);
            assert_eq!(actual.samples, expected.samples);
        }
    }

    #[test]
    fn short_hesitation_does_not_split_probability_state() {
        let mut probabilities = high_frames(35);
        probabilities.extend(low_frames(8)); // 256 ms < configured 500 ms silence.
        probabilities.extend(high_frames(25));
        probabilities.extend(low_frames(18)); // complete the segment.
        let segments = drive_decision(&probabilities);
        assert_eq!(segments.len(), 1);
    }

    #[test]
    fn brief_hesitation_does_not_end_the_utterance() {
        let mut probabilities = high_frames(35);
        probabilities.extend(low_frames(8));
        let (segments, utterance_ends) = drive_decision_with_utterance_ends(&probabilities);
        assert!(segments.is_empty());
        assert_eq!(utterance_ends, 0);
    }

    #[test]
    fn meaningful_silence_finalizes_before_end_of_stream() {
        let mut probabilities = high_frames(35);
        probabilities.extend(low_frames(18));
        let segments = drive_decision(&probabilities);
        assert_eq!(segments.len(), 1);
        assert!(segments[0].end_sample > segments[0].start_sample);
    }

    #[test]
    fn genuine_minimum_silence_marks_exactly_one_utterance_end() {
        let mut probabilities = high_frames(35);
        probabilities.extend(low_frames(18));
        let (segments, utterance_ends) = drive_decision_with_utterance_ends(&probabilities);
        assert_eq!(segments.len(), 1);
        assert_eq!(utterance_ends, 1);
    }

    #[test]
    fn rejected_short_utterance_still_marks_a_genuine_utterance_end() {
        let mut probabilities = high_frames(3); // 96 ms < 200 ms minimum speech.
        probabilities.extend(low_frames(18));
        let (segments, utterance_ends) = drive_decision_with_utterance_ends(&probabilities);
        assert!(segments.is_empty());
        assert_eq!(utterance_ends, 1);
    }

    #[test]
    fn finite_max_duration_forces_uninterrupted_speech_segment() {
        let required_frames = (VAD_MAX_SPEECH_SAMPLES as usize / VAD_FRAME_SAMPLES) + 4;
        let segments = drive_decision(&high_frames(required_frames));
        assert!(
            !segments.is_empty(),
            "maximum duration must force a segment"
        );
        assert!(
            segments[0].end_sample - segments[0].start_sample
                <= VAD_MAX_SPEECH_SAMPLES + VAD_FRAME_SAMPLES as u64
        );
        assert!(segments[0].trailing_overlap);
    }

    #[test]
    fn natural_boundaries_use_speech_padding() {
        let raw = RawSpeechSegment {
            start_sample: 10_000,
            end_sample: 20_000,
            leading_overlap: false,
            trailing_overlap: false,
        };
        assert_eq!(
            VadSegmenter::protected_bounds(raw, 30_000),
            Some((
                10_000 - VAD_SPEECH_PAD_SAMPLES,
                20_000 + VAD_SPEECH_PAD_SAMPLES
            ))
        );
    }

    #[test]
    fn forced_boundaries_use_configured_overlap_protection() {
        let raw = RawSpeechSegment {
            start_sample: 10_000,
            end_sample: 20_000,
            leading_overlap: true,
            trailing_overlap: true,
        };
        let protection = VAD_OVERLAP_SAMPLES.max(VAD_SPEECH_PAD_SAMPLES);
        assert_eq!(
            VadSegmenter::protected_bounds(raw, 30_000),
            Some((10_000 - protection, 20_000 + protection))
        );
    }

    #[test]
    fn forced_split_marks_both_sides_for_overlap_protection() {
        let required_frames = (VAD_MAX_SPEECH_SAMPLES as usize / VAD_FRAME_SAMPLES) + 4;
        let mut state = VadDecisionState::new();
        let mut emitted = Vec::new();
        for (index, probability) in high_frames(required_frames).into_iter().enumerate() {
            let outcome = state
                .consume_probability(probability, (index * VAD_FRAME_SAMPLES) as u64)
                .unwrap();
            assert!(!outcome.utterance_ended);
            emitted.extend(outcome.completed);
            if !emitted.is_empty() {
                break;
            }
        }
        assert_eq!(emitted.len(), 1);
        assert!(emitted[0].trailing_overlap);
        assert!(state.current_leading_overlap);
    }

    #[test]
    fn silence_never_creates_a_segment() {
        let segments = drive_decision(&low_frames(400));
        assert!(segments.is_empty());
    }

    #[test]
    fn real_fixture_segments_are_ordered_and_chunk_invariant() {
        let audio = diagnostic_audio_16k();
        let (a, a_frames) = run_stream(&audio, &[1600]);
        let (b, b_frames) = run_stream(&audio, &[512, 1024]);
        assert!(a.len() >= 3, "expected multiple natural speech segments");
        assert_eq!(a.len(), b.len());
        assert_eq!(a_frames, b_frames);
        assert_eq!(a_frames as usize, audio.len().div_ceil(VAD_FRAME_SAMPLES));
        for (left, right) in a.iter().zip(b.iter()) {
            assert_eq!(left.start_sample, right.start_sample);
            assert_eq!(left.end_sample, right.end_sample);
            assert_eq!(left.samples, right.samples);
        }
        assert!(a
            .windows(2)
            .all(|pair| pair[0].start_sample < pair[1].start_sample));
    }

    #[test]
    fn meaningful_real_pause_emits_before_release() {
        let source = diagnostic_audio_16k();
        let mut audio = speech_clip(&source, 400, 1800);
        audio.extend(vec![0.0; VAD_SAMPLE_RATE * 800 / 1000]);

        let mut segmenter = new_segmenter();
        let mut output = Vec::new();
        for chunk in audio.chunks(1600) {
            output.extend(segmenter.push(chunk).unwrap());
        }
        assert_eq!(
            output.len(),
            1,
            "speech must finalize while capture is active"
        );
        assert_eq!(
            segmenter.utterance_resets(),
            1,
            "one genuine minimum-silence boundary must reset native recurrent state once"
        );
    }

    #[test]
    fn quiet_real_speech_remains_detectable() {
        let source = diagnostic_audio_16k();
        let quiet: Vec<f32> = speech_clip(&source, 400, 1800)
            .into_iter()
            .map(|sample| sample * 0.25)
            .collect();
        let (segments, _) = run_stream(&quiet, &[977, 2048, 333]);
        assert!(
            !segments.is_empty(),
            "quiet speech should remain detectable"
        );
    }

    #[test]
    fn low_background_noise_does_not_create_speech() {
        let samples = VAD_SAMPLE_RATE * 5;
        let noise: Vec<f32> = (0..samples)
            .map(|index| {
                let value = ((index as u64 * 1_103_515_245 + 12_345) & 0xffff) as f32;
                (value / 65_535.0 - 0.5) * 0.02
            })
            .collect();
        let (segments, _) = run_stream(&noise, &[1600, 4096, 777]);
        assert!(segments.is_empty());
    }

    #[test]
    fn final_partial_frame_is_processed_exactly_once() {
        let source = diagnostic_audio_16k();
        let mut audio = speech_clip(&source, 400, 1800);
        while audio.len() % VAD_FRAME_SAMPLES == 0 {
            audio.pop();
        }
        let expected_frames = audio.len().div_ceil(VAD_FRAME_SAMPLES) as u64;
        let (segments, frames) = run_stream(&audio, &[73, 997, 2049]);
        assert_eq!(frames, expected_frames);
        assert!(
            !segments.is_empty(),
            "trailing speech must survive final partial frame"
        );
    }

    #[test]
    fn retained_vad_pcm_is_bounded_during_long_silence() {
        let mut segmenter = new_segmenter();
        let silence = vec![0.0; VAD_SAMPLE_RATE * 3];
        for chunk in silence.chunks(1600) {
            assert!(segmenter.push(chunk).unwrap().is_empty());
        }
        assert!(
            segmenter.retained_audio_samples() <= VAD_SPEECH_PAD_SAMPLES as usize + 2048,
            "idle VAD PCM retention must stay bounded"
        );
    }

    #[test]
    fn bundled_vad_asset_matches_pinned_identity() {
        let path = source_asset_path();
        validate_bundled_vad_model(&path).unwrap();
        assert_eq!(fs::metadata(path).unwrap().len(), BUNDLED_VAD_SIZE_BYTES);
    }

    #[test]
    fn bundled_vad_asset_loads_with_whisper_rs() {
        assert!(load_bundled_vad_context(&source_asset_path()).is_ok());
    }

    #[test]
    fn missing_vad_asset_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            validate_bundled_vad_model(&dir.path().join("missing.bin")),
            Err(VadAssetError::Missing)
        );
    }

    #[test]
    fn modified_vad_asset_is_rejected_before_load() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("vad.bin");
        let mut bytes = fs::read(source_asset_path()).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        fs::write(&target, bytes).unwrap();
        assert_eq!(
            validate_bundled_vad_model(&target),
            Err(VadAssetError::HashMismatch)
        );
    }

    #[test]
    fn materialize_ready_clamps_when_protected_start_precedes_audio_base() {
        let mut segmenter =
            VadSegmenter::new(load_bundled_vad_context(&source_asset_path()).unwrap());
        segmenter.audio = vec![0.1; 1000];
        segmenter.audio_base_sample = 500;
        segmenter.total_audio_samples = 1500;
        segmenter.pending_segments.push_back(RawSpeechSegment {
            start_sample: 550,
            end_sample: 1200,
            leading_overlap: false,
            trailing_overlap: false,
        });
        let materialized = segmenter.materialize_ready(true).expect("must clamp without error");
        assert_eq!(materialized.len(), 1);
        assert_eq!(materialized[0].start_sample, 500);
        assert!(!materialized[0].samples.is_empty());
    }
}
