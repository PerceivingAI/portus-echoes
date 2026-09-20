use tauri::State;

use crate::app_state::AppState;
use crate::models::ProviderId;
use crate::settings::ProviderStatus;

use super::CommandResult;

/// Store an API key in the OS credential store. The result reports only
/// whether that mutation committed; provider readiness is a separate read.
#[tauri::command]
pub fn set_api_key(
    state: State<'_, AppState>,
    provider: ProviderId,
    key: String,
) -> CommandResult<()> {
    state.set_api_key(provider, &key)
}

/// Remove an API key. The result reports only whether that mutation committed;
/// provider readiness is a separate read.
#[tauri::command]
pub fn delete_api_key(state: State<'_, AppState>, provider: ProviderId) -> CommandResult<()> {
    state.delete_api_key(provider)
}

/// Recompute provider configuration from current settings + credential state.
#[tauri::command]
pub fn get_provider_status(state: State<'_, AppState>) -> CommandResult<ProviderStatus> {
    state.provider_status()
}

/// Retrieve the API key for the owner-approved Settings editor.
#[tauri::command]
pub fn get_api_key(state: State<'_, AppState>, provider: ProviderId) -> CommandResult<String> {
    state.api_key(provider).map(|key| key.unwrap_or_default())
}
