use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperState};

use super::budget::{SampleBudget, SampleReservation};
#[cfg(test)]
use super::output::LocalSegmentOutput;
use super::output::{LocalOutputCommitter, LocalTerminalOutput};
use super::LocalTranscriptionError;

pub(super) type SegmentTranscriber = Box<
    dyn FnMut(
            &[f32],
            &mut dyn FnMut(String) -> Result<(), LocalTranscriptionError>,
        ) -> Result<Option<String>, LocalTranscriptionError>
        + Send
        + 'static,
>;
const INFERENCE_SAMPLE_BUDGET: usize = 1800 * 16_000;

/// Local-owned deadline for one Whisper `state.full(...)` inference operation.
/// Normal Local sessions and isolated Local Diagnostics sessions share this policy.
pub(crate) const LOCAL_INFERENCE_TIMEOUT: Duration = Duration::from_secs(30);

fn inference_abort_requested(explicit_abort: bool, now: Instant, deadline: Instant) -> bool {
    explicit_abort || now >= deadline
}

enum SegmentInferenceCommand {
    Segment {
        sequence: u64,
        samples: Vec<f32>,
        _reservation: SampleReservation,
    },
    Finish(Sender<Result<LocalTerminalOutput, LocalTranscriptionError>>),
    Abort,
}

#[derive(Debug, Default)]
struct SegmentInferenceStatus {
    failure: Option<LocalTranscriptionError>,
    queued_segments: u64,
    completed_segments: u64,
}

pub(super) struct SegmentInferenceWorker {
    tx: Option<Sender<SegmentInferenceCommand>>,
    status: Arc<Mutex<SegmentInferenceStatus>>,
    budget: SampleBudget,
    abort_requested: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl SegmentInferenceWorker {
    #[cfg(test)]
    pub(super) fn start(transcribe: SegmentTranscriber) -> Self {
        Self::start_with_abort(transcribe, Arc::new(AtomicBool::new(false)))
    }

    #[cfg(test)]
    pub(super) fn start_with_abort(
        transcribe: SegmentTranscriber,
        abort_requested: Arc<AtomicBool>,
    ) -> Self {
        Self::start_with_committer(
            transcribe,
            abort_requested,
            LocalOutputCommitter::terminal_clipboard(),
        )
    }

    #[cfg(test)]
    pub(super) fn start_with_abort_and_output(
        transcribe: SegmentTranscriber,
        abort_requested: Arc<AtomicBool>,
        on_segment: LocalSegmentOutput,
    ) -> Self {
        Self::start_with_committer(
            transcribe,
            abort_requested,
            LocalOutputCommitter::live(on_segment),
        )
    }

    pub(super) fn start_with_committer(
        transcribe: SegmentTranscriber,
        abort_requested: Arc<AtomicBool>,
        committer: LocalOutputCommitter,
    ) -> Self {
        Self::start_with_committer_budget(
            transcribe,
            abort_requested,
            committer,
            SampleBudget::new(INFERENCE_SAMPLE_BUDGET),
        )
    }

    fn start_with_committer_budget(
        mut transcribe: SegmentTranscriber,
        abort_requested: Arc<AtomicBool>,
        mut committer: LocalOutputCommitter,
        budget: SampleBudget,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<SegmentInferenceCommand>();
        let status = Arc::new(Mutex::new(SegmentInferenceStatus::default()));
        let worker_status = Arc::clone(&status);
        let worker_abort = Arc::clone(&abort_requested);

        let join = std::thread::spawn(move || {
            while let Ok(command) = rx.recv() {
                match command {
                    SegmentInferenceCommand::Segment {
                        sequence,
                        samples,
                        _reservation,
                    } => {
                        if worker_abort.load(Ordering::SeqCst) {
                            continue;
                        }
                        if worker_status.lock().failure.is_some() {
                            continue;
                        }
                        let mut chunk_err = None;
                        let result = transcribe(
                            &samples,
                            &mut |chunk| {
                                if chunk_err.is_none() {
                                    if let Err(err) = committer.commit_chunk(sequence, chunk) {
                                        chunk_err = Some(err);
                                        return Err(err);
                                    }
                                }
                                Ok(())
                            },
                        );
                        if worker_abort.load(Ordering::SeqCst) {
                            continue;
                        }

                        let final_result = match chunk_err {
                            Some(err) => Err(err),
                            None => result.and_then(|_| committer.advance_sequence(sequence)),
                        };

                        match final_result {
                            Ok(()) => {
                                let mut status = worker_status.lock();
                                status.completed_segments =
                                    status.completed_segments.saturating_add(1);
                            }
                            Err(error) => {
                                worker_status.lock().failure = Some(error);
                            }
                        }
                    }
                    SegmentInferenceCommand::Finish(ack) => {
                        let output = committer.finish();
                        let failure = worker_status.lock().failure;
                        let result = match (output, failure) {
                            (LocalTerminalOutput::TerminalClipboard(text), _) => {
                                Ok(LocalTerminalOutput::TerminalClipboard(text))
                            }
                            (LocalTerminalOutput::LiveCommitted, _) => {
                                Ok(LocalTerminalOutput::LiveCommitted)
                            }
                            (LocalTerminalOutput::NoSpeech, Some(error)) => Err(error),
                            (LocalTerminalOutput::NoSpeech, None) => Ok(LocalTerminalOutput::NoSpeech),
                        };
                        let _ = ack.send(result);
                        return;
                    }
                    SegmentInferenceCommand::Abort => return,
                }
            }
        });

        Self {
            tx: Some(tx),
            status,
            budget,
            abort_requested,
            join: Some(join),
        }
    }

    pub(super) fn queue_segment(
        &self,
        sequence: u64,
        samples: Vec<f32>,
    ) -> Result<(), LocalTranscriptionError> {
        self.check_failure()?;
        if self.abort_requested.load(Ordering::SeqCst) {
            return Err(LocalTranscriptionError::FeedClosed);
        }

        let tx = self
            .tx
            .as_ref()
            .ok_or(LocalTranscriptionError::FeedClosed)?;
        let mut status = self.status.lock();
        if sequence != status.queued_segments {
            return Err(LocalTranscriptionError::InferenceProtocol);
        }
        let reservation = self
            .budget
            .reserve(samples.len())
            .ok_or(LocalTranscriptionError::Backpressure)?;
        tx.send(SegmentInferenceCommand::Segment {
            sequence,
            samples,
            _reservation: reservation,
        })
        .map_err(|_| LocalTranscriptionError::FeedClosed)?;
        status.queued_segments = status.queued_segments.saturating_add(1);
        Ok(())
    }

    pub(super) fn check_failure(&self) -> Result<(), LocalTranscriptionError> {
        match self.status.lock().failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    #[allow(dead_code)] // current finalizer; also exercised through test seams
    pub(super) fn finish(&mut self) -> Result<LocalTerminalOutput, LocalTranscriptionError> {
        let (ack_tx, ack_rx) = mpsc::channel();
        self.tx
            .take()
            .ok_or(LocalTranscriptionError::FeedClosed)?
            .send(SegmentInferenceCommand::Finish(ack_tx))
            .map_err(|_| LocalTranscriptionError::FeedClosed)?;
        let result = ack_rx
            .recv()
            .map_err(|_| LocalTranscriptionError::FeedClosed)?;
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        result
    }

    pub(super) fn request_abort(&self) {
        self.abort_requested.store(true, Ordering::SeqCst);
    }

    pub(super) fn abort(&mut self) {
        self.request_abort();
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(SegmentInferenceCommand::Abort);
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }

    #[cfg(test)]
    pub(super) fn counts(&self) -> (u64, u64) {
        let status = self.status.lock();
        (status.queued_segments, status.completed_segments)
    }

    #[cfg(test)]
    pub(super) fn retained_samples(&self) -> usize {
        self.budget.used_samples()
    }

    #[cfg(test)]
    pub(super) fn start_with_budget(transcribe: SegmentTranscriber, max_samples: usize) -> Self {
        Self::start_with_committer_budget(
            transcribe,
            Arc::new(AtomicBool::new(false)),
            LocalOutputCommitter::terminal_clipboard(),
            SampleBudget::new(max_samples),
        )
    }
}

impl Drop for SegmentInferenceWorker {
    fn drop(&mut self) {
        self.abort();
    }
}

#[cfg(test)]
pub(super) fn transcribe_with_context(
    ctx: &WhisperContext,
    audio: &[f32],
    language: Option<&str>,
) -> Result<Option<String>, LocalTranscriptionError> {
    transcribe_with_context_inner(ctx, audio, language, None, LOCAL_INFERENCE_TIMEOUT)
}

#[allow(dead_code)]
pub(super) fn transcribe_with_context_abortable(
    ctx: &WhisperContext,
    audio: &[f32],
    language: Option<&str>,
    abort_requested: Arc<AtomicBool>,
) -> Result<Option<String>, LocalTranscriptionError> {
    transcribe_with_context_inner(
        ctx,
        audio,
        language,
        Some(abort_requested),
        LOCAL_INFERENCE_TIMEOUT,
    )
}

pub(super) fn transcribe_with_state_stream_abortable_timeout(
    state: &mut WhisperState,
    audio: &[f32],
    language: Option<&str>,
    abort_requested: Arc<AtomicBool>,
    timeout: Duration,
    on_segment: &mut dyn FnMut(String) -> Result<(), LocalTranscriptionError>,
) -> Result<Option<String>, LocalTranscriptionError> {
    transcribe_with_state_stream_inner(
        state,
        audio,
        language,
        Some(abort_requested),
        timeout,
        on_segment,
    )
}
#[cfg(test)]
pub(super) fn transcribe_with_context_stream_abortable_timeout(
    ctx: &WhisperContext,
    audio: &[f32],
    language: Option<&str>,
    abort_requested: Arc<AtomicBool>,
    timeout: Duration,
    on_segment: &mut dyn FnMut(String) -> Result<(), LocalTranscriptionError>,
) -> Result<Option<String>, LocalTranscriptionError> {
    let mut state = ctx
        .create_state()
        .map_err(|_| LocalTranscriptionError::StateCreation)?;
    transcribe_with_state_stream_abortable_timeout(
        &mut state,
        audio,
        language,
        abort_requested,
        timeout,
        on_segment,
    )
}

#[allow(dead_code)]
pub(super) fn transcribe_with_context_abortable_timeout(
    ctx: &WhisperContext,
    audio: &[f32],
    language: Option<&str>,
    abort_requested: Arc<AtomicBool>,
    timeout: Duration,
) -> Result<Option<String>, LocalTranscriptionError> {
    transcribe_with_context_inner(ctx, audio, language, Some(abort_requested), timeout)
}
#[cfg(test)]
mod budget_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn local_inference_timeout_policy_is_thirty_seconds() {
        assert_eq!(LOCAL_INFERENCE_TIMEOUT, Duration::from_secs(30));
    }

    #[test]
    fn inference_abort_decision_combines_explicit_abort_and_deadline() {
        let started = Instant::now();
        let deadline = started + Duration::from_secs(1);
        assert!(!inference_abort_requested(false, started, deadline));
        assert!(inference_abort_requested(true, started, deadline));
        assert!(inference_abort_requested(false, deadline, deadline));
        assert!(inference_abort_requested(
            false,
            deadline + Duration::from_millis(1),
            deadline
        ));
    }

    #[test]
    fn inference_budget_is_fixed_at_thirty_minutes_of_16khz_audio() {
        assert_eq!(INFERENCE_SAMPLE_BUDGET, 28_800_000);
    }

    #[test]
    fn terminal_inference_failure_releases_inflight_budget_when_worker_joins() {
        let mut worker = SegmentInferenceWorker::start_with_budget(
            Box::new(|_, _| Err(LocalTranscriptionError::Inference)),
            8,
        );
        worker.queue_segment(0, vec![0.0; 5]).unwrap();
        assert_eq!(worker.finish(), Err(LocalTranscriptionError::Inference));
        assert_eq!(worker.retained_samples(), 0);
    }

    #[test]
    fn inference_budget_counts_inflight_and_queued_audio_and_releases_on_abort() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let mut worker = SegmentInferenceWorker::start_with_budget(
            Box::new(move |_, _| {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(None)
            }),
            8,
        );

        worker.queue_segment(0, vec![0.0; 5]).unwrap();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(worker.retained_samples(), 5);

        worker.queue_segment(1, vec![0.0; 3]).unwrap();
        assert_eq!(worker.retained_samples(), 8);
        assert_eq!(
            worker.queue_segment(2, vec![0.0; 1]),
            Err(LocalTranscriptionError::Backpressure)
        );
        assert_eq!(worker.retained_samples(), 8);

        worker.request_abort();
        release_tx.send(()).unwrap();
        worker.abort();
        assert_eq!(worker.retained_samples(), 0);
    }

    #[test]
    fn optimal_cpu_thread_count_is_bounded_between_one_and_four() {
        let threads = optimal_cpu_thread_count();
        assert!(threads >= 1 && threads <= 4);
    }

    #[test]
    fn sanitize_segment_text_filters_blank_audio_tokens() {
        assert_eq!(sanitize_segment_text(""), None);
        assert_eq!(sanitize_segment_text("   "), None);
        assert_eq!(sanitize_segment_text("[BLANK_AUDIO]"), None);
        assert_eq!(sanitize_segment_text("  [BLANK_AUDIO]  "), None);
        assert_eq!(
            sanitize_segment_text(" hello [BLANK_AUDIO] world "),
            Some(" hello  world ".to_string())
        );
    }

    #[test]
    fn constrained_auto_languages_all_map_to_valid_whisper_lang_ids() {
        for &lang in CONSTRAINED_AUTO_LANGUAGES {
            let id = whisper_rs::get_lang_id(lang);
            assert!(id.is_some(), "language code {lang} must be valid in whisper.cpp");
            assert!(id.unwrap() >= 0);
        }
    }
}

fn transcribe_with_context_inner(
    ctx: &WhisperContext,
    audio: &[f32],
    language: Option<&str>,
    abort_requested: Option<Arc<AtomicBool>>,
    timeout: Duration,
) -> Result<Option<String>, LocalTranscriptionError> {
    let mut state = ctx
        .create_state()
        .map_err(|_| LocalTranscriptionError::StateCreation)?;
    let mut collected = String::new();
    let res = transcribe_with_state_stream_inner(
        &mut state,
        audio,
        language,
        abort_requested,
        timeout,
        &mut |chunk: String| {
            collected.push_str(&chunk);
            Ok(())
        },
    )?;
    if res.is_none() || collected.is_empty() {
        Ok(None)
    } else {
        Ok(Some(collected))
    }
}

fn transcribe_with_state_stream_inner(
    state: &mut WhisperState,
    audio: &[f32],
    language: Option<&str>,
    abort_requested: Option<Arc<AtomicBool>>,
    timeout: Duration,
    on_segment: &mut dyn FnMut(String) -> Result<(), LocalTranscriptionError>,
) -> Result<Option<String>, LocalTranscriptionError> {
    if audio.is_empty() {
        return Ok(None);
    }

    let threads = optimal_cpu_thread_count() as usize;
    let effective_language = match language {
        None | Some("auto") => detect_constrained_auto_language(state, audio, threads),
        Some(specific) => Some(specific),
    };

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_n_threads(threads as i32);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_suppress_blank(true);
    params.set_suppress_nst(true);
    params.set_no_context(true);
    params.set_single_segment(true);
    params.set_language(effective_language);
    if super::COMPILED_MAX_LENGTH > 0 {
        params.set_token_timestamps(true);
        params.set_max_len(super::COMPILED_MAX_LENGTH);
        params.set_split_on_word(true);
    }

    let deadline = Instant::now() + timeout;
    let abort_flag = abort_requested.clone();
    params.set_abort_callback_safe(Some(move || {
        let explicit_abort = abort_flag
            .as_ref()
            .is_some_and(|requested| requested.load(Ordering::SeqCst));
        inference_abort_requested(explicit_abort, Instant::now(), deadline)
    }));

    if super::COMPILED_SEGMENT_DELIVERY {
        use whisper_rs::SegmentCallbackData;
        struct CallbackPayload<'a> {
            on_segment: &'a mut dyn FnMut(String) -> Result<(), LocalTranscriptionError>,
            err: Option<LocalTranscriptionError>,
            emitted: bool,
        }
        let mut payload = CallbackPayload {
            on_segment,
            err: None,
            emitted: false,
        };
        let payload_ptr = &mut payload as *mut CallbackPayload as usize;

        params.set_segment_callback_safe(move |data: SegmentCallbackData| {
            if let Some(cleaned) = sanitize_segment_text(&data.text) {
                let payload = unsafe { &mut *(payload_ptr as *mut CallbackPayload) };
                if payload.err.is_none() {
                    payload.emitted = true;
                    if let Err(err) = (payload.on_segment)(cleaned) {
                        payload.err = Some(err);
                    }
                }
            }
        });

        let result = state.full(params, audio);

        if let Some(err) = payload.err {
            return Err(err);
        }
        result.map_err(|_| LocalTranscriptionError::Inference)?;
        if payload.emitted {
            Ok(Some(String::new()))
        } else {
            Ok(None)
        }
    } else {
        let result = state.full(params, audio);
        result.map_err(|_| LocalTranscriptionError::Inference)?;

        let n_segments = state.full_n_segments();
        if n_segments <= 0 {
            return Ok(None);
        }
        let mut text = String::new();
        for i in 0..n_segments {
            let Some(segment) = state.get_segment(i) else {
                continue;
            };
            let piece = segment
                .to_str()
                .map_err(|_| LocalTranscriptionError::SegmentRead)?;
            text.push_str(piece);
        }
        if let Some(cleaned) = sanitize_segment_text(&text) {
            on_segment(cleaned.clone())?;
            Ok(Some(cleaned))
        } else {
            Ok(None)
        }
    }
}

pub(super) fn optimal_cpu_thread_count() -> i32 {
    let available = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    (available.min(4) as i32).max(1)
}


pub const CONSTRAINED_AUTO_LANGUAGES: &[&str] = &[
    "en", "es", "fr", "de", "pt", "it", "nl", "ja", "zh", "ru",
];

pub(super) fn detect_constrained_auto_language(
    state: &mut WhisperState,
    audio: &[f32],
    threads: usize,
) -> Option<&'static str> {
    if audio.is_empty() {
        return None;
    }
    if state.pcm_to_mel(audio, threads).is_err() {
        return None;
    }
    let (_, probs) = state.lang_detect(0, threads).ok()?;

    let mut best_lang = "en";
    let mut max_prob = -1.0f32;
    for &lang in CONSTRAINED_AUTO_LANGUAGES {
        if let Some(id) = whisper_rs::get_lang_id(lang) {
            if let Some(&prob) = probs.get(id as usize) {
                if prob > max_prob {
                    max_prob = prob;
                    best_lang = lang;
                }
            }
        }
    }
    Some(best_lang)
}
fn sanitize_segment_text(raw: &str) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    let cleaned = raw.replace("[BLANK_AUDIO]", "");
    if cleaned.trim().is_empty() {
        None
    } else {
        Some(cleaned)
    }
}
