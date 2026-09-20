use std::sync::Arc;

use tauri::{AppHandle, State};

use crate::app::local_models;
use crate::app_state::AppState;
use crate::model_download::{self, DownloadManager};
use crate::models::{AppSettings, LocalModelKind, UserErrorCode};
use crate::transcription::local::LocalEngine;

use super::CommandResult;

/// Disk state for the build-time configured Local model list.
#[tauri::command]
pub fn list_local_models(
    downloads: State<'_, Arc<DownloadManager>>,
) -> CommandResult<Vec<crate::model_download::LocalModelInfo>> {
    Ok(downloads.list())
}

/// Start an environment-defined model download in the background.
#[tauri::command]
pub fn download_model(
    app: AppHandle,
    download_path: String,
    downloads: State<'_, Arc<DownloadManager>>,
) -> CommandResult<()> {
    downloads
        .start(&app, &download_path)
        .map_err(|_| UserErrorCode::ModelDownload)
}

/// Cancel the active download; the partial file is removed by its worker.
#[tauri::command]
pub fn cancel_download(
    download_path: String,
    downloads: State<'_, Arc<DownloadManager>>,
) -> CommandResult<()> {
    downloads
        .cancel(&download_path)
        .map_err(|_| UserErrorCode::ModelDownload)
}

fn begin_managed_delete_recovery(
    downloads: &DownloadManager,
    download_path: &str,
) -> CommandResult<()> {
    downloads
        .ensure_configured(download_path)
        .map_err(|_| UserErrorCode::ModelDownload)?;
    let expected_path = downloads.model_path(download_path);
    model_download::begin_selected_deletion_recovery(downloads.models_dir(), &expected_path)
        .map_err(|_| UserErrorCode::ModelDownload)
}

fn clear_deleted_local_selection(
    settings: &mut AppSettings,
    deleted_path: &std::path::Path,
) -> bool {
    if settings.local_model_kind != Some(LocalModelKind::Standard)
        || settings.local_model_path != deleted_path.to_string_lossy()
    {
        return false;
    }
    settings.local_model_path.clear();
    settings.local_model_kind = None;
    true
}

/// Delete a downloaded model and return canonical settings. If this is the
/// selected standard Local model, selection clearing and persistence remain one
/// atomic AppState mutation before the Local engine is unloaded.
#[tauri::command]
pub fn delete_model(
    state: State<'_, AppState>,
    engine: State<'_, LocalEngine>,
    downloads: State<'_, Arc<DownloadManager>>,
    download_path: String,
) -> CommandResult<AppSettings> {
    begin_managed_delete_recovery(&downloads, &download_path)?;

    let path = match downloads.delete(&download_path) {
        Ok(path) => path,
        Err(_) => {
            let _ = model_download::clear_selected_deletion_recovery(downloads.models_dir());
            return Err(UserErrorCode::ModelDownload);
        }
    };

    let result = state.update_settings_with_local_runtime_result(|settings| {
        (clear_deleted_local_selection(settings, &path), false)
    });
    let (settings, _cleared, intent) = match result {
        Ok(result) => result,
        Err(error) => return Err(error),
    };

    // Once selected-state persistence has succeeded (or there was no selected
    // state to change), the deletion journal is no longer needed. Failure to
    // remove an orphan journal cannot roll back the already-committed command;
    // startup safely discards it when it no longer matches saved selection.
    let _ = model_download::clear_selected_deletion_recovery(downloads.models_dir());
    local_models::apply_runtime_intent(None, intent, &engine);
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ProviderId;

    #[test]
    fn unconfigured_delete_is_rejected_before_recovery_journal_write() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        let manager = DownloadManager::new(
            models_dir.clone(),
            vec!["https://models.example.test/managed.bin".to_string()],
        )
        .unwrap();

        assert_eq!(
            begin_managed_delete_recovery(
                &manager,
                "https://unconfigured.example.test/not-managed.bin"
            ),
            Err(UserErrorCode::ModelDownload)
        );
        assert_eq!(
            model_download::pending_selected_deletion_recovery(&models_dir).unwrap(),
            None
        );
    }

    #[test]
    fn deleting_selected_standard_model_clears_its_selection() {
        let path = std::path::PathBuf::from(r"C:\models\selected.bin");
        let mut settings = AppSettings {
            local_model_path: path.to_string_lossy().into_owned(),
            local_model_kind: Some(LocalModelKind::Standard),
            ..Default::default()
        };

        assert!(clear_deleted_local_selection(&mut settings, &path));
        assert!(settings.local_model_path.is_empty());
        assert_eq!(settings.local_model_kind, None);
    }

    #[test]
    fn deleting_unselected_model_preserves_selection() {
        let selected = std::path::PathBuf::from(r"C:\models\selected.bin");
        let mut settings = AppSettings {
            local_model_path: selected.to_string_lossy().into_owned(),
            local_model_kind: Some(LocalModelKind::Standard),
            ..Default::default()
        };

        assert!(!clear_deleted_local_selection(
            &mut settings,
            &std::path::PathBuf::from(r"C:\models\other.bin")
        ));
        assert_eq!(settings.local_model_path, selected.to_string_lossy());
        assert_eq!(settings.local_model_kind, Some(LocalModelKind::Standard));
    }

    #[test]
    fn clearing_selected_model_publishes_unloaded_runtime_intent() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::init(dir.path().join("config.toml"));
        let path = dir.path().join("deleted-model.bin");
        std::fs::write(&path, b"managed model").unwrap();
        let (_settings, (), load_intent) = state
            .update_settings_with_local_runtime_result(|settings| {
                settings.active_provider = Some(ProviderId::Local);
                settings.local_model_path = path.to_string_lossy().into_owned();
                settings.local_model_kind = Some(LocalModelKind::Standard);
                ((), true)
            })
            .unwrap();
        assert!(matches!(
            load_intent.as_ref().map(|intent| intent.target()),
            Some(crate::transcription::local::LocalRuntimeTarget::Load(_))
        ));
        std::fs::remove_file(&path).unwrap();

        let (settings, cleared, intent) = state
            .update_settings_with_local_runtime_result(|settings| {
                (clear_deleted_local_selection(settings, &path), false)
            })
            .unwrap();

        assert!(cleared);
        assert_eq!(settings.local_model_path, "");
        assert_eq!(settings.local_model_kind, None);
        assert!(matches!(
            intent.as_ref().map(|intent| intent.target()),
            Some(crate::transcription::local::LocalRuntimeTarget::Unloaded)
        ));
    }
}
