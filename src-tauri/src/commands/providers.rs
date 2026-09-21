use std::sync::Arc;

use tauri::{AppHandle, State};

use crate::app::{local_models, providers as provider_service};
use crate::app_state::AppState;
use crate::cloud_catalog;
use crate::model_download::{self, DownloadManager};
use crate::models::{AppSettings, CloudModelKind, LocalModelKind, ProviderId, UserErrorCode};
use crate::settings::ProviderStatus;
use crate::transcription::local::LocalEngine;

use super::CommandResult;

#[tauri::command]
pub fn select_active_provider(
    app: AppHandle,
    state: State<'_, AppState>,
    provider: ProviderId,
) -> CommandResult<AppSettings> {
    provider_service::select_active_provider(&app, &state, provider)
}

#[tauri::command]
pub fn select_openai_model(
    state: State<'_, AppState>,
    model: String,
) -> CommandResult<AppSettings> {
    if !cloud_catalog::is_standard_model_id(ProviderId::Openai, &model) {
        return Err(UserErrorCode::Unexpected);
    }
    state.update_settings(|settings| {
        settings.openai_model = model;
        settings.openai_model_kind = Some(CloudModelKind::Standard);
    })
}

#[tauri::command]
pub fn select_openai_custom_model(state: State<'_, AppState>) -> CommandResult<AppSettings> {
    state.update_settings(|settings| {
        settings.openai_model = settings.openai_custom_model.clone();
        settings.openai_model_kind = Some(CloudModelKind::Custom);
    })
}

#[tauri::command]
pub fn set_openai_custom_model(
    state: State<'_, AppState>,
    model: String,
) -> CommandResult<AppSettings> {
    state.update_settings(|settings| {
        settings.openai_custom_model = model.clone();
        if settings.openai_model_kind == Some(CloudModelKind::Custom) {
            settings.openai_model = model;
        }
    })
}

#[tauri::command]
pub fn select_groq_model(state: State<'_, AppState>, model: String) -> CommandResult<AppSettings> {
    if !cloud_catalog::is_standard_model_id(ProviderId::Groq, &model) {
        return Err(UserErrorCode::Unexpected);
    }
    state.update_settings(|settings| {
        settings.groq_model = model;
        settings.groq_model_kind = Some(CloudModelKind::Standard);
    })
}

#[tauri::command]
pub fn select_groq_custom_model(state: State<'_, AppState>) -> CommandResult<AppSettings> {
    state.update_settings(|settings| {
        settings.groq_model = settings.groq_custom_model.clone();
        settings.groq_model_kind = Some(CloudModelKind::Custom);
    })
}

#[tauri::command]
pub fn set_groq_custom_model(
    state: State<'_, AppState>,
    model: String,
) -> CommandResult<AppSettings> {
    state.update_settings(|settings| {
        settings.groq_custom_model = model.clone();
        if settings.groq_model_kind == Some(CloudModelKind::Custom) {
            settings.groq_model = model;
        }
    })
}

#[tauri::command]
pub fn select_local_model(
    app: AppHandle,
    state: State<'_, AppState>,
    engine: State<'_, LocalEngine>,
    downloads: State<'_, Arc<DownloadManager>>,
    path: String,
) -> CommandResult<AppSettings> {
    let selected_path = std::path::PathBuf::from(&path);
    if !downloads.is_managed_model_path(&selected_path) {
        return Err(UserErrorCode::Unexpected);
    }
    let label = model_download::label_for_download_path(&path)
        .or_else(|| {
            if let Some(first_path) =
                crate::model_download::first_configured_model_path(downloads.models_dir())
            {
                if first_path == selected_path {
                    return crate::model_download::first_configured_model_label();
                }
            }
            None
        })
        .unwrap_or(&path)
        .to_string();
    let (settings, (), intent) = state.update_settings_with_local_runtime_result(|settings| {
        settings.local_model = label;
        settings.local_model_path = path;
        settings.local_model_kind = Some(LocalModelKind::Standard);
        ((), true)
    })?;
    if model_download::pending_selected_deletion_recovery(downloads.models_dir())
        .ok()
        .flatten()
        .as_deref()
        == Some(selected_path.as_path())
    {
        let _ = model_download::clear_selected_deletion_recovery(downloads.models_dir());
    }
    local_models::apply_runtime_intent(Some(&app), intent, &engine);
    Ok(settings)
}

#[tauri::command]
pub fn select_local_custom_model(
    app: AppHandle,
    state: State<'_, AppState>,
    engine: State<'_, LocalEngine>,
) -> CommandResult<AppSettings> {
    let (settings, (), intent) = state.update_settings_with_local_runtime_result(|settings| {
        settings.local_model = settings.local_custom_model_path.clone();
        settings.local_model_path = settings.local_custom_model_path.clone();
        settings.local_model_kind = Some(LocalModelKind::Custom);
        ((), true)
    })?;
    local_models::apply_runtime_intent(Some(&app), intent, &engine);
    Ok(settings)
}

#[tauri::command]
pub fn set_local_custom_model_path(
    app: AppHandle,
    state: State<'_, AppState>,
    engine: State<'_, LocalEngine>,
    path: String,
) -> CommandResult<AppSettings> {
    let (settings, (), intent) = state.update_settings_with_local_runtime_result(|settings| {
        let selected = settings.local_model_kind == Some(LocalModelKind::Custom);
        settings.local_custom_model_path = path.clone();
        if selected {
            settings.local_model = path.clone();
            settings.local_model_path = path;
        }
        ((), selected)
    })?;
    local_models::apply_runtime_intent(Some(&app), intent, &engine);
    Ok(settings)
}

fn mark_onboarding_complete(settings: &mut AppSettings, status: &ProviderStatus) -> bool {
    if !(status.openai_configured || status.groq_configured || status.local_configured) {
        return false;
    }
    settings.onboarding_complete = true;
    true
}

#[tauri::command]
pub fn complete_onboarding(state: State<'_, AppState>) -> CommandResult<AppSettings> {
    let current = state.settings_snapshot();
    if current.onboarding_complete {
        return Ok(current);
    }
    let status = state.provider_status()?;
    if !(status.openai_configured || status.groq_configured || status.local_configured) {
        return Ok(current);
    }
    state.update_settings(|settings| {
        let completed = mark_onboarding_complete(settings, &status);
        debug_assert!(completed);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn onboarding_completion_requires_a_configured_provider() {
        let mut settings = AppSettings {
            openai_custom_model: "user-openai".into(),
            groq_custom_model: "user-groq".into(),
            local_custom_model_path: "user-local".into(),
            ..Default::default()
        };
        let original = settings.clone();
        let status = ProviderStatus {
            openai_configured: false,
            groq_configured: false,
            local_configured: false,
            active_provider: None,
            active_configured: false,
        };

        assert!(!mark_onboarding_complete(&mut settings, &status));
        assert_eq!(settings, original);
    }

    #[test]
    fn onboarding_completion_changes_only_the_completion_flag() {
        let mut settings = AppSettings {
            openai_model: "user-model".into(),
            openai_model_kind: Some(CloudModelKind::Custom),
            openai_custom_model: "user-model".into(),
            groq_custom_model: "preserve-groq".into(),
            local_custom_model_path: "preserve-local".into(),
            ..Default::default()
        };
        let mut expected = settings.clone();
        expected.onboarding_complete = true;
        let status = ProviderStatus {
            openai_configured: true,
            groq_configured: false,
            local_configured: false,
            active_provider: None,
            active_configured: false,
        };

        assert!(mark_onboarding_complete(&mut settings, &status));
        assert_eq!(settings, expected);
    }
}
