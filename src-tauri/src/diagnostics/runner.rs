use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use crate::models::{AppSettings, ProviderId};

use super::access::test_transcription_access;
use super::cloud::test_cloud_group;
use super::local::{resolve_local_model_path, test_local_provider_path};
use super::microphone::test_microphone;
use super::storage::format_current_date;
use super::types::{
    CloudDiagnostics, CloudProviderRunInput, DiagnosticsRunError, DiagnosticsRunInputs,
    DiagnosticsSnapshotBody, DiagnosticsStorageData, LocalDiagnostics, MicrophoneDiagnostics,
    TranscriptionAccessDiagnostics,
};

#[derive(Debug, Clone, Default)]
pub struct DiagnosticsRunGuard {
    running: Arc<AtomicBool>,
}

pub struct DiagnosticsRunPermit {
    running: Arc<AtomicBool>,
}

impl DiagnosticsRunGuard {
    pub fn try_acquire(&self) -> Result<DiagnosticsRunPermit, DiagnosticsRunError> {
        self.running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| DiagnosticsRunError::AlreadyRunning)?;
        Ok(DiagnosticsRunPermit {
            running: Arc::clone(&self.running),
        })
    }
}

impl Drop for DiagnosticsRunPermit {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
    }
}

/// Diagnostics only need the technical fact/value from credential lookup.
/// No credential-store error message crosses into the diagnostics layer.
pub trait KeystoreOps {
    fn get_key(&self, provider: ProviderId) -> Result<Option<String>, ()>;
}

impl<S: crate::keystore::CredentialStore> KeystoreOps for crate::keystore::Keystore<S> {
    fn get_key(&self, provider: ProviderId) -> Result<Option<String>, ()> {
        self.get_key_typed(provider).map_err(|_| ())
    }
}

pub fn capture_run_inputs<K: KeystoreOps>(
    settings: &AppSettings,
    keystore: &K,
    local_vad_model_path: Option<PathBuf>,
) -> DiagnosticsRunInputs {
    DiagnosticsRunInputs {
        local_model_path: resolve_local_model_path(settings),
        local_vad_model_path,
        openai: CloudProviderRunInput {
            key_result: keystore.get_key(ProviderId::Openai),
            model: settings.openai_model.clone(),
        },
        groq: CloudProviderRunInput {
            key_result: keystore.get_key(ProviderId::Groq),
            model: settings.groq_model.clone(),
        },
    }
}

pub fn run_on_demand(
    inputs: DiagnosticsRunInputs,
) -> Result<DiagnosticsStorageData, DiagnosticsRunError> {
    let DiagnosticsRunInputs {
        local_model_path,
        local_vad_model_path,
        openai,
        groq,
    } = inputs;

    let (local, cloud, microphone, transcription_access) = run_parallel_groups(
        move || test_local_provider_path(local_model_path, local_vad_model_path),
        move || test_cloud_group(openai, groq),
        test_microphone,
        test_transcription_access,
    )?;

    Ok(DiagnosticsStorageData {
        last_scan_display: format_current_date(),
        snapshot: Some(DiagnosticsSnapshotBody {
            local,
            cloud,
            microphone,
            transcription_access,
        }),
    })
}

pub(super) fn run_parallel_groups<LF, CF, MF, AF>(
    local_runner: LF,
    cloud_runner: CF,
    microphone_runner: MF,
    access_runner: AF,
) -> Result<
    (
        LocalDiagnostics,
        CloudDiagnostics,
        MicrophoneDiagnostics,
        TranscriptionAccessDiagnostics,
    ),
    DiagnosticsRunError,
>
where
    LF: FnOnce() -> LocalDiagnostics + Send,
    CF: FnOnce() -> Result<CloudDiagnostics, DiagnosticsRunError> + Send,
    MF: FnOnce() -> MicrophoneDiagnostics + Send,
    AF: FnOnce() -> TranscriptionAccessDiagnostics + Send,
{
    thread::scope(|scope| {
        let local = scope.spawn(local_runner);
        let cloud = scope.spawn(cloud_runner);
        let microphone = scope.spawn(microphone_runner);
        let access = scope.spawn(access_runner);

        // Join every scoped worker before classifying failures. Returning early
        // after the first failed join would leave later handles to Scope's
        // automatic join path; if one of those siblings also panicked, the
        // scope itself could panic instead of returning WorkerFailed.
        let local = local.join();
        let cloud = cloud.join();
        let microphone = microphone.join();
        let access = access.join();

        let local = local.map_err(|_| DiagnosticsRunError::WorkerFailed)?;
        let cloud = cloud.map_err(|_| DiagnosticsRunError::WorkerFailed)??;
        let microphone = microphone.map_err(|_| DiagnosticsRunError::WorkerFailed)?;
        let access = access.map_err(|_| DiagnosticsRunError::WorkerFailed)?;
        Ok((local, cloud, microphone, access))
    })
}
