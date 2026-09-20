//! Exact-identity Local and Cloud processing ownership.
//!
//! Cloud admission always holds `RecordingCoordinator` first and acquires the
//! Cloud registry second. Registry-only cancel/clear paths never acquire the
//! coordinator, preserving the lock order documented by `docs/CLOUD.md`.

use std::collections::HashMap;

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use crate::models::{CloudRoute, ProviderId, RecordingIdentity};
use crate::transcription::local::LocalProcessingControl;

use super::coordinator::RecordingCoordinator;

/// Local lifecycle ownership remains registered from admission through complete
/// cleanup, independently from provider-neutral recording authority.
struct LocalProcessingRegistryState {
    accepting: bool,
    controls: HashMap<RecordingIdentity, LocalProcessingControl>,
}

pub struct LocalProcessingRegistry {
    state: Mutex<LocalProcessingRegistryState>,
}

impl LocalProcessingRegistry {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(LocalProcessingRegistryState {
                accepting: true,
                controls: HashMap::new(),
            }),
        }
    }

    pub(super) fn register(&self, id: RecordingIdentity, control: LocalProcessingControl) -> bool {
        let mut state = self.state.lock();
        if !state.accepting || state.controls.contains_key(&id) {
            return false;
        }
        state.controls.insert(id, control);
        true
    }

    pub(super) fn request_abort(&self, id: RecordingIdentity) -> bool {
        let control = self.state.lock().controls.get(&id).cloned();
        let Some(control) = control else {
            return false;
        };
        control.request_abort();
        true
    }

    pub(super) fn complete(&self, id: RecordingIdentity) -> bool {
        self.state.lock().controls.remove(&id).is_some()
    }

    pub(super) fn begin_shutdown(&self) -> Vec<(RecordingIdentity, LocalProcessingControl)> {
        let mut state = self.state.lock();
        state.accepting = false;
        state.controls.drain().collect()
    }

    #[cfg(test)]
    pub(super) fn contains(&self, id: RecordingIdentity) -> bool {
        self.state.lock().controls.contains_key(&id)
    }
}

pub(super) fn request_abort_all(processing: &[(RecordingIdentity, LocalProcessingControl)]) {
    for (_, control) in processing {
        control.request_abort();
    }
}

pub(super) fn wait_for_cleanup_all(processing: Vec<(RecordingIdentity, LocalProcessingControl)>) {
    for (_, control) in processing {
        control.wait_for_cleanup();
    }
}

/// Cancellation ownership for one exact-ID Cloud processing lifecycle.
/// Completed jobs acquire it after capture; live jobs acquire it before any
/// remote live session may begin.
#[derive(Clone)]
pub(crate) struct CloudProcessingControl {
    cancellation: CancellationToken,
}

impl CloudProcessingControl {
    pub(crate) fn new() -> Self {
        Self {
            cancellation: CancellationToken::new(),
        }
    }

    fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub(crate) fn cancellation_token(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    #[cfg(test)]
    pub(super) fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }
}

/// Cloud-only processing controls keyed by immutable recording identity.
pub struct CloudProcessingRegistry {
    controls: Mutex<HashMap<RecordingIdentity, CloudProcessingControl>>,
}

impl CloudProcessingRegistry {
    pub fn new() -> Self {
        Self {
            controls: Mutex::new(HashMap::new()),
        }
    }

    pub(super) fn register(&self, id: RecordingIdentity, control: CloudProcessingControl) -> bool {
        let mut controls = self.controls.lock();
        if controls.contains_key(&id) {
            return false;
        }
        controls.insert(id, control);
        true
    }

    pub(super) fn cancel(&self, id: RecordingIdentity) -> bool {
        let control = self.controls.lock().remove(&id);
        let Some(control) = control else {
            return false;
        };
        control.cancel();
        true
    }

    pub(super) fn clear(&self, id: RecordingIdentity) -> bool {
        self.controls.lock().remove(&id).is_some()
    }

    #[cfg(test)]
    pub(super) fn contains(&self, id: RecordingIdentity) -> bool {
        self.controls.lock().contains_key(&id)
    }
}

/// Atomically admit one exact-origin completed Cloud buffer and register its
/// cancellation control. The coordinator lock is held while the Cloud registry
/// is acquired, so supersession cannot interleave between the authority/route
/// check and registration.
pub(super) fn claim_cloud_completed_job_exact(
    coordinator: &RecordingCoordinator,
    registry: &CloudProcessingRegistry,
    id: RecordingIdentity,
    control: CloudProcessingControl,
) -> Option<ProviderId> {
    coordinator
        .with_matching_cloud_route(id, |provider, route| {
            (route == CloudRoute::CompletedAudio)
                .then(|| registry.register(id, control).then_some(provider))
                .flatten()
        })
        .flatten()
}

/// Atomically establish exact-ID cancellation ownership for a startable live
/// Cloud recording. Registration occurs under the same coordinator -> registry
/// lock order as completed admission and must succeed before live transport
/// preparation or remote-session start is allowed to run.
pub(super) fn register_live_cloud_processing_exact(
    coordinator: &RecordingCoordinator,
    registry: &CloudProcessingRegistry,
    id: RecordingIdentity,
    control: CloudProcessingControl,
) -> Option<ProviderId> {
    coordinator
        .with_matching_startable_cloud_route(id, |provider, route| {
            (route == CloudRoute::LiveAudio)
                .then(|| registry.register(id, control).then_some(provider))
                .flatten()
        })
        .flatten()
}

#[cfg(test)]
mod local_registry_tests {
    use std::sync::{mpsc, Arc, Barrier};
    use std::time::Duration;

    use super::*;

    fn tracked_lifecycle(
        registry: &Arc<LocalProcessingRegistry>,
        id: RecordingIdentity,
    ) -> (
        LocalProcessingControl,
        crate::transcription::local::LocalProcessingLease,
    ) {
        let cleanup_registry = Arc::clone(registry);
        LocalProcessingControl::new(move || {
            cleanup_registry.complete(id);
        })
    }

    #[test]
    fn superseded_processing_stays_registered_until_lifecycle_cleanup() {
        let registry = Arc::new(LocalProcessingRegistry::new());
        let id = RecordingIdentity(1);
        let (control, lifecycle) = tracked_lifecycle(&registry, id);
        assert!(registry.register(id, control.clone()));

        assert!(registry.request_abort(id));
        assert!(control.is_abort_requested());
        assert!(registry.contains(id));

        drop(lifecycle);
        assert!(!registry.contains(id));
    }

    #[test]
    fn shutdown_signals_every_generation_before_waiting_for_cleanup() {
        let registry = Arc::new(LocalProcessingRegistry::new());
        let first = RecordingIdentity(1);
        let second = RecordingIdentity(2);
        let (first_control, first_lifecycle) = tracked_lifecycle(&registry, first);
        let (second_control, second_lifecycle) = tracked_lifecycle(&registry, second);
        assert!(registry.register(first, first_control.clone()));
        assert!(registry.register(second, second_control.clone()));
        let processing = registry.begin_shutdown();
        assert_eq!(processing.len(), 2);

        let (signaled_tx, signaled_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        std::thread::spawn(move || {
            request_abort_all(&processing);
            signaled_tx.send(()).unwrap();
            wait_for_cleanup_all(processing);
            finished_tx.send(()).unwrap();
        });

        signaled_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("all aborts must be requested without waiting");
        assert!(first_control.is_abort_requested());
        assert!(second_control.is_abort_requested());
        assert!(finished_rx.recv_timeout(Duration::from_millis(50)).is_err());

        drop(first_lifecycle);
        assert!(finished_rx.recv_timeout(Duration::from_millis(50)).is_err());
        drop(second_lifecycle);
        finished_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("shutdown wait must finish after every lifecycle");
    }

    #[test]
    fn registration_racing_shutdown_is_either_tracked_or_rejected() {
        for generation in 1..=64 {
            let registry = Arc::new(LocalProcessingRegistry::new());
            let id = RecordingIdentity(generation);
            let (control, lifecycle) = tracked_lifecycle(&registry, id);
            let barrier = Arc::new(Barrier::new(2));
            let register_registry = Arc::clone(&registry);
            let register_barrier = Arc::clone(&barrier);
            let registration = std::thread::spawn(move || {
                register_barrier.wait();
                register_registry.register(id, control)
            });

            barrier.wait();
            let processing = registry.begin_shutdown();
            let admitted = registration.join().unwrap();
            assert_eq!(
                admitted,
                processing
                    .iter()
                    .any(|(processing_id, _)| *processing_id == id)
            );

            drop(lifecycle);
            wait_for_cleanup_all(processing);
        }
    }

    #[test]
    fn beginning_shutdown_is_idempotent_and_permanently_closes_admission() {
        let registry = Arc::new(LocalProcessingRegistry::new());
        let id = RecordingIdentity(1);
        let (control, lifecycle) = tracked_lifecycle(&registry, id);
        assert!(registry.register(id, control));

        let first = registry.begin_shutdown();
        assert_eq!(first.len(), 1);
        assert!(registry.begin_shutdown().is_empty());

        let (late_control, late_lifecycle) = tracked_lifecycle(&registry, RecordingIdentity(2));
        assert!(!registry.register(RecordingIdentity(2), late_control));

        drop(lifecycle);
        drop(late_lifecycle);
        wait_for_cleanup_all(first);
    }
}
