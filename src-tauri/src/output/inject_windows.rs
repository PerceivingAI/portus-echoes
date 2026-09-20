//! Exact Windows Caret Inject transport (`docs/DELIVERY.md`).
//! When the system supports Injection, text is injected directly into the focused window/control
//! under the pointer at the active caret position via native Win32 `EM_REPLACESEL` / `WM_CHAR` messages.
//! Injection is strictly isolated from Clipboard delivery; if injection fails, it fails closed.
use winapi::shared::minwindef::{LPARAM, WPARAM};
use winapi::shared::windef::HWND;
use winapi::um::winuser::*;

/// Magic signature retained for compatibility.
pub const PORTUS_INJECT_MAGIC: usize = 0x504F5254;

struct ThreadInputAttachment {
    source: u32,
    target: u32,
}

impl ThreadInputAttachment {
    unsafe fn attach(source: u32, target: u32) -> Result<Option<Self>, ()> {
        if target == 0 || source == target {
            return Ok(None);
        }
        if AttachThreadInput(source, target, 1) == 0 {
            return Err(());
        }
        Ok(Some(Self { source, target }))
    }
}

impl Drop for ThreadInputAttachment {
    fn drop(&mut self) {
        unsafe {
            AttachThreadInput(self.source, self.target, 0);
        }
    }
}

struct FocusedTarget {
    hwnd: HWND,
    _foreground_attachment: Option<ThreadInputAttachment>,
    _target_attachment: Option<ThreadInputAttachment>,
}

unsafe fn focus_window_under_pointer() -> Result<FocusedTarget, ()> {
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

    let target_thread = GetWindowThreadProcessId(root, std::ptr::null_mut());
    if target_thread == 0 {
        return Err(());
    }

    let current_thread = winapi::um::processthreadsapi::GetCurrentThreadId();

    // Ensure this delivery worker has a USER message queue before attaching it.
    let mut message: MSG = std::mem::zeroed();
    PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_NOREMOVE);

    let foreground = GetForegroundWindow();
    let foreground_thread = if foreground.is_null() {
        0
    } else {
        GetWindowThreadProcessId(foreground, std::ptr::null_mut())
    };

    let foreground_attachment =
        ThreadInputAttachment::attach(current_thread, foreground_thread)?;
    let target_attachment = if target_thread == foreground_thread {
        None
    } else {
        ThreadInputAttachment::attach(current_thread, target_thread)?
    };

    if GetForegroundWindow() != root {
        SetForegroundWindow(root);
    }

    let current_focus = focused_window_for_root(root, target_thread).ok();
    let preferred_focus = (pointed != root).then_some(pointed).unwrap_or(root);
    if current_focus.is_none() || (current_focus == Some(root) && preferred_focus != root) {
        SetFocus(preferred_focus);
    }

    let target = focused_window_for_root(root, target_thread).unwrap_or(pointed);
    Ok(FocusedTarget {
        hwnd: target,
        _foreground_attachment: foreground_attachment,
        _target_attachment: target_attachment,
    })
}

unsafe fn focused_window_for_root(
    root: HWND,
    target_thread: u32,
) -> Result<HWND, ()> {
    let mut gui_info: GUITHREADINFO = std::mem::zeroed();
    gui_info.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
    if GetGUIThreadInfo(target_thread, &mut gui_info) == 0
        || gui_info.hwndFocus.is_null()
        || (gui_info.flags & (GUI_INMENUMODE | GUI_INMOVESIZE)) != 0
        || GetAncestor(gui_info.hwndFocus, GA_ROOT) != root
    {
        return Err(());
    }
    Ok(gui_info.hwndFocus)
}

unsafe fn is_edit_control(hwnd: HWND) -> bool {
    let mut class_name = [0u16; 64];
    let len = GetClassNameW(hwnd, class_name.as_mut_ptr(), class_name.len() as i32);
    if len <= 0 {
        return false;
    }
    let name = String::from_utf16_lossy(&class_name[..len as usize]).to_lowercase();
    name.contains("edit")
}

/// Inject `text` at the active caret position of the window under the system pointer.
/// This path is strictly isolated from Clipboard delivery and never writes to the clipboard.
pub fn inject_text(text: &str) -> Result<(), ()> {
    if text.is_empty() {
        return Ok(());
    }

    unsafe {
        let target = focus_window_under_pointer()?;
        let utf16_units: Vec<u16> = text.encode_utf16().collect();

        if is_edit_control(target.hwnd) {
            let wide_null: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            SendMessageW(target.hwnd, EM_REPLACESEL as u32, 1, wide_null.as_ptr() as LPARAM);
        } else {
            for &code_unit in &utf16_units {
                PostMessageW(target.hwnd, WM_CHAR, code_unit as WPARAM, 1);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Once;
    use std::time::{Duration, Instant};
    use winapi::um::libloaderapi::GetModuleHandleW;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    static REGISTER: Once = Once::new();

    unsafe fn create_test_window(x: i32, y: i32) -> (HWND, HWND) {
        let h_instance = GetModuleHandleW(std::ptr::null());
        let class = wide("PEOutputTest");
        REGISTER.call_once(|| {
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(DefWindowProcW);
            wc.hInstance = h_instance;
            wc.lpszClassName = class.as_ptr();
            assert_ne!(RegisterClassW(&wc), 0, "RegisterClassW failed");
        });
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            wide("pe-output-test").as_ptr(),
            WS_OVERLAPPED,
            x,
            y,
            400,
            120,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            h_instance,
            std::ptr::null_mut(),
        );
        assert!(!hwnd.is_null(), "CreateWindowExW (frame) failed");
        let edit = CreateWindowExW(
            0,
            wide("EDIT").as_ptr(),
            std::ptr::null(),
            WS_CHILD | WS_VISIBLE | ES_MULTILINE | ES_AUTOVSCROLL,
            0,
            0,
            400,
            120,
            hwnd,
            std::ptr::null_mut(),
            h_instance,
            std::ptr::null_mut(),
        );
        assert!(!edit.is_null(), "CreateWindowExW (edit) failed");
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);
        (hwnd, edit)
    }

    fn pump_for(duration: Duration) {
        let deadline = Instant::now() + duration;
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while Instant::now() < deadline {
                while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    fn edit_text(edit: HWND) -> String {
        unsafe {
            let len = GetWindowTextLengthW(edit) as usize;
            let mut buf = vec![0u16; len + 1];
            GetWindowTextW(edit, buf.as_mut_ptr(), buf.len() as i32);
            String::from_utf16_lossy(&buf[..len])
        }
    }
    unsafe fn point_inside(window: HWND) -> winapi::shared::windef::POINT {
        let mut point = winapi::shared::windef::POINT { x: 20, y: 20 };
        assert_ne!(ClientToScreen(window, &mut point), 0);
        point
    }

    #[test]
    #[ignore = "drives the system pointer and keyboard focus"]
    fn inject_text_follows_pointer_for_each_delivery() {
        let (first_hwnd, first_edit) = unsafe { create_test_window(80, 80) };
        let (second_hwnd, second_edit) = unsafe { create_test_window(560, 80) };
        pump_for(Duration::from_millis(200));

        unsafe {
            let previous_foreground = GetForegroundWindow();
            let mut previous_pointer = winapi::shared::windef::POINT { x: 0, y: 0 };
            assert_ne!(GetCursorPos(&mut previous_pointer), 0);

            let first_point = point_inside(first_edit);
            assert_ne!(SetCursorPos(first_point.x, first_point.y), 0);
            inject_text("first ").expect("pointer-targeted injection must succeed");
            pump_for(Duration::from_millis(200));

            let second_point = point_inside(second_edit);
            assert_ne!(SetCursorPos(second_point.x, second_point.y), 0);
            inject_text("second").expect("the next injection must resolve the pointer again");
            pump_for(Duration::from_millis(200));

            assert_eq!(edit_text(first_edit), "first ");
            assert_eq!(edit_text(second_edit), "second");

            SetCursorPos(previous_pointer.x, previous_pointer.y);
            if !previous_foreground.is_null() {
                SetForegroundWindow(previous_foreground);
            }
            DestroyWindow(second_hwnd);
            DestroyWindow(first_hwnd);
        }
    }

    #[test]
    fn sequential_chunks_append_at_caret_without_overwriting() {
        unsafe {
            let (hwnd, edit) = create_test_window(100, 100);
            pump_for(Duration::from_millis(50));

            let chunk1 = wide(" First we");
            SendMessageW(edit, EM_REPLACESEL as u32, 1, chunk1.as_ptr() as LPARAM);

            let chunk2 = wide(" went to");
            SendMessageW(edit, EM_REPLACESEL as u32, 1, chunk2.as_ptr() as LPARAM);

            let chunk3 = wide(" the park.");
            SendMessageW(edit, EM_REPLACESEL as u32, 1, chunk3.as_ptr() as LPARAM);

            pump_for(Duration::from_millis(50));
            assert_eq!(edit_text(edit), " First we went to the park.");

            DestroyWindow(edit);
            DestroyWindow(hwnd);
        }
    }

    #[test]
    fn is_edit_control_detects_edit_class() {
        unsafe {
            let (hwnd, edit) = create_test_window(200, 200);
            assert!(is_edit_control(edit));
            assert!(!is_edit_control(hwnd));
            DestroyWindow(edit);
            DestroyWindow(hwnd);
        }
    }

    #[test]
    fn same_thread_focus_requires_no_input_queue_attachment() {
        let attachment = unsafe { ThreadInputAttachment::attach(7, 7) }.unwrap();
        assert!(attachment.is_none());
    }
}
