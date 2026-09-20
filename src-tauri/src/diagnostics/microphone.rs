use std::fs;
use std::path::Path;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait};
use serde::{Deserialize, Serialize};

use super::process_probe::run_self_microphone_probe;
use super::types::{DiagnosticGroupStatus, MicrophoneDiagnostics};

pub(super) const NONE_NOT_VALID: &str = "None/Not Valid";
pub(super) const MICROPHONE_INSPECTION_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MicrophoneProbeState {
    pub(super) name: Option<String>,
    pub(super) specs: Option<(u32, u16)>,
}

pub(super) fn test_microphone() -> MicrophoneDiagnostics {
    let probe = run_self_microphone_probe(MICROPHONE_INSPECTION_TIMEOUT);
    classify_microphone(probe.name, probe.specs)
}

/// Run the real CPAL inspection inside the minimal internal child process.
///
/// The child persists the valid name before entering default-input-config
/// inspection. If config inspection later fails or is terminated by the parent
/// timeout, the parent can therefore preserve the successful Name fact while
/// failing Specs exactly as the Diagnostics contract requires.
pub(super) fn run_internal_microphone_probe(result_path: &Path) -> bool {
    let Some(device) = cpal::default_host().default_input_device() else {
        return false;
    };

    let name = device
        .description()
        .ok()
        .map(|description| description.name().trim().to_string())
        .filter(|name| !name.is_empty());
    let Some(name) = name else {
        return false;
    };

    let mut state = MicrophoneProbeState {
        name: Some(name),
        specs: None,
    };
    if !persist_probe_state(result_path, &state) {
        return false;
    }

    let Some(specs) = device
        .default_input_config()
        .ok()
        .map(|config| (config.sample_rate(), config.channels()))
    else {
        return false;
    };

    state.specs = Some(specs);
    persist_probe_state(result_path, &state)
}

fn persist_probe_state(path: &Path, state: &MicrophoneProbeState) -> bool {
    serde_json::to_vec(state)
        .ok()
        .and_then(|bytes| fs::write(path, bytes).ok())
        .is_some()
}

pub(super) fn classify_microphone(
    name: Option<String>,
    specs: Option<(u32, u16)>,
) -> MicrophoneDiagnostics {
    let Some(name) = name.filter(|name| !name.trim().is_empty()) else {
        return MicrophoneDiagnostics {
            passed: DiagnosticGroupStatus::Fail,
            name: NONE_NOT_VALID.to_string(),
            specs: NONE_NOT_VALID.to_string(),
        };
    };

    match specs.filter(|(sample_rate, channels)| *sample_rate > 0 && *channels > 0) {
        Some((sample_rate, channels)) => MicrophoneDiagnostics {
            passed: DiagnosticGroupStatus::Pass,
            name,
            specs: format!("{sample_rate} Hz, {channels} Channels"),
        },
        None => MicrophoneDiagnostics {
            passed: DiagnosticGroupStatus::Fail,
            name,
            specs: NONE_NOT_VALID.to_string(),
        },
    }
}
