//! Provider-neutral live Cloud PCM handoff contract.
//!
//! This module owns only the application-facing boundary between shared
//! microphone capture and a later live Cloud processing branch. Provider
//! endpoints, credentials, request/event formats, language fields, transcript
//! presentation, and provider error payloads do not belong here.

use std::sync::Arc;

// Phase D is the first production owner that will construct these typed
// failures through a real live processing feed.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloudLiveFeedError {
    Closed,
    Backpressure,
}

#[derive(Clone)]
pub(crate) struct CloudLiveFeed {
    on_start: Arc<dyn Fn(u32) -> Result<(), CloudLiveFeedError> + Send + Sync>,
    on_samples: Arc<dyn Fn(Vec<f32>) -> Result<(), CloudLiveFeedError> + Send + Sync>,
    on_stop: Arc<dyn Fn() -> Result<(), CloudLiveFeedError> + Send + Sync>,
    on_abort: Arc<dyn Fn() + Send + Sync>,
}

impl CloudLiveFeed {
    // Phase D is the first production caller; Phase C defines and verifies the
    // contract without inventing a provider/live-processing owner early.
    #[allow(dead_code)]
    pub(crate) fn new(
        on_start: impl Fn(u32) -> Result<(), CloudLiveFeedError> + Send + Sync + 'static,
        on_samples: impl Fn(Vec<f32>) -> Result<(), CloudLiveFeedError> + Send + Sync + 'static,
        on_stop: impl Fn() -> Result<(), CloudLiveFeedError> + Send + Sync + 'static,
        on_abort: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            on_start: Arc::new(on_start),
            on_samples: Arc::new(on_samples),
            on_stop: Arc::new(on_stop),
            on_abort: Arc::new(on_abort),
        }
    }

    pub(crate) fn start(&self, sample_rate: u32) -> Result<(), CloudLiveFeedError> {
        (self.on_start)(sample_rate)
    }

    pub(crate) fn push(&self, samples: Vec<f32>) -> Result<(), CloudLiveFeedError> {
        (self.on_samples)(samples)
    }

    pub(crate) fn stop(&self) -> Result<(), CloudLiveFeedError> {
        (self.on_stop)()
    }

    pub(crate) fn abort(&self) {
        (self.on_abort)();
    }
}
