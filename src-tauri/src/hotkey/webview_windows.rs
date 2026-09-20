//! Focused Settings WebView2 keyboard ingress for Windows.

use std::sync::Arc;

use parking_lot::Mutex;
use tauri::{AppHandle, Manager};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN, COREWEBVIEW2_KEY_EVENT_KIND_KEY_UP,
    COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN, COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_UP,
    COREWEBVIEW2_PHYSICAL_KEY_STATUS,
};
use webview2_com::{AcceleratorKeyPressedEventHandler, FocusChangedEventHandler};
use winapi::um::winuser::{GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT};

use super::combo::{MODBIT_ALT, MODBIT_CTRL, MODBIT_SHIFT, MODBIT_SUPER};
use super::engine::HotkeyEngine;

fn key_is_down(vk: i32) -> bool {
    unsafe { GetAsyncKeyState(vk) < 0 }
}

fn current_modifiers() -> u8 {
    (if key_is_down(VK_CONTROL) {
        MODBIT_CTRL
    } else {
        0
    }) | (if key_is_down(VK_MENU) { MODBIT_ALT } else { 0 })
        | (if key_is_down(VK_SHIFT) {
            MODBIT_SHIFT
        } else {
            0
        })
        | (if key_is_down(VK_LWIN) || key_is_down(VK_RWIN) {
            MODBIT_SUPER
        } else {
            0
        })
}

pub(super) fn install(app: &AppHandle) -> Result<(), String> {
    let settings = app
        .get_webview_window("onboarding")
        .ok_or_else(|| "Settings WebView is unavailable".to_string())?;
    let registration = Arc::new(Mutex::new(None));
    let registration_result = registration.clone();
    let accelerator_app = app.clone();
    let lost_focus_app = app.clone();

    settings
        .with_webview(move |webview| {
            let controller = webview.controller();
            let accelerator =
                AcceleratorKeyPressedEventHandler::create(Box::new(move |_sender, args| {
                    let Some(args) = args else {
                        return Ok(());
                    };
                    let mut kind = Default::default();
                    let mut vk = 0;
                    let mut status = COREWEBVIEW2_PHYSICAL_KEY_STATUS::default();
                    unsafe {
                        args.KeyEventKind(&mut kind)?;
                        args.VirtualKey(&mut vk)?;
                        args.PhysicalKeyStatus(&mut status)?;
                    }
                    let pressed = match kind {
                        COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
                        | COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN => true,
                        COREWEBVIEW2_KEY_EVENT_KIND_KEY_UP
                        | COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_UP => false,
                        _ => return Ok(()),
                    };
                    let mut modifiers = current_modifiers();
                    let modifier = super::windows::modifier_bit(vk);
                    if modifier != 0 {
                        if pressed {
                            modifiers |= modifier;
                        } else {
                            modifiers &= !modifier;
                        }
                    }
                    let handled = accelerator_app
                        .state::<HotkeyEngine>()
                        .focused_windows_key_event(
                            vk,
                            modifiers,
                            pressed,
                            pressed && status.WasKeyDown.as_bool(),
                        );
                    if handled {
                        unsafe { args.SetHandled(true)? };
                    }
                    Ok(())
                }));
            let lost_focus = FocusChangedEventHandler::create(Box::new(move |_sender, _args| {
                let engine = lost_focus_app.state::<HotkeyEngine>();
                engine.focused_input_lost();
                engine.set_hotkey_capture_active(false);
                Ok(())
            }));
            let result = (|| {
                let mut accelerator_token = 0;
                let mut lost_focus_token = 0;
                unsafe {
                    controller
                        .add_AcceleratorKeyPressed(&accelerator, &mut accelerator_token)
                        .map_err(|error| error.to_string())?;
                    controller
                        .add_LostFocus(&lost_focus, &mut lost_focus_token)
                        .map_err(|error| error.to_string())?;
                }
                Ok(())
            })();
            *registration_result.lock() = Some(result);
        })
        .map_err(|error| error.to_string())?;

    let result = registration
        .lock()
        .take()
        .unwrap_or_else(|| Err("Settings WebView callback registration did not run".to_string()));
    result
}
