#[cfg(test)]
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::sync::{Condvar, Mutex as StdMutex};

use parking_lot::Mutex;
use tauri::{AppHandle, Manager};

use super::budget::{SampleBudget, SampleReservation};
#[cfg(test)]
use super::output::LocalSegmentOutput;
use super::output::{
    complete_once, deliver_text, LocalCompletion, LocalOutputCommitter, LocalTerminalOutput,
};
use super::recording::LocalSession;
use super::{LocalOutputPolicy, LocalTranscriptionError, PreparedLocalRecording, ReadyLocalModel};
use crate::audio::convert::StreamingResampler;
use crate::models::RecordingIdentity;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalAudioFeedError {
    FeedClosed,
    Backpressure,
}

#[derive(Debug, Default)]
struct LocalCleanupState {
    done: bool,
}

type LocalCleanupSignal = Arc<(StdMutex<LocalCleanupState>, Condvar)>;

#[derive(Clone)]
pub(crate) struct LocalProcessingControl {
    abort_requested: Arc<AtomicBool>,
    command_tx: Arc<Mutex<Option<Sender<LocalFeedCommand>>>>,
    cleanup: LocalCleanupSignal,
}

pub(crate) struct LocalProcessingLease {
    abort_requested: Arc<AtomicBool>,
    command_tx: Arc<Mutex<Option<Sender<LocalFeedCommand>>>>,
    cleanup: LocalCleanupSignal,
    on_finished: Option<Box<dyn FnOnce() + Send + 'static>>,
}

impl LocalProcessingControl {
    pub(crate) fn new(on_finished: impl FnOnce() + Send + 'static) -> (Self, LocalProcessingLease) {
        let abort_requested = Arc::new(AtomicBool::new(false));
        let command_tx = Arc::new(Mutex::new(None));
        let cleanup = Arc::new((StdMutex::new(LocalCleanupState::default()), Condvar::new()));
        (
            Self {
                abort_requested: Arc::clone(&abort_requested),
                command_tx: Arc::clone(&command_tx),
                cleanup: Arc::clone(&cleanup),
            },
            LocalProcessingLease {
                abort_requested,
                command_tx,
                cleanup,
                on_finished: Some(Box::new(on_finished)),
            },
        )
    }

    pub(crate) fn request_abort(&self) {
        self.abort_requested.store(true, Ordering::SeqCst);
        if let Some(tx) = self.command_tx.lock().take() {
            let _ = tx.send(LocalFeedCommand::Abort);
        }
    }

    pub(crate) fn wait_for_cleanup(&self) {
        let (lock, condvar) = &*self.cleanup;
        let mut state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        while !state.done {
            state = condvar
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    #[cfg(test)]
    pub(crate) fn is_abort_requested(&self) -> bool {
        self.abort_requested.load(Ordering::SeqCst)
    }
}

impl LocalProcessingLease {
    fn control(&self) -> LocalProcessingControl {
        LocalProcessingControl {
            abort_requested: Arc::clone(&self.abort_requested),
            command_tx: Arc::clone(&self.command_tx),
            cleanup: Arc::clone(&self.cleanup),
        }
    }
}

impl Drop for LocalProcessingLease {
    fn drop(&mut self) {
        if let Some(on_finished) = self.on_finished.take() {
            on_finished();
        }

        let (lock, condvar) = &*self.cleanup;
        let mut state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        state.done = true;
        condvar.notify_all();
    }
}

struct PendingLocalStart {
    model: Option<ReadyLocalModel>,
    lifecycle: Option<LocalProcessingLease>,
}

impl PendingLocalStart {
    fn new(model: ReadyLocalModel, lifecycle: LocalProcessingLease) -> Self {
        Self {
            model: Some(model),
            lifecycle: Some(lifecycle),
        }
    }

    fn into_parts(mut self) -> (ReadyLocalModel, LocalProcessingLease) {
        (
            self.model.take().expect("pending Local model must exist"),
            self.lifecycle
                .take()
                .expect("pending Local lifecycle must exist"),
        )
    }
}

impl Drop for PendingLocalStart {
    fn drop(&mut self) {
        // Worker ownership must be released/reaped before lifecycle completion
        // becomes visible to explicit shutdown.
        drop(self.model.take());
        drop(self.lifecycle.take());
    }
}

#[derive(Clone)]
pub struct LocalAudioFeed {
    on_start: Arc<dyn Fn(u32) -> bool + Send + Sync>,
    on_samples: Arc<dyn Fn(Vec<f32>) -> Result<(), LocalAudioFeedError> + Send + Sync>,
    on_stop: Arc<dyn Fn() -> bool + Send + Sync>,
    on_abort: Arc<dyn Fn() + Send + Sync>,
}

impl LocalAudioFeed {
    pub fn new(
        on_start: impl Fn(u32) -> bool + Send + Sync + 'static,
        on_samples: impl Fn(Vec<f32>) -> Result<(), LocalAudioFeedError> + Send + Sync + 'static,
        on_stop: impl Fn() -> bool + Send + Sync + 'static,
        on_abort: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            on_start: Arc::new(on_start),
            on_samples: Arc::new(on_samples),
            on_stop: Arc::new(on_stop),
            on_abort: Arc::new(on_abort),
        }
    }

    pub(crate) fn start(&self, sample_rate: u32) -> bool {
        (self.on_start)(sample_rate)
    }

    pub(crate) fn push(&self, samples: Vec<f32>) -> Result<(), LocalAudioFeedError> {
        (self.on_samples)(samples)
    }

    pub(crate) fn stop(&self) -> bool {
        (self.on_stop)()
    }

    pub(crate) fn abort(&self) {
        (self.on_abort)();
    }
}

const LOCAL_PCM_BUDGET_SECONDS: usize = 1800;

fn pcm_budget_for_sample_rate(sample_rate: u32) -> SampleBudget {
    let max_samples = (sample_rate as usize)
        .checked_mul(LOCAL_PCM_BUDGET_SECONDS)
        .expect("device sample rate budget must fit usize");
    SampleBudget::new(max_samples)
}

enum LocalFeedCommand {
    Samples {
        samples: Vec<f32>,
        _reservation: SampleReservation,
    },
    Stop,
    Abort,
}

fn enqueue_samples(
    tx: &Sender<LocalFeedCommand>,
    budget: &SampleBudget,
    samples: Vec<f32>,
) -> Result<(), LocalAudioFeedError> {
    let reservation = budget
        .reserve(samples.len())
        .ok_or(LocalAudioFeedError::Backpressure)?;
    tx.send(LocalFeedCommand::Samples {
        samples,
        _reservation: reservation,
    })
    .map_err(|_| LocalAudioFeedError::FeedClosed)
}

pub(super) fn build_audio_feed(
    prepared: PreparedLocalRecording,
    lifecycle: LocalProcessingLease,
    app: AppHandle,
    recording_id: RecordingIdentity,
    on_terminal: impl Fn(Result<(), LocalTranscriptionError>) + Send + Sync + 'static,
) -> LocalAudioFeed {
    let output_policy = prepared.output;
    let language = prepared.language.language().map(str::to_owned);
    let model_path = prepared.model_path;
    let vad_model_path = prepared.vad_model_path;

    let committer = match output_policy {
        LocalOutputPolicy::LiveInject => {
            let segment_app = app.clone();
            LocalOutputCommitter::live(Arc::new(move |text| {
                crate::recording::run_if_authoritative(&segment_app, recording_id, || {
                    deliver_text(
                        &segment_app,
                        text,
                        crate::output::DeliveryCapability::Inject,
                    )
                })
                .unwrap_or(Ok(()))
            }))
        }
        LocalOutputPolicy::TerminalClipboard => LocalOutputCommitter::terminal_clipboard(),
    };

    let terminal: Arc<dyn Fn(Result<(), LocalTranscriptionError>) + Send + Sync> =
        Arc::new(on_terminal);
    let completion_app = app.clone();
    let terminal_complete = Arc::clone(&terminal);
    let on_complete: LocalCompletion = Arc::new(move |result| {
        let result = match result {
            Ok(LocalTerminalOutput::TerminalClipboard(text)) => {
                crate::recording::run_if_authoritative(&completion_app, recording_id, || {
                    deliver_text(
                        &completion_app,
                        text,
                        crate::output::DeliveryCapability::Clipboard,
                    )
                })
                .unwrap_or(Ok(()))
            }
            Ok(LocalTerminalOutput::NoSpeech | LocalTerminalOutput::LiveCommitted) => Ok(()),
            Err(error) => Err(error),
        };
        terminal_complete(result);
    });

    let command_tx = Arc::clone(&lifecycle.command_tx);
    let pcm_budget: Arc<Mutex<Option<SampleBudget>>> = Arc::new(Mutex::new(None));
    let abort_requested = Arc::clone(&lifecycle.abort_requested);
    let abort_control = lifecycle.control();
    let start_ownership = Arc::new(Mutex::new(Some(PendingLocalStart::new(
        prepared.model,
        lifecycle,
    ))));
    let completed = Arc::new(AtomicBool::new(false));

    let start_tx = Arc::clone(&command_tx);
    let start_budget = Arc::clone(&pcm_budget);
    let start_abort = Arc::clone(&abort_requested);
    let start_ownership = Arc::clone(&start_ownership);
    let start_completed = Arc::clone(&completed);
    let start_complete = Arc::clone(&on_complete);
    let start_committer = Arc::new(Mutex::new(Some(committer)));
    let samples_tx = Arc::clone(&command_tx);
    let samples_budget = Arc::clone(&pcm_budget);
    let stop_tx = Arc::clone(&command_tx);
    let stop_budget = Arc::clone(&pcm_budget);
    let abort_budget = Arc::clone(&pcm_budget);
    let draining = Arc::new(AtomicBool::new(false));
    let draining_start = Arc::clone(&draining);
    let draining_samples = Arc::clone(&draining);
    let draining_stop = Arc::clone(&draining);
    let samples_app = app.clone();
    LocalAudioFeed::new(
        move |sample_rate| {
            if start_abort.load(Ordering::SeqCst) {
                return false;
            }
            let resampler = match StreamingResampler::new(sample_rate) {
                Ok(resampler) => resampler,
                Err(error) => {
                    complete_once(
                        &start_completed,
                        &start_complete,
                        Err(LocalTranscriptionError::AudioConversion(error)),
                    );
                    return false;
                }
            };
            let Some(committer) = start_committer.lock().take() else {
                complete_once(
                    &start_completed,
                    &start_complete,
                    Err(LocalTranscriptionError::FeedClosed),
                );
                return false;
            };
            let Some(ownership) = start_ownership.lock().take() else {
                complete_once(
                    &start_completed,
                    &start_complete,
                    Err(LocalTranscriptionError::FeedClosed),
                );
                return false;
            };
            let (model, lifecycle) = ownership.into_parts();
            let session = match LocalSession::from_ready_model_with_committer(
                &model_path,
                language.as_deref(),
                &vad_model_path,
                model,
                resampler,
                committer,
                Arc::clone(&start_abort),
            ) {
                Ok(session) => session,
                Err(error) => {
                    complete_once(&start_completed, &start_complete, Err(error));
                    return false;
                }
            };

            let budget = pcm_budget_for_sample_rate(sample_rate);
            let (tx, rx) = mpsc::channel::<LocalFeedCommand>();
            *start_budget.lock() = Some(budget);
            *start_tx.lock() = Some(tx);

            let worker_abort = Arc::clone(&start_abort);
            let worker_draining = Arc::clone(&draining_start);
            let worker_budget = Arc::clone(&start_budget);
            let finalize_app = app.clone();
            let worker = std::thread::spawn(move || {
                let mut session = session;
                while let Ok(command) = rx.recv() {
                    if worker_abort.load(Ordering::SeqCst) {
                        session.abort();
                        return None;
                    }
                    match command {
                        LocalFeedCommand::Samples {
                            samples,
                            _reservation,
                        } => {
                            match session.feed_audio(&samples) {
                                Ok(()) => {}
                                Err(LocalTranscriptionError::Backpressure) => {
                                    worker_draining.store(true, Ordering::SeqCst);
                                    worker_budget.lock().take();
                                    finalize_app
                                        .state::<Arc<crate::audio::AudioEngine>>()
                                        .stop_for(recording_id);
                                    finalize_app
                                        .state::<crate::app_state::AppState>()
                                        .set_recording_phase(
                                            &finalize_app,
                                            crate::models::RecordingPhase::Finalizing,
                                        );
                                    return Some(session.finalize());
                                }
                                Err(error) => {
                                    return Some(Err(error));
                                }
                            }
                        }
                        LocalFeedCommand::Stop => {
                            return Some(session.finalize());
                        }
                        LocalFeedCommand::Abort => {
                            session.abort();
                            return None;
                        }
                    }
                }
                session.abort();
                None
            });

            let reaper_completed = Arc::clone(&start_completed);
            let reaper_complete = Arc::clone(&start_complete);
            std::thread::spawn(move || {
                let outcome = worker
                    .join()
                    .unwrap_or(Some(Err(LocalTranscriptionError::FeedClosed)));
                if let Some(result) = outcome {
                    complete_once(&reaper_completed, &reaper_complete, result);
                }
                drop(lifecycle);
            });
            true
        },
        move |samples| {
            if draining_samples.load(Ordering::SeqCst) {
                return Ok(());
            }
            let budget = match samples_budget.lock().as_ref().cloned() {
                Some(b) => b,
                None => return Ok(()),
            };
            let guard = samples_tx.lock();
            let tx = match guard.as_ref() {
                Some(tx) => tx,
                None => return Ok(()),
            };
            match enqueue_samples(tx, &budget, samples) {
                Ok(()) => Ok(()),
                Err(LocalAudioFeedError::FeedClosed) => Ok(()),
                Err(LocalAudioFeedError::Backpressure) => {
                    draining_samples.store(true, Ordering::SeqCst);
                    samples_budget.lock().take();
                    samples_app
                        .state::<Arc<crate::audio::AudioEngine>>()
                        .stop_for(recording_id);
                    samples_app
                        .state::<crate::app_state::AppState>()
                        .set_recording_phase(
                            &samples_app,
                            crate::models::RecordingPhase::Finalizing,
                        );
                    Ok(())
                }
            }
        },
        move || {
            draining_stop.store(true, Ordering::SeqCst);
            let Some(tx) = stop_tx.lock().take() else {
                return false;
            };
            stop_budget.lock().take();
            tx.send(LocalFeedCommand::Stop).is_ok()
        },
        move || {
            abort_budget.lock().take();
            abort_control.request_abort();
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_budget_is_thirty_minutes_at_active_device_rate() {
        let budget = pcm_budget_for_sample_rate(48_000);
        assert_eq!(budget.max_samples(), 86_400_000);
        assert_eq!(budget.used_samples(), 0);
    }

    #[test]
    fn full_pcm_budget_rejects_audio_without_blocking_control_messages() {
        let budget = SampleBudget::new(4);
        let (tx, rx) = mpsc::channel();

        enqueue_samples(&tx, &budget, vec![0.0; 2]).unwrap();
        enqueue_samples(&tx, &budget, vec![0.0; 2]).unwrap();
        assert_eq!(budget.used_samples(), 4);
        assert_eq!(
            enqueue_samples(&tx, &budget, vec![0.0; 1]),
            Err(LocalAudioFeedError::Backpressure)
        );
        assert_eq!(budget.used_samples(), 4);

        tx.send(LocalFeedCommand::Stop).unwrap();
        drop(tx);

        let first = rx.recv().unwrap();
        let second = rx.recv().unwrap();
        let control = rx.recv().unwrap();
        assert!(matches!(&first, LocalFeedCommand::Samples { .. }));
        assert!(matches!(&second, LocalFeedCommand::Samples { .. }));
        assert!(matches!(&control, LocalFeedCommand::Stop));
        assert_eq!(budget.used_samples(), 4);
        drop(first);
        drop(second);
        assert_eq!(budget.used_samples(), 0);
    }

    #[test]
    fn processing_abort_request_does_not_wait_for_lifecycle_cleanup() {
        let (control, lifecycle) = LocalProcessingControl::new(|| {});
        let (tx, rx) = mpsc::channel();
        *control.command_tx.lock() = Some(tx);

        let started = std::time::Instant::now();
        control.request_abort();
        assert!(started.elapsed() < std::time::Duration::from_millis(100));
        assert!(control.is_abort_requested());
        assert!(matches!(
            rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap(),
            LocalFeedCommand::Abort
        ));

        drop(lifecycle);
    }

    #[test]
    fn pending_processing_waits_until_its_lifecycle_lease_finishes() {
        let (control, lifecycle) = LocalProcessingControl::new(|| {});
        let (returned_tx, returned_rx) = mpsc::channel();

        std::thread::spawn(move || {
            control.wait_for_cleanup();
            let _ = returned_tx.send(());
        });

        assert!(returned_rx
            .recv_timeout(std::time::Duration::from_millis(50))
            .is_err());
        drop(lifecycle);
        returned_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("wait must return only after pending lifecycle cleanup");
    }

    #[test]
    fn cleanup_wait_includes_the_finished_callback() {
        let (callback_entered_tx, callback_entered_rx) = mpsc::channel();
        let (release_callback_tx, release_callback_rx) = mpsc::channel();
        let (control, lifecycle) = LocalProcessingControl::new(move || {
            callback_entered_tx.send(()).unwrap();
            release_callback_rx.recv().unwrap();
        });

        let dropper = std::thread::spawn(move || drop(lifecycle));
        callback_entered_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("lifecycle completion callback must start");

        let (waited_tx, waited_rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            control.wait_for_cleanup();
            waited_tx.send(()).unwrap();
        });

        assert!(
            waited_rx
                .recv_timeout(std::time::Duration::from_millis(50))
                .is_err(),
            "cleanup wait must not finish while the lifecycle completion callback is still running"
        );

        release_callback_tx.send(()).unwrap();
        waited_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("cleanup wait must finish after the lifecycle callback returns");
        dropper.join().unwrap();
        waiter.join().unwrap();
    }

    struct DropInference {
        dropped: Arc<AtomicBool>,
    }

    impl Drop for DropInference {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }

    impl super::super::worker::LocalInferenceClient for DropInference {
        fn transcribe(
            &self,
            _audio: &[f32],
            _language: Option<&str>,
            _abort_requested: Arc<AtomicBool>,
        ) -> Result<Option<String>, LocalTranscriptionError> {
            Ok(None)
        }
    }

    #[test]
    fn lifecycle_completion_is_published_after_ready_handle_drop() {
        let model_dropped = Arc::new(AtomicBool::new(false));
        let callback_dropped = Arc::clone(&model_dropped);
        let (finished_tx, finished_rx) = mpsc::channel();
        let (control, lifecycle) = LocalProcessingControl::new(move || {
            assert!(callback_dropped.load(Ordering::SeqCst));
            finished_tx.send(()).unwrap();
        });
        let model = ReadyLocalModel::test_with_inference(
            PathBuf::from("test-model.bin"),
            Arc::new(DropInference {
                dropped: Arc::clone(&model_dropped),
            }),
        );

        drop(PendingLocalStart::new(model, lifecycle));
        control.wait_for_cleanup();

        assert!(model_dropped.load(Ordering::SeqCst));
        finished_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("registry completion must follow Ready handle release");
    }

    #[test]
    fn retired_ready_handle_is_released_by_tracked_lifecycle() {
        let model_dropped = Arc::new(AtomicBool::new(false));
        let cached = ReadyLocalModel::test_with_inference(
            PathBuf::from("retired-model.bin"),
            Arc::new(DropInference {
                dropped: Arc::clone(&model_dropped),
            }),
        );
        let recording = cached.clone();
        drop(cached);
        assert!(!model_dropped.load(Ordering::SeqCst));

        let (control, lifecycle) = LocalProcessingControl::new(|| {});
        let pending = PendingLocalStart::new(recording, lifecycle);
        control.request_abort();
        drop(pending);
        control.wait_for_cleanup();

        assert!(model_dropped.load(Ordering::SeqCst));
    }

    #[test]
    fn failed_pcm_send_releases_its_reservation() {
        let budget = SampleBudget::new(4);
        let (tx, rx) = mpsc::channel();
        drop(rx);
        assert_eq!(
            enqueue_samples(&tx, &budget, vec![0.0; 4]),
            Err(LocalAudioFeedError::FeedClosed)
        );
        assert_eq!(budget.used_samples(), 0);
    }
}

#[cfg(test)]
pub(super) fn build_test_audio_feed(
    model: ReadyLocalModel,
    language: Option<String>,
    vad_model_path: PathBuf,
    on_segment: impl Fn(String) -> Result<(), LocalTranscriptionError> + Send + Sync + 'static,
    on_complete: impl Fn(Result<LocalTerminalOutput, LocalTranscriptionError>) + Send + Sync + 'static,
) -> LocalAudioFeed {
    let command_tx: Arc<Mutex<Option<Sender<LocalFeedCommand>>>> = Arc::new(Mutex::new(None));
    let pcm_budget: Arc<Mutex<Option<SampleBudget>>> = Arc::new(Mutex::new(None));
    let failed = Arc::new(AtomicBool::new(false));
    let completed = Arc::new(AtomicBool::new(false));
    let on_complete: LocalCompletion = Arc::new(on_complete);
    let on_segment: LocalSegmentOutput = Arc::new(on_segment);

    let start_tx = Arc::clone(&command_tx);
    let start_budget = Arc::clone(&pcm_budget);
    let start_failed = Arc::clone(&failed);
    let start_completed = Arc::clone(&completed);
    let start_complete = Arc::clone(&on_complete);
    let start_segment = Arc::clone(&on_segment);
    let samples_tx = Arc::clone(&command_tx);
    let samples_budget = Arc::clone(&pcm_budget);
    let stop_tx = Arc::clone(&command_tx);
    let stop_budget = Arc::clone(&pcm_budget);
    let stop_failed = Arc::clone(&failed);
    let stop_completed = Arc::clone(&completed);
    let stop_complete = Arc::clone(&on_complete);
    let abort_tx = Arc::clone(&command_tx);
    let abort_budget = Arc::clone(&pcm_budget);
    let abort_failed = Arc::clone(&failed);
    let abort_completed = Arc::clone(&completed);
    let abort_complete = Arc::clone(&on_complete);

    LocalAudioFeed::new(
        move |sample_rate| {
            let resampler = match StreamingResampler::new(sample_rate) {
                Ok(resampler) => resampler,
                Err(error) => {
                    start_failed.store(true, Ordering::SeqCst);
                    complete_once(
                        &start_completed,
                        &start_complete,
                        Err(LocalTranscriptionError::AudioConversion(error)),
                    );
                    return false;
                }
            };
            let model_path = model.path().to_path_buf();
            let session = match LocalSession::from_ready_model_with_committer(
                &model_path,
                language.as_deref(),
                &vad_model_path,
                model.clone(),
                resampler,
                LocalOutputCommitter::live(Arc::clone(&start_segment)),
                Arc::new(AtomicBool::new(false)),
            ) {
                Ok(session) => session,
                Err(error) => {
                    start_failed.store(true, Ordering::SeqCst);
                    complete_once(&start_completed, &start_complete, Err(error));
                    return false;
                }
            };

            let budget = pcm_budget_for_sample_rate(sample_rate);
            let (tx, rx) = mpsc::channel::<LocalFeedCommand>();
            *start_budget.lock() = Some(budget);
            *start_tx.lock() = Some(tx);
            let worker_failed = Arc::clone(&start_failed);
            let worker_completed = Arc::clone(&start_completed);
            let worker_complete = Arc::clone(&start_complete);
            std::thread::spawn(move || {
                let mut session = session;
                while let Ok(command) = rx.recv() {
                    match command {
                        LocalFeedCommand::Samples {
                            samples,
                            _reservation,
                        } => {
                            if let Err(error) = session.feed_audio(&samples) {
                                worker_failed.store(true, Ordering::SeqCst);
                                complete_once(&worker_completed, &worker_complete, Err(error));
                                return;
                            }
                        }
                        LocalFeedCommand::Stop => {
                            let result = session.finalize();
                            if result.is_err() {
                                worker_failed.store(true, Ordering::SeqCst);
                            }
                            complete_once(&worker_completed, &worker_complete, result);
                            return;
                        }
                        LocalFeedCommand::Abort => {
                            session.abort();
                            return;
                        }
                    }
                }
            });
            true
        },
        move |samples| {
            let budget = samples_budget
                .lock()
                .as_ref()
                .cloned()
                .ok_or(LocalAudioFeedError::FeedClosed)?;
            let guard = samples_tx.lock();
            let tx = guard.as_ref().ok_or(LocalAudioFeedError::FeedClosed)?;
            enqueue_samples(tx, &budget, samples)
        },
        move || {
            let Some(tx) = stop_tx.lock().take() else {
                return false;
            };
            stop_budget.lock().take();
            if tx.send(LocalFeedCommand::Stop).is_err() {
                if !stop_failed.swap(true, Ordering::SeqCst) {
                    complete_once(
                        &stop_completed,
                        &stop_complete,
                        Err(LocalTranscriptionError::FeedClosed),
                    );
                }
                return false;
            }
            true
        },
        move || {
            let Some(tx) = abort_tx.lock().take() else {
                return;
            };
            abort_budget.lock().take();
            if tx.send(LocalFeedCommand::Abort).is_err()
                && !abort_failed.swap(true, Ordering::SeqCst)
            {
                complete_once(
                    &abort_completed,
                    &abort_complete,
                    Err(LocalTranscriptionError::FeedClosed),
                );
            }
        },
    )
}
