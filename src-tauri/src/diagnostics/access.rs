use std::time::Duration;

use crate::output::{self, DeliveryCapability, DeliveryMethod};

use super::process_probe::{run_self_probe, InternalProbe};

use super::types::{
    DiagnosticGroupStatus, DiagnosticsStorageData, TranscriptionAccessDiagnostics,
    TranscriptionFunction, TranscriptionType,
};

pub(super) const CLIPBOARD_VALIDATION_TIMEOUT: Duration = Duration::from_secs(3);

pub(super) fn test_transcription_access() -> TranscriptionAccessDiagnostics {
    let method = output::detect_delivery_method();
    classify_transcription_access(method, || {
        run_self_probe(InternalProbe::Clipboard, CLIPBOARD_VALIDATION_TIMEOUT)
    })
}

pub(super) fn classify_transcription_access(
    method: DeliveryMethod,
    clipboard_probe: impl FnOnce() -> bool,
) -> TranscriptionAccessDiagnostics {
    match method {
        DeliveryMethod::SendInput => {
            access_pass(TranscriptionFunction::SendInput, TranscriptionType::Cursor)
        }
        DeliveryMethod::Xdotool => {
            access_pass(TranscriptionFunction::Xdotool, TranscriptionType::Cursor)
        }
        DeliveryMethod::Clipboard => {
            if clipboard_probe() {
                access_pass(TranscriptionFunction::System, TranscriptionType::Clipboard)
            } else {
                access_fail()
            }
        }
    }
}

pub fn delivery_capability_from_snapshot(data: &DiagnosticsStorageData) -> DeliveryCapability {
    match data.snapshot.as_ref().map(|snapshot| {
        (
            snapshot.transcription_access.function,
            snapshot.transcription_access.access_type,
        )
    }) {
        Some((TranscriptionFunction::SendInput, TranscriptionType::Cursor))
        | Some((TranscriptionFunction::Xdotool, TranscriptionType::Cursor)) => {
            DeliveryCapability::Inject
        }
        Some((TranscriptionFunction::System, TranscriptionType::Clipboard))
        | Some((TranscriptionFunction::NoneNotValid, TranscriptionType::NoneNotValid))
        | None => DeliveryCapability::Clipboard,
        Some(_) => DeliveryCapability::Clipboard,
    }
}

pub(super) fn access_pass(
    function: TranscriptionFunction,
    access_type: TranscriptionType,
) -> TranscriptionAccessDiagnostics {
    TranscriptionAccessDiagnostics {
        passed: DiagnosticGroupStatus::Pass,
        function,
        access_type,
    }
}

pub(super) fn access_fail() -> TranscriptionAccessDiagnostics {
    TranscriptionAccessDiagnostics {
        passed: DiagnosticGroupStatus::Fail,
        function: TranscriptionFunction::NoneNotValid,
        access_type: TranscriptionType::NoneNotValid,
    }
}
