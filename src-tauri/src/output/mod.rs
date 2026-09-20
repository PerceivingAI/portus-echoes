//! Shared Local/Cloud transcript delivery (`docs/DELIVERY.md`).
//!
//! Shared concrete-method selection chooses SendInput, xdotool, or Clipboard
//! from compile-time OS support and the required Linux runtime environment
//! checks. Startup derives the in-memory capability from that method, and a
//! completed explicit Diagnostics run may replace the capability. Individual
//! transcript successes/failures never change it, and Inject never falls back
//! to Clipboard.

#[cfg(target_os = "windows")]
pub mod inject_windows;

// Compiled on Windows as well so the Linux path is typechecked on every build;
// only invoked on Linux.
#[cfg(any(target_os = "linux", target_os = "windows"))]
#[allow(dead_code)]
pub mod inject_linux;

use tauri::{AppHandle, Emitter};

use crate::models::TranscriptResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryCapability {
    Inject,
    Clipboard,
}

/// Concrete delivery method selected for the current platform/environment.
/// Local and Cloud consume only the derived `DeliveryCapability`; callers that
/// need to report the actual implementation use this closed method domain.
// Some concrete methods are necessarily unconstructed on a given OS build.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeliveryMethod {
    SendInput,
    Xdotool,
    Clipboard,
}

impl DeliveryMethod {
    pub(crate) fn capability(self) -> DeliveryCapability {
        match self {
            Self::SendInput | Self::Xdotool => DeliveryCapability::Inject,
            Self::Clipboard => DeliveryCapability::Clipboard,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryError {
    Inject,
    Clipboard,
}

#[cfg(any(target_os = "windows", test))]
fn windows_delivery_method() -> DeliveryMethod {
    DeliveryMethod::SendInput
}

#[cfg(any(target_os = "linux", test))]
fn linux_delivery_method(
    _wayland: bool,
    _xdotool_available: impl FnOnce() -> bool,
) -> DeliveryMethod {
    // Linux uses Clipboard delivery for both X11 and Wayland for this release.
    // Caret injection parity with Windows is deferred post-launch (LINUX_DELIVERY.md).
    DeliveryMethod::Clipboard
}

#[cfg(any(not(any(target_os = "windows", target_os = "linux")), test))]
fn unsupported_delivery_method() -> DeliveryMethod {
    DeliveryMethod::Clipboard
}

/// Select the concrete delivery implementation for the current build target
/// and runtime environment. Compile-time OS knowledge owns the platform branch;
/// only Linux requires runtime display-session/tool availability checks.
pub(crate) fn detect_delivery_method() -> DeliveryMethod {
    #[cfg(target_os = "windows")]
    {
        return windows_delivery_method();
    }
    #[cfg(target_os = "linux")]
    {
        return linux_delivery_method(
            crate::hotkey::wayland_detected(),
            inject_linux::xdotool_available,
        );
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        unsupported_delivery_method()
    }
}

/// Deliver one transcript via the already-selected application capability.
pub fn deliver(
    app: &AppHandle,
    result: &TranscriptResult,
    capability: DeliveryCapability,
) -> Result<(), DeliveryError> {
    let outcome = deliver_with(
        &result.text,
        capability,
        |text| inject(app, text),
        set_clipboard,
    );
    if outcome.is_ok() && capability == DeliveryCapability::Clipboard {
        let _ = app.emit(
            crate::models::DELIVERY_CLIPBOARD_STATUS_EVENT,
            crate::models::ClipboardStatusPayload {
                status: crate::models::ClipboardDeliveryStatus::Copied,
            },
        );
        crate::app::windows::schedule_overlay_hide(app, std::time::Duration::from_millis(1700));
    }
    outcome
}

/// Core fixed-capability delivery policy with platform operations injected for
/// tests. There is deliberately no method substitution after a failure.
fn deliver_with(
    text: &str,
    capability: DeliveryCapability,
    inject: impl Fn(&str) -> Result<(), ()>,
    set_clipboard: impl Fn(&str) -> Result<(), ()>,
) -> Result<(), DeliveryError> {
    match capability {
        DeliveryCapability::Inject => inject(text).map_err(|_| DeliveryError::Inject),
        DeliveryCapability::Clipboard => set_clipboard(text).map_err(|_| DeliveryError::Clipboard),
    }
}

static PERSISTENT_CLIPBOARD: parking_lot::Mutex<Option<arboard::Clipboard>> =
    parking_lot::Mutex::new(None);

pub(super) fn set_clipboard(text: &str) -> Result<(), ()> {
    let mut guard = PERSISTENT_CLIPBOARD.lock();
    for attempt in 0..5 {
        if guard.is_none() {
            *guard = arboard::Clipboard::new().ok();
        }
        if let Some(cb) = guard.as_mut() {
            match cb.set_text(text) {
                Ok(()) => return Ok(()),
                Err(_) => {
                    *guard = None;
                }
            }
        }
        if attempt < 4 {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    Err(())
}

#[cfg(target_os = "windows")]
pub(crate) fn inject(_app: &AppHandle, text: &str) -> Result<(), ()> {
    inject_windows::inject_text(text)
}

#[cfg(target_os = "linux")]
pub(crate) fn inject(app: &AppHandle, text: &str) -> Result<(), ()> {
    inject_linux::inject_text(app, text)
}
#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn shared_selector_windows_is_sendinput() {
        assert_eq!(windows_delivery_method(), DeliveryMethod::SendInput);
    }

    #[test]
    fn shared_selector_linux_uses_clipboard_on_wayland_and_x11() {
        let probes = AtomicUsize::new(0);
        let method_wayland = linux_delivery_method(true, || {
            probes.fetch_add(1, Ordering::SeqCst);
            true
        });
        assert_eq!(method_wayland, DeliveryMethod::Clipboard);
        assert_eq!(
            probes.load(Ordering::SeqCst),
            0,
            "Wayland must select Clipboard without probing xdotool"
        );

        let method_x11 = linux_delivery_method(false, || {
            probes.fetch_add(1, Ordering::SeqCst);
            true
        });
        assert_eq!(method_x11, DeliveryMethod::Clipboard);
    }
    #[test]
    fn shared_selector_unsupported_uses_clipboard() {
        assert_eq!(unsupported_delivery_method(), DeliveryMethod::Clipboard);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn current_windows_build_detects_sendinput() {
        assert_eq!(detect_delivery_method(), DeliveryMethod::SendInput);
    }

    #[test]
    fn concrete_delivery_method_maps_to_runtime_capability() {
        assert_eq!(
            DeliveryMethod::SendInput.capability(),
            DeliveryCapability::Inject
        );
        assert_eq!(
            DeliveryMethod::Xdotool.capability(),
            DeliveryCapability::Inject
        );
        assert_eq!(
            DeliveryMethod::Clipboard.capability(),
            DeliveryCapability::Clipboard
        );
    }

    struct Spy {
        calls: AtomicUsize,
        last_text: Mutex<Option<String>>,
        fail: bool,
    }

    impl Spy {
        fn new(fail: bool) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                last_text: Mutex::new(None),
                fail,
            }
        }
        fn call(&self, text: &str) -> Result<(), ()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_text.lock() = Some(text.to_string());
            if self.fail {
                Err(())
            } else {
                Ok(())
            }
        }
        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
        fn text(&self) -> Option<String> {
            self.last_text.lock().clone()
        }
    }

    #[test]
    fn inject_success_invokes_inject_only() {
        let inject = Spy::new(false);
        let clipboard = Spy::new(false);
        deliver_with(
            "hello",
            DeliveryCapability::Inject,
            |text| inject.call(text),
            |text| clipboard.call(text),
        )
        .unwrap();
        assert_eq!(inject.calls(), 1);
        assert_eq!(inject.text().as_deref(), Some("hello"));
        assert_eq!(clipboard.calls(), 0);
    }

    #[test]
    fn inject_failure_is_delivery_failure_without_clipboard_fallback() {
        let inject = Spy::new(true);
        let clipboard = Spy::new(false);
        assert_eq!(
            deliver_with(
                "not-rescued",
                DeliveryCapability::Inject,
                |text| inject.call(text),
                |text| clipboard.call(text),
            ),
            Err(DeliveryError::Inject)
        );
        assert_eq!(inject.calls(), 1);
        assert_eq!(
            clipboard.calls(),
            0,
            "Inject failure must not substitute Clipboard"
        );
    }

    #[test]
    fn subsequent_transcript_keeps_selected_inject_after_failure() {
        let attempts = AtomicUsize::new(0);
        let clipboard = Spy::new(false);
        let inject = |_: &str| {
            let call = attempts.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                Err(())
            } else {
                Ok(())
            }
        };

        assert_eq!(
            deliver_with("first", DeliveryCapability::Inject, &inject, |text| {
                clipboard.call(text)
            }),
            Err(DeliveryError::Inject)
        );
        deliver_with("second", DeliveryCapability::Inject, &inject, |text| {
            clipboard.call(text)
        })
        .unwrap();
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(clipboard.calls(), 0);
    }

    #[test]
    fn clipboard_mode_never_injects() {
        let inject = Spy::new(false);
        let clipboard = Spy::new(false);
        deliver_with(
            "copied",
            DeliveryCapability::Clipboard,
            |text| inject.call(text),
            |text| clipboard.call(text),
        )
        .unwrap();
        assert_eq!(inject.calls(), 0);
        assert_eq!(clipboard.calls(), 1);
        assert_eq!(clipboard.text().as_deref(), Some("copied"));
    }

    #[test]
    fn clipboard_failure_is_delivery_failure() {
        let inject = Spy::new(false);
        let clipboard = Spy::new(true);
        assert_eq!(
            deliver_with(
                "x",
                DeliveryCapability::Clipboard,
                |text| inject.call(text),
                |text| clipboard.call(text),
            ),
            Err(DeliveryError::Clipboard)
        );
        assert_eq!(inject.calls(), 0);
        assert_eq!(clipboard.calls(), 1);
    }

    #[test]
    #[ignore = "overwrites the real clipboard"]
    fn clipboard_roundtrip_matches_exactly() {
        let text = "PortusEchoes clipboard ✓ — exact match, é🎹";
        set_clipboard(text).unwrap();
        let mut clipboard = arboard::Clipboard::new().unwrap();
        let got = clipboard.get_text().unwrap();
        assert_eq!(got, text);
    }

    #[test]
    #[ignore = "overwrites the real clipboard"]
    fn clipboard_handles_sequential_updates() {
        set_clipboard("first transcript").unwrap();
        let mut clipboard = arboard::Clipboard::new().unwrap();
        assert_eq!(clipboard.get_text().unwrap(), "first transcript");

        set_clipboard("second transcript").unwrap();
        assert_eq!(clipboard.get_text().unwrap(), "second transcript");
    }
}
