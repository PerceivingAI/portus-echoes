//! Linux Transcript Delivery Transport (`docs/DELIVERY.md`).
//!
//! Delivers transcribed text by staging the transcript to the system clipboard
//! and triggering paste into the window under the mouse pointer:
//! 1. Stages `text` onto the system clipboard.
//! 2. Resolves the mouse pointer position and activates/focuses the window under the pointer.
//! 3. Triggers the paste event (`ctrl+v`) into the focused window.
//!
//! Rejects Wayland defensively (Wayland uses Clipboard delivery).

use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tauri::AppHandle;

/// Shared selector-owned deadline for the Linux X11 availability probe.
pub(crate) const XDOTOOL_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const XDOTOOL_POLL_INTERVAL: Duration = Duration::from_millis(10);
const XDOTOOL_PASTE_TIMEOUT: Duration = Duration::from_secs(3);

trait ManagedChild {
    fn try_exited_success(&mut self) -> Result<Option<bool>, ()>;
    fn kill(&mut self) -> Result<(), ()>;
    fn wait(&mut self) -> Result<(), ()>;
}

impl ManagedChild for Child {
    fn try_exited_success(&mut self) -> Result<Option<bool>, ()> {
        match self.try_wait() {
            Ok(Some(status)) => Ok(Some(status.success())),
            Ok(None) => Ok(None),
            Err(_) => Err(()),
        }
    }

    fn kill(&mut self) -> Result<(), ()> {
        self.kill().map_err(|_| ())
    }

    fn wait(&mut self) -> Result<(), ()> {
        self.wait().map(|_| ()).map_err(|_| ())
    }
}

fn terminate_and_reap(child: &mut impl ManagedChild) {
    let _ = child.kill();
    let _ = child.wait();
}

fn child_succeeds_with_timeout(child: &mut impl ManagedChild, timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    loop {
        match child.try_exited_success() {
            Ok(Some(true)) => return true,
            Ok(Some(false)) | Err(()) => {
                terminate_and_reap(child);
                return false;
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    terminate_and_reap(child);
                    return false;
                }
                std::thread::sleep(XDOTOOL_POLL_INTERVAL);
            }
        }
    }
}

fn spawned_child_succeeds_with_timeout<C: ManagedChild>(
    spawn: impl FnOnce() -> Result<C, ()>,
    timeout: Duration,
) -> bool {
    let mut child = match spawn() {
        Ok(child) => child,
        Err(()) => return false,
    };
    child_succeeds_with_timeout(&mut child, timeout)
}

pub fn xdotool_available() -> bool {
    spawned_child_succeeds_with_timeout(
        || {
            Command::new("xdotool")
                .arg("--version")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|_| ())
        },
        XDOTOOL_PROBE_TIMEOUT,
    )
}

fn xdotool_paste_args() -> [&'static str; 8] {
    [
        "getmouselocation",
        "windowactivate",
        "--sync",
        "windowfocus",
        "--sync",
        "key",
        "--clearmodifiers",
        "ctrl+v",
    ]
}

/// Deliver `text` directly by copying to clipboard, focusing the window
/// under the pointer, and pasting.
pub fn inject_text(_app: &AppHandle, text: &str) -> Result<(), ()> {
    if text.is_empty() {
        return Ok(());
    }

    if crate::hotkey::wayland_detected() {
        return Err(());
    }

    // 1. Stage the transcribed text onto the system clipboard
    crate::output::set_clipboard(text)?;

    // 2. Focus window under pointer and paste
    if spawned_child_succeeds_with_timeout(
        || {
            Command::new("xdotool")
                .args(xdotool_paste_args())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|_| ())
        },
        XDOTOOL_PASTE_TIMEOUT,
    ) {
        Ok(())
    } else {
        Err(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Default)]
    struct FakeChild {
        polls: VecDeque<Result<Option<bool>, ()>>,
        kill_calls: usize,
        wait_calls: usize,
    }

    impl FakeChild {
        fn with_polls(polls: impl IntoIterator<Item = Result<Option<bool>, ()>>) -> Self {
            Self {
                polls: polls.into_iter().collect(),
                ..Self::default()
            }
        }
    }

    impl ManagedChild for FakeChild {
        fn try_exited_success(&mut self) -> Result<Option<bool>, ()> {
            self.polls.pop_front().unwrap_or(Ok(None))
        }

        fn kill(&mut self) -> Result<(), ()> {
            self.kill_calls += 1;
            Ok(())
        }

        fn wait(&mut self) -> Result<(), ()> {
            self.wait_calls += 1;
            Ok(())
        }
    }

    #[test]
    fn xdotool_probe_timeout_policy_is_two_seconds() {
        assert_eq!(XDOTOOL_PROBE_TIMEOUT, Duration::from_secs(2));
    }

    #[test]
    fn pointer_focused_paste_uses_ordered_args() {
        assert_eq!(
            xdotool_paste_args(),
            [
                "getmouselocation",
                "windowactivate",
                "--sync",
                "windowfocus",
                "--sync",
                "key",
                "--clearmodifiers",
                "ctrl+v",
            ]
        );
    }
}
