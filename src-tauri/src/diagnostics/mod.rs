//! On-demand diagnostics governed by `docs/DIAGNOSTICS.md`.
//! Local, Cloud, and Microphone probes are informational. After durable commit,
//! Transcription Access may replace the in-memory delivery capability.

mod access;
mod cloud;
mod local;
mod microphone;
mod process_probe;
mod runner;
mod storage;
mod types;

pub use access::delivery_capability_from_snapshot;
pub(crate) use process_probe::run_internal_probe_from_process_args;
pub use runner::{capture_run_inputs, run_on_demand, DiagnosticsRunGuard};
pub use storage::{load_storage, save_storage};
pub(crate) use types::DiagnosticsRunInputs;
pub use types::{DiagnosticsRunError, DiagnosticsStorageData};

#[cfg(test)]
pub(crate) use runner::KeystoreOps;
#[cfg(test)]
pub(crate) use types::{
    CloudDiagnostics, CloudProviderRunInput, DiagnosticGroupStatus, DiagnosticsSnapshotBody,
    LocalDiagnostics, MicrophoneDiagnostics, ProviderValidation, TranscriptionAccessDiagnostics,
    TranscriptionFunction, TranscriptionType,
};

#[cfg(test)]
use crate::{
    audio::convert::encode_wav_16bit_mono,
    models::{AppSettings, ProviderId},
    output::{DeliveryCapability, DeliveryMethod},
};
#[cfg(test)]
use access::{
    access_fail, access_pass, classify_transcription_access, test_transcription_access,
    CLIPBOARD_VALIDATION_TIMEOUT,
};
#[cfg(test)]
use cloud::{
    classify_cloud_probe, cloud_both_invalid, cloud_probe_spec, evaluate_cloud_provider,
    provider_passes, run_parallel_cloud, send_cloud_probe_to, send_openai_live_probe_to,
    CloudProbeOutcome, OPENAI_LIVE_DIAGNOSTICS_TIMEOUT,
};
#[cfg(test)]
use local::{
    diagnostic_speech_input, local_fail_unreachable, resolve_local_model_path,
    test_local_provider_path,
};
#[cfg(test)]
use microphone::{classify_microphone, MICROPHONE_INSPECTION_TIMEOUT, NONE_NOT_VALID};
#[cfg(test)]
use runner::run_parallel_groups;
#[cfg(test)]
use std::{
    fs, io,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
#[cfg(test)]
use storage::format_current_date;

#[cfg(test)]
mod tests;
