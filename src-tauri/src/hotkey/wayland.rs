//! Wayland best-effort Tauri global-shortcut path.

use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

use crate::models::HotkeyEvent;

use super::combo::{HotkeyError, ParsedCombo};
use super::engine::{HotkeyEventSink, PlatformRegistration};

fn shortcut_string(combo: &ParsedCombo) -> String {
    format!(
        "{}{}{}{}{}",
        if combo.ctrl { "Ctrl+" } else { "" },
        if combo.alt { "Alt+" } else { "" },
        if combo.shift { "Shift+" } else { "" },
        if combo.super_ { "Super+" } else { "" },
        combo.key,
    )
}

fn hotkey_event(state: ShortcutState) -> HotkeyEvent {
    match state {
        ShortcutState::Pressed => HotkeyEvent::Pressed,
        ShortcutState::Released => HotkeyEvent::Released,
    }
}

/// Forward the Tauri global-shortcut press/release lifecycle directly into
/// the shared PortusEchoes hotkey event channel. Wayland does not invent an
/// alternate press-on/press-off interaction.
pub fn start(
    app: &tauri::AppHandle,
    combo: ParsedCombo,
    sink: HotkeyEventSink,
) -> Result<PlatformRegistration, HotkeyError> {
    let combo_str = shortcut_string(&combo);
    app.global_shortcut()
        .on_shortcut(combo_str.as_str(), move |_app, _shortcut, event| {
            sink.send(hotkey_event(event.state()));
        })
        .map_err(|_| HotkeyError::RegistrationFailed)?;
    let combo_unregister = combo_str.clone();
    let app_handle = app.clone();
    Ok(PlatformRegistration::new(
        Box::new(move || {
            let _ = app_handle
                .global_shortcut()
                .unregister(combo_unregister.as_str());
        }),
        // No thread to join for the plugin registration; use a completed
        // handle so the registration shape stays uniform.
        std::thread::spawn(|| {}),
    ))
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use tauri_plugin_global_shortcut::Shortcut;

    use super::*;

    #[test]
    fn tauri_shortcut_states_map_directly_to_shared_hotkey_events() {
        assert_eq!(hotkey_event(ShortcutState::Pressed), HotkeyEvent::Pressed);
        assert_eq!(hotkey_event(ShortcutState::Released), HotkeyEvent::Released);
    }

    #[test]
    fn shortcut_string_preserves_every_configured_modifier() {
        let combo = ParsedCombo {
            ctrl: true,
            alt: true,
            shift: true,
            super_: true,
            key: "space".to_string(),
        };
        let shortcut = shortcut_string(&combo);
        assert_eq!(shortcut, "Ctrl+Alt+Shift+Super+space");
        assert!(Shortcut::from_str(&shortcut).is_ok());
    }
}
