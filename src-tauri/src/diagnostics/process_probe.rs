use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::microphone::MicrophoneProbeState;

const INTERNAL_PROBE_FLAG: &str = "--internal-diagnostics-probe";
const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(10);
static PROBE_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InternalProbe {
    Clipboard,
    Microphone,
}

impl InternalProbe {
    fn arg(self) -> &'static str {
        match self {
            Self::Clipboard => "clipboard",
            Self::Microphone => "microphone",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedInternalProbe {
    probe: InternalProbe,
    result_path: Option<PathBuf>,
}

trait ManagedChild {
    /// `Ok(Some(success))` means the child exited and was reaped by the poll.
    fn try_exited_success(&mut self) -> Result<Option<bool>, ()>;
    fn kill(&mut self) -> Result<(), ()>;
    fn wait(&mut self) -> Result<(), ()>;
}

impl ManagedChild for Child {
    fn try_exited_success(&mut self) -> Result<Option<bool>, ()> {
        self.try_wait()
            .map(|status| status.map(|status| status.success()))
            .map_err(|_| ())
    }

    fn kill(&mut self) -> Result<(), ()> {
        Child::kill(self).map_err(|_| ())
    }

    fn wait(&mut self) -> Result<(), ()> {
        Child::wait(self).map(|_| ()).map_err(|_| ())
    }
}

fn terminate_and_reap(child: &mut impl ManagedChild) {
    // The child may have exited between the last poll and kill. wait() is still
    // required so a timed Diagnostics probe never remains alive or unreaped.
    let _ = child.kill();
    let _ = child.wait();
}

fn child_succeeds_with_timeout(child: &mut impl ManagedChild, timeout: Duration) -> bool {
    let started = Instant::now();
    loop {
        match child.try_exited_success() {
            Ok(Some(success)) => return success,
            Ok(None) => {}
            Err(()) => {
                terminate_and_reap(child);
                return false;
            }
        }

        if started.elapsed() >= timeout {
            terminate_and_reap(child);
            return false;
        }
        std::thread::sleep(CHILD_POLL_INTERVAL.min(timeout.saturating_sub(started.elapsed())));
    }
}

fn spawned_child_succeeds_with_timeout<C: ManagedChild>(
    spawn: impl FnOnce() -> Result<C, ()>,
    timeout: Duration,
) -> bool {
    let Ok(mut child) = spawn() else {
        return false;
    };
    child_succeeds_with_timeout(&mut child, timeout)
}

pub(super) fn run_self_probe(probe: InternalProbe, timeout: Duration) -> bool {
    let Ok(current_exe) = std::env::current_exe() else {
        return false;
    };
    spawned_child_succeeds_with_timeout(
        || {
            Command::new(current_exe)
                .arg(INTERNAL_PROBE_FLAG)
                .arg(probe.arg())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|_| ())
        },
        timeout,
    )
}

pub(super) fn run_self_microphone_probe(timeout: Duration) -> MicrophoneProbeState {
    let result_path = unique_probe_result_path();
    let _ = fs::remove_file(&result_path);

    let Some(current_exe) = std::env::current_exe().ok() else {
        return MicrophoneProbeState::default();
    };

    let _completed = spawned_child_succeeds_with_timeout(
        || {
            Command::new(current_exe)
                .arg(INTERNAL_PROBE_FLAG)
                .arg(InternalProbe::Microphone.arg())
                .arg(&result_path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|_| ())
        },
        timeout,
    );

    let result = read_microphone_probe_state(&result_path).unwrap_or_default();
    let _ = fs::remove_file(&result_path);
    result
}

fn unique_probe_result_path() -> PathBuf {
    let counter = PROBE_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "portus-echoes-microphone-probe-{}-{counter}-{nanos}.json",
        std::process::id()
    ))
}

fn read_microphone_probe_state(path: &Path) -> Option<MicrophoneProbeState> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn parse_internal_probe_args(
    args: impl IntoIterator<Item = OsString>,
) -> Option<Result<ParsedInternalProbe, ()>> {
    let mut args = args.into_iter();
    let first = args.next()?;
    if first != INTERNAL_PROBE_FLAG {
        return None;
    }

    let kind = args.next().and_then(|arg| arg.into_string().ok());
    let parsed = match kind.as_deref() {
        Some("clipboard") => {
            if args.next().is_some() {
                return Some(Err(()));
            }
            ParsedInternalProbe {
                probe: InternalProbe::Clipboard,
                result_path: None,
            }
        }
        Some("microphone") => {
            let Some(result_path) = args.next().map(PathBuf::from) else {
                return Some(Err(()));
            };
            if args.next().is_some() {
                return Some(Err(()));
            }
            ParsedInternalProbe {
                probe: InternalProbe::Microphone,
                result_path: Some(result_path),
            }
        }
        _ => return Some(Err(())),
    };
    Some(Ok(parsed))
}

/// Run a minimal internal Diagnostics probe before Tauri/app setup.
///
/// Exit code 0 means the technical probe completed successfully; any non-zero
/// exit means unavailable/failure. The parent Diagnostics process owns timeout
/// classification and, for Microphone, may recover a valid staged Name fact
/// after killing/reaping a child that blocked during config inspection.
pub(crate) fn run_internal_probe_from_process_args() -> Option<i32> {
    let parsed = parse_internal_probe_args(std::env::args_os().skip(1))?;
    let Ok(parsed) = parsed else {
        return Some(2);
    };

    let succeeded = match parsed.probe {
        InternalProbe::Clipboard => arboard::Clipboard::new().is_ok(),
        InternalProbe::Microphone => {
            let Some(result_path) = parsed.result_path.as_deref() else {
                return Some(2);
            };
            super::microphone::run_internal_microphone_probe(result_path)
        }
    };
    Some(if succeeded { 0 } else { 1 })
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
    fn internal_probe_parser_accepts_only_exact_supported_modes() {
        assert_eq!(
            parse_internal_probe_args([
                OsString::from(INTERNAL_PROBE_FLAG),
                OsString::from("clipboard")
            ]),
            Some(Ok(ParsedInternalProbe {
                probe: InternalProbe::Clipboard,
                result_path: None,
            }))
        );
        assert_eq!(
            parse_internal_probe_args([
                OsString::from(INTERNAL_PROBE_FLAG),
                OsString::from("microphone"),
                OsString::from("probe-result.json")
            ]),
            Some(Ok(ParsedInternalProbe {
                probe: InternalProbe::Microphone,
                result_path: Some(PathBuf::from("probe-result.json")),
            }))
        );
        assert_eq!(parse_internal_probe_args([OsString::from("--other")]), None);
        assert_eq!(
            parse_internal_probe_args([
                OsString::from(INTERNAL_PROBE_FLAG),
                OsString::from("unknown")
            ]),
            Some(Err(()))
        );
        assert_eq!(
            parse_internal_probe_args([
                OsString::from(INTERNAL_PROBE_FLAG),
                OsString::from("clipboard"),
                OsString::from("extra")
            ]),
            Some(Err(()))
        );
        assert_eq!(
            parse_internal_probe_args([
                OsString::from(INTERNAL_PROBE_FLAG),
                OsString::from("microphone")
            ]),
            Some(Err(()))
        );
    }

    #[test]
    fn microphone_probe_state_round_trips_name_only_for_config_failure_or_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("microphone.json");
        let expected = MicrophoneProbeState {
            name: Some("USB Mic".to_string()),
            specs: None,
        };
        fs::write(&path, serde_json::to_vec(&expected).unwrap()).unwrap();
        assert_eq!(read_microphone_probe_state(&path), Some(expected));
    }

    #[test]
    fn self_probe_launch_failure_is_unavailable() {
        assert!(!spawned_child_succeeds_with_timeout::<FakeChild>(
            || Err(()),
            Duration::from_secs(1)
        ));
    }

    #[test]
    fn self_probe_success_does_not_run_cleanup() {
        let mut child = FakeChild::with_polls([Ok(Some(true))]);
        assert!(child_succeeds_with_timeout(&mut child, Duration::ZERO));
        assert_eq!(child.kill_calls, 0);
        assert_eq!(child.wait_calls, 0);
    }

    #[test]
    fn self_probe_failure_does_not_run_cleanup_after_reap() {
        let mut child = FakeChild::with_polls([Ok(Some(false))]);
        assert!(!child_succeeds_with_timeout(&mut child, Duration::ZERO));
        assert_eq!(child.kill_calls, 0);
        assert_eq!(child.wait_calls, 0);
    }

    #[test]
    fn self_probe_timeout_kills_and_reaps_before_returning() {
        let mut child = FakeChild::with_polls([Ok(None)]);
        assert!(!child_succeeds_with_timeout(&mut child, Duration::ZERO));
        assert_eq!(child.kill_calls, 1);
        assert_eq!(child.wait_calls, 1);
    }

    #[test]
    fn self_probe_poll_failure_kills_and_reaps_before_returning() {
        let mut child = FakeChild::with_polls([Err(())]);
        assert!(!child_succeeds_with_timeout(
            &mut child,
            Duration::from_secs(1)
        ));
        assert_eq!(child.kill_calls, 1);
        assert_eq!(child.wait_calls, 1);
    }
}
