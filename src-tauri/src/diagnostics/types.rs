use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderValidation {
    Validated,
    NoneNotValid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticGroupStatus {
    Pass,
    Fail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionFunction {
    SendInput,
    Xdotool,
    System,
    NoneNotValid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionType {
    Cursor,
    Clipboard,
    NoneNotValid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalDiagnostics {
    pub passed: DiagnosticGroupStatus,
    pub model_path: ProviderValidation,
    pub model_file: ProviderValidation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloudDiagnostics {
    pub passed: DiagnosticGroupStatus,
    pub openai_api_key: ProviderValidation,
    pub openai_model: ProviderValidation,
    pub groq_api_key: ProviderValidation,
    pub groq_model: ProviderValidation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MicrophoneDiagnostics {
    pub passed: DiagnosticGroupStatus,
    pub name: String,
    pub specs: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TranscriptionAccessDiagnostics {
    pub passed: DiagnosticGroupStatus,
    pub function: TranscriptionFunction,
    pub access_type: TranscriptionType,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticsSnapshotBody {
    pub local: LocalDiagnostics,
    pub cloud: CloudDiagnostics,
    pub microphone: MicrophoneDiagnostics,
    pub transcription_access: TranscriptionAccessDiagnostics,
}

/// Storage/wire payload for Diagnostics. Only the latest completed snapshot is retained.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticsStorageData {
    pub last_scan_display: String,
    pub snapshot: Option<DiagnosticsSnapshotBody>,
}

#[derive(Debug, Clone)]
pub struct CloudProviderRunInput {
    pub key_result: Result<Option<String>, ()>,
    pub model: String,
}

#[derive(Debug, Clone)]
pub struct DiagnosticsRunInputs {
    pub local_model_path: Option<PathBuf>,
    pub local_vad_model_path: Option<PathBuf>,
    pub openai: CloudProviderRunInput,
    pub groq: CloudProviderRunInput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticsRunError {
    AlreadyRunning,
    WorkerFailed,
    PersistenceFailed,
}
