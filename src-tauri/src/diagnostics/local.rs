use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::models::AppSettings;
use crate::transcription::local::start_isolated_session;

use super::types::{DiagnosticGroupStatus, LocalDiagnostics, ProviderValidation};

const DIAGNOSTIC_SPEECH_WAV: &[u8] = include_bytes!("../../assets/diagnostics_speech.wav");

pub(super) fn resolve_local_model_path(settings: &AppSettings) -> Option<PathBuf> {
    let trimmed = settings.local_model_path.trim();
    (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
}

pub(super) fn test_local_provider_path(
    model_path: Option<PathBuf>,
    vad_model_path: Option<PathBuf>,
) -> LocalDiagnostics {
    let Some(model_path) = model_path else {
        return local_fail_unreachable();
    };

    if fs::File::open(&model_path).is_err() {
        return local_fail_unreachable();
    }
    let inference_passed = vad_model_path
        .as_deref()
        .is_some_and(|vad_model_path| diagnostics_local_inference(&model_path, vad_model_path));

    LocalDiagnostics {
        passed: if inference_passed {
            DiagnosticGroupStatus::Pass
        } else {
            DiagnosticGroupStatus::Fail
        },
        model_path: ProviderValidation::Validated,
        model_file: if inference_passed {
            ProviderValidation::Validated
        } else {
            ProviderValidation::NoneNotValid
        },
    }
}

pub(super) fn local_fail_unreachable() -> LocalDiagnostics {
    LocalDiagnostics {
        passed: DiagnosticGroupStatus::Fail,
        model_path: ProviderValidation::NoneNotValid,
        model_file: ProviderValidation::NoneNotValid,
    }
}

fn diagnostics_local_inference(model_path: &Path, vad_model_path: &Path) -> bool {
    let Some((audio, sample_rate)) = diagnostic_speech_input() else {
        return false;
    };

    let transcript = Arc::new(Mutex::new(String::new()));
    let collected = Arc::clone(&transcript);
    let Ok(mut session) = start_isolated_session(
        model_path,
        Some("en"),
        sample_rate,
        vad_model_path,
        Arc::new(move |text| {
            collected
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push_str(&text);
            Ok(())
        }),
    ) else {
        return false;
    };

    if session.feed_audio(&audio).is_err() || session.finalize().is_err() {
        return false;
    }

    let has_text = !transcript
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .trim()
        .is_empty();
    has_text
}

pub(super) fn diagnostic_speech_input() -> Option<(Vec<f32>, u32)> {
    let mut reader = hound::WavReader::new(Cursor::new(DIAGNOSTIC_SPEECH_WAV)).ok()?;
    let spec = reader.spec();
    if spec.channels != 1
        || spec.bits_per_sample != 16
        || spec.sample_format != hound::SampleFormat::Int
        || spec.sample_rate == 0
    {
        return None;
    }
    let samples = reader
        .samples::<i16>()
        .map(|sample| sample.ok().map(|value| value as f32 / 32768.0))
        .collect::<Option<Vec<_>>>()?;
    Some((samples, spec.sample_rate))
}
