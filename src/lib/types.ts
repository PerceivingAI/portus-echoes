/** Mirror of the Rust wire shapes (src-tauri/src/models.rs et al.). */

export type ProviderId = "openai" | "groq" | "local";
export type RecordingPhase =
  | "idle"
  | "preparing"
  | "recording"
  | "muted"
  | "finalizing";

export interface RecordingState {
  phase: RecordingPhase;
  revision: number;
}

export type SettingsNavigationTab = "settings" | "diagnostics";

export interface SettingsNavigationState {
  tab: SettingsNavigationTab;
  revision: number;
}
export type CloudModelKind = "" | "standard" | "custom";
export type LocalModelKind = "" | "standard" | "custom";

export const USER_ERROR_CODES = [
  "credential_storage",
  "invalid_shortcut",
  "model_download",
  "network_connection",
  "cloud_service",
  "local_model_not_found",
  "settings_save",
  "unsupported_whisper_model",
  "insufficient_ram",
  "api_key_rejected",
  "rate_limit",
  "recording_too_large",
  "delivery",
  "unexpected",
] as const;

export type UserErrorCode = (typeof USER_ERROR_CODES)[number];

export interface UserErrorPayload {
  code: UserErrorCode;
}

const USER_ERROR_CODE_SET: ReadonlySet<string> = new Set(USER_ERROR_CODES);

export function isUserErrorCode(value: unknown): value is UserErrorCode {
  return typeof value === "string" && USER_ERROR_CODE_SET.has(value);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isNonNegativeFiniteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0;
}

function isRevision(value: unknown): value is number {
  return Number.isSafeInteger(value) && isNonNegativeFiniteNumber(value);
}

export function isProviderId(value: unknown): value is ProviderId {
  return value === "local" || value === "openai" || value === "groq";
}

export function isRecordingState(value: unknown): value is RecordingState {
  if (!isRecord(value) || !isRevision(value.revision)) return false;
  return (
    value.phase === "idle" ||
    value.phase === "preparing" ||
    value.phase === "recording" ||
    value.phase === "muted" ||
    value.phase === "finalizing"
  );
}

export function isSettingsNavigationState(
  value: unknown
): value is SettingsNavigationState {
  return (
    isRecord(value) &&
    isRevision(value.revision) &&
    (value.tab === "settings" || value.tab === "diagnostics")
  );
}

export function isDownloadProgress(value: unknown): value is DownloadProgress {
  if (!isRecord(value) || typeof value.download_path !== "string") return false;
  if (!isNonNegativeFiniteNumber(value.downloaded_bytes)) return false;
  return (
    value.total_bytes === null || isNonNegativeFiniteNumber(value.total_bytes)
  );
}

export function isDownloadComplete(value: unknown): value is DownloadComplete {
  return (
    isRecord(value) &&
    typeof value.download_path === "string" &&
    typeof value.path === "string"
  );
}

export function isDownloadCancelled(value: unknown): value is DownloadCancelled {
  return isRecord(value) && typeof value.download_path === "string";
}

export function downloadErrorFields(
  value: unknown
): { download_path: string; code: unknown } | null {
  return isRecord(value) && typeof value.download_path === "string"
    ? { download_path: value.download_path, code: value.code }
    : null;
}

/** The persisted provider used for the next transcription. */
export interface AppSettings {
  active_provider: ProviderId;
  openai_model: string;
  openai_model_kind: CloudModelKind;
  openai_custom_model: string;
  groq_model: string;
  groq_model_kind: CloudModelKind;
  groq_custom_model: string;
  local_model: string;
  local_model_path: string;
  local_model_kind: LocalModelKind;
  local_custom_model_path: string;
  language: string;
  hotkey: string;
  onboarding_complete: boolean;
}

export interface ProviderStatus {
  openai_configured: boolean;
  groq_configured: boolean;
  local_configured: boolean;
  active_provider: ProviderId | null;
  active_configured: boolean;
}

export type ProviderValidation = "validated" | "none_not_valid";
export type DiagnosticGroupStatus = "pass" | "fail";
export type TranscriptionFunction =
  | "send_input"
  | "xdotool"
  | "system"
  | "none_not_valid";
export type TranscriptionType = "cursor" | "clipboard" | "none_not_valid";
export interface LocalDiagnostics {
  passed: DiagnosticGroupStatus;
  model_path: ProviderValidation;
  model_file: ProviderValidation;
}

export interface CloudDiagnostics {
  passed: DiagnosticGroupStatus;
  openai_api_key: ProviderValidation;
  openai_model: ProviderValidation;
  groq_api_key: ProviderValidation;
  groq_model: ProviderValidation;
}

export interface MicrophoneDiagnostics {
  passed: DiagnosticGroupStatus;
  name: string;
  specs: string;
}

export interface TranscriptionAccessDiagnostics {
  passed: DiagnosticGroupStatus;
  function: TranscriptionFunction;
  access_type: TranscriptionType;
}

export interface DiagnosticsSnapshotBody {
  local: LocalDiagnostics;
  cloud: CloudDiagnostics;
  microphone: MicrophoneDiagnostics;
  transcription_access: TranscriptionAccessDiagnostics;
}

export interface DiagnosticsStorageData {
  last_scan_display: string;
  snapshot: DiagnosticsSnapshotBody | null;
}

export interface LocalModelInfo {
  download_path: string;
  path: string;
  downloaded: boolean;
  size_bytes: number | null;
}

export interface DownloadProgress {
  download_path: string;
  downloaded_bytes: number;
  total_bytes: number | null;
}

export interface DownloadComplete {
  download_path: string;
  path: string;
}

export interface DownloadCancelled {
  download_path: string;
}

export type DownloadUiState =
  | { phase: "idle" }
  | {
      phase: "downloading";
      downloadPath: string;
      downloadedBytes: number;
      totalBytes: number | null;
      progressReceived?: boolean;
      cancelRequested?: boolean;
    }
  | { phase: "success"; downloadPath: string };
