use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tauri::AppHandle;

use crate::models::{ProviderId, TranscriptResult};
use crate::output::DeliveryCapability;

use super::LocalTranscriptionError;

pub(super) type LocalSegmentOutput =
    Arc<dyn Fn(String) -> Result<(), LocalTranscriptionError> + Send + Sync + 'static>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LocalTerminalOutput {
    NoSpeech,
    LiveCommitted,
    TerminalClipboard(String),
}

pub(super) enum LocalOutputCommitter {
    LiveInject {
        next_sequence: u64,
        saw_output: bool,
        deliver: LocalSegmentOutput,
    },
    TerminalClipboard {
        next_sequence: u64,
        text: String,
    },
}

impl LocalOutputCommitter {
    pub(super) fn live(deliver: LocalSegmentOutput) -> Self {
        Self::LiveInject {
            next_sequence: 0,
            saw_output: false,
            deliver,
        }
    }

    pub(super) fn terminal_clipboard() -> Self {
        Self::TerminalClipboard {
            next_sequence: 0,
            text: String::new(),
        }
    }

    pub(super) fn commit_chunk(
        &mut self,
        sequence: u64,
        chunk: String,
    ) -> Result<(), LocalTranscriptionError> {
        match self {
            Self::LiveInject {
                next_sequence,
                saw_output,
                deliver,
            } => {
                if sequence != *next_sequence {
                    return Err(LocalTranscriptionError::InferenceProtocol);
                }
                if !chunk.is_empty() {
                    deliver(chunk)?;
                    *saw_output = true;
                }
            }
            Self::TerminalClipboard {
                next_sequence,
                text,
            } => {
                if sequence != *next_sequence {
                    return Err(LocalTranscriptionError::InferenceProtocol);
                }
                text.push_str(&chunk);
            }
        }
        Ok(())
    }

    pub(super) fn advance_sequence(&mut self, sequence: u64) -> Result<(), LocalTranscriptionError> {
        match self {
            Self::LiveInject { next_sequence, .. } | Self::TerminalClipboard { next_sequence, .. } => {
                if sequence != *next_sequence {
                    return Err(LocalTranscriptionError::InferenceProtocol);
                }
                *next_sequence = next_sequence.saturating_add(1);
            }
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub(super) fn commit(
        &mut self,
        sequence: u64,
        segment_text: Option<String>,
    ) -> Result<(), LocalTranscriptionError> {
        if let Some(text) = segment_text {
            self.commit_chunk(sequence, text)?;
        }
        self.advance_sequence(sequence)
    }

    pub(super) fn finish(self) -> LocalTerminalOutput {
        match self {
            Self::LiveInject { saw_output, .. } => {
                if saw_output {
                    LocalTerminalOutput::LiveCommitted
                } else {
                    LocalTerminalOutput::NoSpeech
                }
            }
            Self::TerminalClipboard { text, .. } => {
                if text.is_empty() {
                    LocalTerminalOutput::NoSpeech
                } else {
                    LocalTerminalOutput::TerminalClipboard(text)
                }
            }
        }
    }

    #[cfg(test)]
    pub(super) fn retained_text_len(&self) -> usize {
        match self {
            Self::LiveInject { .. } => 0,
            Self::TerminalClipboard { text, .. } => text.len(),
        }
    }
}

pub(super) type LocalCompletion =
    Arc<dyn Fn(Result<LocalTerminalOutput, LocalTranscriptionError>) + Send + Sync + 'static>;

pub(super) fn complete_once(
    completed: &AtomicBool,
    callback: &LocalCompletion,
    result: Result<LocalTerminalOutput, LocalTranscriptionError>,
) {
    if !completed.swap(true, Ordering::SeqCst) {
        callback(result);
    }
}

pub(super) fn deliver_text(
    app: &AppHandle,
    text: String,
    capability: DeliveryCapability,
) -> Result<(), LocalTranscriptionError> {
    let transcript = TranscriptResult {
        text,
        provider: ProviderId::Local,
    };
    crate::output::deliver(app, &transcript, capability)
        .map_err(|_| LocalTranscriptionError::Delivery)
}
