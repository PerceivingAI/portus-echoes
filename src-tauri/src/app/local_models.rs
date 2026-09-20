//! Shared application service for Local model preload side effects.

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::models::UserErrorCode;
use crate::transcription::local::{LocalEngine, LocalPreloadError, LocalRuntimeIntent};

#[derive(Clone, Serialize)]
struct PreloadErrorPayload {
    path: String,
    code: UserErrorCode,
}

pub(crate) fn preload_error_code(error: LocalPreloadError) -> UserErrorCode {
    match error {
        LocalPreloadError::ModelNotFound => UserErrorCode::LocalModelNotFound,
        LocalPreloadError::UnsupportedModel => UserErrorCode::UnsupportedWhisperModel,
        LocalPreloadError::InsufficientRam => UserErrorCode::InsufficientRam,
        LocalPreloadError::Unexpected => UserErrorCode::Unexpected,
    }
}

pub(crate) fn apply_runtime_intent(
    app: Option<&AppHandle>,
    intent: Option<LocalRuntimeIntent>,
    engine: &LocalEngine,
) {
    let Some(intent) = intent else {
        return;
    };
    let app_handle = app.cloned();
    engine.apply_intent(intent, move |path, result| {
        let Err(error) = result else {
            return;
        };
        if let Some(app) = app_handle.as_ref() {
            let _ = app.emit(
                "model:preload:error",
                PreloadErrorPayload {
                    path: path.to_string_lossy().into_owned(),
                    code: preload_error_code(error),
                },
            );
        }
    });
}

pub(crate) fn preload_completed_download_if_selected(
    app: &AppHandle,
    final_path: &std::path::Path,
) {
    use tauri::Manager;

    let Some(engine) = app.try_state::<LocalEngine>() else {
        return;
    };
    let Some(app_state) = app.try_state::<crate::app_state::AppState>() else {
        return;
    };
    let Some(intent) = app_state.issue_local_reload_if_selected(final_path) else {
        return;
    };

    let engine = engine.inner().clone();
    let preload_app = app.clone();
    engine.apply_intent(intent, move |_, result| {
        if let Err(error) = result {
            let _ = preload_app.emit(
                "app:error",
                crate::models::UserErrorPayload {
                    code: preload_error_code(error),
                },
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_state::AppState;
    use crate::models::{LocalModelKind, ProviderId};

    #[test]
    fn missing_standard_selection_stays_unloaded_without_spawning_preload() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::init(dir.path().join("config.toml"));
        let engine = LocalEngine::with_authority(state.local_runtime_authority());
        let (_settings, (), intent) = state
            .update_settings_with_local_runtime_result(|settings| {
                settings.active_provider = Some(ProviderId::Local);
                settings.local_model_path = "missing-model-path.bin".into();
                settings.local_model_kind = Some(LocalModelKind::Standard);
                ((), true)
            })
            .unwrap();

        assert!(intent.is_none());
        apply_runtime_intent(None, intent, &engine);

        assert_eq!(engine.cached_path(), None);
        assert_eq!(engine.state_generation_and_path().2, "unloaded");
    }

    #[test]
    fn startup_selected_local_intent_starts_worker_and_reaches_ready() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("startup.bin");
        std::fs::write(&path, b"controlled model").unwrap();
        let state = AppState::init(dir.path().join("config.toml"));
        let (_settings, (), intent) = state
            .update_settings_with_local_runtime_result(|settings| {
                settings.active_provider = Some(ProviderId::Local);
                settings.local_model_path = path.to_string_lossy().into_owned();
                settings.local_model_kind = Some(LocalModelKind::Standard);
                ((), true)
            })
            .unwrap();
        let (engine, spawned) =
            LocalEngine::with_controlled_workers(state.local_runtime_authority());

        apply_runtime_intent(None, intent, &engine);
        let worker = spawned
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        assert_eq!(worker.path(), path);
        worker.complete_ready();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while !engine.is_ready_for(&path) {
            assert!(
                std::time::Instant::now() < deadline,
                "startup Local intent must publish Ready"
            );
            std::thread::yield_now();
        }
    }

    #[test]
    fn cloud_intent_revokes_ready_worker_and_reaps_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("selected.bin");
        std::fs::write(&path, b"controlled model").unwrap();
        let state = AppState::init(dir.path().join("config.toml"));
        let (_settings, (), local_intent) = state
            .update_settings_with_local_runtime_result(|settings| {
                settings.active_provider = Some(ProviderId::Local);
                settings.local_model_path = path.to_string_lossy().into_owned();
                settings.local_model_kind = Some(LocalModelKind::Standard);
                ((), true)
            })
            .unwrap();
        let (engine, spawned) =
            LocalEngine::with_controlled_workers(state.local_runtime_authority());
        apply_runtime_intent(None, local_intent, &engine);
        let worker = spawned
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        worker.complete_ready();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while !engine.is_ready_for(&path) {
            assert!(
                std::time::Instant::now() < deadline,
                "controlled Local model must become Ready before Cloud revocation"
            );
            std::thread::yield_now();
        }

        let (_settings, cloud_intent) = state.persist_active_provider(ProviderId::Openai).unwrap();
        apply_runtime_intent(None, cloud_intent, &engine);

        assert_eq!(engine.cached_path(), None);
        assert_eq!(engine.state_generation_and_path().2, "unloaded");
        assert!(worker.wait_until_reaped(std::time::Duration::from_secs(1)));
    }

    #[test]
    fn completed_download_reload_requires_current_selected_active_path() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::init(dir.path().join("config.toml"));
        let selected = dir.path().join("models").join("selected.bin");
        let other = dir.path().join("models").join("other.bin");
        state
            .update_settings_with_local_runtime_result(|settings| {
                settings.active_provider = Some(ProviderId::Local);
                settings.local_model_path = selected.to_string_lossy().into_owned();
                settings.local_model_kind = Some(LocalModelKind::Standard);
                ((), true)
            })
            .unwrap();

        assert!(state.issue_local_reload_if_selected(&selected).is_none());
        std::fs::create_dir_all(selected.parent().unwrap()).unwrap();
        std::fs::write(&selected, b"downloaded model").unwrap();
        assert!(state.issue_local_reload_if_selected(&selected).is_some());
        assert!(state.issue_local_reload_if_selected(&other).is_none());

        state.persist_active_provider(ProviderId::Openai).unwrap();
        assert!(state.issue_local_reload_if_selected(&selected).is_none());
    }
    #[test]
    fn preload_errors_map_to_stable_codes_without_message_text() {
        assert_eq!(
            preload_error_code(LocalPreloadError::ModelNotFound),
            UserErrorCode::LocalModelNotFound
        );
        assert_eq!(
            preload_error_code(LocalPreloadError::UnsupportedModel),
            UserErrorCode::UnsupportedWhisperModel
        );
        assert_eq!(
            preload_error_code(LocalPreloadError::InsufficientRam),
            UserErrorCode::InsufficientRam
        );
        assert_eq!(
            preload_error_code(LocalPreloadError::Unexpected),
            UserErrorCode::Unexpected
        );
    }
}
