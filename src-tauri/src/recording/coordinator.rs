//! Provider-neutral recording identity and authority state machine.

use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;

use crate::models::{CloudRoute, ProviderId, RecordingIdentity};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActiveRecordingState {
    id: RecordingIdentity,
    provider: ProviderId,
    cloud_route: Option<CloudRoute>,
    released: bool,
    failure_reported: bool,
}

impl ActiveRecordingState {
    fn new(id: RecordingIdentity, provider: ProviderId, cloud_route: Option<CloudRoute>) -> Self {
        assert!(
            matches!(
                (provider, cloud_route),
                (ProviderId::Local, None) | (ProviderId::Openai | ProviderId::Groq, Some(_))
            ),
            "recording admission must carry exactly one Cloud route for Cloud providers"
        );
        Self {
            id,
            provider,
            cloud_route,
            released: false,
            failure_reported: false,
        }
    }
}

/// Shared owner of the newest push-to-talk generation.
pub struct RecordingCoordinator {
    next_identity: AtomicU64,
    active: Mutex<Option<ActiveRecordingState>>,
}

impl RecordingCoordinator {
    pub fn new() -> Self {
        Self {
            next_identity: AtomicU64::new(0),
            active: Mutex::new(None),
        }
    }

    /// Replace the current owner unconditionally. The returned identity lost
    /// authority before this method returns. `on_commit` runs while ownership
    /// is locked, providing the linearization point for the public lifecycle.
    pub(super) fn supersede_with(
        &self,
        provider: ProviderId,
        cloud_route: Option<CloudRoute>,
        on_commit: impl FnOnce(RecordingIdentity),
    ) -> (RecordingIdentity, Option<RecordingIdentity>) {
        let mut active = self.active.lock();
        let id = RecordingIdentity(self.next_identity.fetch_add(1, Ordering::SeqCst) + 1);
        let superseded = active.map(|state| state.id);
        *active = Some(ActiveRecordingState::new(id, provider, cloud_route));
        on_commit(id);
        (id, superseded)
    }

    #[cfg(test)]
    pub(super) fn supersede(
        &self,
        provider: ProviderId,
        cloud_route: Option<CloudRoute>,
    ) -> (RecordingIdentity, Option<RecordingIdentity>) {
        self.supersede_with(provider, cloud_route, |_| {})
    }

    pub(super) fn complete_if_matching_with(
        &self,
        id: RecordingIdentity,
        on_complete: impl FnOnce(),
    ) -> bool {
        let mut active = self.active.lock();
        if active.map(|state| state.id) != Some(id) {
            return false;
        }
        *active = None;
        on_complete();
        true
    }

    #[cfg(test)]
    pub(super) fn complete_if_matching(&self, id: RecordingIdentity) -> bool {
        self.complete_if_matching_with(id, || {})
    }

    pub(super) fn active_identity(&self) -> Option<RecordingIdentity> {
        self.active.lock().map(|state| state.id)
    }

    pub(super) fn is_authoritative(&self, id: RecordingIdentity) -> bool {
        self.active_identity() == Some(id)
    }

    pub(super) fn is_startable(&self, id: RecordingIdentity) -> bool {
        self.active
            .lock()
            .as_ref()
            .is_some_and(|state| state.id == id && !state.released)
    }

    pub(super) fn cloud_route_for(&self, id: RecordingIdentity) -> Option<CloudRoute> {
        self.active
            .lock()
            .as_ref()
            .and_then(|state| (state.id == id).then_some(state.cloud_route).flatten())
    }

    /// Run `action` only while the exact identity is an authoritative Cloud
    /// recording, exposing its frozen provider and route under the coordinator
    /// lock. This is the route-aware authority gate used by Cloud processing
    /// admission before acquiring the Cloud registry second.
    pub(super) fn with_matching_cloud_route<R>(
        &self,
        id: RecordingIdentity,
        action: impl FnOnce(ProviderId, CloudRoute) -> R,
    ) -> Option<R> {
        let active = self.active.lock();
        let state = active.as_ref()?;
        if state.id != id {
            return None;
        }
        let route = state.cloud_route?;
        match state.provider {
            provider @ (ProviderId::Openai | ProviderId::Groq) => Some(action(provider, route)),
            ProviderId::Local => None,
        }
    }

    /// Route-aware Cloud authority gate that additionally requires recording
    /// start permission to remain open. Live processing registration uses this
    /// before any remote live session may be prepared or started.
    pub(super) fn with_matching_startable_cloud_route<R>(
        &self,
        id: RecordingIdentity,
        action: impl FnOnce(ProviderId, CloudRoute) -> R,
    ) -> Option<R> {
        let active = self.active.lock();
        let state = active.as_ref()?;
        if state.id != id || state.released {
            return None;
        }
        let route = state.cloud_route?;
        match state.provider {
            provider @ (ProviderId::Openai | ProviderId::Groq) => Some(action(provider, route)),
            ProviderId::Local => None,
        }
    }

    pub(super) fn mark_released_with(
        &self,
        id: RecordingIdentity,
        on_release: impl FnOnce(),
    ) -> bool {
        let mut active = self.active.lock();
        let Some(state) = active.as_mut() else {
            return false;
        };
        if state.id != id || state.released {
            return false;
        }
        state.released = true;
        on_release();
        true
    }

    #[cfg(test)]
    pub(super) fn mark_released(&self, id: RecordingIdentity) -> bool {
        self.mark_released_with(id, || {})
    }

    /// Mark an authoritative capture as technically stopped. This closes start
    /// permission even when the stop came from the Cloud recording-limit timer,
    /// while retaining authority for provider finalization and terminal output.
    pub(super) fn mark_capture_stopped_with(
        &self,
        id: RecordingIdentity,
        on_stopped: impl FnOnce(),
    ) -> bool {
        let mut active = self.active.lock();
        let Some(state) = active.as_mut() else {
            return false;
        };
        if state.id != id {
            return false;
        }
        state.released = true;
        on_stopped();
        true
    }

    pub(super) fn is_released(&self, id: RecordingIdentity) -> bool {
        self.active
            .lock()
            .as_ref()
            .is_some_and(|state| state.id == id && state.released)
    }

    #[cfg(test)]
    pub(super) fn claim_failure(&self, id: RecordingIdentity) -> bool {
        self.claim_failure_with(id, || {})
    }

    pub(super) fn run_if_authoritative<R>(
        &self,
        id: RecordingIdentity,
        action: impl FnOnce() -> R,
    ) -> Option<R> {
        let active = self.active.lock();
        if active.as_ref().map(|state| state.id) != Some(id) {
            return None;
        }
        Some(action())
    }

    pub(super) fn run_if_startable<R>(
        &self,
        id: RecordingIdentity,
        action: impl FnOnce() -> R,
    ) -> Result<R, bool> {
        let active = self.active.lock();
        let Some(state) = active.as_ref() else {
            return Err(false);
        };
        if state.id != id {
            return Err(false);
        }
        if state.released {
            return Err(true);
        }
        Ok(action())
    }

    pub(super) fn claim_failure_with(
        &self,
        id: RecordingIdentity,
        on_claim: impl FnOnce(),
    ) -> bool {
        let mut active = self.active.lock();
        let Some(state) = active.as_mut() else {
            return false;
        };
        if state.id != id || state.failure_reported {
            return false;
        }
        state.failure_reported = true;
        on_claim();
        true
    }

    pub(super) fn claim_startable_failure_with(
        &self,
        id: RecordingIdentity,
        on_claim: impl FnOnce(),
    ) -> bool {
        let mut active = self.active.lock();
        let Some(state) = active.as_mut() else {
            return false;
        };
        if state.id != id || state.released || state.failure_reported {
            return false;
        }
        state.failure_reported = true;
        on_claim();
        true
    }

    pub(super) fn claim_failure_for_provider_with(
        &self,
        id: RecordingIdentity,
        provider: ProviderId,
        on_claim: impl FnOnce(),
    ) -> bool {
        let mut active = self.active.lock();
        let Some(state) = active.as_mut() else {
            return false;
        };
        if state.id != id || state.provider != provider || state.failure_reported {
            return false;
        }
        state.failure_reported = true;
        on_claim();
        true
    }
}
