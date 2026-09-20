//! Cloud transcription branch contracts.
//!
//! Shared application orchestration lives in `app/cloud_jobs.rs`. This module
//! exposes only typed Cloud branch outcomes upward; completed-file implementation
//! and errors are isolated under `completed`, and future live transport must own
//! a separate recording-scoped session implementation and error type.

pub(crate) mod completed;
pub(crate) mod live;

/// Cloud branch terminal outcome. Cancellation is lifecycle state, not a
/// transport/application error. Provider branches return only final text,
/// no-result, or cancellation facts; they do not construct `TranscriptResult`,
/// inspect recording authority, or own desktop delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudTranscriptionOutcome {
    Transcript(String),
    StreamedLive(String),
    NoSpeech,
    Cancelled,
}
