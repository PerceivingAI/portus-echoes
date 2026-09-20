//! Shared application orchestration for exact-ID Cloud processing jobs.
//!
//! This module owns recording identity/provider/route/cancellation context,
//! provider-scoped credential outcome, typed terminal outcome publication,
//! authority-gated delivery/errors, and exact completion. Completed-file
//! preprocessing/protocol details remain in `transcription/cloud/completed.rs`; later live
//! transport plugs into this application boundary without inheriting them.

use std::sync::mpsc::Receiver;

use tauri::{AppHandle, Manager};
use tokio_util::sync::CancellationToken;

use crate::app_state::AppState;
use crate::audio::CloudCapturedAudio;
use crate::models::{CloudRoute, ProviderId, RecordingIdentity, TranscriptResult, UserErrorCode};
use crate::output::DeliveryCapability;
use crate::transcription::TranscriptionError;
use crate::{output, recording, transcription};

use super::{emit_error_code, emit_transcription_error, TRANSCRIPTION_ERROR_EVENT};

struct CloudJobContext {
    recording_id: RecordingIdentity,
    provider: ProviderId,
    route: CloudRoute,
    cancellation: CancellationToken,
}

impl CloudJobContext {
    fn completed(
        recording_id: RecordingIdentity,
        provider: ProviderId,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            recording_id,
            provider,
            route: CloudRoute::CompletedAudio,
            cancellation,
        }
    }
}

#[derive(Debug, PartialEq)]
enum CloudJobTerminal {
    Transcript(TranscriptResult),
    StreamedLive,
    NoResult,
    Cancelled,
    TranscriptionError(TranscriptionError),
    CredentialError(UserErrorCode),
}

fn classify_credential(
    result: Result<Option<String>, UserErrorCode>,
) -> Result<String, CloudJobTerminal> {
    match result {
        Ok(Some(key)) if !key.is_empty() => Ok(key),
        Ok(_) => Err(CloudJobTerminal::NoResult),
        Err(code) => Err(CloudJobTerminal::CredentialError(code)),
    }
}

fn resolve_provider_credential(
    state: &AppState,
    provider: ProviderId,
) -> Result<String, CloudJobTerminal> {
    classify_credential(state.api_key(provider))
}

fn terminal_from_completed_transport(
    provider: ProviderId,
    result: Result<
        transcription::cloud::CloudTranscriptionOutcome,
        transcription::cloud::completed::CompletedCloudError,
    >,
) -> CloudJobTerminal {
    match result {
        Ok(transcription::cloud::CloudTranscriptionOutcome::Transcript(text)) => {
            CloudJobTerminal::Transcript(TranscriptResult { text, provider })
        }
        Ok(transcription::cloud::CloudTranscriptionOutcome::StreamedLive(_)) => {
            CloudJobTerminal::StreamedLive
        }
        Ok(transcription::cloud::CloudTranscriptionOutcome::NoSpeech) => CloudJobTerminal::NoResult,
        Ok(transcription::cloud::CloudTranscriptionOutcome::Cancelled) => {
            CloudJobTerminal::Cancelled
        }
        Err(error) => CloudJobTerminal::TranscriptionError(error.into()),
    }
}

async fn resolve_completed_cloud_job(
    app: &AppHandle,
    captured: CloudCapturedAudio,
    context: &CloudJobContext,
) -> CloudJobTerminal {
    debug_assert_eq!(context.route, CloudRoute::CompletedAudio);
    debug_assert_eq!(captured.recording_id, context.recording_id);

    if context.cancellation.is_cancelled() {
        return CloudJobTerminal::Cancelled;
    }

    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    if !crate::cloud_catalog::current_selection_matches_route(
        &settings,
        context.provider,
        context.route,
    ) {
        return CloudJobTerminal::NoResult;
    }
    let Some(model) =
        transcription::cloud::completed::completed_model_for_provider(&settings, context.provider)
    else {
        return CloudJobTerminal::NoResult;
    };

    if context.cancellation.is_cancelled() {
        return CloudJobTerminal::Cancelled;
    }

    let api_key = match resolve_provider_credential(&state, context.provider) {
        Ok(key) => key,
        Err(terminal) => return terminal,
    };

    if context.cancellation.is_cancelled() {
        return CloudJobTerminal::Cancelled;
    }

    let recording_id = context.recording_id;
    let app_handle = app.clone();
    let capability = state.delivery_capability();
    let mut on_delta_fn = move |delta: String| {
        let delivered = recording::run_if_authoritative(&app_handle, recording_id, || {
            let capability = app_handle.state::<AppState>().delivery_capability();
            if capability == DeliveryCapability::Inject {
                crate::output::inject(&app_handle, &delta).map_err(|_| ())
            } else {
                Ok(())
            }
        });
        if matches!(delivered, Some(Err(_))) {
            Err(transcription::cloud::completed::CompletedTransportError::Delivery)
        } else {
            Ok(())
        }
    };
    let on_delta: Option<&mut (dyn FnMut(String) -> Result<(), transcription::cloud::completed::CompletedTransportError> + Send)> =
        if context.provider == ProviderId::Openai && capability == DeliveryCapability::Inject {
            Some(&mut on_delta_fn)
        } else {
            None
        };

    terminal_from_completed_transport(
        context.provider,
        transcription::cloud::completed::transcribe_completed_audio(
            captured.samples,
            captured.sample_rate,
            context.provider,
            &api_key,
            &model,
            &context.cancellation,
            on_delta,
        )
        .await,
    )
}

fn finish_cloud_job(app: &AppHandle, context: &CloudJobContext, terminal: CloudJobTerminal) {
    match terminal {
        CloudJobTerminal::Transcript(result) => {
            let delivery = recording::run_if_authoritative(app, context.recording_id, || {
                let capability = app.state::<AppState>().delivery_capability();
                output::deliver(app, &result, capability)
            });
            if matches!(delivery, Some(Err(_))) {
                recording::run_if_authoritative(app, context.recording_id, || {
                    emit_transcription_error(app, transcription::TranscriptionError::Delivery);
                });
            }
        }
        CloudJobTerminal::StreamedLive
        | CloudJobTerminal::NoResult
        | CloudJobTerminal::Cancelled => {}
        CloudJobTerminal::TranscriptionError(error) => {
            recording::run_if_authoritative(app, context.recording_id, || {
                emit_transcription_error(app, error);
            });
        }
        CloudJobTerminal::CredentialError(code) => {
            recording::run_if_authoritative(app, context.recording_id, || {
                emit_error_code(app, TRANSCRIPTION_ERROR_EVENT, code);
            });
        }
    }

    recording::complete_cloud(app, context.recording_id);
}

async fn run_completed_cloud_job(
    app: AppHandle,
    captured: CloudCapturedAudio,
    context: CloudJobContext,
) {
    let terminal = resolve_completed_cloud_job(&app, captured, &context).await;
    finish_cloud_job(&app, &context, terminal);
}

/// Consume identity-bound completed Cloud buffers without serializing request work.
pub(crate) fn start_receiver(app: AppHandle, audio_rx: Receiver<CloudCapturedAudio>) {
    std::thread::spawn(move || {
        while let Ok(captured) = audio_rx.recv() {
            let recording_id = captured.recording_id;
            let control = recording::CloudProcessingControl::new();
            let cancellation = control.cancellation_token();
            let Some(provider) = recording::claim_completed_cloud_job(&app, recording_id, control)
            else {
                continue;
            };
            let context = CloudJobContext::completed(recording_id, provider, cancellation);
            let job_app = app.clone();
            tauri::async_runtime::spawn(run_completed_cloud_job(job_app, captured, context));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_context_owns_shared_identity_provider_route_and_cancellation() {
        let cancellation = CancellationToken::new();
        let context = CloudJobContext::completed(
            RecordingIdentity(17),
            ProviderId::Groq,
            cancellation.clone(),
        );

        assert_eq!(context.recording_id, RecordingIdentity(17));
        assert_eq!(context.provider, ProviderId::Groq);
        assert_eq!(context.route, CloudRoute::CompletedAudio);
        assert!(!context.cancellation.is_cancelled());
        cancellation.cancel();
        assert!(context.cancellation.is_cancelled());
    }

    #[test]
    fn credential_outcome_is_shared_and_provider_transport_free() {
        assert_eq!(
            classify_credential(Ok(Some("key".to_string()))),
            Ok("key".to_string())
        );
        assert_eq!(
            classify_credential(Ok(Some(String::new()))),
            Err(CloudJobTerminal::NoResult)
        );
        assert_eq!(
            classify_credential(Ok(None)),
            Err(CloudJobTerminal::NoResult)
        );
        assert_eq!(
            classify_credential(Err(UserErrorCode::CredentialStorage)),
            Err(CloudJobTerminal::CredentialError(
                UserErrorCode::CredentialStorage
            ))
        );
    }

    #[test]
    fn provider_transport_owns_no_credential_persistence_or_route_specific_copy() {
        let transport = include_str!("../transcription/cloud/completed.rs");
        let contracts = include_str!("../transcription/cloud/mod.rs");
        let sources = [transport, contracts].concat();
        for forbidden in [
            ["key", "ring::"].concat(),
            ["keystore", "::"].concat(),
            ["openai_", "api_key"].concat(),
            ["groq_", "api_key"].concat(),
            ["live_", "api_key"].concat(),
            ["completed_", "api_key"].concat(),
        ] {
            assert!(
                !sources.contains(&forbidden),
                "provider transport must receive credentials from shared orchestration, never persist or duplicate them: {forbidden}"
            );
        }
    }

    #[test]
    fn final_cloud_delivery_uses_current_capability_inside_exact_authority_gate() {
        let source = include_str!("cloud_jobs.rs");
        let finish = source
            .split_once("fn finish_cloud_job(")
            .expect("finish_cloud_job must exist")
            .1
            .split_once("async fn run_completed_cloud_job(")
            .expect("run_completed_cloud_job must follow finish_cloud_job")
            .0;
        let transcript_branch = finish
            .split_once("CloudJobTerminal::Transcript(result) =>")
            .expect("shared transcript terminal branch must exist")
            .1
            .split_once("CloudJobTerminal::NoResult")
            .expect("transcript branch must end before non-result terminals")
            .0;

        let authority = ["run_if_", "authoritative"].concat();
        let capability = ["delivery_", "capability()"].concat();
        let deliver = ["output::", "deliver"].concat();
        let delivery_error = ["TranscriptionError::", "Delivery"].concat();

        let first_authority = transcript_branch
            .find(authority.as_str())
            .expect("transcript delivery must be exact-ID authority-gated");
        let capability_lookup = transcript_branch
            .find(capability.as_str())
            .expect("Cloud must read the current delivery capability at final delivery");
        let delivery_call = transcript_branch
            .find(deliver.as_str())
            .expect("shared Cloud terminal must call shared output delivery");
        let error_mapping = transcript_branch
            .find(delivery_error.as_str())
            .expect("delivery failure must map through the shared transcription error boundary");

        assert!(first_authority < capability_lookup);
        assert!(capability_lookup < delivery_call);
        assert!(delivery_call < error_mapping);
        assert_eq!(
            transcript_branch.matches(authority.as_str()).count(),
            2,
            "delivery effect and delivery-error publication must each be authority-gated"
        );

        let context = source
            .split_once("struct CloudJobContext")
            .expect("CloudJobContext must exist")
            .1
            .split_once("impl CloudJobContext")
            .expect("CloudJobContext impl must follow its fields")
            .0;
        assert!(
            !context.contains(capability.as_str()),
            "Cloud delivery capability lookup must not be frozen into recording/job context"
        );
        let capability_type = ["Delivery", "Capability"].concat();
        assert!(
            !context.contains(capability_type.as_str()),
            "CloudJobContext must not freeze a delivery capability value"
        );
    }

    #[test]
    fn completed_transport_outcomes_map_to_shared_terminal_outcomes() {
        assert_eq!(
            terminal_from_completed_transport(
                ProviderId::Openai,
                Ok(transcription::cloud::CloudTranscriptionOutcome::Transcript(
                    "hello".to_string()
                ))
            ),
            CloudJobTerminal::Transcript(TranscriptResult {
                text: "hello".to_string(),
                provider: ProviderId::Openai,
            })
        );
        assert_eq!(
            terminal_from_completed_transport(
                ProviderId::Groq,
                Ok(transcription::cloud::CloudTranscriptionOutcome::NoSpeech)
            ),
            CloudJobTerminal::NoResult
        );
        assert_eq!(
            terminal_from_completed_transport(
                ProviderId::Groq,
                Ok(transcription::cloud::CloudTranscriptionOutcome::Cancelled)
            ),
            CloudJobTerminal::Cancelled
        );
        assert_eq!(
            terminal_from_completed_transport(
                ProviderId::Openai,
                Err(transcription::cloud::completed::CompletedCloudError::AudioConversion)
            ),
            CloudJobTerminal::TranscriptionError(
                transcription::TranscriptionError::AudioConversion
            )
        );
    }

    #[test]
    fn shared_orchestration_contains_no_completed_transport_mechanics() {
        let source = include_str!("cloud_jobs.rs");
        let branch_owned_symbols = [
            ["to_", "wav_bytes"].concat(),
            ["req", "west::"].concat(),
            ["multi", "part"].concat(),
            ["response_", "format"].concat(),
        ];
        for branch_owned_symbol in branch_owned_symbols {
            assert!(
                !source.contains(&branch_owned_symbol),
                "shared Cloud orchestration must not own completed transport mechanic: {branch_owned_symbol}"
            );
        }
    }
}
