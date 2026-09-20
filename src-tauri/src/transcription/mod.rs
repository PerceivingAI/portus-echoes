//! Transcription contracts and the shared application error boundary.
//!
//! Local transcription is owned by the live stateful VAD session in
//! `local.rs`. Completed Cloud transport/preprocessing is branch-owned under
//! `cloud/completed.rs`; future live transport owns a separate session branch.
//! This router normalizes branch-specific failures without assuming a transport
//! lifecycle or exposing provider protocol details.

pub mod cloud;
pub mod local;

use crate::models::UserErrorCode;

pub(crate) fn configure_native_logging() {
    let raw_logs_enabled = matches!(
        std::env::var("PORTUS_ECHOES_NATIVE_LOG").as_deref(),
        Ok("1")
    );

    if !raw_logs_enabled {
        whisper_rs::install_logging_hooks();
    }
}

/// Provider/transport-neutral Cloud failure categories at the application
/// boundary. Completed HTTP and future live-session transports may own different
/// internal error enums and normalize them independently into these categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudFailure {
    RecordingTooLarge,
    Network,
    Authentication,
    RateLimit,
    Service,
    Unexpected,
}

/// Structured failures from the normal transcription pipeline.
///
/// Variants contain technical/application facts only. No message text, API key,
/// provider response body, or provider-protocol payload is retained here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptionError {
    AudioConversion,
    LocalModelNotFound,
    LocalEngine,
    Cloud(CloudFailure),
    Delivery,
}

impl From<cloud::completed::CompletedTransportError> for CloudFailure {
    fn from(error: cloud::completed::CompletedTransportError) -> Self {
        match error {
            cloud::completed::CompletedTransportError::PayloadTooLarge
            | cloud::completed::CompletedTransportError::HttpStatus(413) => Self::RecordingTooLarge,
            cloud::completed::CompletedTransportError::TransportFailed => Self::Network,
            cloud::completed::CompletedTransportError::HttpStatus(401)
            | cloud::completed::CompletedTransportError::HttpStatus(403) => Self::Authentication,
            cloud::completed::CompletedTransportError::HttpStatus(429) => Self::RateLimit,
            cloud::completed::CompletedTransportError::HttpStatus(_)
            | cloud::completed::CompletedTransportError::ResponseReadFailed
            | cloud::completed::CompletedTransportError::ResponseInvalid => Self::Service,
            cloud::completed::CompletedTransportError::UnsupportedProvider
            | cloud::completed::CompletedTransportError::RequestBuildFailed
            | cloud::completed::CompletedTransportError::Delivery => Self::Unexpected,
        }
    }
}

impl From<cloud::completed::CompletedCloudError> for TranscriptionError {
    fn from(error: cloud::completed::CompletedCloudError) -> Self {
        match error {
            cloud::completed::CompletedCloudError::AudioConversion => Self::AudioConversion,
            cloud::completed::CompletedCloudError::Transport(error) => {
                Self::Cloud(CloudFailure::from(error))
            }
        }
    }
}

impl From<local::vad::VadAssetError> for TranscriptionError {
    fn from(_: local::vad::VadAssetError) -> Self {
        Self::LocalEngine
    }
}

impl From<local::LocalTranscriptionError> for TranscriptionError {
    fn from(error: local::LocalTranscriptionError) -> Self {
        match error {
            local::LocalTranscriptionError::ModelNotFound => Self::LocalModelNotFound,
            local::LocalTranscriptionError::AudioConversion(_) => Self::AudioConversion,
            local::LocalTranscriptionError::ModelLoad
            | local::LocalTranscriptionError::VadAsset(_)
            | local::LocalTranscriptionError::VadProcessing(_)
            | local::LocalTranscriptionError::StateCreation
            | local::LocalTranscriptionError::Inference
            | local::LocalTranscriptionError::SegmentRead
            | local::LocalTranscriptionError::InferenceProtocol
            | local::LocalTranscriptionError::FeedClosed
            | local::LocalTranscriptionError::Backpressure => Self::LocalEngine,
            local::LocalTranscriptionError::Delivery => Self::Delivery,
        }
    }
}

impl TranscriptionError {
    /// The single normal-application mapping boundary from structured facts to
    /// one of the approved GUI routing codes.
    pub const fn user_error_code(self) -> UserErrorCode {
        match self {
            Self::AudioConversion | Self::LocalEngine => UserErrorCode::Unexpected,
            Self::Delivery => UserErrorCode::Delivery,
            Self::LocalModelNotFound => UserErrorCode::LocalModelNotFound,
            Self::Cloud(CloudFailure::RecordingTooLarge) => UserErrorCode::RecordingTooLarge,
            Self::Cloud(CloudFailure::Network) => UserErrorCode::NetworkConnection,
            Self::Cloud(CloudFailure::Authentication) => UserErrorCode::ApiKeyRejected,
            Self::Cloud(CloudFailure::RateLimit) => UserErrorCode::RateLimit,
            Self::Cloud(CloudFailure::Service) => UserErrorCode::CloudService,
            Self::Cloud(CloudFailure::Unexpected) => UserErrorCode::Unexpected,
        }
    }
}

#[cfg(test)]
mod tests;
