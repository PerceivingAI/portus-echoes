use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use whisper_rs::WhisperContext;

use crate::audio::convert::StreamingResampler;

use super::engine::create_whisper_context_with_selection;
#[cfg(test)]
use super::engine::LocalContextSelection;
use super::inference::{SegmentInferenceWorker, SegmentTranscriber};
use super::output::{LocalOutputCommitter, LocalSegmentOutput, LocalTerminalOutput};
use super::vad;
#[cfg(test)]
use super::LocalEngine;
use super::{LocalTranscriptionError, ReadyLocalModel};

fn bind_language_to_segment_transcriber(
    language: Option<String>,
    mut transcribe: impl FnMut(
            &[f32],
            Option<&str>,
            &mut dyn FnMut(String) -> Result<(), LocalTranscriptionError>,
        ) -> Result<Option<String>, LocalTranscriptionError>
        + Send
        + 'static,
) -> SegmentTranscriber {
    Box::new(move |samples, on_segment| {
        transcribe(samples, language.as_deref(), on_segment)
    })
}

pub(crate) struct LocalSession {
    #[allow(dead_code)]
    model_path: PathBuf,
    #[allow(dead_code)]
    language: Option<String>,
    vad_segmenter: vad::VadSegmenter,
    resampler: StreamingResampler,
    #[cfg(test)]
    speech_samples_queued: u64,
    inference: SegmentInferenceWorker,
    next_segment_sequence: u64,
}

/// Diagnostics-only Local session. Transcript aggregation belongs to the
/// diagnostic caller through `on_segment`; the production runtime retains no
/// hidden full-transcript accumulator for live delivery.
pub(crate) fn start_isolated_session(
    model_path: &Path,
    language: Option<&str>,
    input_sample_rate: u32,
    vad_model_path: &Path,
    on_segment: LocalSegmentOutput,
) -> Result<LocalSession, LocalTranscriptionError> {
    let resampler = StreamingResampler::new(input_sample_rate)
        .map_err(LocalTranscriptionError::AudioConversion)?;
    if !model_path.is_file() {
        return Err(LocalTranscriptionError::ModelNotFound);
    }
    let context = create_whisper_context_with_selection(model_path)
        .map_err(|_| LocalTranscriptionError::ModelLoad)?
        .0;
    LocalSession::from_context_with_committer(
        model_path,
        language,
        vad_model_path,
        Arc::new(context),
        resampler,
        LocalOutputCommitter::live(on_segment),
        Arc::new(AtomicBool::new(false)),
    )
}

#[cfg(test)]
pub(crate) fn start_isolated_session_with_selection(
    model_path: &Path,
    language: Option<&str>,
    input_sample_rate: u32,
    vad_model_path: &Path,
) -> Result<(LocalSession, LocalContextSelection), LocalTranscriptionError> {
    let resampler = StreamingResampler::new(input_sample_rate)
        .map_err(LocalTranscriptionError::AudioConversion)?;
    if !model_path.is_file() {
        return Err(LocalTranscriptionError::ModelNotFound);
    }
    let (context, selection) = create_whisper_context_with_selection(model_path)
        .map_err(|_| LocalTranscriptionError::ModelLoad)?;
    let session = LocalSession::from_context(
        model_path,
        language,
        vad_model_path,
        Arc::new(context),
        resampler,
    )?;
    Ok((session, selection))
}

#[cfg(test)]
mod language_tests {
    use super::*;
    use parking_lot::Mutex;

    #[test]
    fn automatic_language_is_bound_to_every_segment_inference() {
        let observed = Arc::new(Mutex::new(Vec::new()));
        let calls = Arc::clone(&observed);
        let transcriber =
            bind_language_to_segment_transcriber(Some("auto".to_string()), move |_, language, _| {
                calls.lock().push(language.map(str::to_owned));
                Ok(None)
            });
        let mut worker = SegmentInferenceWorker::start(transcriber);

        worker.queue_segment(0, vec![0.1]).unwrap();
        worker.queue_segment(1, vec![0.2]).unwrap();
        worker.queue_segment(2, vec![0.3]).unwrap();
        worker.finish().unwrap();

        assert_eq!(
            &*observed.lock(),
            &[
                Some("auto".to_string()),
                Some("auto".to_string()),
                Some("auto".to_string()),
            ]
        );
    }

    #[test]
    fn language_binding_is_session_scoped_and_not_cached_between_sessions() {
        let observed = Arc::new(Mutex::new(Vec::new()));

        for language in ["auto", "fr"] {
            let calls = Arc::clone(&observed);
            let mut transcriber = bind_language_to_segment_transcriber(
                Some(language.to_string()),
                move |_, bound, _| {
                    calls.lock().push(bound.map(str::to_owned));
                    Ok(None)
                },
            );
            transcriber(&[0.1], &mut |_| Ok(())).unwrap();
        }

        assert_eq!(
            &*observed.lock(),
            &[Some("auto".to_string()), Some("fr".to_string())]
        );
    }
}

impl LocalSession {
    #[cfg(test)]
    pub(super) fn test_with_transcriber(
        vad_model_path: &Path,
        transcriber: SegmentTranscriber,
        committer: LocalOutputCommitter,
    ) -> Self {
        let vad_context =
            vad::load_bundled_vad_context(vad_model_path).expect("bundled VAD fixture must load");
        Self {
            model_path: PathBuf::from("test-only-model.bin"),
            language: Some("en".to_string()),
            vad_segmenter: vad::VadSegmenter::new(vad_context),
            resampler: StreamingResampler::new(crate::audio::convert::TARGET_SAMPLE_RATE)
                .expect("test sample rate must be valid"),
            speech_samples_queued: 0,
            inference: SegmentInferenceWorker::start_with_committer(
                transcriber,
                Arc::new(AtomicBool::new(false)),
                committer,
            ),
            next_segment_sequence: 0,
        }
    }

    #[cfg(test)]
    pub(super) fn from_context(
        model_path: &Path,
        language: Option<&str>,
        vad_model_path: &Path,
        context: Arc<WhisperContext>,
        resampler: StreamingResampler,
    ) -> Result<Self, LocalTranscriptionError> {
        Self::from_context_with_committer(
            model_path,
            language,
            vad_model_path,
            context,
            resampler,
            LocalOutputCommitter::terminal_clipboard(),
            Arc::new(AtomicBool::new(false)),
        )
    }

    pub(super) fn from_ready_model_with_committer(
        model_path: &Path,
        language: Option<&str>,
        vad_model_path: &Path,
        model: ReadyLocalModel,
        resampler: StreamingResampler,
        committer: LocalOutputCommitter,
        inference_abort: Arc<AtomicBool>,
    ) -> Result<Self, LocalTranscriptionError> {
        let vad_context = vad::load_bundled_vad_context(vad_model_path)
            .map_err(LocalTranscriptionError::VadAsset)?;

        let session_language = language.map(str::to_owned);
        let transcribe_abort = Arc::clone(&inference_abort);
        let inference_model = model;
        let inference = SegmentInferenceWorker::start_with_committer(
            bind_language_to_segment_transcriber(
                session_language.clone(),
                move |samples, inference_language, on_segment| {
                    inference_model.transcribe_stream(
                        samples,
                        inference_language,
                        Arc::clone(&transcribe_abort),
                        on_segment,
                    )
                },
            ),
            inference_abort,
            committer,
        );

        Ok(Self {
            model_path: model_path.to_path_buf(),
            language: session_language,
            vad_segmenter: vad::VadSegmenter::new(vad_context),
            resampler,
            #[cfg(test)]
            speech_samples_queued: 0,
            inference,
            next_segment_sequence: 0,
        })
    }

    pub(super) fn from_context_with_committer(
        model_path: &Path,
        language: Option<&str>,
        vad_model_path: &Path,
        context: Arc<WhisperContext>,
        resampler: StreamingResampler,
        committer: LocalOutputCommitter,
        inference_abort: Arc<AtomicBool>,
    ) -> Result<Self, LocalTranscriptionError> {
        let vad_context = vad::load_bundled_vad_context(vad_model_path)
            .map_err(LocalTranscriptionError::VadAsset)?;

        let state = match context.create_state() {
            Ok(state) => Arc::new(parking_lot::Mutex::new(state)),
            Err(_) => return Err(LocalTranscriptionError::StateCreation),
        };
        let session_language = language.map(str::to_owned);
        let transcribe_abort = Arc::clone(&inference_abort);
        let inference = SegmentInferenceWorker::start_with_committer(
            bind_language_to_segment_transcriber(
                session_language.clone(),
                move |samples, inference_language, on_segment| {
                    let mut state = state.lock();
                    super::inference::transcribe_with_state_stream_abortable_timeout(
                        &mut state,
                        samples,
                        inference_language,
                        Arc::clone(&transcribe_abort),
                        super::inference::LOCAL_INFERENCE_TIMEOUT,
                        on_segment,
                    )
                },
            ),
            inference_abort,
            committer,
        );

        Ok(Self {
            model_path: model_path.to_path_buf(),
            language: session_language,
            vad_segmenter: vad::VadSegmenter::new(vad_context),
            resampler,
            #[cfg(test)]
            speech_samples_queued: 0,
            inference,
            next_segment_sequence: 0,
        })
    }

    pub(crate) fn feed_audio(&mut self, audio: &[f32]) -> Result<(), LocalTranscriptionError> {
        self.inference.check_failure()?;
        let converted = self
            .resampler
            .feed(audio)
            .map_err(LocalTranscriptionError::AudioConversion)?;
        let segments = self
            .vad_segmenter
            .push(&converted)
            .map_err(LocalTranscriptionError::VadProcessing)?;
        self.queue_segments(segments)?;
        self.inference.check_failure()?;
        Ok(())
    }

    fn flush_audio(&mut self) -> Result<(), LocalTranscriptionError> {
        self.inference.check_failure()?;
        let tail = self
            .resampler
            .flush()
            .map_err(LocalTranscriptionError::AudioConversion)?;
        let mut segments = self
            .vad_segmenter
            .push(&tail)
            .map_err(LocalTranscriptionError::VadProcessing)?;
        segments.extend(
            self.vad_segmenter
                .finish()
                .map_err(LocalTranscriptionError::VadProcessing)?,
        );
        self.queue_segments(segments)?;
        self.inference.check_failure()?;
        Ok(())
    }

    /// Normal terminal path: flush resampler and trailing VAD exactly once,
    /// then join all accepted inference before returning terminal output metadata.
    pub(crate) fn finalize(mut self) -> Result<LocalTerminalOutput, LocalTranscriptionError> {
        let _ = self.flush_audio();
        self.inference.finish()
    }

    fn queue_segments(
        &mut self,
        segments: Vec<vad::VadSpeechSegment>,
    ) -> Result<(), LocalTranscriptionError> {
        for segment in segments {
            let sequence = self.next_segment_sequence;
            #[cfg(test)]
            {
                self.speech_samples_queued = self
                    .speech_samples_queued
                    .saturating_add(segment.samples.len() as u64);
            }
            self.inference.queue_segment(sequence, segment.samples)?;
            self.next_segment_sequence = self.next_segment_sequence.saturating_add(1);
        }
        Ok(())
    }

    pub(crate) fn abort(mut self) {
        self.inference.abort();
    }

    #[cfg(test)]
    pub(super) fn model_path(&self) -> &Path {
        &self.model_path
    }


    #[cfg(test)]
    pub(super) fn inference_counts(&self) -> (u64, u64) {
        self.inference.counts()
    }

    #[cfg(test)]
    pub(super) fn language(&self) -> Option<&str> {
        self.language.as_deref()
    }
}

#[cfg(test)]
impl LocalEngine {
    pub fn start_ready_session(
        &self,
        model_path: &Path,
        language: Option<&str>,
        input_sample_rate: u32,
        vad_model_path: &Path,
    ) -> Result<LocalSession, LocalTranscriptionError> {
        let resampler = StreamingResampler::new(input_sample_rate)
            .map_err(LocalTranscriptionError::AudioConversion)?;
        let model = self
            .ready_model(model_path)
            .ok_or(LocalTranscriptionError::ModelLoad)?;
        LocalSession::from_ready_model_with_committer(
            model_path,
            language,
            vad_model_path,
            model,
            resampler,
            LocalOutputCommitter::terminal_clipboard(),
            Arc::new(AtomicBool::new(false)),
        )
    }
}
