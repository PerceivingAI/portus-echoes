//! Authoritative push-to-talk recording ownership and runtime façade.
//!
//! `coordinator` owns only provider-neutral identity/authority state.
//! `processing` owns exact-ID Local/Cloud worker controls and Cloud admission.
//! `runtime` owns application orchestration. Audio and transcription mechanics
//! remain in their respective subsystems.

mod coordinator;
mod processing;
mod runtime;

pub use coordinator::RecordingCoordinator;
pub(crate) use processing::CloudProcessingControl;
pub use processing::{CloudProcessingRegistry, LocalProcessingRegistry};
pub use runtime::{
    abort_active_recording, begin_recording, handle_capture_event, shutdown, stop_recording,
    toggle_recording,
};
pub(crate) use runtime::{claim_completed_cloud_job, complete_cloud, run_if_authoritative};

#[cfg(test)]
use processing::{claim_cloud_completed_job_exact, register_live_cloud_processing_exact};

#[cfg(test)]
mod tests;
