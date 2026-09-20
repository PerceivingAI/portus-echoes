//! Windows `WH_KEYBOARD_LL` registration and conflict probing.

use std::cell::RefCell;
use std::ptr::null_mut;
use std::sync::mpsc::channel;

use winapi::shared::minwindef::{LPARAM, LRESULT, WPARAM};
use winapi::um::libloaderapi::GetModuleHandleW;
use winapi::um::processthreadsapi::GetCurrentThreadId;
use winapi::um::winuser::*;

use super::combo::{
    ComboState, HotkeyError, ParsedCombo, MODBIT_ALT, MODBIT_CTRL, MODBIT_SHIFT, MODBIT_SUPER,
};
use super::engine::{HotkeyEventSink, PlatformRegistration};

struct HookThreadState {
    trigger_vk: u32,
    state: ComboState,
    sink: HotkeyEventSink,
}

thread_local! {
    static HOOK_STATE: RefCell<Option<HookThreadState>> = const { RefCell::new(None) };
}

// Test-only gate (debug builds): when PORTUS_HOOK_ALLOW_INJECTED is set,
// Portus-owned injected events are processed too, letting SendInput drive the
// hook in the end-to-end test. Normal/release behavior filters Portus-owned
// delivery events by magic while still allowing synthetic events from other tools.
#[cfg(debug_assertions)]
static ALLOW_INJECTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[inline]
fn injected_allowed() -> bool {
    #[cfg(debug_assertions)]
    {
        ALLOW_INJECTED.load(std::sync::atomic::Ordering::Relaxed)
    }
    #[cfg(not(debug_assertions))]
    {
        false
    }
}

#[inline]
fn should_process_hook_event(extra_info: usize) -> bool {
    extra_info != crate::output::inject_windows::PORTUS_INJECT_MAGIC || injected_allowed()
}

pub(super) fn modifier_bit(vk: u32) -> u8 {
    match vk as i32 {
        VK_LCONTROL | VK_RCONTROL => MODBIT_CTRL,
        VK_LMENU | VK_RMENU => MODBIT_ALT,
        VK_LSHIFT | VK_RSHIFT => MODBIT_SHIFT,
        VK_LWIN | VK_RWIN => MODBIT_SUPER,
        _ => 0,
    }
}

pub(super) fn browser_modifier_bit(key: &str) -> u8 {
    match key {
        "Control" => MODBIT_CTRL,
        "Alt" => MODBIT_ALT,
        "Shift" => MODBIT_SHIFT,
        "Meta" => MODBIT_SUPER,
        _ => 0,
    }
}

pub(super) fn browser_key_to_vk(key: &str) -> Option<u32> {
    let canonical = match key {
        " " | "Spacebar" => "space",
        "ArrowUp" => "up",
        "ArrowDown" => "down",
        "ArrowLeft" => "left",
        "ArrowRight" => "right",
        "Escape" => "esc",
        "Control" | "Alt" | "Shift" | "Meta" => return None,
        _ if key.len() == 1 => return key_to_vk(key),
        _ => return key_to_vk(&key.to_ascii_lowercase()),
    };
    key_to_vk(canonical)
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam);
    }
    let kb = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
    // Only drop PortusEchoes' own injected text events. External macro software
    // (AutoHotkey, Stream Deck, etc.) sending synthetic hotkeys are accepted.
    if should_process_hook_event(kb.dwExtraInfo) {
        let vk = kb.vkCode;
        let mod_bit = modifier_bit(vk);
        HOOK_STATE.with(|cell| {
            let mut borrow = cell.borrow_mut();
            if let Some(st) = borrow.as_mut() {
                let event = match wparam as u32 {
                    WM_KEYDOWN | WM_SYSKEYDOWN => {
                        st.state.on_key_down(vk == st.trigger_vk, mod_bit)
                    }
                    WM_KEYUP | WM_SYSKEYUP => st.state.on_key_up(vk == st.trigger_vk, mod_bit),
                    _ => None,
                };
                if let Some(event) = event {
                    st.sink.send(event);
                }
            }
        });
    }
    CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
}

pub fn key_to_vk(key: &str) -> Option<u32> {
    let named: i32 = match key {
        "space" => VK_SPACE,
        "tab" => VK_TAB,
        "enter" | "return" => VK_RETURN,
        "esc" | "escape" => VK_ESCAPE,
        "backspace" => VK_BACK,
        "delete" | "del" => VK_DELETE,
        "insert" | "ins" => VK_INSERT,
        "home" => VK_HOME,
        "end" => VK_END,
        "pageup" | "pgup" => VK_PRIOR,
        "pagedown" | "pgdn" => VK_NEXT,
        "up" => VK_UP,
        "down" => VK_DOWN,
        "left" => VK_LEFT,
        "right" => VK_RIGHT,
        "capslock" => VK_CAPITAL,
        "numlock" => VK_NUMLOCK,
        "scrolllock" => VK_SCROLL,
        "printscreen" => VK_SNAPSHOT,
        "pause" => VK_PAUSE,
        _ => {
            if let Some(f) = key
                .strip_prefix('f')
                .and_then(|n| n.parse::<i32>().ok())
                .filter(|n| (1..=24).contains(n))
            {
                VK_F1 + f - 1
            } else if key.chars().count() == 1 {
                let c = key.as_bytes()[0] as i8;
                let scan = unsafe { VkKeyScanA(c) } as i16;
                if scan == -1 {
                    return None;
                }
                (scan & 0xFF) as i32
            } else {
                return None;
            }
        }
    };
    Some(named as u32)
}

pub fn key_supported(key: &str) -> bool {
    key_to_vk(key).is_some()
}

pub fn conflicts_with_existing(combo: &ParsedCombo) -> Result<bool, HotkeyError> {
    let vk = key_to_vk(&combo.key).ok_or(HotkeyError::UnsupportedKey)?;
    let mut mods = 0u32;
    if combo.ctrl {
        mods |= MOD_CONTROL as u32;
    }
    if combo.alt {
        mods |= MOD_ALT as u32;
    }
    if combo.shift {
        mods |= MOD_SHIFT as u32;
    }
    if combo.super_ {
        mods |= MOD_WIN as u32;
    }
    const PROBE_ID: i32 = 0x4F53;
    // RegisterHotKey fails if another application owns the combo.
    let ok = unsafe { RegisterHotKey(std::ptr::null_mut(), PROBE_ID, mods, vk) };
    if ok == 0 {
        let err = unsafe { winapi::um::errhandlingapi::GetLastError() };
        if err == winapi::shared::winerror::ERROR_HOTKEY_ALREADY_REGISTERED {
            return Ok(true);
        }
        return Err(HotkeyError::PlatformFailure);
    }
    unsafe {
        UnregisterHotKey(std::ptr::null_mut(), PROBE_ID);
    }
    Ok(false)
}

pub fn start(
    combo: ParsedCombo,
    sink: HotkeyEventSink,
) -> Result<PlatformRegistration, HotkeyError> {
    let trigger_vk = key_to_vk(&combo.key).ok_or(HotkeyError::UnsupportedKey)?;
    #[cfg(debug_assertions)]
    ALLOW_INJECTED.store(
        std::env::var_os("PORTUS_HOOK_ALLOW_INJECTED").is_some(),
        std::sync::atomic::Ordering::Relaxed,
    );
    let (ready_tx, ready_rx) = channel::<Result<u32, HotkeyError>>();
    let join = std::thread::spawn(move || unsafe {
        let tid = GetCurrentThreadId();
        HOOK_STATE.with(|cell| {
            *cell.borrow_mut() = Some(HookThreadState {
                trigger_vk,
                state: ComboState::new(&combo),
                sink,
            });
        });
        let hook = SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(hook_proc),
            GetModuleHandleW(null_mut()),
            0,
        );
        if hook.is_null() {
            let _ = ready_tx.send(Err(HotkeyError::RegistrationFailed));
            return;
        }
        let _ = ready_tx.send(Ok(tid));
        let mut msg: MSG = std::mem::zeroed();
        // Returns 0 on WM_QUIT (posted by stop()).
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {}
        UnhookWindowsHookEx(hook);
        HOOK_STATE.with(|cell| *cell.borrow_mut() = None);
    });
    match ready_rx.recv() {
        Ok(Ok(tid)) => Ok(PlatformRegistration::new(
            Box::new(move || unsafe {
                PostThreadMessageW(tid, WM_QUIT, 0, 0);
            }),
            join,
        )),
        Ok(Err(e)) => {
            let _ = join.join();
            Err(e)
        }
        Err(_) => Err(HotkeyError::WorkerStopped),
    }
}

#[cfg(test)]
mod tests {
    use super::super::combo::{parse_combo, ComboState};
    use super::super::engine::HotkeyEngine;
    use super::*;

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_key_table_covers_documented_keys() {
        // A representative sample of keys users can map.
        for key in [
            "space", "tab", "enter", "esc", "f9", "v", "1", "up", "delete",
        ] {
            assert!(
                super::key_to_vk(key).is_some(),
                "{key} must map to a virtual key"
            );
        }
        assert!(super::key_to_vk("definitely-not-a-key").is_none());
    }

    #[test]
    fn browser_adapter_covers_every_settings_key_family() {
        for (configured, browser) in [
            ("space", " "),
            ("tab", "Tab"),
            ("enter", "Enter"),
            ("esc", "Escape"),
            ("backspace", "Backspace"),
            ("delete", "Delete"),
            ("insert", "Insert"),
            ("home", "Home"),
            ("end", "End"),
            ("pageup", "PageUp"),
            ("pagedown", "PageDown"),
            ("up", "ArrowUp"),
            ("down", "ArrowDown"),
            ("left", "ArrowLeft"),
            ("right", "ArrowRight"),
            ("capslock", "CapsLock"),
            ("numlock", "NumLock"),
            ("scrolllock", "ScrollLock"),
            ("printscreen", "PrintScreen"),
            ("pause", "Pause"),
        ] {
            assert_eq!(browser_key_to_vk(browser), key_to_vk(configured));
        }
        for number in 1..=24 {
            let configured = format!("f{number}");
            let browser = format!("F{number}");
            assert_eq!(browser_key_to_vk(&browser), key_to_vk(&configured));
        }
        for byte in b'!'..=b'~' {
            let key = char::from(byte).to_string();
            if let Some(expected) = key_to_vk(&key) {
                assert_eq!(browser_key_to_vk(&key), Some(expected));
            }
        }
    }

    #[test]
    fn focused_browser_matrix_supports_every_key_and_modifier_mask() {
        let verify = |configured: &str, browser: &str| {
            if parse_combo(configured).is_err() {
                return;
            }
            assert_eq!(
                browser_key_to_vk(browser),
                key_to_vk(configured),
                "{browser} must use the configured key's virtual key"
            );
            for mask in 0_u8..16 {
                let mut parts = Vec::with_capacity(5);
                if mask & MODBIT_CTRL != 0 {
                    parts.push("Ctrl");
                }
                if mask & MODBIT_ALT != 0 {
                    parts.push("Alt");
                }
                if mask & MODBIT_SHIFT != 0 {
                    parts.push("Shift");
                }
                if mask & MODBIT_SUPER != 0 {
                    parts.push("Super");
                }
                parts.push(configured);
                let parsed = parse_combo(&parts.join("+")).unwrap();
                let mut state = ComboState::new(&parsed);

                assert_eq!(
                    state.on_key_down_with_mods(true, 0, mask),
                    Some(HotkeyEvent::Pressed)
                );
                assert_eq!(state.on_key_down_with_mods(true, 0, mask), None);
                assert_eq!(
                    state.on_key_up_with_mods(true, 0, mask),
                    Some(HotkeyEvent::Released)
                );
                assert_eq!(state.on_key_up_with_mods(true, 0, mask), None);
            }
        };

        for (configured, browser) in [
            ("space", " "),
            ("tab", "Tab"),
            ("enter", "Enter"),
            ("esc", "Escape"),
            ("backspace", "Backspace"),
            ("delete", "Delete"),
            ("insert", "Insert"),
            ("home", "Home"),
            ("end", "End"),
            ("pageup", "PageUp"),
            ("pagedown", "PageDown"),
            ("up", "ArrowUp"),
            ("down", "ArrowDown"),
            ("left", "ArrowLeft"),
            ("right", "ArrowRight"),
            ("capslock", "CapsLock"),
            ("numlock", "NumLock"),
            ("scrolllock", "ScrollLock"),
            ("printscreen", "PrintScreen"),
            ("pause", "Pause"),
        ] {
            verify(configured, browser);
        }
        for number in 1..=24 {
            let key = format!("f{number}");
            verify(&key, &key.to_ascii_uppercase());
        }
        for byte in b'!'..=b'~' {
            let key = char::from(byte).to_string();
            if key_to_vk(&key).is_some() {
                verify(&key, &key);
            }
        }
    }

    #[test]
    fn browser_modifier_adapter_maps_all_supported_modifiers() {
        assert_eq!(browser_modifier_bit("Control"), MODBIT_CTRL);
        assert_eq!(browser_modifier_bit("Alt"), MODBIT_ALT);
        assert_eq!(browser_modifier_bit("Shift"), MODBIT_SHIFT);
        assert_eq!(browser_modifier_bit("Meta"), MODBIT_SUPER);
        assert_eq!(browser_modifier_bit("F13"), 0);
    }

    #[test]
    fn portus_owned_delivery_magic_is_filtered_without_filtering_external_synthetic_events() {
        #[cfg(debug_assertions)]
        ALLOW_INJECTED.store(false, std::sync::atomic::Ordering::Relaxed);

        assert!(!should_process_hook_event(
            crate::output::inject_windows::PORTUS_INJECT_MAGIC
        ));
        assert!(should_process_hook_event(0));
        assert!(should_process_hook_event(
            crate::output::inject_windows::PORTUS_INJECT_MAGIC.wrapping_add(1)
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn conflict_probe_detects_registerhotkey_ownership() {
        use winapi::um::winuser::{
            RegisterHotKey, UnregisterHotKey, MOD_CONTROL, MOD_SHIFT, VK_F24,
        };

        let combo = parse_combo("Ctrl+Shift+F24").unwrap();
        // Free combo → no conflict.
        assert!(!super::conflicts_with_existing(&combo).unwrap());

        // Claim the combo at the OS level → the probe must see it.
        const ID: i32 = 0x4E44;
        let claimed = unsafe {
            RegisterHotKey(
                std::ptr::null_mut(),
                ID,
                (MOD_CONTROL | MOD_SHIFT) as u32,
                VK_F24 as u32,
            )
        };
        assert_ne!(claimed, 0, "RegisterHotKey claim must succeed");
        assert!(super::conflicts_with_existing(&combo).unwrap());
        unsafe {
            UnregisterHotKey(std::ptr::null_mut(), ID);
        }
        assert!(!super::conflicts_with_existing(&combo).unwrap());
    }

    use crate::models::HotkeyEvent;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    fn send_key(vk: u16, down: bool) {
        let dw_flags = if down { 0 } else { KEYEVENTF_KEYUP };
        let mut input: INPUT = unsafe { std::mem::zeroed() };
        input.type_ = INPUT_KEYBOARD;
        unsafe {
            *input.u.ki_mut() = KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: dw_flags,
                time: 0,
                dwExtraInfo: 0,
            };
            let sent = SendInput(1, &mut input, std::mem::size_of::<INPUT>() as i32);
            assert_eq!(sent, 1, "SendInput must deliver the event");
        }
        // Give the OS a moment to dispatch through the LL hook.
        std::thread::sleep(Duration::from_millis(40));
    }

    fn hold_combo(vk: u16, hold: Duration) {
        send_key(VK_LCONTROL as u16, true);
        send_key(VK_LSHIFT as u16, true);
        send_key(vk, true);
        std::thread::sleep(hold);
        send_key(vk, false);
        send_key(VK_LSHIFT as u16, false);
        send_key(VK_LCONTROL as u16, false);
        std::thread::sleep(Duration::from_millis(150));
    }

    fn recv_pressed(rx: &std::sync::mpsc::Receiver<HotkeyEvent>) {
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            HotkeyEvent::Pressed
        );
    }

    fn recv_released(rx: &std::sync::mpsc::Receiver<HotkeyEvent>) {
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            HotkeyEvent::Released
        );
    }

    fn recv_nothing(rx: &std::sync::mpsc::Receiver<HotkeyEvent>, within: Duration) {
        assert!(
            rx.recv_timeout(within).is_err(),
            "no event expected within {within:?}"
        );
    }

    #[test]
    #[ignore = "drives the system keyboard state via SendInput"]
    fn hook_press_release_reload_and_stop() {
        std::env::set_var("PORTUS_HOOK_ALLOW_INJECTED", "1");
        let (tx, rx) = channel::<HotkeyEvent>();
        let engine = HotkeyEngine::new(tx);

        // 1. Default combo: exactly one Pressed per hold, one Released.
        engine
            .start_windows_for_test(parse_combo("Ctrl+Shift+Space").unwrap())
            .unwrap();
        hold_combo(VK_SPACE as u16, Duration::from_millis(500));
        recv_pressed(&rx);
        recv_released(&rx);
        recv_nothing(&rx, Duration::from_millis(300));

        // 2. Auto-repeat: repeated keydowns while held → still one Pressed.
        send_key(VK_LCONTROL as u16, true);
        send_key(VK_LSHIFT as u16, true);
        for _ in 0..8 {
            send_key(VK_SPACE as u16, true);
        }
        send_key(VK_SPACE as u16, false);
        send_key(VK_LSHIFT as u16, false);
        send_key(VK_LCONTROL as u16, false);
        std::thread::sleep(Duration::from_millis(150));
        recv_pressed(&rx);
        recv_released(&rx);
        recv_nothing(&rx, Duration::from_millis(300));

        // 3. Runtime reload: old combo inert, new combo live, no restart.
        engine.stop();
        engine
            .start_windows_for_test(parse_combo("Ctrl+Shift+F9").unwrap())
            .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        hold_combo(VK_SPACE as u16, Duration::from_millis(200));
        recv_nothing(&rx, Duration::from_millis(400));
        hold_combo(VK_F9 as u16, Duration::from_millis(200));
        recv_pressed(&rx);
        recv_released(&rx);

        // 4. Stop: nothing fires; join in stop() means no thread leak.
        engine.stop();
        hold_combo(VK_F9 as u16, Duration::from_millis(200));
        recv_nothing(&rx, Duration::from_millis(400));
    }
}
