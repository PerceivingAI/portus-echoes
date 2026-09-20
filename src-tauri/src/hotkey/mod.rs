//! Platform-native hold-to-talk detection.
//!
//! Shared combo semantics are platform-neutral; Windows, X11, and Wayland
//! registrations live in separate implementation modules. See `docs/HOTKEY.md`.

mod combo;
mod engine;
#[cfg(any(target_os = "linux", target_os = "windows"))]
#[allow(dead_code)]
mod wayland;
#[cfg(target_os = "windows")]
mod webview_windows;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "linux")]
mod x11;

pub(crate) use combo::{
    parse_combo, HotkeyError, ParsedCombo, MODBIT_ALT, MODBIT_CTRL, MODBIT_SHIFT, MODBIT_SUPER,
};
pub(crate) use engine::HotkeyEngine;

#[cfg(target_os = "linux")]
pub(crate) fn wayland_detected() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
}

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)]
pub(crate) fn wayland_detected() -> bool {
    false
}

#[cfg(target_os = "windows")]
pub(crate) fn install_focused_webview_bridge(app: &tauri::AppHandle) -> Result<(), String> {
    webview_windows::install(app)
}

pub(crate) fn key_supported(key: &str) -> bool {
    #[cfg(target_os = "windows")]
    {
        return windows::key_supported(key);
    }
    #[cfg(target_os = "linux")]
    {
        return x11::key_to_keysym(key).is_some();
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = key;
        false
    }
}

pub(crate) fn conflicts_with_existing(combo: &ParsedCombo) -> Result<bool, HotkeyError> {
    #[cfg(target_os = "windows")]
    {
        return windows::conflicts_with_existing(combo);
    }
    #[cfg(target_os = "linux")]
    {
        // XGrabKey/Wayland registration itself is the conflict signal on Linux.
        let _ = combo;
        return Ok(false);
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = combo;
        Err(HotkeyError::PlatformFailure)
    }
}

#[cfg(test)]
mod tests {
    use super::combo::{ComboState, MODBIT_ALT, MODBIT_CTRL, MODBIT_SHIFT};
    use super::*;
    use crate::models::HotkeyEvent;

    fn combo(ctrl: bool, alt: bool, shift: bool, super_: bool) -> ParsedCombo {
        ParsedCombo {
            ctrl,
            alt,
            shift,
            super_,
            key: "space".to_string(),
        }
    }

    #[test]
    fn parse_documented_default() {
        let c = parse_combo("Ctrl+Alt+Space").unwrap();
        assert!(c.ctrl && !c.shift && c.alt && !c.super_);
        assert_eq!(c.key, "space");
        assert_eq!(c.required_mask(), MODBIT_CTRL | MODBIT_ALT);
    }

    #[test]
    fn parse_synonyms_and_case() {
        let c = parse_combo("control+META+f9").unwrap();
        assert!(c.ctrl && c.super_);
        assert_eq!(c.key, "f9");
        assert!(parse_combo("Bogus+Space").is_err());
        assert!(parse_combo("Ctrl+Shift").is_err());
        let modifierless = parse_combo("Space").unwrap();
        assert_eq!(modifierless.required_mask(), 0);
        assert_eq!(modifierless.key, "space");
    }

    #[test]
    fn single_hold_emits_one_pressed_one_released() {
        let mut s = ComboState::new(&combo(true, false, false, false));
        // Ctrl down, Space down → Pressed.
        assert_eq!(s.on_key_down(false, MODBIT_CTRL), None);
        assert_eq!(s.on_key_down(true, 0), Some(HotkeyEvent::Pressed));
        // Auto-repeat keydowns while held → nothing more.
        for _ in 0..10 {
            assert_eq!(s.on_key_down(true, 0), None);
        }
        // Space up → Released. Ctrl up → nothing further.
        assert_eq!(s.on_key_up(true, 0), Some(HotkeyEvent::Released));
        assert_eq!(s.on_key_up(false, MODBIT_CTRL), None);
        assert!(!s.is_active());
    }

    #[test]
    fn releasing_a_required_modifier_ends_the_hold() {
        let mut s = ComboState::new(&combo(true, true, false, false));
        s.on_key_down(false, MODBIT_CTRL);
        s.on_key_down(false, MODBIT_ALT);
        assert_eq!(s.on_key_down(true, 0), Some(HotkeyEvent::Pressed));
        // Release Alt while Space still held → Released.
        assert_eq!(s.on_key_up(false, MODBIT_ALT), Some(HotkeyEvent::Released));
        assert_eq!(s.on_key_up(true, 0), None);
    }

    #[test]
    fn extra_modifiers_suppress_the_combo() {
        let mut s = ComboState::new(&combo(true, false, false, false));
        s.on_key_down(false, MODBIT_CTRL);
        s.on_key_down(false, MODBIT_ALT); // extra
        assert_eq!(s.on_key_down(true, 0), None);
        assert!(!s.is_active());
    }

    #[test]
    fn wrong_modifiers_suppress_the_combo() {
        let mut s = ComboState::new(&combo(true, false, true, false));
        s.on_key_down(false, MODBIT_CTRL); // missing Shift
        assert_eq!(s.on_key_down(true, 0), None);
        assert!(!s.is_active());
    }

    #[test]
    fn repeated_holds_each_fire_once() {
        let mut s = ComboState::new(&combo(true, false, false, false));
        for _ in 0..3 {
            s.on_key_down(false, MODBIT_CTRL);
            assert_eq!(s.on_key_down(true, 0), Some(HotkeyEvent::Pressed));
            assert_eq!(s.on_key_up(true, 0), Some(HotkeyEvent::Released));
            assert_eq!(s.on_key_up(false, MODBIT_CTRL), None);
        }
    }

    #[test]
    fn grabbed_style_events_match_x11_semantics() {
        let mut s = ComboState::new(&combo(true, false, true, false));
        assert_eq!(s.on_grabbed_press(MODBIT_CTRL), None); // missing Shift
        assert_eq!(
            s.on_grabbed_press(MODBIT_CTRL | MODBIT_SHIFT),
            Some(HotkeyEvent::Pressed)
        );
        // Repeat press (X11 repeat path already consumed) → no re-fire.
        assert_eq!(s.on_grabbed_press(MODBIT_CTRL | MODBIT_SHIFT), None);
        assert_eq!(s.on_grabbed_release(), Some(HotkeyEvent::Released));
        assert_eq!(s.on_grabbed_release(), None);
    }
}
