//! Microphone capture subsystem.
//!
//! `capture` owns recording-bound lifecycle/destination state; `device` owns
//! CPAL device/config/stream mechanics; `convert` owns resampling/WAV work.
//! See `docs/AUDIO.md`.

pub mod convert;
pub mod status;

mod capture;
pub(crate) mod cloud_live;
mod device;

pub use capture::{AudioEngine, CloudCapturedAudio};
pub(crate) use capture::{CaptureEvent, CaptureEventKind, StopCaptureResult};
pub use status::{check_microphone_status, MicrophoneStatus};
