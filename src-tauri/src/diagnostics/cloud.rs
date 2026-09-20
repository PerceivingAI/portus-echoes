use std::thread;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use reqwest::blocking::{multipart, Client};
use reqwest::redirect::Policy;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use crate::audio::convert::encode_wav_16bit_mono;
use crate::models::ProviderId;
use crate::transcription::cloud::completed::COMPLETED_FILE_REQUEST_TIMEOUT;
use crate::transcription::cloud::live::OPENAI_REALTIME_WS_BASE_URL;

use super::types::{
    CloudDiagnostics, CloudProviderRunInput, DiagnosticGroupStatus, DiagnosticsRunError,
    ProviderValidation,
};

pub(super) const OPENAI_LIVE_DIAGNOSTICS_TIMEOUT: Duration = Duration::from_secs(5);

const OPENAI_DIAGNOSTICS_URL: &str = "https://api.openai.com/v1/audio/transcriptions";
const GROQ_DIAGNOSTICS_URL: &str = "https://api.groq.com/openai/v1/audio/transcriptions";
#[derive(Clone, Copy)]
pub(super) struct CloudProbeSpec {
    pub(super) url: &'static str,
    pub(super) response_format: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CloudProbeOutcome {
    HttpStatus(u16),
    NoResponse,
}

pub(super) fn cloud_probe_spec(provider: ProviderId) -> Option<CloudProbeSpec> {
    match provider {
        ProviderId::Openai => Some(CloudProbeSpec {
            url: OPENAI_DIAGNOSTICS_URL,
            response_format: "json",
        }),
        ProviderId::Groq => Some(CloudProbeSpec {
            url: GROQ_DIAGNOSTICS_URL,
            response_format: "verbose_json",
        }),
        ProviderId::Local => None,
    }
}

pub(super) fn provider_passes(api_key: ProviderValidation, model: ProviderValidation) -> bool {
    api_key == ProviderValidation::Validated && model == ProviderValidation::Validated
}

pub(super) fn test_cloud_group(
    openai: CloudProviderRunInput,
    groq: CloudProviderRunInput,
) -> Result<CloudDiagnostics, DiagnosticsRunError> {
    let ((openai_api_key, openai_model), (groq_api_key, groq_model)) = run_parallel_cloud(
        move || test_cloud_provider_input(ProviderId::Openai, openai),
        move || test_cloud_provider_input(ProviderId::Groq, groq),
    )?;

    Ok(CloudDiagnostics {
        passed: if provider_passes(openai_api_key, openai_model)
            || provider_passes(groq_api_key, groq_model)
        {
            DiagnosticGroupStatus::Pass
        } else {
            DiagnosticGroupStatus::Fail
        },
        openai_api_key,
        openai_model,
        groq_api_key,
        groq_model,
    })
}

pub(super) fn run_parallel_cloud<OF, GF>(
    openai_runner: OF,
    groq_runner: GF,
) -> Result<
    (
        (ProviderValidation, ProviderValidation),
        (ProviderValidation, ProviderValidation),
    ),
    DiagnosticsRunError,
>
where
    OF: FnOnce() -> (ProviderValidation, ProviderValidation) + Send,
    GF: FnOnce() -> (ProviderValidation, ProviderValidation) + Send,
{
    thread::scope(|scope| {
        let openai = scope.spawn(openai_runner);
        let groq = scope.spawn(groq_runner);
        // Join both providers before classifying either result. This prevents a
        // second provider panic from escaping through Scope's automatic join
        // path after the first provider has already failed.
        let openai = openai.join();
        let groq = groq.join();

        let openai = openai.map_err(|_| DiagnosticsRunError::WorkerFailed)?;
        let groq = groq.map_err(|_| DiagnosticsRunError::WorkerFailed)?;
        Ok((openai, groq))
    })
}

fn test_cloud_provider_input(
    provider: ProviderId,
    input: CloudProviderRunInput,
) -> (ProviderValidation, ProviderValidation) {
    evaluate_cloud_provider(input.key_result, &input.model, |api_key, model| {
        if provider == ProviderId::Openai && model == "gpt-live-transcribe" {
            send_openai_live_probe(api_key, model)
        } else {
            send_cloud_probe(provider, api_key, model)
        }
    })
}

pub(super) fn evaluate_cloud_provider<F>(
    key_result: Result<Option<String>, ()>,
    model: &str,
    probe: F,
) -> (ProviderValidation, ProviderValidation)
where
    F: FnOnce(&str, &str) -> CloudProbeOutcome,
{
    let Ok(Some(api_key)) = key_result else {
        return cloud_both_invalid();
    };
    let api_key = api_key.trim();
    let model = model.trim();
    if api_key.is_empty() || model.is_empty() {
        return cloud_both_invalid();
    }

    classify_cloud_probe(probe(api_key, model))
}

pub(super) fn cloud_both_invalid() -> (ProviderValidation, ProviderValidation) {
    (
        ProviderValidation::NoneNotValid,
        ProviderValidation::NoneNotValid,
    )
}

pub(super) fn classify_cloud_probe(
    outcome: CloudProbeOutcome,
) -> (ProviderValidation, ProviderValidation) {
    match outcome {
        CloudProbeOutcome::HttpStatus(status) if (200..300).contains(&status) => {
            (ProviderValidation::Validated, ProviderValidation::Validated)
        }
        CloudProbeOutcome::HttpStatus(401 | 403) | CloudProbeOutcome::NoResponse => {
            cloud_both_invalid()
        }
        CloudProbeOutcome::HttpStatus(_) => (
            ProviderValidation::Validated,
            ProviderValidation::NoneNotValid,
        ),
    }
}

fn send_cloud_probe(provider: ProviderId, api_key: &str, model: &str) -> CloudProbeOutcome {
    let Some(spec) = cloud_probe_spec(provider) else {
        return CloudProbeOutcome::NoResponse;
    };
    let Ok(wav) = encode_wav_16bit_mono(&vec![0.0f32; 16_000]) else {
        return CloudProbeOutcome::NoResponse;
    };
    send_cloud_probe_to(spec, spec.url, api_key, model, &wav)
}

pub(super) fn send_cloud_probe_to(
    spec: CloudProbeSpec,
    url: &str,
    api_key: &str,
    model: &str,
    wav: &[u8],
) -> CloudProbeOutcome {
    let Ok(client) = Client::builder()
        .timeout(COMPLETED_FILE_REQUEST_TIMEOUT)
        .redirect(Policy::none())
        .build()
    else {
        return CloudProbeOutcome::NoResponse;
    };

    let Ok(file_part) = multipart::Part::bytes(wav.to_vec())
        .file_name("diagnostics.wav")
        .mime_str("audio/wav")
    else {
        return CloudProbeOutcome::NoResponse;
    };
    let form = multipart::Form::new()
        .part("file", file_part)
        .text("model", model.to_string())
        .text("response_format", spec.response_format.to_string());

    match client.post(url).bearer_auth(api_key).multipart(form).send() {
        Ok(response) => CloudProbeOutcome::HttpStatus(response.status().as_u16()),
        Err(_) => CloudProbeOutcome::NoResponse,
    }
}

fn send_openai_live_probe(api_key: &str, model: &str) -> CloudProbeOutcome {
    send_openai_live_probe_to(OPENAI_REALTIME_WS_BASE_URL, api_key, model)
}

pub(super) fn send_openai_live_probe_to(
    url: &str,
    api_key: &str,
    model: &str,
) -> CloudProbeOutcome {
    tauri::async_runtime::block_on(send_openai_live_probe_async(url, api_key, model))
}

pub(super) async fn send_openai_live_probe_async(
    url: &str,
    api_key: &str,
    model: &str,
) -> CloudProbeOutcome {
    let connect_future = async {
        let mut request = match url.into_client_request() {
            Ok(req) => req,
            Err(_) => return CloudProbeOutcome::NoResponse,
        };
        let auth_val = match format!("Bearer {}", api_key.trim()).parse() {
            Ok(h) => h,
            Err(_) => return CloudProbeOutcome::HttpStatus(401),
        };
        request.headers_mut().insert("Authorization", auth_val);
        let (ws_stream, response) = match tokio_tungstenite::connect_async(request).await {
            Ok(res) => res,
            Err(tokio_tungstenite::tungstenite::Error::Http(http_resp)) => {
                return CloudProbeOutcome::HttpStatus(http_resp.status().as_u16());
            }
            Err(_) => return CloudProbeOutcome::NoResponse,
        };

        if response.status()
            != tokio_tungstenite::tungstenite::http::StatusCode::SWITCHING_PROTOCOLS
        {
            return CloudProbeOutcome::HttpStatus(response.status().as_u16());
        }

        let (mut ws_writer, mut ws_reader) = ws_stream.split();

        let session_update = serde_json::json!({
            "type": "session.update",
            "session": {
                "type": "transcription",
                "audio": {
                    "input": {
                        "format": {
                            "type": "audio/pcm",
                            "rate": 24000
                        },
                        "transcription": {
                            "model": model.trim()
                        },
                        "turn_detection": null
                    }
                }
            }
        });

        let session_json = match serde_json::to_string(&session_update) {
            Ok(s) => s,
            Err(_) => return CloudProbeOutcome::NoResponse,
        };

        if ws_writer
            .send(Message::Text(session_json.into()))
            .await
            .is_err()
        {
            return CloudProbeOutcome::NoResponse;
        }

        while let Some(msg_result) = ws_reader.next().await {
            let msg = match msg_result {
                Ok(m) => m,
                Err(_) => return CloudProbeOutcome::NoResponse,
            };

            if let Message::Text(text) = msg {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                    let event_type = val
                        .get("type")
                        .and_then(|t| t.as_str())
                        .unwrap_or_default();
                    if event_type == "session.updated" {
                        let _ = ws_writer.close().await;
                        return CloudProbeOutcome::HttpStatus(200);
                    } else if event_type == "error" {
                        let error_code = val
                            .get("error")
                            .and_then(|e| e.get("code"))
                            .and_then(|c| c.as_str())
                            .unwrap_or_default();
                        let _ = ws_writer.close().await;
                        if error_code == "invalid_api_key" || error_code == "unauthorized" {
                            return CloudProbeOutcome::HttpStatus(401);
                        } else {
                            return CloudProbeOutcome::HttpStatus(400);
                        }
                    }
                }
            }
        }

        CloudProbeOutcome::NoResponse
    };

    match tokio::time::timeout(OPENAI_LIVE_DIAGNOSTICS_TIMEOUT, connect_future).await {
        Ok(outcome) => outcome,
        Err(_) => CloudProbeOutcome::NoResponse,
    }
}
