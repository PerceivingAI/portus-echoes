//! Shared application state and stateful services managed by Tauri.
//!
//! This module owns persisted settings state, credential access, Diagnostics
//! cache/run state, recording lifecycle state, and the in-memory delivery
//! capability. It exposes no Tauri IPC commands.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use tauri::{AppHandle, Emitter};

use crate::diagnostics::{
    self, DiagnosticsRunError, DiagnosticsRunGuard, DiagnosticsRunInputs, DiagnosticsStorageData,
};
use crate::keystore::{platform, Keystore, PlatformStore};
use crate::models::{
    AppSettings, CloudModelKind, LocalModelKind, ProviderId, RecordingPhase, RecordingState,
    UserErrorCode, RECORDING_STATE_EVENT,
};
use crate::settings::{self, ProviderStatus};
use crate::transcription::local::{LocalRuntimeAuthority, LocalRuntimeIntent, LocalRuntimeTarget};

fn local_runtime_target(settings: &AppSettings) -> LocalRuntimeTarget {
    if settings.active_provider != Some(ProviderId::Local) || settings.local_model_path.is_empty() {
        return LocalRuntimeTarget::Unloaded;
    }

    let path = PathBuf::from(&settings.local_model_path);
    match settings.local_model_kind {
        Some(LocalModelKind::Standard) if path.is_file() => LocalRuntimeTarget::Load(path),
        Some(LocalModelKind::Custom) => LocalRuntimeTarget::Load(path),
        Some(LocalModelKind::Standard) | None => LocalRuntimeTarget::Unloaded,
    }
}

fn apply_clean_install_model_defaults(settings: &mut AppSettings, models_dir: &Path) {
    if let Some(model_id) = crate::cloud_catalog::first_standard_model_id(ProviderId::Openai) {
        settings.openai_model = model_id.to_string();
        settings.openai_model_kind = Some(CloudModelKind::Standard);
    }
    if let Some(model_id) = crate::cloud_catalog::first_standard_model_id(ProviderId::Groq) {
        settings.groq_model = model_id.to_string();
        settings.groq_model_kind = Some(CloudModelKind::Standard);
    }
    if let Some(model_path) = crate::model_download::first_configured_model_path(models_dir) {
        settings.local_model_path = model_path.to_string_lossy().into_owned();
        settings.local_model_kind = Some(LocalModelKind::Standard);
    }
}

/// Shared application state managed by Tauri.
pub struct AppState {
    settings: Mutex<AppSettings>,
    local_runtime_authority: Arc<LocalRuntimeAuthority>,
    settings_path: PathBuf,
    diagnostics_path: PathBuf,
    diagnostics_cache: Mutex<DiagnosticsStorageData>,
    diagnostics_run_guard: DiagnosticsRunGuard,
    keystore: Keystore<PlatformStore>,
    recording_state: Mutex<RecordingState>,
    delivery_capability: Mutex<crate::output::DeliveryCapability>,
}

impl AppState {
    /// Load settings from `settings_path` (defaults on missing/corrupt) and
    /// build the shared state.
    pub fn init(settings_path: PathBuf) -> Self {
        let diagnostics_path = settings_path
            .parent()
            .map(|p| p.join("diagnostics.json"))
            .unwrap_or_else(|| PathBuf::from("diagnostics.json"));
        let diagnostics_cache = Mutex::new(diagnostics::load_storage(&diagnostics_path));
        let clean_install = !settings_path.exists();
        let models_dir = settings_path
            .parent()
            .map(|parent| parent.join("models"))
            .unwrap_or_else(|| PathBuf::from("models"));
        let mut initial_settings = settings::load(&settings_path);

        if clean_install {
            // Installation defaults are real saved selections, not readiness.
            // All populated providers select Standard slot 1 from the same
            // build-time catalogs used by the frontend. The Local managed path
            // may legitimately be absent until the user downloads that model.
            apply_clean_install_model_defaults(&mut initial_settings, &models_dir);
            let _ = settings::save(&settings_path, &initial_settings);
        } else if let Ok(Some(pending_path)) =
            crate::model_download::pending_selected_deletion_recovery(&models_dir)
        {
            let selected_matches = initial_settings.local_model_kind
                == Some(LocalModelKind::Standard)
                && initial_settings.local_model_path == pending_path.to_string_lossy();
            if selected_matches && !pending_path.is_file() {
                // A managed deletion committed but its later selected-state
                // persistence did not. Correct memory before Local runtime
                // intent; retain the journal until the correction persists.
                initial_settings.local_model_path.clear();
                initial_settings.local_model_kind = None;
                if settings::save(&settings_path, &initial_settings).is_ok() {
                    let _ = crate::model_download::clear_selected_deletion_recovery(&models_dir);
                }
            } else {
                // File deletion did not commit, selection already changed, or
                // successful cleanup left only an orphaned journal.
                let _ = crate::model_download::clear_selected_deletion_recovery(&models_dir);
            }
        }
        let local_runtime_authority = Arc::new(LocalRuntimeAuthority::new(local_runtime_target(
            &initial_settings,
        )));

        Self {
            settings: Mutex::new(initial_settings),
            local_runtime_authority,
            settings_path,
            diagnostics_path,
            diagnostics_cache,
            diagnostics_run_guard: DiagnosticsRunGuard::default(),
            keystore: platform(),
            recording_state: Mutex::new(RecordingState::default()),
            delivery_capability: Mutex::new(crate::output::detect_delivery_method().capability()),
        }
    }

    fn persist(&self, settings: &AppSettings) -> Result<(), UserErrorCode> {
        settings::save(&self.settings_path, settings).map_err(|_| UserErrorCode::SettingsSave)
    }

    pub(crate) fn update_settings(
        &self,
        update: impl FnOnce(&mut AppSettings),
    ) -> Result<AppSettings, UserErrorCode> {
        self.update_settings_with_result(|settings| {
            update(settings);
        })
        .map(|(settings, ())| settings)
    }

    /// Atomically mutate settings and return a command-level fact derived from
    /// the same locked snapshot. Persistence occurs before the authoritative
    /// in-memory settings value is replaced.
    pub(crate) fn update_settings_with_result<R>(
        &self,
        update: impl FnOnce(&mut AppSettings) -> R,
    ) -> Result<(AppSettings, R), UserErrorCode> {
        let mut guard = self.settings.lock();
        let mut next = guard.clone();
        let result = update(&mut next);
        if next == *guard {
            return Ok((guard.clone(), result));
        }
        self.persist(&next)?;
        *guard = next;
        Ok((guard.clone(), result))
    }

    pub(crate) fn update_settings_with_local_runtime_result<R>(
        &self,
        update: impl FnOnce(&mut AppSettings) -> (R, bool),
    ) -> Result<(AppSettings, R, Option<LocalRuntimeIntent>), UserErrorCode> {
        let mut guard = self.settings.lock();
        let previous_target = self.local_runtime_authority.current_target();
        let mut next = guard.clone();
        let (result, force_reload) = update(&mut next);
        if next != *guard {
            self.persist(&next)?;
            *guard = next;
        }
        let next_target = local_runtime_target(&guard);
        let publish = previous_target != next_target
            || (force_reload && matches!(next_target, LocalRuntimeTarget::Load(_)));
        let intent = publish.then(|| self.local_runtime_authority.publish(next_target));
        Ok((guard.clone(), result, intent))
    }

    pub(crate) fn local_runtime_authority(&self) -> Arc<LocalRuntimeAuthority> {
        Arc::clone(&self.local_runtime_authority)
    }

    pub(crate) fn local_runtime_intent(&self) -> LocalRuntimeIntent {
        self.local_runtime_authority.current()
    }

    pub(crate) fn issue_local_reload_if_selected(
        &self,
        model_path: &Path,
    ) -> Option<LocalRuntimeIntent> {
        let settings = self.settings.lock();
        let target = local_runtime_target(&settings);
        match &target {
            LocalRuntimeTarget::Load(selected) if selected == model_path => {
                Some(self.local_runtime_authority.publish(target))
            }
            LocalRuntimeTarget::Unloaded | LocalRuntimeTarget::Load(_) => None,
        }
    }

    pub(crate) fn persist_active_provider(
        &self,
        provider: ProviderId,
    ) -> Result<(AppSettings, Option<LocalRuntimeIntent>), UserErrorCode> {
        self.update_settings_with_local_runtime_result(|settings| {
            settings.active_provider = Some(provider);
            ((), provider == ProviderId::Local)
        })
        .map(|(settings, (), intent)| (settings, intent))
    }

    /// Clone of the current settings (contains no secrets by construction).
    pub fn settings_snapshot(&self) -> AppSettings {
        self.settings.lock().clone()
    }

    /// Freeze the settings and delivery capability used by one recording
    /// admission at a single lock boundary. Lock order is settings, then
    /// delivery capability; no I/O or callbacks run while both are held.
    pub(crate) fn recording_admission_snapshot(
        &self,
    ) -> (AppSettings, crate::output::DeliveryCapability) {
        let settings = self.settings.lock();
        let delivery_capability = self.delivery_capability.lock();
        (settings.clone(), *delivery_capability)
    }

    /// The configured hotkey combo string (settings is the source of truth).
    pub fn hotkey(&self) -> String {
        self.settings.lock().hotkey.clone()
    }

    fn status_for(&self, settings: &AppSettings) -> Result<ProviderStatus, UserErrorCode> {
        let keys = self
            .keystore
            .key_presence_typed()
            .map_err(|_| UserErrorCode::CredentialStorage)?;
        Ok(settings::provider_status(settings, keys))
    }

    pub(crate) fn provider_status(&self) -> Result<ProviderStatus, UserErrorCode> {
        self.status_for(&self.settings.lock())
    }

    pub(crate) fn set_api_key(&self, provider: ProviderId, key: &str) -> Result<(), UserErrorCode> {
        self.keystore
            .set_key_typed(provider, key)
            .map_err(|_| UserErrorCode::CredentialStorage)
    }

    pub(crate) fn delete_api_key(&self, provider: ProviderId) -> Result<(), UserErrorCode> {
        self.keystore
            .delete_key_typed(provider)
            .map_err(|_| UserErrorCode::CredentialStorage)
    }

    /// The stored API key for a provider, straight from the OS credential
    /// store. Key material is returned only to the approved Settings editor or
    /// the normal Cloud request-scoped job; it is never logged or emitted.
    pub fn api_key(&self, provider: ProviderId) -> Result<Option<String>, UserErrorCode> {
        self.keystore
            .get_key_typed(provider)
            .map_err(|_| UserErrorCode::CredentialStorage)
    }

    pub(crate) fn diagnostics_snapshot(&self) -> DiagnosticsStorageData {
        self.diagnostics_cache.lock().clone()
    }

    pub(crate) fn diagnostics_run_guard(&self) -> &DiagnosticsRunGuard {
        &self.diagnostics_run_guard
    }

    pub(crate) fn capture_diagnostics_inputs(
        &self,
        local_vad_model_path: Option<PathBuf>,
    ) -> DiagnosticsRunInputs {
        let settings = self.settings_snapshot();
        diagnostics::capture_run_inputs(&settings, &self.keystore, local_vad_model_path)
    }

    pub(crate) fn commit_diagnostics(
        &self,
        data: DiagnosticsStorageData,
    ) -> Result<(), DiagnosticsRunError> {
        let delivery_capability = diagnostics::delivery_capability_from_snapshot(&data);
        diagnostics::save_storage(&self.diagnostics_path, &data)
            .map_err(|_| DiagnosticsRunError::PersistenceFailed)?;
        *self.diagnostics_cache.lock() = data;
        self.set_delivery_capability(delivery_capability);
        Ok(())
    }

    /// Shared terminal recording bound for both completed and live Cloud capture routes.
    pub fn cloud_recording_limit(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.settings.lock().cloud_recording_limit_secs as u64)
    }

    /// Build-time configured recording limit for live Cloud streaming (gpt-live-transcribe).
    /// Returns None if configured to 0 (unlimited/untimed), or Some(Duration) if > 0.
    pub fn live_cloud_recording_limit(&self) -> Option<std::time::Duration> {
        const COMPILED_LIMIT: u32 = match compile_time_limit(env!("PORTUS_LIVE_RECORDING_LIMIT_SECS")) {
            Ok(val) => val,
            Err(_) => 1800,
        };
        if COMPILED_LIMIT == 0 {
            None
        } else {
            Some(std::time::Duration::from_secs(COMPILED_LIMIT as u64))
        }
    }

    /// Application-session transcript delivery capability. It is selected at
    /// startup and may be replaced only by a completed explicit Diagnostics run.
    pub fn delivery_capability(&self) -> crate::output::DeliveryCapability {
        *self.delivery_capability.lock()
    }

    fn set_delivery_capability(&self, capability: crate::output::DeliveryCapability) {
        *self.delivery_capability.lock() = capability;
    }

    /// Current authoritative recording lifecycle snapshot.
    pub fn recording_state(&self) -> RecordingState {
        *self.recording_state.lock()
    }

    /// Commit and publish one recording lifecycle transition. Duplicate phases
    /// are idempotent and preserve the current revision.
    pub fn set_recording_phase(&self, app: &AppHandle, phase: RecordingPhase) -> RecordingState {
        let snapshot = {
            let mut current = self.recording_state.lock();
            let Some(next) = transition_recording_state(&mut current, phase) else {
                return *current;
            };
            next
        };

        let capability = self.delivery_capability();
        crate::app::windows::sync_recording_ui_with_capability(app, snapshot.phase, capability);
        let _ = app.emit(RECORDING_STATE_EVENT, snapshot);

        if capability == crate::output::DeliveryCapability::Clipboard {
            if snapshot.phase == RecordingPhase::Finalizing {
                let _ = app.emit(
                    crate::models::DELIVERY_CLIPBOARD_STATUS_EVENT,
                    crate::models::ClipboardStatusPayload {
                        status: crate::models::ClipboardDeliveryStatus::Processing,
                    },
                );
            } else if snapshot.phase == RecordingPhase::Preparing
                || snapshot.phase == RecordingPhase::Recording
            {
                let _ = app.emit(
                    crate::models::DELIVERY_CLIPBOARD_STATUS_EVENT,
                    crate::models::ClipboardStatusPayload {
                        status: crate::models::ClipboardDeliveryStatus::Idle,
                    },
                );
            }
        }
        snapshot
    }
}

const fn compile_time_limit(s: &str) -> Result<u32, ()> {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return Ok(0);
    }
    let mut val: u32 = 0;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b < b'0' || b > b'9' {
            return Err(());
        }
        val = val * 10 + (b - b'0') as u32;
        i += 1;
    }
    Ok(val)
}

fn transition_recording_state(
    current: &mut RecordingState,
    phase: RecordingPhase,
) -> Option<RecordingState> {
    if current.phase == phase {
        return None;
    }
    current.phase = phase;
    current.revision = current.revision.saturating_add(1);
    Some(*current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::{
        CloudDiagnostics, DiagnosticGroupStatus, DiagnosticsSnapshotBody, LocalDiagnostics,
        MicrophoneDiagnostics, ProviderValidation, TranscriptionAccessDiagnostics,
        TranscriptionFunction, TranscriptionType,
    };

    #[test]
    fn app_state_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AppState>();
    }

    #[test]
    fn credential_mutation_contracts_return_only_mutation_success() {
        let _set: fn(&AppState, ProviderId, &str) -> Result<(), UserErrorCode> =
            AppState::set_api_key;
        let _delete: fn(&AppState, ProviderId) -> Result<(), UserErrorCode> =
            AppState::delete_api_key;
    }

    #[test]
    fn clean_install_selects_first_standard_slot_for_every_populated_provider() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        assert!(!config_path.exists());

        let state = AppState::init(config_path.clone());
        let settings = state.settings_snapshot();

        let openai = crate::cloud_catalog::first_standard_model_id(ProviderId::Openai);
        assert_eq!(settings.openai_model, openai.unwrap_or_default());
        assert_eq!(
            settings.openai_model_kind,
            openai.map(|_| CloudModelKind::Standard)
        );

        let groq = crate::cloud_catalog::first_standard_model_id(ProviderId::Groq);
        assert_eq!(settings.groq_model, groq.unwrap_or_default());
        assert_eq!(
            settings.groq_model_kind,
            groq.map(|_| CloudModelKind::Standard)
        );

        let expected_local =
            crate::model_download::first_configured_model_path(&dir.path().join("models"));
        assert_eq!(
            settings.local_model_path,
            expected_local
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default()
        );
        assert_eq!(
            settings.local_model_kind,
            expected_local.map(|_| LocalModelKind::Standard)
        );
        assert!(matches!(
            state.local_runtime_intent().target(),
            LocalRuntimeTarget::Unloaded
        ));
        assert_eq!(settings::load(&config_path), settings);
    }

    #[test]
    fn startup_pending_managed_deletion_clears_and_persists_missing_standard_selection() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let models_dir = dir.path().join("models");
        let missing = models_dir.join("missing-standard.bin");
        let configured = AppSettings {
            active_provider: Some(ProviderId::Local),
            local_model_path: missing.to_string_lossy().into_owned(),
            local_model_kind: Some(crate::models::LocalModelKind::Standard),
            ..Default::default()
        };
        settings::save(&config_path, &configured).unwrap();
        crate::model_download::begin_selected_deletion_recovery(&models_dir, &missing).unwrap();

        let state = AppState::init(config_path.clone());
        let runtime = state.local_runtime_intent();

        assert!(state.settings_snapshot().local_model_path.is_empty());
        assert_eq!(state.settings_snapshot().local_model_kind, None);
        assert!(matches!(runtime.target(), LocalRuntimeTarget::Unloaded));
        let persisted = settings::load(&config_path);
        assert!(persisted.local_model_path.is_empty());
        assert_eq!(persisted.local_model_kind, None);
        assert_eq!(
            crate::model_download::pending_selected_deletion_recovery(&models_dir).unwrap(),
            None
        );
    }

    #[test]
    fn startup_preserves_missing_standard_selection_without_deletion_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let missing = dir.path().join("models").join("not-yet-downloaded.bin");
        let configured = AppSettings {
            active_provider: Some(ProviderId::Local),
            local_model_path: missing.to_string_lossy().into_owned(),
            local_model_kind: Some(crate::models::LocalModelKind::Standard),
            ..Default::default()
        };
        settings::save(&config_path, &configured).unwrap();

        let state = AppState::init(config_path.clone());

        assert_eq!(state.settings_snapshot(), configured);
        assert!(matches!(
            state.local_runtime_intent().target(),
            LocalRuntimeTarget::Unloaded
        ));
        assert_eq!(settings::load(&config_path), configured);
    }

    #[test]
    fn startup_pending_deletion_uses_corrected_memory_even_when_rewrite_fails() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let models_dir = dir.path().join("models");
        let missing = models_dir.join("missing-standard.bin");
        let configured = AppSettings {
            active_provider: Some(ProviderId::Local),
            local_model_path: missing.to_string_lossy().into_owned(),
            local_model_kind: Some(crate::models::LocalModelKind::Standard),
            ..Default::default()
        };
        settings::save(&config_path, &configured).unwrap();
        let mut tmp = config_path.as_os_str().to_owned();
        crate::model_download::begin_selected_deletion_recovery(&models_dir, &missing).unwrap();
        tmp.push(".tmp");
        std::fs::create_dir(std::path::PathBuf::from(tmp)).unwrap();

        let state = AppState::init(config_path.clone());

        assert!(state.settings_snapshot().local_model_path.is_empty());
        assert_eq!(state.settings_snapshot().local_model_kind, None);
        assert!(matches!(
            state.local_runtime_intent().target(),
            LocalRuntimeTarget::Unloaded
        ));
        let persisted = settings::load(&config_path);
        assert_eq!(persisted.local_model_path, configured.local_model_path);
        assert_eq!(
            persisted.local_model_kind,
            Some(crate::models::LocalModelKind::Standard)
        );
    }

    #[test]
    fn startup_preserves_present_standard_local_selection() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let model = dir.path().join("present-standard.bin");
        std::fs::write(&model, b"managed model bytes").unwrap();
        let configured = AppSettings {
            active_provider: Some(ProviderId::Local),
            local_model_path: model.to_string_lossy().into_owned(),
            local_model_kind: Some(crate::models::LocalModelKind::Standard),
            ..Default::default()
        };
        settings::save(&config_path, &configured).unwrap();

        let state = AppState::init(config_path.clone());

        assert_eq!(
            state.settings_snapshot().local_model_path,
            configured.local_model_path
        );
        assert_eq!(
            state.settings_snapshot().local_model_kind,
            Some(crate::models::LocalModelKind::Standard)
        );
        assert_eq!(settings::load(&config_path), configured);
    }

    #[test]
    fn startup_never_reconciles_missing_custom_local_path() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let missing = dir.path().join("user-owned-moved-or-deleted.bin");
        let configured = AppSettings {
            active_provider: Some(ProviderId::Local),
            local_model_path: missing.to_string_lossy().into_owned(),
            local_model_kind: Some(crate::models::LocalModelKind::Custom),
            local_custom_model_path: missing.to_string_lossy().into_owned(),
            ..Default::default()
        };
        settings::save(&config_path, &configured).unwrap();

        let state = AppState::init(config_path.clone());

        assert_eq!(state.settings_snapshot(), configured);
        assert_eq!(settings::load(&config_path), configured);
    }

    #[test]
    fn startup_delivery_capability_comes_from_shared_method_selector() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::init(dir.path().join("config.toml"));
        let expected = crate::output::detect_delivery_method().capability();
        assert_eq!(state.delivery_capability(), expected);

        #[cfg(target_os = "windows")]
        assert_eq!(
            state.delivery_capability(),
            crate::output::DeliveryCapability::Inject,
            "Windows startup must derive Inject from the shared SendInput method"
        );

        #[cfg(target_os = "linux")]
        assert_eq!(
            state.delivery_capability(),
            crate::output::DeliveryCapability::Clipboard,
            "Linux startup must derive Clipboard across both X11 and Wayland for this release"
        );
    }

    #[test]
    fn delivery_capability_changes_only_through_explicit_state_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::init(dir.path().join("config.toml"));
        let startup = state.delivery_capability();
        let replacement = match startup {
            crate::output::DeliveryCapability::Inject => {
                crate::output::DeliveryCapability::Clipboard
            }
            crate::output::DeliveryCapability::Clipboard => {
                crate::output::DeliveryCapability::Inject
            }
        };
        state.set_delivery_capability(replacement);
        assert_eq!(state.delivery_capability(), replacement);
        assert_ne!(state.delivery_capability(), startup);
    }

    #[test]
    fn recording_admission_snapshot_freezes_settings_and_delivery_together() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::init(dir.path().join("config.toml"));
        state
            .update_settings(|settings| {
                settings.active_provider = Some(ProviderId::Local);
                settings.local_model_path = "model-a.bin".to_string();
            })
            .unwrap();
        state.set_delivery_capability(crate::output::DeliveryCapability::Inject);

        let (first_settings, first_delivery) = state.recording_admission_snapshot();

        state
            .update_settings(|settings| {
                settings.local_model_path = "model-b.bin".to_string();
            })
            .unwrap();
        state.set_delivery_capability(crate::output::DeliveryCapability::Clipboard);
        let (second_settings, second_delivery) = state.recording_admission_snapshot();

        assert_eq!(first_settings.local_model_path, "model-a.bin");
        assert_eq!(first_delivery, crate::output::DeliveryCapability::Inject);
        assert_eq!(second_settings.local_model_path, "model-b.bin");
        assert_eq!(
            second_delivery,
            crate::output::DeliveryCapability::Clipboard
        );
    }

    #[test]
    fn recording_state_transitions_are_versioned_and_idempotent() {
        let mut state = RecordingState::default();
        assert_eq!(
            transition_recording_state(&mut state, RecordingPhase::Idle),
            None
        );

        let preparing = transition_recording_state(&mut state, RecordingPhase::Preparing).unwrap();
        assert_eq!(preparing.phase, RecordingPhase::Preparing);
        assert_eq!(preparing.revision, 1);

        let recording = transition_recording_state(&mut state, RecordingPhase::Recording).unwrap();
        assert_eq!(recording.phase, RecordingPhase::Recording);
        assert_eq!(recording.revision, 2);
        assert_eq!(
            transition_recording_state(&mut state, RecordingPhase::Recording),
            None
        );

        let finalizing =
            transition_recording_state(&mut state, RecordingPhase::Finalizing).unwrap();
        assert_eq!(finalizing.phase, RecordingPhase::Finalizing);
        assert_eq!(finalizing.revision, 3);

        let idle = transition_recording_state(&mut state, RecordingPhase::Idle).unwrap();
        assert_eq!(idle.phase, RecordingPhase::Idle);
        assert_eq!(idle.revision, 4);

        let muted = transition_recording_state(&mut state, RecordingPhase::Muted).unwrap();
        assert_eq!(muted.phase, RecordingPhase::Muted);
        assert_eq!(muted.revision, 5);
        assert_eq!(
            transition_recording_state(&mut state, RecordingPhase::Muted),
            None
        );

        let finalizing_from_muted =
            transition_recording_state(&mut state, RecordingPhase::Finalizing).unwrap();
        assert_eq!(finalizing_from_muted.phase, RecordingPhase::Finalizing);
        assert_eq!(finalizing_from_muted.revision, 6);
    }
    fn diagnostics_snapshot_with_date(date: &str) -> DiagnosticsStorageData {
        DiagnosticsStorageData {
            last_scan_display: date.to_string(),
            snapshot: Some(DiagnosticsSnapshotBody {
                local: LocalDiagnostics {
                    passed: DiagnosticGroupStatus::Pass,
                    model_path: ProviderValidation::Validated,
                    model_file: ProviderValidation::Validated,
                },
                cloud: CloudDiagnostics {
                    passed: DiagnosticGroupStatus::Fail,
                    openai_api_key: ProviderValidation::NoneNotValid,
                    openai_model: ProviderValidation::NoneNotValid,
                    groq_api_key: ProviderValidation::NoneNotValid,
                    groq_model: ProviderValidation::NoneNotValid,
                },
                microphone: MicrophoneDiagnostics {
                    passed: DiagnosticGroupStatus::Pass,
                    name: "Test Microphone".to_string(),
                    specs: "48000 Hz, 2 Channels".to_string(),
                },
                transcription_access: TranscriptionAccessDiagnostics {
                    passed: DiagnosticGroupStatus::Pass,
                    function: TranscriptionFunction::System,
                    access_type: TranscriptionType::Clipboard,
                },
            }),
        }
    }

    #[test]
    fn diagnostics_cache_is_loaded_once_at_app_state_initialization() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let diagnostics_path = dir.path().join("diagnostics.json");
        let first = diagnostics_snapshot_with_date("21 Mar 2026");
        diagnostics::save_storage(&diagnostics_path, &first).unwrap();

        let state = AppState::init(config_path);
        assert_eq!(state.diagnostics_snapshot(), first);

        let second = diagnostics_snapshot_with_date("22 Mar 2026");
        diagnostics::save_storage(&diagnostics_path, &second).unwrap();
        assert_eq!(
            state.diagnostics_snapshot(),
            first,
            "cache reads must not reload diagnostics.json"
        );
    }

    #[test]
    fn diagnostics_commit_persists_before_replacing_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let diagnostics_path = dir.path().join("diagnostics.json");
        let state = AppState::init(config_path);
        let committed = diagnostics_snapshot_with_date("21 Mar 2026");

        state.commit_diagnostics(committed.clone()).unwrap();

        assert_eq!(state.diagnostics_snapshot(), committed);
        assert_eq!(
            state.delivery_capability(),
            crate::output::DeliveryCapability::Clipboard,
            "a durably committed Clipboard access result must replace runtime delivery capability"
        );
        assert_eq!(diagnostics::load_storage(&diagnostics_path), committed);
    }

    #[test]
    fn persisted_sendinput_diagnostics_replaces_clipboard_capability_with_inject() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let diagnostics_path = dir.path().join("diagnostics.json");
        let state = AppState::init(config_path);
        state.set_delivery_capability(crate::output::DeliveryCapability::Clipboard);

        let mut committed = diagnostics_snapshot_with_date("21 Mar 2026");
        committed.snapshot.as_mut().unwrap().transcription_access =
            TranscriptionAccessDiagnostics {
                passed: DiagnosticGroupStatus::Pass,
                function: TranscriptionFunction::SendInput,
                access_type: TranscriptionType::Cursor,
            };

        state.commit_diagnostics(committed.clone()).unwrap();

        assert_eq!(diagnostics::load_storage(&diagnostics_path), committed);
        assert_eq!(
            state.delivery_capability(),
            crate::output::DeliveryCapability::Inject,
            "a durably committed SendInput/Cursor result must replace Clipboard with Inject"
        );
    }

    #[test]
    fn diagnostics_persistence_failure_keeps_previous_cache_authoritative() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let diagnostics_path = dir.path().join("diagnostics.json");
        let first = diagnostics_snapshot_with_date("21 Mar 2026");
        diagnostics::save_storage(&diagnostics_path, &first).unwrap();
        let state = AppState::init(config_path);

        let tmp = diagnostics_path.with_extension("json.tmp");
        state.set_delivery_capability(crate::output::DeliveryCapability::Inject);
        std::fs::create_dir(&tmp).unwrap();
        let second = diagnostics_snapshot_with_date("22 Mar 2026");
        assert_eq!(
            state.commit_diagnostics(second),
            Err(DiagnosticsRunError::PersistenceFailed)
        );
        assert_eq!(state.diagnostics_snapshot(), first);
        assert_eq!(diagnostics::load_storage(&diagnostics_path), first);
        assert_eq!(
            state.delivery_capability(),
            crate::output::DeliveryCapability::Inject,
            "failed diagnostics persistence must not replace runtime delivery capability"
        );
    }

    #[test]
    fn active_provider_selection_persists_without_configuration_validation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let state = AppState::init(path.clone());

        let (selected, _intent) = state.persist_active_provider(ProviderId::Openai).unwrap();
        state
            .update_settings(|settings| {
                settings.openai_model.clear();
                settings.openai_model_kind = None;
            })
            .unwrap();

        assert_eq!(selected.active_provider, Some(ProviderId::Openai));
        assert_eq!(
            settings::load(&path).active_provider,
            Some(ProviderId::Openai)
        );
        assert!(!state.provider_status().unwrap().openai_configured);
    }

    #[test]
    fn update_settings_with_result_preserves_atomic_change_fact() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::init(dir.path().join("config.toml"));
        let selected = std::path::Path::new("selected.bin");
        state
            .update_settings(|settings| {
                settings.local_model_path = selected.to_string_lossy().into_owned();
                settings.local_model_kind = Some(crate::models::LocalModelKind::Standard);
            })
            .unwrap();

        let (settings, changed) = state
            .update_settings_with_result(|settings| {
                if settings.local_model_path == selected.to_string_lossy() {
                    settings.local_model_path.clear();
                    settings.local_model_kind = None;
                    true
                } else {
                    false
                }
            })
            .unwrap();

        assert!(changed);
        assert!(settings.local_model_path.is_empty());
        assert_eq!(settings.local_model_kind, None);
    }

    #[test]
    fn local_runtime_intents_follow_selection_order_and_force_same_path_retry() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::init(dir.path().join("config.toml"));
        let model_a_path = dir.path().join("a.bin");
        let model_b_path = dir.path().join("b.bin");
        std::fs::write(&model_a_path, b"model-a").unwrap();
        std::fs::write(&model_b_path, b"model-b").unwrap();

        let (_settings, (), model_a) = state
            .update_settings_with_local_runtime_result(|settings| {
                settings.active_provider = Some(ProviderId::Local);
                settings.local_model_path = model_a_path.to_string_lossy().into_owned();
                settings.local_model_kind = Some(LocalModelKind::Standard);
                ((), true)
            })
            .unwrap();
        let (_settings, (), model_b) = state
            .update_settings_with_local_runtime_result(|settings| {
                settings.local_model_path = model_b_path.to_string_lossy().into_owned();
                settings.local_model_kind = Some(LocalModelKind::Standard);
                ((), true)
            })
            .unwrap();
        let (_settings, (), model_b_retry) = state
            .update_settings_with_local_runtime_result(|_settings| ((), true))
            .unwrap();

        let model_a = model_a.unwrap();
        let model_b = model_b.unwrap();
        let model_b_retry = model_b_retry.unwrap();
        assert!(model_a.generation() < model_b.generation());
        assert!(model_b.generation() < model_b_retry.generation());
        assert_eq!(
            model_b_retry.target(),
            &LocalRuntimeTarget::Load(model_b_path)
        );
    }

    #[test]
    fn live_cloud_recording_limit_parses_compile_time_env() {
        assert_eq!(super::compile_time_limit(""), Ok(0));
        assert_eq!(super::compile_time_limit("0"), Ok(0));
        assert_eq!(super::compile_time_limit("1800"), Ok(1800));
        assert_eq!(super::compile_time_limit("60"), Ok(60));
        assert!(super::compile_time_limit("invalid").is_err());

        let dir = tempfile::tempdir().unwrap();
        let state = AppState::init(dir.path().join("config.toml"));
        let limit = state.live_cloud_recording_limit();
        let expected_env: u32 = env!("PORTUS_LIVE_RECORDING_LIMIT_SECS").parse().unwrap_or(1800);
        if expected_env == 0 {
            assert_eq!(limit, None);
        } else {
            assert_eq!(limit, Some(std::time::Duration::from_secs(expected_env as u64)));
        }
    }
}
