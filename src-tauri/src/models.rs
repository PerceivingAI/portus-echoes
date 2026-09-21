//! Canonical domain types shared across the backend.
//!
//! Owns `AppSettings`, `ProviderId`, `TranscriptResult`, and
//! `HotkeyEvent` as specified in `docs/SETTINGS.md`
//! and `docs/HOTKEY.md`.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Transcription providers supported by PortusEchoes.
///
/// Serialized as the lowercase strings used in `config.toml`
/// (`"openai"` | `"groq"` | `"local"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    Openai,
    Groq,
    Local,
}

/// Stable identifiers for the 14 approved GUI error messages.
///
/// These values are routing codes, not message text. They are the only error
/// identifiers allowed to cross the Tauri boundary once a subsystem is
/// migrated to the typed error contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserErrorCode {
    CredentialStorage,
    InvalidShortcut,
    ModelDownload,
    NetworkConnection,
    CloudService,
    LocalModelNotFound,
    SettingsSave,
    UnsupportedWhisperModel,
    InsufficientRam,
    ApiKeyRejected,
    RateLimit,
    RecordingTooLarge,
    Delivery,
    Unexpected,
}

impl UserErrorCode {
    #[cfg(test)]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CredentialStorage => "credential_storage",
            Self::InvalidShortcut => "invalid_shortcut",
            Self::ModelDownload => "model_download",
            Self::NetworkConnection => "network_connection",
            Self::CloudService => "cloud_service",
            Self::LocalModelNotFound => "local_model_not_found",
            Self::SettingsSave => "settings_save",
            Self::UnsupportedWhisperModel => "unsupported_whisper_model",
            Self::InsufficientRam => "insufficient_ram",
            Self::ApiKeyRejected => "api_key_rejected",
            Self::RateLimit => "rate_limit",
            Self::RecordingTooLarge => "recording_too_large",
            Self::Delivery => "delivery",
            Self::Unexpected => "unexpected",
        }
    }
}

#[cfg(test)]
pub const ALL_USER_ERROR_CODES: [UserErrorCode; 14] = [
    UserErrorCode::CredentialStorage,
    UserErrorCode::InvalidShortcut,
    UserErrorCode::ModelDownload,
    UserErrorCode::NetworkConnection,
    UserErrorCode::CloudService,
    UserErrorCode::LocalModelNotFound,
    UserErrorCode::SettingsSave,
    UserErrorCode::UnsupportedWhisperModel,
    UserErrorCode::InsufficientRam,
    UserErrorCode::ApiKeyRejected,
    UserErrorCode::RateLimit,
    UserErrorCode::RecordingTooLarge,
    UserErrorCode::Delivery,
    UserErrorCode::Unexpected,
];

#[allow(dead_code)] // normal code-only application/transcription event payload (`docs/CLOUD.md`)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserErrorPayload {
    pub code: UserErrorCode,
}

/// Which cloud model card is selected. Model ID contents never determine this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloudModelKind {
    Standard,
    Custom,
}

/// Provider-neutral Cloud transport lifecycle frozen at recording admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub(crate) enum CloudRoute {
    #[serde(rename = "completed")]
    CompletedAudio,
    #[serde(rename = "live")]
    LiveAudio,
}

/// How the selected Local model path must be evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LocalModelKind {
    Standard,
    Custom,
}

/// Monotonically increasing identity for one push-to-talk interaction.
///
/// The recording coordinator is the authority for which identity is current.
/// Capture and provider work carry this opaque value so stale asynchronous work
/// can be rejected without provider-specific lifecycle rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RecordingIdentity(pub(crate) u64);

/// Global hold-to-talk hook events, sent from the hook thread to the app via
/// a channel. Internal only — never serialized.
#[allow(dead_code)] // consumed by the hotkey engine (Phase 3)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    Pressed,
    Released,
}
pub const SETTINGS_NAVIGATION_EVENT: &str = "settings:navigation-requested";

/// Tray destinations supported by the shared onboarding/Settings window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingsNavigationTab {
    Settings,
    Diagnostics,
}

/// Versioned in-memory tray navigation request. Revisions let the frontend
/// reconcile an event with a startup snapshot without losing an early click.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsNavigationState {
    pub tab: SettingsNavigationTab,
    pub revision: u64,
}

pub const RECORDING_STATE_EVENT: &str = "recording:state-changed";
pub const DELIVERY_CLIPBOARD_STATUS_EVENT: &str = "delivery:clipboard-status";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClipboardDeliveryStatus {
    Processing,
    Copied,
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardStatusPayload {
    pub status: ClipboardDeliveryStatus,
}

/// Authoritative recording lifecycle published to the overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordingPhase {
    Idle,
    Preparing,
    Recording,
    Muted,
    Finalizing,
}

/// Versioned recording snapshot. Revisions let the frontend reject stale
/// command responses that race with newer state-change events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingState {
    pub phase: RecordingPhase,
    pub revision: u64,
}

impl Default for RecordingState {
    fn default() -> Self {
        Self {
            phase: RecordingPhase::Idle,
            revision: 0,
        }
    }
}

/// A completed transcription ready for delivery to the output engine.
#[allow(dead_code)] // consumed by transcription + output (Phases 5–7)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptResult {
    pub text: String,
    pub provider: ProviderId,
}

/// Serde helper: `active_provider` is `""` in TOML when unset, and a provider
/// string otherwise. Maps `""` ↔ `None`.
mod empty_string_as_none {
    use super::*;

    pub fn serialize<S, T>(value: &Option<T>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        T: Serialize,
    {
        match value {
            Some(v) => v.serialize(serializer),
            None => serializer.serialize_str(""),
        }
    }

    pub fn deserialize<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
    where
        D: Deserializer<'de>,
        T: Deserialize<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        if raw.is_empty() {
            return Ok(None);
        }
        T::deserialize(serde::de::value::StrDeserializer::<serde::de::value::Error>::new(&raw))
            .map(Some)
            .map_err(serde::de::Error::custom)
    }
}

/// Application settings — the exact `config.toml` schema from
/// `docs/SETTINGS.md`. Missing keys fall back to defaults on load.
///
/// API keys are NOT part of this struct; they live in `keystore.rs` only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    #[serde(with = "empty_string_as_none")]
    pub active_provider: Option<ProviderId>,
    pub openai_model: String,
    #[serde(with = "empty_string_as_none")]
    pub openai_model_kind: Option<CloudModelKind>,
    pub openai_custom_model: String,
    pub groq_model: String,
    #[serde(with = "empty_string_as_none")]
    pub groq_model_kind: Option<CloudModelKind>,
    pub groq_custom_model: String,
    pub local_model: String,
    #[serde(with = "empty_string_as_none")]
    pub local_model_kind: Option<LocalModelKind>,
    pub local_model_path: String,
    pub local_custom_model_path: String,
    pub language: String,
    pub hotkey: String,
    pub onboarding_complete: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            active_provider: Some(ProviderId::Local),
            openai_model: crate::cloud_catalog::first_standard_model_id(ProviderId::Openai)
                .unwrap_or_default()
                .to_string(),
            openai_model_kind: crate::cloud_catalog::first_standard_model_id(ProviderId::Openai)
                .map(|_| CloudModelKind::Standard),
            openai_custom_model: String::new(),
            groq_model: crate::cloud_catalog::first_standard_model_id(ProviderId::Groq)
                .unwrap_or_default()
                .to_string(),
            groq_model_kind: crate::cloud_catalog::first_standard_model_id(ProviderId::Groq)
                .map(|_| CloudModelKind::Standard),
            groq_custom_model: String::new(),
            local_model: crate::model_download::first_configured_model_label()
                .unwrap_or_default()
                .to_string(),
            local_model_kind: crate::model_download::first_configured_model_label()
                .map(|_| LocalModelKind::Standard),
            local_model_path: String::new(),
            local_custom_model_path: String::new(),
            language: "auto".to_string(),
            hotkey: "Ctrl+Alt+Space".to_string(),
            onboarding_complete: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_error_codes_match_shared_frontend_contract() {
        let expected: Vec<String> =
            serde_json::from_str(include_str!("../../src/lib/user-error-codes.json")).unwrap();
        let actual: Vec<&str> = ALL_USER_ERROR_CODES
            .iter()
            .copied()
            .map(UserErrorCode::as_str)
            .collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn user_error_payload_serializes_with_stable_snake_case_code() {
        let payload = UserErrorPayload {
            code: UserErrorCode::ApiKeyRejected,
        };
        assert_eq!(
            serde_json::to_string(&payload).unwrap(),
            r#"{"code":"api_key_rejected"}"#
        );
    }

    #[test]
    fn settings_navigation_state_serializes_as_the_stable_versioned_contract() {
        let state = SettingsNavigationState {
            tab: SettingsNavigationTab::Diagnostics,
            revision: 9,
        };
        assert_eq!(
            serde_json::to_string(&state).unwrap(),
            r#"{"tab":"diagnostics","revision":9}"#
        );
    }

    #[test]
    fn recording_state_serializes_as_the_stable_versioned_contract() {
        let state = RecordingState {
            phase: RecordingPhase::Recording,
            revision: 7,
        };
        assert_eq!(
            serde_json::to_string(&state).unwrap(),
            r#"{"phase":"recording","revision":7}"#
        );
    }

    #[test]
    fn defaults_match_documented_schema() {
        let s = AppSettings::default();
        assert_eq!(s.active_provider, Some(ProviderId::Local));
        assert_eq!(s.openai_model, "gpt-live-transcribe");
        assert_eq!(s.openai_model_kind, Some(CloudModelKind::Standard));
        assert_eq!(s.groq_model, "whisper-large-v3-turbo");
        assert_eq!(s.groq_model_kind, Some(CloudModelKind::Standard));
        assert_eq!(s.local_model, "Whisper Small Q8");
        assert_eq!(s.local_model_kind, Some(LocalModelKind::Standard));
        assert_eq!(s.local_model_path, "");
        assert_eq!(s.language, "auto");
        assert_eq!(s.hotkey, "Ctrl+Alt+Space");
        assert!(!s.onboarding_complete);
    }
    #[test]
    fn parses_documented_example() {
        let toml_src = r#"
active_provider = "groq"
openai_model = "environment-openai-model"
openai_model_kind = "standard"
groq_model = "environment-groq-model"
groq_model_kind = "custom"
local_model_path = ""
local_model_kind = ""
language = "auto"
hotkey = "Ctrl+Alt+Space"
onboarding_complete = false
"#;
        let s: AppSettings = toml::from_str(toml_src).unwrap();
        assert_eq!(s.openai_model_kind, Some(CloudModelKind::Standard));
        assert_eq!(s.groq_model_kind, Some(CloudModelKind::Custom));
        assert_eq!(s.active_provider, Some(ProviderId::Groq));
    }

    #[test]
    fn empty_optional_model_kinds_map_to_none_and_back() {
        let s: AppSettings = toml::from_str(
            "active_provider = \"\"\nopenai_model_kind = \"\"\ngroq_model_kind = \"\"\nlocal_model_kind = \"\"",
        )
        .unwrap();
        assert_eq!(s.active_provider, None);
        assert_eq!(s.openai_model_kind, None);
        assert_eq!(s.groq_model_kind, None);
        assert_eq!(s.local_model_kind, None);

        let out = toml::to_string(&s).unwrap();
        assert!(out.contains(r#"active_provider = """#), "got:\n{out}");
        assert!(out.contains(r#"openai_model_kind = """#), "got:\n{out}");
        assert!(out.contains(r#"groq_model_kind = """#), "got:\n{out}");
        assert!(out.contains(r#"local_model_kind = """#), "got:\n{out}");
    }

    #[test]
    fn missing_keys_fall_back_to_defaults() {
        let s: AppSettings = toml::from_str(r#"hotkey = "Alt+V""#).unwrap();
        assert_eq!(s.hotkey, "Alt+V");
        assert_eq!(s.language, "auto");
        assert_eq!(s.active_provider, Some(ProviderId::Local));
        assert_eq!(s.openai_model, "gpt-live-transcribe");
        assert_eq!(s.openai_model_kind, Some(CloudModelKind::Standard));
        assert_eq!(s.groq_model, "whisper-large-v3-turbo");
        assert_eq!(s.groq_model_kind, Some(CloudModelKind::Standard));
        assert_eq!(s.local_model, "Whisper Small Q8");
        assert_eq!(s.local_model_kind, Some(LocalModelKind::Standard));
    }

    #[test]
    fn settings_serialization_does_not_persist_resolved_cloud_route() {
        let out = toml::to_string(&AppSettings::default()).unwrap();
        assert!(
            !out.contains("cloud_route") && !out.contains("\nroute ="),
            "resolved Cloud route is recording-only state and must not persist in settings TOML:\n{out}"
        );
    }

    #[test]
    fn settings_serialization_contains_no_removed_output_mode() {
        let out = toml::to_string(&AppSettings::default()).unwrap();
        assert!(
            !out.contains("output_mode"),
            "removed transcript-delivery setting must not reappear in settings TOML:\n{out}"
        );
    }

    #[test]
    fn settings_serialization_contains_no_secret_fields() {
        let out = toml::to_string(&AppSettings::default()).unwrap();
        for needle in [
            "api_key",
            "openai_api_key",
            "groq_api_key",
            "secret",
            "token",
            "password",
        ] {
            assert!(
                !out.to_lowercase().contains(needle),
                "settings TOML must not contain '{needle}':\n{out}"
            );
        }
    }
}
