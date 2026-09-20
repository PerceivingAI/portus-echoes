use tauri::{AppHandle, State};

use crate::app_state::AppState;
use crate::hotkey::{self as hotkey_runtime, HotkeyEngine};
use crate::models::{AppSettings, UserErrorCode};

use super::CommandResult;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusedHotkeyKeyEvent {
    key: String,
    ctrl_key: bool,
    alt_key: bool,
    shift_key: bool,
    meta_key: bool,
    pressed: bool,
    repeat: bool,
}

#[tauri::command]
pub fn set_hotkey(
    app: AppHandle,
    state: State<'_, AppState>,
    engine: State<'_, HotkeyEngine>,
    hotkey: String,
) -> CommandResult<AppSettings> {
    let previous = state.hotkey();
    if hotkey == previous {
        return Ok(state.settings_snapshot());
    }

    let parsed =
        hotkey_runtime::parse_combo(&hotkey).map_err(|_| UserErrorCode::InvalidShortcut)?;
    if !hotkey_runtime::key_supported(&parsed.key) {
        return Err(UserErrorCode::InvalidShortcut);
    }
    let prepared = engine
        .prepare(&app, &hotkey)
        .map_err(|_| UserErrorCode::InvalidShortcut)?;
    let settings = state
        .update_settings(|settings| settings.hotkey = hotkey)
        .map_err(|_| UserErrorCode::InvalidShortcut)?;
    engine.commit_prepared(prepared);
    Ok(settings)
}

#[tauri::command]
pub fn is_hotkey_active(engine: State<'_, HotkeyEngine>) -> bool {
    engine.is_active()
}

#[tauri::command]
pub fn focused_hotkey_key_event(
    engine: State<'_, HotkeyEngine>,
    event: FocusedHotkeyKeyEvent,
) -> bool {
    let modifiers = (if event.ctrl_key {
        hotkey_runtime::MODBIT_CTRL
    } else {
        0
    }) | (if event.alt_key {
        hotkey_runtime::MODBIT_ALT
    } else {
        0
    }) | (if event.shift_key {
        hotkey_runtime::MODBIT_SHIFT
    } else {
        0
    }) | (if event.meta_key {
        hotkey_runtime::MODBIT_SUPER
    } else {
        0
    });
    engine.focused_browser_key_event(&event.key, modifiers, event.pressed, event.repeat)
}

#[tauri::command]
pub fn set_hotkey_capture_active(engine: State<'_, HotkeyEngine>, active: bool) {
    engine.set_hotkey_capture_active(active);
}

#[tauri::command]
pub fn reset_focused_hotkey_input(engine: State<'_, HotkeyEngine>) {
    engine.focused_input_lost();
}

/// Settings-UI conflict check: true when another application owns the combo.
#[tauri::command]
pub fn is_hotkey_registered(combo: String) -> CommandResult<bool> {
    let parsed = hotkey_runtime::parse_combo(&combo).map_err(|_| UserErrorCode::InvalidShortcut)?;
    hotkey_runtime::conflicts_with_existing(&parsed).map_err(|_| UserErrorCode::InvalidShortcut)
}
