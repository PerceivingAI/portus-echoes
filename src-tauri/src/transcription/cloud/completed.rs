//! Completed-file OpenAI/Groq Cloud transcription transport.
//!
//! This module exclusively owns the completed-file HTTP lifecycle: one shared
//! async reqwest client for completed OpenAI/Groq requests, complete-buffer WAV
//! preprocessing, multipart construction, payload/timeout policy, provider file
//! fields/endpoints, response parsing, and completed-file no-result rules.
//! Future live Cloud transports must not use this client/cache or multipart path.

use std::time::Duration;

use parking_lot::Mutex;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::{
    audio::convert,
    models::{AppSettings, ProviderId},
};

use super::CloudTranscriptionOutcome;

/// Structured failures owned only by the completed-file HTTP transport.
/// These are technical facts, not application routing codes or user-facing
/// messages, and never carry provider response bodies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompletedTransportError {
    UnsupportedProvider,
    PayloadTooLarge,
    RequestBuildFailed,
    TransportFailed,
    HttpStatus(u16),
    ResponseReadFailed,
    ResponseInvalid,
    Delivery,
}
/// Completed-audio branch failures before normalization at the shared
/// transcription/application error boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompletedCloudError {
    AudioConversion,
    Transport(CompletedTransportError),
}

/// Hard completed-file pre-upload cap. Payloads larger than this are rejected
/// before any request is sent.
pub(crate) const MAX_FILE_UPLOAD_BYTES: usize = 25 * 1024 * 1024;

/// Authoritative timeout for one completed-file transcription HTTP request.
/// Diagnostics intentionally reuses this bounded policy while owning a separate
/// blocking client.
pub(crate) const COMPLETED_FILE_REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

const OPENAI_URL: &str = "https://api.openai.com/v1/audio/transcriptions";
const GROQ_URL: &str = "https://api.groq.com/openai/v1/audio/transcriptions";

static COMPLETED_FILE_HTTP_CLIENT: Mutex<Option<reqwest::Client>> = Mutex::new(None);

/// Explicitly clears only the completed-file client cache. In-flight jobs
/// retain their cheap client clones and stop only through their recording
/// cancellation token.
pub(crate) fn close_completed_file_client() {
    *COMPLETED_FILE_HTTP_CLIENT.lock() = None;
}

/// Return a cheap clone of the completed-file async client. The cache mutex
/// covers only create/clone state and is released before any network future is
/// polled.
fn completed_file_client() -> Result<reqwest::Client, CompletedTransportError> {
    let mut guard = COMPLETED_FILE_HTTP_CLIENT.lock();
    if guard.is_none() {
        let client = reqwest::Client::builder()
            .timeout(COMPLETED_FILE_REQUEST_TIMEOUT)
            .build()
            .map_err(|_| CompletedTransportError::RequestBuildFailed)?;
        *guard = Some(client);
    }
    Ok(guard
        .as_ref()
        .expect("completed-file HTTP client initialized")
        .clone())
}

/// Provider-specific completed-file request/response behavior.
struct ProviderSpec {
    url: &'static str,
    stream: bool,
    response_format: Option<&'static str>,
    no_speech: NoSpeechRule,
}

/// Provider-specific completed-file no-speech rules (`docs/CLOUD.md`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum NoSpeechRule {
    /// OpenAI: empty `text` after trimming.
    EmptyText,
    /// Groq: all `segments[].no_speech_prob >= 0.6` (empty text also counts).
    GroqSegments,
}

fn spec_for(provider: ProviderId) -> Result<ProviderSpec, CompletedTransportError> {
    match provider {
        ProviderId::Openai => Ok(ProviderSpec {
            url: OPENAI_URL,
            stream: true,
            response_format: None,
            no_speech: NoSpeechRule::EmptyText,
        }),
        ProviderId::Groq => Ok(ProviderSpec {
            url: GROQ_URL,
            stream: false,
            response_format: Some("verbose_json"),
            no_speech: NoSpeechRule::GroqSegments,
        }),
        ProviderId::Local => Err(CompletedTransportError::UnsupportedProvider),
    }
}
/// Resolve the completion-time model for the provider frozen at recording
/// admission. This is a completed-file branch field; ambient `active_provider`
/// is deliberately ignored.
pub(crate) fn completed_model_for_provider(
    settings: &AppSettings,
    provider: ProviderId,
) -> Option<String> {
    let (model, kind) = match provider {
        ProviderId::Openai => (&settings.openai_model, settings.openai_model_kind),
        ProviderId::Groq => (&settings.groq_model, settings.groq_model_kind),
        ProviderId::Local => return None,
    };
    if kind.is_none() || model.is_empty() {
        None
    } else {
        Some(model.clone())
    }
}

/// Resolve the completed-file provider language field. `None` means the
/// multipart field is omitted so the provider performs automatic detection.
pub(crate) fn completed_provider_language(provider: ProviderId) -> Option<&'static str> {
    let language = match provider {
        ProviderId::Openai => env!("PORTUS_OPENAI_LANGUAGE"),
        ProviderId::Groq => env!("PORTUS_GROQ_LANGUAGE"),
        ProviderId::Local => return None,
    };
    if language == "auto" || language.is_empty() {
        None
    } else {
        Some(language)
    }
}

#[derive(Debug, Deserialize)]
struct TranscriptionResponse {
    text: String,
    #[serde(default)]
    segments: Vec<TranscriptionSegment>,
}

#[derive(Debug, Deserialize)]
struct TranscriptionSegment {
    no_speech_prob: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct OpenAiSsePayload {
    #[serde(rename = "type")]
    event_type: Option<String>,
    delta: Option<String>,
    text: Option<String>,
}
#[derive(Debug, Deserialize)]
struct OpenAiSseErrorPayload {
    #[allow(dead_code)]
    error: serde_json::Value,
}
fn build_file_part(
    audio_bytes: Vec<u8>,
    mime: &str,
) -> Result<reqwest::multipart::Part, CompletedTransportError> {
    reqwest::multipart::Part::bytes(audio_bytes)
        .file_name("audio.mp3")
        .mime_str(mime)
        .map_err(|_| CompletedTransportError::RequestBuildFailed)
}

/// Completed-audio branch entrypoint from captured device-rate mono PCM.
/// One-shot resampling/MP3 creation and all provider file-transport details
/// remain below this branch boundary rather than in shared Cloud orchestration.
pub(crate) async fn transcribe_completed_audio(
    samples: Vec<f32>,
    sample_rate: u32,
    provider: ProviderId,
    api_key: &str,
    model: &str,
    cancellation: &CancellationToken,
    on_delta: Option<&mut (dyn FnMut(String) -> Result<(), CompletedTransportError> + Send)>,
) -> Result<CloudTranscriptionOutcome, CompletedCloudError> {
    if cancellation.is_cancelled() {
        return Ok(CloudTranscriptionOutcome::Cancelled);
    }

    let audio_result =
        tokio::task::spawn_blocking(move || convert::to_mp3_bytes(&samples, sample_rate)).await;

    if cancellation.is_cancelled() {
        return Ok(CloudTranscriptionOutcome::Cancelled);
    }

    let audio = audio_result
        .map_err(|_| CompletedCloudError::AudioConversion)?
        .map_err(|_| CompletedCloudError::AudioConversion)?;
    let language = completed_provider_language(provider);
    transcribe_completed_file(
        provider,
        api_key,
        model,
        audio,
        language,
        cancellation,
        on_delta,
    )
    .await
    .map_err(CompletedCloudError::Transport)
}

/// Typed completed-file transport entrypoint after WAV preprocessing.
async fn transcribe_completed_file(
    provider: ProviderId,
    api_key: &str,
    model: &str,
    audio: Vec<u8>,
    language: Option<&str>,
    cancellation: &CancellationToken,
    on_delta: Option<&mut (dyn FnMut(String) -> Result<(), CompletedTransportError> + Send)>,
) -> Result<CloudTranscriptionOutcome, CompletedTransportError> {
    let spec = spec_for(provider)?;
    transcribe_with(
        provider,
        &spec,
        spec.url,
        api_key,
        model,
        audio,
        language,
        cancellation,
        on_delta,
    )
    .await
}

async fn transcribe_with(
    provider: ProviderId,
    spec: &ProviderSpec,
    url: &str,
    api_key: &str,
    model: &str,
    audio: Vec<u8>,
    language: Option<&str>,
    cancellation: &CancellationToken,
    mut on_delta: Option<&mut (dyn FnMut(String) -> Result<(), CompletedTransportError> + Send)>,
) -> Result<CloudTranscriptionOutcome, CompletedTransportError> {
    if cancellation.is_cancelled() {
        return Ok(CloudTranscriptionOutcome::Cancelled);
    }
    if audio.len() > MAX_FILE_UPLOAD_BYTES {
        return Err(CompletedTransportError::PayloadTooLarge);
    }

    let file_part = build_file_part(audio, "audio/mpeg")?;
    let mut form = reqwest::multipart::Form::new()
        .part("file", file_part)
        .text("model", model.to_string());

    if spec.stream {
        form = form.text("stream", "true");
    }
    if let Some(format) = spec.response_format {
        form = form.text("response_format", format);
    }

    if let Some(language) = language {
        if provider == ProviderId::Openai && model == "gpt-transcribe" {
            form = form.text("languages[]", language.to_string());
        } else {
            form = form.text("language", language.to_string());
        }
    }

    let client = completed_file_client()?;
    let send = client.post(url).bearer_auth(api_key).multipart(form).send();
    let mut response = tokio::select! {
        _ = cancellation.cancelled() => return Ok(CloudTranscriptionOutcome::Cancelled),
        response = send => response.map_err(|_| CompletedTransportError::TransportFailed)?,
    };
    if cancellation.is_cancelled() {
        return Ok(CloudTranscriptionOutcome::Cancelled);
    }

    let status = response.status();
    if !status.is_success() {
        return Err(CompletedTransportError::HttpStatus(status.as_u16()));
    }

    if spec.stream {
        let mut buffer = Vec::<u8>::new();
        let mut raw_body = Vec::<u8>::new();
        let mut accumulated_text = String::new();
        let mut emitted_live_delta = false;
        let mut parsed_any_sse = false;

        loop {
            let chunk = tokio::select! {
                _ = cancellation.cancelled() => return Ok(CloudTranscriptionOutcome::Cancelled),
                chunk = response.chunk() => chunk.map_err(|_| CompletedTransportError::ResponseReadFailed)?,
            };
            let Some(chunk) = chunk else {
                break;
            };
            buffer.extend_from_slice(&chunk);
            raw_body.extend_from_slice(&chunk);

            while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
                let line_bytes = buffer.drain(..=pos).collect::<Vec<u8>>();
                let line = String::from_utf8_lossy(&line_bytes);
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with(':') {
                    continue;
                }
                if let Some(data) = trimmed.strip_prefix("data:") {
                    let data = data.trim();
                    if data == "[DONE]" {
                        break;
                    }
                    if let Ok(event) = serde_json::from_str::<OpenAiSsePayload>(data) {
                        parsed_any_sse = true;
                        if let Some(delta) = event.delta {
                            if !delta.is_empty() {
                                accumulated_text.push_str(&delta);
                                if let Some(on_delta) = on_delta.as_mut() {
                                    on_delta(delta)?;
                                    emitted_live_delta = true;
                                }
                            }
                        } else if event.event_type.as_deref() == Some("transcript.text.delta") {
                            if let Some(text) = event.text {
                                if !text.is_empty() {
                                    accumulated_text.push_str(&text);
                                    if let Some(on_delta) = on_delta.as_mut() {
                                        on_delta(text)?;
                                        emitted_live_delta = true;
                                    }
                                }
                            }
                        } else if event.event_type.as_deref() == Some("transcript.text.done") {
                            if accumulated_text.is_empty() {
                                if let Some(t) = event.text {
                                    accumulated_text = t;
                                }
                            }
                        } else if let Some(text) = event.text {
                            if accumulated_text.is_empty() {
                                accumulated_text = text;
                            }
                        }
                    } else if serde_json::from_str::<OpenAiSseErrorPayload>(data).is_ok() {
                        return Err(CompletedTransportError::ResponseInvalid);
                    }
                }
            }
        }

        if !parsed_any_sse {
            let body_str = String::from_utf8_lossy(&raw_body);
            let trimmed_body = body_str.trim();
            if !trimmed_body.is_empty() {
                if let Ok(parsed) = serde_json::from_str::<TranscriptionResponse>(trimmed_body) {
                    accumulated_text = parsed.text;
                } else {
                    return Err(CompletedTransportError::ResponseInvalid);
                }
            }
        }

        let final_text = accumulated_text.trim().to_string();
        if final_text.is_empty() {
            Ok(CloudTranscriptionOutcome::NoSpeech)
        } else if emitted_live_delta {
            Ok(CloudTranscriptionOutcome::StreamedLive(final_text))
        } else {
            Ok(CloudTranscriptionOutcome::Transcript(final_text))
        }
    } else {
        let body = tokio::select! {
            _ = cancellation.cancelled() => return Ok(CloudTranscriptionOutcome::Cancelled),
            body = response.text() => body.map_err(|_| CompletedTransportError::ResponseReadFailed)?,
        };
        if cancellation.is_cancelled() {
            return Ok(CloudTranscriptionOutcome::Cancelled);
        }

        let parsed: TranscriptionResponse =
            serde_json::from_str(&body).map_err(|_| CompletedTransportError::ResponseInvalid)?;
        let text = parsed.text.trim().to_string();
        let no_speech = match spec.no_speech {
            NoSpeechRule::EmptyText => text.is_empty(),
            NoSpeechRule::GroqSegments => {
                text.is_empty()
                    || (!parsed.segments.is_empty()
                        && parsed
                            .segments
                            .iter()
                            .all(|segment| segment.no_speech_prob.unwrap_or(0.0) >= 0.6))
            }
        };

        if no_speech {
            return Ok(CloudTranscriptionOutcome::NoSpeech);
        }
        Ok(CloudTranscriptionOutcome::Transcript(text))
    }
}

#[cfg(test)]
mod tests;
