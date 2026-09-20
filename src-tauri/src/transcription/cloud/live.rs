//! OpenAI Realtime WebSocket live transcription transport (`docs/OPENAI.md`).
//!
//! This module owns the WebSocket connection lifecycle to the OpenAI Realtime API:
//! session initialization, streaming audio chunks via `input_audio_buffer.append`,
//! turn commitment on hotkey release, real-time delta event parsing, and pointer-targeted
//! live text injection.

use std::time::Duration;

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use crate::app_state::AppState;
use crate::audio::convert::StreamingResampler;
use crate::audio::cloud_live::{CloudLiveFeed, CloudLiveFeedError};
use crate::models::RecordingIdentity;
use crate::output::DeliveryCapability;
use crate::recording;
use crate::transcription::cloud::CloudTranscriptionOutcome;

pub(crate) const OPENAI_REALTIME_WS_BASE_URL: &str = "wss://api.openai.com/v1/realtime?intent=transcription";

const TARGET_LIVE_SAMPLE_RATE: u32 = 24_000;

#[derive(Debug)]
pub(crate) enum LiveAudioCommand {
    Start(u32),
    Samples(Vec<f32>),
    Stop,
    Abort,
}

#[derive(Debug, Serialize)]
struct SessionUpdateMessage<'a> {
    #[serde(rename = "type")]
    msg_type: &'static str,
    session: LiveSessionConfig<'a>,
}

#[derive(Debug, Serialize)]
struct LiveSessionConfig<'a> {
    #[serde(rename = "type")]
    session_type: &'static str,
    audio: LiveAudioConfig<'a>,
}

#[derive(Debug, Serialize)]
struct LiveAudioConfig<'a> {
    input: LiveAudioInputConfig<'a>,
}

#[derive(Debug, Serialize)]
struct LiveAudioInputConfig<'a> {
    format: LiveAudioFormat,
    transcription: LiveTranscriptionParams<'a>,
    turn_detection: Option<()>,
}

#[derive(Debug, Serialize)]
struct LiveAudioFormat {
    #[serde(rename = "type")]
    format_type: &'static str,
    rate: u32,
}

#[derive(Debug, Serialize)]
struct LiveTranscriptionParams<'a> {
    model: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    languages: Option<Vec<&'a str>>,
}

#[derive(Debug, Serialize)]
struct AudioAppendMessage {
    #[serde(rename = "type")]
    msg_type: &'static str,
    audio: String,
}

#[derive(Debug, Serialize)]
struct AudioCommitMessage {
    #[serde(rename = "type")]
    msg_type: &'static str,
}

#[derive(Debug, Deserialize)]
struct OpenAiSsePayload {
    #[serde(rename = "type")]
    event_type: Option<String>,
    delta: Option<String>,
    transcript: Option<String>,
    error: Option<serde_json::Value>,
}

pub(crate) fn create_cloud_live_feed(
    audio_tx: mpsc::Sender<LiveAudioCommand>,
) -> CloudLiveFeed {
    let start_tx = audio_tx.clone();
    let samples_tx = audio_tx.clone();
    let stop_tx = audio_tx.clone();
    let abort_tx = audio_tx;

    CloudLiveFeed::new(
        move |rate| {
            start_tx
                .blocking_send(LiveAudioCommand::Start(rate))
                .map_err(|_| CloudLiveFeedError::Closed)
        },
        move |samples| {
            samples_tx
                .blocking_send(LiveAudioCommand::Samples(samples))
                .map_err(|_| CloudLiveFeedError::Closed)
        },
        move || {
            stop_tx
                .blocking_send(LiveAudioCommand::Stop)
                .map_err(|_| CloudLiveFeedError::Closed)
        },
        move || {
            let _ = abort_tx.blocking_send(LiveAudioCommand::Abort);
        },
    )
}

fn float_samples_to_pcm16_base64(samples: &[f32]) -> String {
    let mut pcm16 = Vec::with_capacity(samples.len() * 2);
    for &sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        let sample_i16 = (clamped * 32767.0) as i16;
        pcm16.extend_from_slice(&sample_i16.to_le_bytes());
    }
    base64::engine::general_purpose::STANDARD.encode(&pcm16)
}

pub(crate) async fn run_live_transcription(
    app: AppHandle,
    id: RecordingIdentity,
    api_key: String,
    model: String,
    mut audio_rx: mpsc::Receiver<LiveAudioCommand>,
    cancellation: CancellationToken,
) {
    let outcome = run_live_transcription_inner(
        &app,
        id,
        OPENAI_REALTIME_WS_BASE_URL,
        &api_key,
        &model,
        &mut audio_rx,
        &cancellation,
    )
    .await;

    match outcome {
        Ok(CloudTranscriptionOutcome::Transcript(text)) => {
            recording::run_if_authoritative(&app, id, || {
                let capability = app.state::<AppState>().delivery_capability();
                let result = crate::models::TranscriptResult {
                    text,
                    provider: crate::models::ProviderId::Openai,
                };
                let _ = crate::output::deliver(&app, &result, capability);
            });
            recording::complete_cloud(&app, id);
        }
        Ok(CloudTranscriptionOutcome::StreamedLive(_)) => {
            recording::complete_cloud(&app, id);
        }
        Ok(CloudTranscriptionOutcome::NoSpeech | CloudTranscriptionOutcome::Cancelled) => {
            recording::complete_cloud(&app, id);
        }
        Err(err) => {
            recording::run_if_authoritative(&app, id, || {
                crate::app::emit_transcription_error(&app, err);
            });
            recording::complete_cloud(&app, id);
        }
    }
}

pub(crate) async fn run_live_transcription_inner(
    app: &AppHandle,
    id: RecordingIdentity,
    ws_url: &str,
    api_key: &str,
    model: &str,
    audio_rx: &mut mpsc::Receiver<LiveAudioCommand>,
    cancellation: &CancellationToken,
) -> Result<CloudTranscriptionOutcome, crate::transcription::TranscriptionError> {
    if cancellation.is_cancelled() {
        return Ok(CloudTranscriptionOutcome::Cancelled);
    }

    let mut request = ws_url
        .into_client_request()
        .map_err(|_| crate::transcription::TranscriptionError::Cloud(crate::transcription::CloudFailure::Unexpected))?;

    let headers = request.headers_mut();
    headers.insert(
        "Authorization",
        format!("Bearer {api_key}")
            .parse()
            .map_err(|_| crate::transcription::TranscriptionError::Cloud(crate::transcription::CloudFailure::Authentication))?,
    );

    let connect_future = tokio_tungstenite::connect_async(request);
    let (ws_stream, _response) = tokio::select! {
        _ = cancellation.cancelled() => return Ok(CloudTranscriptionOutcome::Cancelled),
        res = connect_future => {
            match res {
                Ok((stream, resp)) => {
                    (stream, resp)
                }
                Err(_) => {
                    return Err(crate::transcription::TranscriptionError::Cloud(crate::transcription::CloudFailure::Service));
                }
            }
        }
    };

    let (mut ws_writer, mut ws_reader) = ws_stream.split();

    let language_setting = env!("PORTUS_OPENAI_LANGUAGE");
    let languages = if language_setting == "auto" || language_setting.is_empty() {
        None
    } else {
        Some(vec![language_setting])
    };

    let session_update = SessionUpdateMessage {
        msg_type: "session.update",
        session: LiveSessionConfig {
            session_type: "transcription",
            audio: LiveAudioConfig {
                input: LiveAudioInputConfig {
                    format: LiveAudioFormat {
                        format_type: "audio/pcm",
                        rate: TARGET_LIVE_SAMPLE_RATE,
                    },
                    transcription: LiveTranscriptionParams {
                        model,
                        languages,
                    },
                    turn_detection: None,
                },
            },
        },
    };

    let session_json = serde_json::to_string(&session_update)
        .map_err(|_| crate::transcription::TranscriptionError::Cloud(crate::transcription::CloudFailure::Unexpected))?;

    ws_writer
        .send(Message::Text(session_json.into()))
        .await
        .map_err(|_| {
            crate::transcription::TranscriptionError::Cloud(crate::transcription::CloudFailure::Network)
        })?;

    let mut resampler: Option<StreamingResampler> = None;
    let app_for_reader = app.clone();
    let reader_cancellation = cancellation.clone();

    // Spawn reader task to handle inbound text deltas
    let (reader_done_tx, mut reader_done_rx) = mpsc::channel::<Result<String, ()>>(1);
    let reader_handle = tokio::spawn(async move {
        while let Some(msg) = ws_reader.next().await {
            if reader_cancellation.is_cancelled() {
                break;
            }
            let Ok(msg) = msg else {
                break;
            };
            if let Message::Text(text) = msg {
                if let Ok(event) = serde_json::from_str::<OpenAiSsePayload>(&text) {
                    if event.error.is_some() {
                        let _ = reader_done_tx.send(Err(())).await;
                        return;
                    }
                    if let Some(delta) = event.delta {
                        if !delta.is_empty() {
                            let _ = recording::run_if_authoritative(&app_for_reader, id, || {
                                let capability = app_for_reader.state::<AppState>().delivery_capability();
                                if capability == DeliveryCapability::Inject {
                                    let _ = crate::output::inject(&app_for_reader, &delta);
                                }
                            });
                        }
                    } else if event.event_type.as_deref() == Some("conversation.item.input_audio_transcription.completed") {
                        let final_transcript = event.transcript.unwrap_or_default();
                        let _ = reader_done_tx.send(Ok(final_transcript)).await;
                        return;
                    }
                }
            }
        }
    });

    // Outbound pump loop
    loop {
        let cmd = tokio::select! {
            _ = cancellation.cancelled() => {
                reader_handle.abort();
                return Ok(CloudTranscriptionOutcome::Cancelled);
            }
            cmd = audio_rx.recv() => {
                match cmd {
                    Some(c) => c,
                    None => break,
                }
            }
        };

        match cmd {
            LiveAudioCommand::Start(rate) => {
                if rate != TARGET_LIVE_SAMPLE_RATE {
                    resampler = StreamingResampler::to_rate(rate, TARGET_LIVE_SAMPLE_RATE).ok();
                }
            }
            LiveAudioCommand::Samples(samples) => {
                let converted_samples = if let Some(r) = resampler.as_mut() {
                    r.feed(&samples).unwrap_or_default()
                } else {
                    samples
                };
                if !converted_samples.is_empty() {
                    let base64_audio = float_samples_to_pcm16_base64(&converted_samples);
                    let append_msg = AudioAppendMessage {
                        msg_type: "input_audio_buffer.append",
                        audio: base64_audio,
                    };
                    if let Ok(json) = serde_json::to_string(&append_msg) {
                        let _ = ws_writer.send(Message::Text(json.into())).await;
                    }
                }
            }
            LiveAudioCommand::Stop => {
                let commit_msg = AudioCommitMessage {
                    msg_type: "input_audio_buffer.commit",
                };
                if let Ok(json) = serde_json::to_string(&commit_msg) {
                    let _ = ws_writer.send(Message::Text(json.into())).await;
                }
                break;
            }
            LiveAudioCommand::Abort => {
                reader_handle.abort();
                return Ok(CloudTranscriptionOutcome::Cancelled);
            }
        }
    }

    // Wait for final completion event from reader
    let completion_result = tokio::select! {
        _ = cancellation.cancelled() => {
            reader_handle.abort();
            return Ok(CloudTranscriptionOutcome::Cancelled);
        }
        res = reader_done_rx.recv() => res,
        _ = tokio::time::sleep(Duration::from_secs(10)) => None,
    };

    let _ = ws_writer.close().await;
    reader_handle.abort();

    match completion_result {
        Some(Ok(final_text)) => {
            let trimmed = final_text.trim().to_string();
            if trimmed.is_empty() {
                Ok(CloudTranscriptionOutcome::NoSpeech)
            } else {
                let capability = app.state::<AppState>().delivery_capability();
                if capability == DeliveryCapability::Inject {
                    Ok(CloudTranscriptionOutcome::StreamedLive(trimmed))
                } else {
                    Ok(CloudTranscriptionOutcome::Transcript(trimmed))
                }
            }
        }
        Some(Err(())) => Err(crate::transcription::TranscriptionError::Cloud(crate::transcription::CloudFailure::Service)),
        None => Ok(CloudTranscriptionOutcome::NoSpeech),
    }
}

#[cfg(test)]
mod tests;
