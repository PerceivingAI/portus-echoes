//! Exact Windows Caret Inject transport (`docs/DELIVERY.md`).
//! When the system supports Injection, text is injected directly into the focused window/control
//! under the pointer at the active caret position via native Win32 `SendInput(KEYEVENTF_UNICODE)`.
//! Injection is strictly isolated from Clipboard delivery; if injection fails, it fails closed.

use winapi::um::winuser::*;

/// Magic signature retained for compatibility and hotkey loop isolation.
pub const PORTUS_INJECT_MAGIC: usize = 0x504F5254;

unsafe fn focus_window_under_pointer() -> Result<(), ()> {
    let mut point: winapi::shared::windef::POINT = std::mem::zeroed();
    if GetCursorPos(&mut point) == 0 {
        return Err(());
    }

    let pointed = WindowFromPoint(point);
    if pointed.is_null() || IsWindow(pointed) == 0 {
        return Err(());
    }

    let root = GetAncestor(pointed, GA_ROOT);
    if root.is_null() || IsWindow(root) == 0 {
        return Err(());
    }

    if GetForegroundWindow() != root {
        SetForegroundWindow(root);
    }

    try_focus_element_via_uia(point);

    Ok(())
}

fn try_focus_element_via_uia(pt: winapi::shared::windef::POINT) {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_MULTITHREADED,
        );

        let uia: Result<windows::Win32::UI::Accessibility::IUIAutomation, _> =
            windows::Win32::System::Com::CoCreateInstance(
                &windows::Win32::UI::Accessibility::CUIAutomation,
                None,
                windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
            );

        if let Ok(automation) = uia {
            let point = windows::Win32::Foundation::POINT { x: pt.x, y: pt.y };
            if let Ok(element) = automation.ElementFromPoint(point) {
                if let Ok(control_type) = element.CurrentControlType() {
                    use windows::Win32::UI::Accessibility::*;
                    if control_type == UIA_EditControlTypeId
                        || control_type == UIA_DocumentControlTypeId
                        || control_type == UIA_ComboBoxControlTypeId
                        || control_type == UIA_CustomControlTypeId
                    {
                        let _ = element.SetFocus();
                    }
                }
            }
        }
    }
}

/// Inject `text` at the active caret position of the window under the system pointer.
/// This path is strictly isolated from Clipboard delivery and never writes to the clipboard.
pub fn inject_text(text: &str) -> Result<(), ()> {
    if text.is_empty() {
        return Ok(());
    }

    unsafe {
        let _ = focus_window_under_pointer();
        let utf16_units: Vec<u16> = text.encode_utf16().collect();
        let mut inputs: Vec<INPUT> = Vec::with_capacity(utf16_units.len() * 2);

        for &code_unit in &utf16_units {
            // Key down
            let mut down: INPUT = std::mem::zeroed();
            down.type_ = INPUT_KEYBOARD;
            *down.u.ki_mut() = KEYBDINPUT {
                wVk: 0,
                wScan: code_unit,
                dwFlags: KEYEVENTF_UNICODE,
                time: 0,
                dwExtraInfo: PORTUS_INJECT_MAGIC,
            };
            inputs.push(down);

            // Key up
            let mut up: INPUT = std::mem::zeroed();
            up.type_ = INPUT_KEYBOARD;
            *up.u.ki_mut() = KEYBDINPUT {
                wVk: 0,
                wScan: code_unit,
                dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                time: 0,
                dwExtraInfo: PORTUS_INJECT_MAGIC,
            };
            inputs.push(up);
        }

        let sent = SendInput(
            inputs.len() as u32,
            inputs.as_mut_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        );

        if sent == inputs.len() as u32 {
            Ok(())
        } else {
            Err(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_signature_matches_expected_port() {
        assert_eq!(PORTUS_INJECT_MAGIC, 0x504F5254);
    }

    #[test]
    fn empty_text_returns_ok_without_input_injection() {
        assert_eq!(inject_text(""), Ok(()));
    }
}
