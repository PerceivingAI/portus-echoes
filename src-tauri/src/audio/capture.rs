//! Recording-bound audio capture lifecycle and Local/Cloud destination ownership.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar as StdCondvar, Mutex as StdMutex, Weak};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::cloud_live::{CloudLiveFeed, CloudLiveFeedError};
use crate::models::RecordingIdentity;
use crate::transcription::local::{LocalAudioFeed, LocalAudioFeedError};

#[cfg(test)]
use super::convert::TARGET_SAMPLE_RATE;

/// A finished Cloud capture permanently bound to its originating recording.
/// Cloud is the only producer/consumer path; Local owns a `LocalAudioFeed` and
/// never produces `CloudCapturedAudio`.
#[derive(Debug)]
pub struct CloudCapturedAudio {
    pub(crate) recording_id: RecordingIdentity,
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CloudCaptureBuffer {
    samples: Arc<Mutex<Vec<f32>>>,
}

impl CloudCaptureBuffer {
    pub(super) fn push<T: Copy>(&self, data: &[T], channels: usize, convert: fn(T) -> f32) {
        let mut samples = self.samples.lock();
        if channels == 1 {
            samples.extend(data.iter().map(|&sample| convert(sample)));
        } else {
            samples.extend(data.chunks(channels).map(|frame| {
                frame.iter().map(|&sample| convert(sample)).sum::<f32>() / frame.len() as f32
            }));
        }
    }

    fn take(&self) -> Vec<f32> {
        std::mem::take(&mut *self.samples.lock())
    }
}

#[derive(Clone)]
pub(crate) enum CaptureDestination {
    LocalLive(LocalAudioFeed),
    CloudCompleted(CloudCaptureBuffer),
    CloudLive(CloudLiveFeed),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaptureFailureKind {
    Stream,
    FeedClosed,
    LocalBackpressure,
    CloudBackpressure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaptureEventKind {
    Started,
    Stopped,
    Aborted,
    Failed(CaptureFailureKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopCaptureResult {
    Stopped,
    AlreadyStopping,
    NotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CaptureEvent {
    pub(crate) recording_id: RecordingIdentity,
    pub(crate) kind: CaptureEventKind,
}

#[cfg(test)]
#[allow(non_upper_case_globals, non_snake_case)]
impl CaptureEvent {
    const Started: Self = Self {
        recording_id: RecordingIdentity(1),
        kind: CaptureEventKind::Started,
    };
    const Stopped: Self = Self {
        recording_id: RecordingIdentity(1),
        kind: CaptureEventKind::Stopped,
    };
    fn Failed(kind: CaptureFailureKind) -> Self {
        Self {
            recording_id: RecordingIdentity(1),
            kind: CaptureEventKind::Failed(kind),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CaptureFailure {
    pub(super) generation: u64,
    pub(super) kind: CaptureFailureKind,
}

#[derive(Clone, Debug, Default)]
pub(super) struct CaptureFailureState {
    kind: Arc<AtomicU8>,
}

impl CaptureFailureState {
    pub(super) fn record(&self, kind: CaptureFailureKind) {
        let _ = self.kind.compare_exchange(
            0,
            failure_kind_code(kind),
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }

    fn pending(&self) -> Option<CaptureFailureKind> {
        failure_kind_from_code(self.kind.load(Ordering::SeqCst))
    }
}

fn failure_kind_code(kind: CaptureFailureKind) -> u8 {
    match kind {
        CaptureFailureKind::Stream => 1,
        CaptureFailureKind::FeedClosed => 2,
        CaptureFailureKind::LocalBackpressure => 3,
        CaptureFailureKind::CloudBackpressure => 4,
    }
}

fn failure_kind_from_code(code: u8) -> Option<CaptureFailureKind> {
    match code {
        1 => Some(CaptureFailureKind::Stream),
        2 => Some(CaptureFailureKind::FeedClosed),
        3 => Some(CaptureFailureKind::LocalBackpressure),
        4 => Some(CaptureFailureKind::CloudBackpressure),
        _ => None,
    }
}

impl From<LocalAudioFeedError> for CaptureFailureKind {
    fn from(error: LocalAudioFeedError) -> Self {
        match error {
            LocalAudioFeedError::FeedClosed => Self::FeedClosed,
            LocalAudioFeedError::Backpressure => Self::LocalBackpressure,
        }
    }
}
impl From<CloudLiveFeedError> for CaptureFailureKind {
    fn from(error: CloudLiveFeedError) -> Self {
        match error {
            CloudLiveFeedError::Closed => Self::FeedClosed,
            CloudLiveFeedError::Backpressure => Self::CloudBackpressure,
        }
    }
}

pub(super) trait CaptureStream: Send {
    fn play(&self) -> bool;
}

/// One active capture session. Dropping `stream` stops OS-level capture.
struct Session {
    recording_id: RecordingIdentity,
    stream: Box<dyn CaptureStream>,
    destination: CaptureDestination,
    sample_rate: u32,
    generation: u64,
    failure_state: CaptureFailureState,
}
/// Cloud recording-limit coordination: a generation counter plus a condvar so
/// `stop` wakes the sleeping Cloud timer immediately instead of leaving it to
/// expire on its own.
struct CloudLimitTimer {
    generation: StdMutex<u64>,
    condvar: StdCondvar,
}

/// Owns mic capture sessions. Hotkey/tray stop applies to every installed
/// destination; both Cloud capture routes share the same elapsed-time interaction limit.
pub struct AudioEngine {
    session: Mutex<Option<Session>>,
    stopping: Mutex<HashSet<RecordingIdentity>>,
    cloud_limit_timer: Arc<CloudLimitTimer>,
    generation: AtomicU64,
    cloud_tx: Sender<CloudCapturedAudio>,
    capture_failure_tx: Sender<CaptureFailure>,
    on_capture_event: Box<dyn Fn(CaptureEvent) + Send + Sync>,
    self_ref: Mutex<Weak<AudioEngine>>,
}

enum CaptureStart {
    LocalLive(LocalAudioFeed),
    CloudCompleted {
        recording_limit: Option<Duration>,
    },
    CloudLive {
        feed: CloudLiveFeed,
        recording_limit: Option<Duration>,
    },
}

type StartGuard = Arc<dyn Fn() -> bool + Send + Sync>;

impl AudioEngine {
    /// Create the engine and its capture channel. `on_state_change(true)`
    /// fires when a session is actually capturing (stream playing), `false`
    /// when it stops or is silently aborted.
    pub fn new(
        on_capture_event: impl Fn(CaptureEvent) + Send + Sync + 'static,
    ) -> (Arc<Self>, Receiver<CloudCapturedAudio>) {
        let (cloud_tx, rx) = mpsc::channel();
        let (capture_failure_tx, capture_failure_rx) = mpsc::channel();
        let engine = Arc::new_cyclic(|weak| Self {
            session: Mutex::new(None),
            stopping: Mutex::new(HashSet::new()),
            cloud_limit_timer: Arc::new(CloudLimitTimer {
                generation: StdMutex::new(0),
                condvar: StdCondvar::new(),
            }),
            generation: AtomicU64::new(0),
            cloud_tx,
            capture_failure_tx,
            on_capture_event: Box::new(on_capture_event),
            self_ref: Mutex::new(weak.clone()),
        });

        let weak = Arc::downgrade(&engine);
        std::thread::spawn(move || {
            while let Ok(failure) = capture_failure_rx.recv() {
                let Some(engine) = weak.upgrade() else {
                    return;
                };
                engine.abort_failure(failure);
            }
        });

        (engine, rx)
    }

    /// Whether any capture session is active.
    #[cfg(test)]
    pub fn is_active(&self) -> bool {
        self.session.lock().is_some()
    }

    /// Start one completed-audio Cloud capture. This path alone owns the
    /// recording-wide in-memory Cloud buffer and completed payload emission.
    pub fn start_cloud_completed_for(
        &self,
        recording_id: RecordingIdentity,
        recording_limit: Option<Duration>,
        start_allowed: StartGuard,
    ) -> bool {
        self.start_capture(
            recording_id,
            CaptureStart::CloudCompleted { recording_limit },
            start_allowed,
            open_session,
        )
    }

    /// Start one live Cloud capture through the provider-neutral PCM feed.
    /// Provider transport/session ownership is deliberately outside AudioEngine;
    /// the shared Cloud recording interaction limit remains capture-owned.
    #[allow(dead_code)]
    pub(crate) fn start_cloud_live_for(
        &self,
        recording_id: RecordingIdentity,
        feed: CloudLiveFeed,
        recording_limit: Option<Duration>,
        start_allowed: StartGuard,
    ) -> bool {
        self.start_capture(
            recording_id,
            CaptureStart::CloudLive {
                feed,
                recording_limit,
            },
            start_allowed,
            open_session,
        )
    }

    /// Start one Local live capture with the same identity-aware capture contract.
    pub fn start_local_for(
        &self,
        recording_id: RecordingIdentity,
        feed: LocalAudioFeed,
        start_allowed: StartGuard,
    ) -> bool {
        self.start_capture(
            recording_id,
            CaptureStart::LocalLive(feed),
            start_allowed,
            open_session,
        )
    }

    #[cfg(test)]
    fn start_cloud_completed_with_identity<F>(
        &self,
        recording_id: RecordingIdentity,
        recording_limit: impl Into<Option<Duration>>,
        start_allowed: StartGuard,
        open: F,
    ) -> bool
    where
        F: FnOnce(
            Sender<CaptureFailure>,
            u64,
            RecordingIdentity,
            CaptureDestination,
        ) -> Option<Session>,
    {
        self.start_capture(
            recording_id,
            CaptureStart::CloudCompleted {
                recording_limit: recording_limit.into(),
            },
            start_allowed,
            open,
        )
    }

    #[cfg(test)]
    fn start_cloud_live_with_identity<F>(
        &self,
        recording_id: RecordingIdentity,
        feed: CloudLiveFeed,
        recording_limit: Option<Duration>,
        start_allowed: StartGuard,
        open: F,
    ) -> bool
    where
        F: FnOnce(
            Sender<CaptureFailure>,
            u64,
            RecordingIdentity,
            CaptureDestination,
        ) -> Option<Session>,
    {
        self.start_capture(
            recording_id,
            CaptureStart::CloudLive {
                feed,
                recording_limit,
            },
            start_allowed,
            open,
        )
    }

    #[cfg(test)]
    fn start_local_with_identity<F>(
        &self,
        recording_id: RecordingIdentity,
        feed: LocalAudioFeed,
        start_allowed: StartGuard,
        open: F,
    ) -> bool
    where
        F: FnOnce(
            Sender<CaptureFailure>,
            u64,
            RecordingIdentity,
            CaptureDestination,
        ) -> Option<Session>,
    {
        self.start_capture(
            recording_id,
            CaptureStart::LocalLive(feed),
            start_allowed,
            open,
        )
    }

    #[cfg(test)]
    pub fn start_cloud_completed(&self, recording_limit: impl Into<Option<Duration>>) -> bool {
        self.start_cloud_completed_for(
            RecordingIdentity(1),
            recording_limit.into(),
            Arc::new(|| true),
        )
    }

    #[cfg(test)]
    pub fn start_local(&self, feed: LocalAudioFeed) -> bool {
        self.start_local_for(RecordingIdentity(1), feed, Arc::new(|| true))
    }

    #[cfg(test)]
    fn start_cloud_completed_with<F>(&self, recording_limit: impl Into<Option<Duration>>, open: F) -> bool
    where
        F: FnOnce(Sender<CaptureFailure>, u64, CaptureDestination) -> Option<Session>,
    {
        self.start_cloud_completed_with_identity(
            RecordingIdentity(1),
            recording_limit,
            Arc::new(|| true),
            move |tx, generation, recording_id, destination| {
                let mut session = open(tx, generation, destination)?;
                session.recording_id = recording_id;
                Some(session)
            },
        )
    }

    #[cfg(test)]
    fn start_local_with<F>(&self, feed: LocalAudioFeed, open: F) -> bool
    where
        F: FnOnce(Sender<CaptureFailure>, u64, CaptureDestination) -> Option<Session>,
    {
        self.start_local_with_identity(
            RecordingIdentity(1),
            feed,
            Arc::new(|| true),
            move |tx, generation, recording_id, destination| {
                let mut session = open(tx, generation, destination)?;
                session.recording_id = recording_id;
                Some(session)
            },
        )
    }

    fn start_capture<F>(
        &self,
        recording_id: RecordingIdentity,
        request: CaptureStart,
        start_allowed: StartGuard,
        open: F,
    ) -> bool
    where
        F: FnOnce(
            Sender<CaptureFailure>,
            u64,
            RecordingIdentity,
            CaptureDestination,
        ) -> Option<Session>,
    {
        if !start_allowed() {
            return false;
        }
        if self
            .session
            .lock()
            .as_ref()
            .is_some_and(|session| session.recording_id == recording_id)
        {
            return true;
        }

        let (destination, cloud_recording_limit) = match request {
            CaptureStart::LocalLive(feed) => (CaptureDestination::LocalLive(feed), None),
            CaptureStart::CloudCompleted { recording_limit } => (
                CaptureDestination::CloudCompleted(CloudCaptureBuffer::default()),
                recording_limit,
            ),
            CaptureStart::CloudLive {
                feed,
                recording_limit,
            } => (CaptureDestination::CloudLive(feed), recording_limit),
        };
        let generation = self.allocate_generation();
        let Some(session) = open(
            self.capture_failure_tx.clone(),
            generation,
            recording_id,
            destination,
        ) else {
            return false;
        };

        if !start_allowed() {
            abort_detached_session(session);
            return false;
        }

        match &session.destination {
            CaptureDestination::LocalLive(feed) => {
                if !feed.start(session.sample_rate) {
                    feed.abort();
                    return false;
                }
            }
            CaptureDestination::CloudLive(feed) => {
                if let Err(error) = feed.start(session.sample_rate) {
                    let kind = CaptureFailureKind::from(error);
                    abort_detached_session(session);
                    (self.on_capture_event)(CaptureEvent {
                        recording_id,
                        kind: CaptureEventKind::Failed(kind),
                    });
                    return false;
                }
            }
            CaptureDestination::CloudCompleted(_) => {}
        }

        if !start_allowed() {
            abort_detached_session(session);
            return false;
        }

        let replaced = {
            let mut guard = self.session.lock();
            if !start_allowed() {
                drop(guard);
                abort_detached_session(session);
                return false;
            }
            if guard
                .as_ref()
                .is_some_and(|current| current.recording_id == recording_id)
            {
                drop(guard);
                abort_detached_session(session);
                return true;
            }
            let replaced = guard.take();
            *guard = Some(session);
            self.publish_active_generation(generation);
            replaced
        };

        if let Some(old) = replaced {
            let old_id = old.recording_id;
            abort_detached_session(old);
            (self.on_capture_event)(CaptureEvent {
                recording_id: old_id,
                kind: CaptureEventKind::Aborted,
            });
        }

        let started = {
            let guard = self.session.lock();
            guard
                .as_ref()
                .filter(|session| session.recording_id == recording_id)
                .map(|session| session.stream.play())
                .unwrap_or(false)
        };
        if !started || !start_allowed() {
            let failed = self.take_matching_session(recording_id);
            if let Some(session) = failed {
                abort_detached_session(session);
            }
            return false;
        }

        if let Some(recording_limit) = cloud_recording_limit {
            self.spawn_cloud_recording_limit(generation, recording_id, recording_limit);
        }
        (self.on_capture_event)(CaptureEvent {
            recording_id,
            kind: CaptureEventKind::Started,
        });
        true
    }

    fn take_matching_session(&self, recording_id: RecordingIdentity) -> Option<Session> {
        let mut guard = self.session.lock();
        if guard
            .as_ref()
            .is_some_and(|session| session.recording_id == recording_id)
        {
            let session = guard.take();
            self.publish_active_generation(0);
            session
        } else {
            None
        }
    }

    /// Stop only the capture belonging to `recording_id`. A stale release can
    /// never stop a newer session. `AlreadyStopping` distinguishes an exact
    /// session already claimed by another terminal interaction (for example the
    /// Cloud recording-limit timer) from a preparation window where no capture
    /// was ever installed.
    pub(crate) fn stop_result_for(&self, recording_id: RecordingIdentity) -> StopCaptureResult {
        let session = {
            let mut guard = self.session.lock();
            if guard
                .as_ref()
                .is_some_and(|session| session.recording_id == recording_id)
            {
                let session = guard.take().expect("matching session exists");
                self.stopping.lock().insert(recording_id);
                self.publish_active_generation(0);
                Some(session)
            } else {
                None
            }
        };
        let Some(session) = session else {
            return if self.stopping.lock().contains(&recording_id) {
                StopCaptureResult::AlreadyStopping
            } else {
                StopCaptureResult::NotFound
            };
        };
        let Session {
            stream,
            destination,
            sample_rate,
            failure_state,
            ..
        } = session;
        drop(stream);

        if let Some(kind) = failure_state.pending() {
            match destination {
                CaptureDestination::LocalLive(feed) => feed.abort(),
                CaptureDestination::CloudLive(feed) => feed.abort(),
                CaptureDestination::CloudCompleted(_) => {}
            }
            (self.on_capture_event)(CaptureEvent {
                recording_id,
                kind: CaptureEventKind::Failed(kind),
            });
            self.stopping.lock().remove(&recording_id);
            return StopCaptureResult::Stopped;
        }

        match destination {
            CaptureDestination::LocalLive(feed) => {
                (self.on_capture_event)(CaptureEvent {
                    recording_id,
                    kind: CaptureEventKind::Stopped,
                });
                let _ = feed.stop();
            }
            CaptureDestination::CloudCompleted(buffer) => {
                (self.on_capture_event)(CaptureEvent {
                    recording_id,
                    kind: CaptureEventKind::Stopped,
                });
                let _ = self.cloud_tx.send(CloudCapturedAudio {
                    recording_id,
                    samples: buffer.take(),
                    sample_rate,
                });
            }
            CaptureDestination::CloudLive(feed) => match feed.stop() {
                Ok(()) => {
                    (self.on_capture_event)(CaptureEvent {
                        recording_id,
                        kind: CaptureEventKind::Stopped,
                    });
                }
                Err(error) => {
                    feed.abort();
                    (self.on_capture_event)(CaptureEvent {
                        recording_id,
                        kind: CaptureEventKind::Failed(CaptureFailureKind::from(error)),
                    });
                }
            },
        }
        self.stopping.lock().remove(&recording_id);
        StopCaptureResult::Stopped
    }

    pub fn stop_for(&self, recording_id: RecordingIdentity) -> bool {
        !matches!(
            self.stop_result_for(recording_id),
            StopCaptureResult::NotFound
        )
    }

    #[cfg(test)]
    pub fn stop(&self) {
        let _ = self.stop_for(RecordingIdentity(1));
    }

    /// Abort only the matching capture without producing Cloud audio or a Local
    /// terminal result.
    pub fn abort_for(&self, recording_id: RecordingIdentity) -> bool {
        let Some(session) = self.take_matching_session(recording_id) else {
            return false;
        };
        abort_detached_session(session);
        (self.on_capture_event)(CaptureEvent {
            recording_id,
            kind: CaptureEventKind::Aborted,
        });
        true
    }

    /// Request matching capture abort away from the hotkey critical path.
    pub fn abort_async(&self, recording_id: RecordingIdentity) {
        let weak = self.self_ref.lock().clone();
        std::thread::spawn(move || {
            if let Some(engine) = weak.upgrade() {
                let _ = engine.abort_for(recording_id);
            }
        });
    }

    fn allocate_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Publish only the generation that actually owns the installed capture
    /// session. Candidate starts may allocate generations while opening a
    /// stream, but they must not disarm the active Cloud timer unless they win
    /// session installation.
    fn publish_active_generation(&self, generation: u64) {
        *lock_timer(&self.cloud_limit_timer.generation) = generation;
        self.cloud_limit_timer.condvar.notify_all();
    }

    fn spawn_cloud_recording_limit(
        &self,
        generation: u64,
        recording_id: RecordingIdentity,
        recording_limit: Duration,
    ) {
        let timer = Arc::clone(&self.cloud_limit_timer);
        let weak = self.self_ref.lock().clone();
        std::thread::spawn(move || {
            let deadline = Instant::now() + recording_limit;
            let mut guard = lock_timer(&timer.generation);
            loop {
                if *guard != generation {
                    return; // disarmed by stop(), abort, failure, or a newer installed session
                }
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                let (g, _) = timer
                    .condvar
                    .wait_timeout(guard, deadline - now)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                guard = g;
            }
            drop(guard);
            if let Some(engine) = weak.upgrade() {
                let _ = engine.stop_for(recording_id);
            }
        });
    }

    /// Abort only the generation that reported a typed capture failure. The
    /// failure fact distinguishes OS stream failure, Local feed closure, and
    /// Local backpressure even though all three fail the active capture closed.
    fn abort_failure(&self, failure: CaptureFailure) {
        let session = {
            let mut guard = self.session.lock();
            if guard.as_ref().map(|session| session.generation) != Some(failure.generation) {
                return;
            }
            let session = guard.take();
            self.publish_active_generation(0);
            session
        };
        let Some(session) = session else {
            return;
        };

        let recording_id = session.recording_id;
        let kind = session.failure_state.pending().unwrap_or(failure.kind);
        abort_detached_session(session);
        (self.on_capture_event)(CaptureEvent {
            recording_id,
            kind: CaptureEventKind::Failed(kind),
        });
    }
}

fn open_session(
    capture_failure_tx: Sender<CaptureFailure>,
    generation: u64,
    recording_id: RecordingIdentity,
    destination: CaptureDestination,
) -> Option<Session> {
    let failure_state = CaptureFailureState::default();
    let opened = super::device::open_capture_stream(
        capture_failure_tx,
        generation,
        destination.clone(),
        failure_state.clone(),
    )?;
    Some(Session {
        recording_id,
        stream: opened.stream,
        destination,
        sample_rate: opened.sample_rate,
        generation,
        failure_state,
    })
}

fn abort_detached_session(session: Session) {
    let Session {
        stream,
        destination,
        ..
    } = session;
    drop(stream);
    match destination {
        CaptureDestination::LocalLive(feed) => feed.abort(),
        CaptureDestination::CloudLive(feed) => feed.abort(),
        CaptureDestination::CloudCompleted(_) => {}
    }
}

fn lock_timer(mutex: &StdMutex<u64>) -> std::sync::MutexGuard<'_, u64> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests;
