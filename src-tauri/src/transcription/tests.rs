use super::*;
use crate::models::{AppSettings, CloudModelKind, ProviderId};
use tokio_util::sync::CancellationToken;

fn settings() -> AppSettings {
    AppSettings::default()
}
#[test]
fn every_cloud_application_failure_maps_to_one_approved_code() {
    let cases = [
        (
            CloudFailure::RecordingTooLarge,
            UserErrorCode::RecordingTooLarge,
        ),
        (CloudFailure::Network, UserErrorCode::NetworkConnection),
        (CloudFailure::Authentication, UserErrorCode::ApiKeyRejected),
        (CloudFailure::RateLimit, UserErrorCode::RateLimit),
        (CloudFailure::Service, UserErrorCode::CloudService),
        (CloudFailure::Unexpected, UserErrorCode::Unexpected),
    ];

    for (failure, expected) in cases {
        assert_eq!(
            TranscriptionError::Cloud(failure).user_error_code(),
            expected
        );
    }
}

#[test]
fn completed_http_errors_normalize_before_application_code_mapping() {
    use cloud::completed::{CompletedCloudError, CompletedTransportError};

    let cases = [
        (
            CompletedTransportError::UnsupportedProvider,
            CloudFailure::Unexpected,
        ),
        (
            CompletedTransportError::PayloadTooLarge,
            CloudFailure::RecordingTooLarge,
        ),
        (
            CompletedTransportError::RequestBuildFailed,
            CloudFailure::Unexpected,
        ),
        (
            CompletedTransportError::TransportFailed,
            CloudFailure::Network,
        ),
        (
            CompletedTransportError::HttpStatus(401),
            CloudFailure::Authentication,
        ),
        (
            CompletedTransportError::HttpStatus(403),
            CloudFailure::Authentication,
        ),
        (
            CompletedTransportError::HttpStatus(413),
            CloudFailure::RecordingTooLarge,
        ),
        (
            CompletedTransportError::HttpStatus(429),
            CloudFailure::RateLimit,
        ),
        (
            CompletedTransportError::HttpStatus(500),
            CloudFailure::Service,
        ),
        (
            CompletedTransportError::ResponseReadFailed,
            CloudFailure::Service,
        ),
        (
            CompletedTransportError::ResponseInvalid,
            CloudFailure::Service,
        ),
    ];

    for (transport_error, expected) in cases {
        assert_eq!(CloudFailure::from(transport_error), expected);
        assert_eq!(
            TranscriptionError::from(CompletedCloudError::Transport(transport_error)),
            TranscriptionError::Cloud(expected)
        );
    }
}

#[test]
fn vad_asset_failures_use_the_existing_local_engine_error_route() {
    for error in [
        local::vad::VadAssetError::Resolve,
        local::vad::VadAssetError::Missing,
        local::vad::VadAssetError::Read,
        local::vad::VadAssetError::SizeMismatch,
        local::vad::VadAssetError::HashMismatch,
        local::vad::VadAssetError::InvalidPath,
        local::vad::VadAssetError::Load,
    ] {
        assert_eq!(
            TranscriptionError::from(error).user_error_code(),
            UserErrorCode::Unexpected
        );
    }
}

#[test]
fn local_session_failures_use_existing_error_routes() {
    assert_eq!(
        TranscriptionError::from(local::LocalTranscriptionError::ModelNotFound),
        TranscriptionError::LocalModelNotFound
    );
    assert_eq!(
        TranscriptionError::from(local::LocalTranscriptionError::VadAsset(
            local::vad::VadAssetError::Missing,
        )),
        TranscriptionError::LocalEngine
    );
    assert_eq!(
        TranscriptionError::from(local::LocalTranscriptionError::VadProcessing(
            local::vad::VadProcessingError::Inference,
        )),
        TranscriptionError::LocalEngine
    );
    assert_eq!(
        TranscriptionError::from(local::LocalTranscriptionError::InferenceProtocol),
        TranscriptionError::LocalEngine
    );
    assert_eq!(
        TranscriptionError::from(local::LocalTranscriptionError::FeedClosed),
        TranscriptionError::LocalEngine
    );
    assert_eq!(
        TranscriptionError::from(local::LocalTranscriptionError::Backpressure),
        TranscriptionError::LocalEngine
    );
    assert_eq!(
        TranscriptionError::from(local::LocalTranscriptionError::Delivery),
        TranscriptionError::Delivery
    );
    assert_eq!(
        TranscriptionError::from(local::LocalTranscriptionError::AudioConversion(
            crate::audio::convert::AudioConversionError::InvalidSampleRate,
        )),
        TranscriptionError::AudioConversion
    );
    assert_eq!(
        TranscriptionError::from(local::LocalTranscriptionError::VadAsset(
            local::vad::VadAssetError::Missing,
        ))
        .user_error_code(),
        UserErrorCode::Unexpected
    );
}

#[test]
fn local_pipeline_variants_map_without_message_text() {
    assert_eq!(
        TranscriptionError::LocalModelNotFound.user_error_code(),
        UserErrorCode::LocalModelNotFound
    );
    assert_eq!(
        TranscriptionError::AudioConversion.user_error_code(),
        UserErrorCode::Unexpected
    );
    assert_eq!(
        TranscriptionError::LocalEngine.user_error_code(),
        UserErrorCode::Unexpected
    );
    assert_eq!(
        TranscriptionError::Delivery.user_error_code(),
        UserErrorCode::Delivery
    );
}

#[test]
fn delivery_maps_to_dedicated_code_not_unexpected() {
    assert_eq!(
        TranscriptionError::Delivery.user_error_code(),
        UserErrorCode::Delivery
    );
    assert_ne!(
        TranscriptionError::Delivery.user_error_code(),
        UserErrorCode::Unexpected
    );
}

#[test]
fn hard_cutover_source_guards_reject_legacy_local_runtime_paths() {
    let local_source = concat!(
        include_str!("local/mod.rs"),
        include_str!("local/engine.rs"),
        include_str!("local/recording.rs"),
        include_str!("local/feed.rs"),
        include_str!("local/inference.rs"),
        include_str!("local/output.rs"),
    );
    let vad_source = include_str!("local/vad.rs");
    let router_source = include_str!("mod.rs");
    let manifest = include_str!("../../Cargo.toml");
    let audio_source = include_str!("../audio/mod.rs");
    let models_source = include_str!("../models.rs");

    for removed_symbol in [
        "pending_16k_audio",
        "release_requested",
        "background_inference_cancelled",
        "cancel_background_inference",
    ] {
        assert!(
            !local_source.contains(removed_symbol),
            "removed Local staging symbol was reintroduced: {removed_symbol}"
        );
    }

    assert!(
        !local_source.contains("CloudCapturedAudio"),
        "Local runtime must never gain CloudCapturedAudio ownership"
    );
    assert!(
        !local_source.contains("pub fn transcribe("),
        "LocalEngine whole-recording transcribe entrypoint must remain removed"
    );
    assert!(
        !local_source.contains("segments_from_samples"),
        "Local runtime must not regain the removed batch VAD helper"
    );
    assert!(
        !vad_source.contains("segments_from_samples"),
        "bounded-tail/batch VAD segmentation helper must remain absent"
    );
    let removed_audio_apis = [
        ["Live", "AudioFeed"].concat(),
        ["start_", "live("].concat(),
        ["start_with_", "feed("].concat(),
        ["auto", "_stop"].concat(),
    ];
    for removed_audio_api in &removed_audio_apis {
        assert!(
            !audio_source.contains(removed_audio_api),
            "removed shared/Local capture API was reintroduced: {removed_audio_api}"
        );
    }
    let removed_shared_duration_field = ["auto", "_stop_secs"].concat();
    assert!(
        !models_source.contains(&removed_shared_duration_field),
        "the removed provider-agnostic recording-duration setting must remain absent"
    );
    let removed_local_outputs = [
        ["live_cursor_", "delivery"].concat(),
        ["Transcript", "Accumulator"].concat(),
    ];
    for removed_local_output in &removed_local_outputs {
        assert!(
            !local_source.contains(removed_local_output),
            "removed Local output compatibility path was reintroduced: {removed_local_output}"
        );
    }

    for completed_assumption in [
        "CloudCapturedAudio",
        "spawn_blocking",
        "to_wav_bytes",
        "to_mp3_bytes",
        "dispatch_cloud",
    ] {
        assert!(
            !router_source.contains(completed_assumption),
            "shared transcription router must not own completed-audio assumption: {completed_assumption}"
        );
    }

    let whisper_dependencies = manifest
        .lines()
        .filter(|line| line.trim_start().starts_with("whisper-rs ="))
        .count();
    assert_eq!(
        whisper_dependencies, 1,
        "the app manifest must contain exactly one whisper-rs dependency"
    );
    assert!(
        manifest
            .contains("whisper-rs = { path = \"../vendor/whisper-rs\", features = [\"vulkan\"] }"),
        "the sole whisper-rs dependency must remain the repository-owned Vulkan-enabled binding"
    );
}

#[test]
fn invalid_sample_rate_is_typed() {
    let cancellation = CancellationToken::new();
    let result = tokio::runtime::Runtime::new().unwrap().block_on(
        cloud::completed::transcribe_completed_audio(
            vec![0.0; 1600],
            0,
            ProviderId::Openai,
            "key",
            "environment-openai-model",
            &cancellation,
            None,
        ),
    );
    assert_eq!(
        result,
        Err(cloud::completed::CompletedCloudError::AudioConversion)
    );
}

#[test]
fn cloud_cancellation_is_not_a_transcription_error() {
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let outcome = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(cloud::completed::transcribe_completed_audio(
            vec![0.0; 1600],
            16_000,
            ProviderId::Openai,
            "key",
            "environment-openai-model",
            &cancellation,
            None,
        ))
        .unwrap();
    assert_eq!(outcome, cloud::CloudTranscriptionOutcome::Cancelled);
}

#[test]
fn frozen_provider_model_resolution_ignores_ambient_active_provider() {
    let mut settings = settings();
    settings.active_provider = Some(ProviderId::Groq);
    settings.openai_model = "openai-at-completion".to_string();
    settings.openai_model_kind = Some(CloudModelKind::Standard);
    settings.groq_model = "groq-current".to_string();
    settings.groq_model_kind = Some(CloudModelKind::Standard);

    assert_eq!(
        cloud::completed::completed_model_for_provider(&settings, ProviderId::Openai),
        Some("openai-at-completion".to_string())
    );
    assert_eq!(
        cloud::completed::completed_model_for_provider(&settings, ProviderId::Groq),
        Some("groq-current".to_string())
    );
}

#[test]
fn missing_model_is_evaluated_for_the_frozen_provider_only() {
    let mut settings = settings();
    settings.active_provider = Some(ProviderId::Groq);
    settings.groq_model = "configured-groq".to_string();
    settings.groq_model_kind = Some(CloudModelKind::Standard);
    settings.openai_model.clear();
    settings.openai_model_kind = Some(CloudModelKind::Standard);

    assert_eq!(
        cloud::completed::completed_model_for_provider(&settings, ProviderId::Openai),
        None
    );
    assert_eq!(
        cloud::completed::completed_model_for_provider(&settings, ProviderId::Groq),
        Some("configured-groq".to_string())
    );
}

#[test]
fn cancellation_preempts_invalid_audio_before_preprocessing_error() {
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let outcome = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(cloud::completed::transcribe_completed_audio(
            vec![0.0; 16],
            0,
            ProviderId::Openai,
            "key",
            "model",
            &cancellation,
            None,
        ))
        .unwrap();
    assert_eq!(outcome, cloud::CloudTranscriptionOutcome::Cancelled);
}

#[test]
fn completed_provider_language_auto_omits_the_request_hint() {
    assert_eq!(
        cloud::completed::completed_provider_language(ProviderId::Openai),
        None
    );
    assert_eq!(
        cloud::completed::completed_provider_language(ProviderId::Groq),
        None
    );
}
