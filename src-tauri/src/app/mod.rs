//! Application-shell responsibilities above the recording/transcription layers.

pub(crate) mod cloud_jobs;
pub(crate) mod local_models;
pub(crate) mod providers;
pub(crate) mod tray;
pub(crate) mod windows;

use tauri::{AppHandle, Emitter};

use crate::{models, transcription};

pub(crate) const TRANSCRIPTION_ERROR_EVENT: &str = "transcription:error";
pub(crate) const APP_ERROR_EVENT: &str = "app:error";

pub(crate) fn emit_error_code(app: &AppHandle, event: &str, code: models::UserErrorCode) {
    let _ = app.emit(event, models::UserErrorPayload { code });
}

pub(crate) fn emit_transcription_error(app: &AppHandle, error: transcription::TranscriptionError) {
    windows::hide_overlay(app);
    let _ = app.emit(
        models::DELIVERY_CLIPBOARD_STATUS_EVENT,
        models::ClipboardStatusPayload {
            status: models::ClipboardDeliveryStatus::Idle,
        },
    );
    emit_error_code(app, TRANSCRIPTION_ERROR_EVENT, error.user_error_code());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_error_payload_contains_only_the_dedicated_stable_code() {
        let payload = models::UserErrorPayload {
            code: transcription::TranscriptionError::Delivery.user_error_code(),
        };
        assert_eq!(
            serde_json::to_string(&payload).unwrap(),
            r#"{"code":"delivery"}"#
        );
    }

    #[test]
    fn transcription_error_payload_contains_only_the_stable_code() {
        let payload = models::UserErrorPayload {
            code: transcription::TranscriptionError::Cloud(
                transcription::CloudFailure::Authentication,
            )
            .user_error_code(),
        };
        assert_eq!(
            serde_json::to_string(&payload).unwrap(),
            r#"{"code":"api_key_rejected"}"#
        );
    }
}
