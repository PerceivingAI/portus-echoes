//! Local transcription runtime ownership.
//!
//! Local model readiness is established before recording. Recording freezes an
//! already-Ready model worker, language, and output policy before CPAL begins;
//! recording-owned VAD, resampling, inference ordering, feed, and output behavior
//! live in dedicated modules.

mod engine;
mod feed;
mod inference;
mod output;
mod recording;
pub(crate) mod vad;
mod worker;

mod budget {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[derive(Clone, Debug)]
    pub(super) struct SampleBudget {
        inner: Arc<SampleBudgetInner>,
    }

    #[derive(Debug)]
    struct SampleBudgetInner {
        max_samples: usize,
        used_samples: AtomicUsize,
    }

    #[derive(Debug)]
    pub(super) struct SampleReservation {
        budget: SampleBudget,
        samples: usize,
    }

    impl SampleBudget {
        pub(super) fn new(max_samples: usize) -> Self {
            Self {
                inner: Arc::new(SampleBudgetInner {
                    max_samples,
                    used_samples: AtomicUsize::new(0),
                }),
            }
        }

        pub(super) fn reserve(&self, samples: usize) -> Option<SampleReservation> {
            let mut used = self.inner.used_samples.load(Ordering::Acquire);
            loop {
                if samples > self.inner.max_samples.saturating_sub(used) {
                    return None;
                }
                match self.inner.used_samples.compare_exchange_weak(
                    used,
                    used + samples,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        return Some(SampleReservation {
                            budget: self.clone(),
                            samples,
                        });
                    }
                    Err(actual) => used = actual,
                }
            }
        }

        fn release(&self, samples: usize) {
            let mut used = self.inner.used_samples.load(Ordering::Acquire);
            loop {
                let next = used
                    .checked_sub(samples)
                    .expect("SampleBudget reservation release underflow");
                match self.inner.used_samples.compare_exchange_weak(
                    used,
                    next,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => return,
                    Err(actual) => used = actual,
                }
            }
        }

        #[cfg(test)]
        pub(super) fn used_samples(&self) -> usize {
            self.inner.used_samples.load(Ordering::Acquire)
        }

        #[cfg(test)]
        pub(super) fn max_samples(&self) -> usize {
            self.inner.max_samples
        }
    }

    impl Drop for SampleReservation {
        fn drop(&mut self) {
            self.budget.release(self.samples);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn reservations_are_atomic_bounded_and_release_exactly() {
            let budget = SampleBudget::new(8);
            let first = budget.reserve(5).unwrap();
            assert_eq!(budget.used_samples(), 5);
            assert!(budget.reserve(4).is_none());
            assert_eq!(budget.used_samples(), 5);
            let second = budget.reserve(3).unwrap();
            assert_eq!(budget.used_samples(), 8);
            drop(first);
            assert_eq!(budget.used_samples(), 3);
            drop(second);
            assert_eq!(budget.used_samples(), 0);
        }
    }
}

use std::path::PathBuf;

use tauri::AppHandle;

use crate::audio::convert::AudioConversionError;
use crate::models::AppSettings;
use crate::output::DeliveryCapability;

pub use engine::LocalEngine;
#[cfg(test)]
use engine::{
    create_context_with_fallback, create_whisper_context_with_selection, preferred_whisper_backend,
    LocalInferenceBackend, PreferredWhisperBackend,
};
pub(crate) use engine::{
    LocalRuntimeAuthority, LocalRuntimeIntent, LocalRuntimeTarget, ReadyLocalModel,
};
pub use feed::{LocalAudioFeed, LocalAudioFeedError};
pub(crate) use feed::{LocalProcessingControl, LocalProcessingLease};
#[cfg(test)]
use inference::{SegmentInferenceWorker, SegmentTranscriber};
#[cfg(test)]
use output::{
    complete_once, LocalCompletion, LocalOutputCommitter, LocalSegmentOutput, LocalTerminalOutput,
};
pub(crate) use recording::start_isolated_session;
#[cfg(test)]
use recording::LocalSession;
#[cfg(test)]
pub(crate) use vad::BUNDLED_VAD_RESOURCE_PATH;
pub(crate) use worker::run_internal_worker_from_process_args;

/// Structured failures from the normal Local transcription path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalTranscriptionError {
    ModelNotFound,
    ModelLoad,
    VadAsset(vad::VadAssetError),
    AudioConversion(AudioConversionError),
    VadProcessing(vad::VadProcessingError),
    StateCreation,
    Inference,
    SegmentRead,
    InferenceProtocol,
    Delivery,
    FeedClosed,
    Backpressure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalPreloadError {
    ModelNotFound,
    UnsupportedModel,
    InsufficientRam,
    Unexpected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageCode(String);

impl LanguageCode {
    fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalLanguagePolicy {
    Fixed(LanguageCode),
    AutoPerSegment,
}

impl LocalLanguagePolicy {
    fn from_build_time(value: &str) -> Self {
        if value == "auto" {
            Self::AutoPerSegment
        } else {
            Self::Fixed(LanguageCode::new(value))
        }
    }

    pub(super) fn language(&self) -> Option<&str> {
        match self {
            Self::Fixed(code) => Some(code.as_str()),
            Self::AutoPerSegment => Some("auto"),
        }
    }
}

/// Build-time switch: true = paste segments as they decode; false = wait for full VAD segment decode.
pub(crate) const COMPILED_SEGMENT_DELIVERY: bool = match env!("PORTUS_LOCAL_SEGMENT_DELIVERY").as_bytes() {
    b"true" => true,
    _ => false,
};

/// Build-time character limit per segment chunk: 0 = unconstrained natural Whisper segments; >0 = wrap at word boundaries.
pub(crate) const COMPILED_MAX_LENGTH: i32 = match compile_time_max_length(env!("PORTUS_LOCAL_MAX_LENGTH")) {
    Ok(val) => val,
    Err(_) => 0,
};

const fn compile_time_max_length(s: &str) -> Result<i32, ()> {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return Ok(0);
    }
    let mut val: i32 = 0;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b < b'0' || b > b'9' {
            return Err(());
        }
        val = val * 10 + (b - b'0') as i32;
        i += 1;
    }
    Ok(val)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalOutputPolicy {
    LiveInject,
    TerminalClipboard,
}

impl LocalOutputPolicy {
    fn from_delivery_capability(capability: DeliveryCapability) -> Self {
        match capability {
            DeliveryCapability::Inject => Self::LiveInject,
            DeliveryCapability::Clipboard => Self::TerminalClipboard,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalRecordingConfig {
    pub(super) model_path: PathBuf,
    pub(super) language: LocalLanguagePolicy,
    pub(super) output: LocalOutputPolicy,
}

impl LocalRecordingConfig {
    pub fn from_settings(settings: &AppSettings, delivery_capability: DeliveryCapability) -> Self {
        let language = LocalLanguagePolicy::from_build_time(env!("PORTUS_LOCAL_LANGUAGE"));
        Self {
            model_path: PathBuf::from(&settings.local_model_path),
            language,
            output: LocalOutputPolicy::from_delivery_capability(delivery_capability),
        }
    }

    pub(crate) fn model_path(&self) -> &std::path::Path {
        &self.model_path
    }
}

pub struct PreparedLocalRecording {
    pub(super) model_path: PathBuf,
    pub(super) language: LocalLanguagePolicy,
    pub(super) output: LocalOutputPolicy,
    pub(super) vad_model_path: PathBuf,
    pub(super) model: ReadyLocalModel,
}

impl PreparedLocalRecording {
    pub fn prepare(
        app: &AppHandle,
        model: ReadyLocalModel,
        config: LocalRecordingConfig,
    ) -> Result<Self, LocalTranscriptionError> {
        if model.path() != config.model_path.as_path() {
            return Err(LocalTranscriptionError::ModelLoad);
        }
        let vad_model_path =
            vad::resolve_bundled_vad_model(app).map_err(LocalTranscriptionError::VadAsset)?;

        Ok(Self {
            model_path: config.model_path,
            language: config.language,
            output: config.output,
            vad_model_path,
            model,
        })
    }

    pub fn into_audio_feed(
        self,
        lifecycle: LocalProcessingLease,
        app: AppHandle,
        recording_id: crate::models::RecordingIdentity,
        on_terminal: impl Fn(Result<(), LocalTranscriptionError>) + Send + Sync + 'static,
    ) -> LocalAudioFeed {
        feed::build_audio_feed(self, lifecycle, app, recording_id, on_terminal)
    }
}

pub(crate) fn resolve_bundled_vad_model(app: &AppHandle) -> Result<PathBuf, vad::VadAssetError> {
    vad::resolve_bundled_vad_model(app)
}

#[cfg(test)]
const LOCAL_FEED_CONTROL_BUDGET: std::time::Duration = std::time::Duration::from_secs(2);

#[cfg(test)]
mod tests;
