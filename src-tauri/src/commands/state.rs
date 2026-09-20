use tauri::State;

use crate::app::windows::SettingsNavigationOwner;
use crate::app_state::AppState;
use crate::models::{AppSettings, RecordingState, SettingsNavigationState};

/// Read the current settings. Contains no secrets by construction.
#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> AppSettings {
    state.settings_snapshot()
}

/// Read the authoritative recording lifecycle after the frontend listener is
/// registered. The version lets the caller reject a stale command response.
#[tauri::command]
pub fn get_recording_state(state: State<'_, AppState>) -> RecordingState {
    state.recording_state()
}

/// Read the latest in-memory tray navigation request after the frontend event
/// listener is registered. `None` means no tray destination has been requested
/// in this process.
#[tauri::command]
pub fn get_settings_navigation(
    state: State<'_, SettingsNavigationOwner>,
) -> Option<SettingsNavigationState> {
    state.snapshot()
}
