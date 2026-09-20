//! CPAL-specific microphone device, configuration, and stream mechanics.

use std::sync::mpsc::Sender;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use super::capture::{
    CaptureDestination, CaptureFailure, CaptureFailureKind, CaptureFailureState, CaptureStream,
};
use super::convert::TARGET_SAMPLE_RATE;

pub(super) struct OpenedCapture {
    pub(super) stream: Box<dyn CaptureStream>,
    pub(super) sample_rate: u32,
}

struct CpalCaptureStream(cpal::Stream);

impl CaptureStream for CpalCaptureStream {
    fn play(&self) -> bool {
        self.0.play().is_ok()
    }
}

/// Open the default input device and build one typed CPAL stream. Recording
/// identity/session ownership remains in `capture.rs`.
pub(super) fn open_capture_stream(
    capture_failure_tx: Sender<CaptureFailure>,
    generation: u64,
    destination: CaptureDestination,
    failure_state: CaptureFailureState,
) -> Option<OpenedCapture> {
    let host = cpal::default_host();
    let device = host.default_input_device()?;
    let supported = select_config(&device)?;
    let config = supported.config();
    let sample_rate = config.sample_rate;
    let channels = config.channels as usize;
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => build_stream(
            &device,
            &config,
            channels,
            destination.clone(),
            &capture_failure_tx,
            generation,
            failure_state.clone(),
            normalize_f32,
        ),
        cpal::SampleFormat::I16 => build_stream(
            &device,
            &config,
            channels,
            destination.clone(),
            &capture_failure_tx,
            generation,
            failure_state.clone(),
            normalize_i16,
        ),
        cpal::SampleFormat::U16 => build_stream(
            &device,
            &config,
            channels,
            destination,
            &capture_failure_tx,
            generation,
            failure_state,
            normalize_u16,
        ),
        _ => return None,
    }?;

    Some(OpenedCapture {
        stream,
        sample_rate,
    })
}

fn normalize_f32(sample: f32) -> f32 {
    sample
}

fn normalize_i16(sample: i16) -> f32 {
    f32::from(sample) / 32768.0
}

fn normalize_u16(sample: u16) -> f32 {
    (f32::from(sample) - 32768.0) / 32768.0
}

fn select_config(device: &cpal::Device) -> Option<cpal::SupportedStreamConfig> {
    let preferred = device.supported_input_configs().ok()?.find(|range| {
        range.channels() == 1
            && range.sample_format() == cpal::SampleFormat::F32
            && range.min_sample_rate() <= TARGET_SAMPLE_RATE
            && range.max_sample_rate() >= TARGET_SAMPLE_RATE
    });
    match preferred {
        Some(range) => Some(range.with_sample_rate(TARGET_SAMPLE_RATE)),
        // Fall back to the device's native config; convert.rs resamples.
        None => device.default_input_config().ok(),
    }
}

/// Convert one CPAL block to mono and dispatch it to exactly one explicit
/// destination. Live-feed failures retain their typed cause for the control worker.
pub(super) fn capture_input_block<T: Copy>(
    data: &[T],
    channels: usize,
    convert: fn(T) -> f32,
    destination: &CaptureDestination,
) -> Result<(), CaptureFailureKind> {
    match destination {
        CaptureDestination::LocalLive(feed) => {
            let mono: Vec<f32> = if channels == 1 {
                data.iter().map(|&sample| convert(sample)).collect()
            } else {
                data.chunks(channels)
                    .map(|frame| {
                        frame.iter().map(|&sample| convert(sample)).sum::<f32>()
                            / frame.len() as f32
                    })
                    .collect()
            };
            feed.push(mono).map_err(CaptureFailureKind::from)
        }
        CaptureDestination::CloudCompleted(buffer) => {
            buffer.push(data, channels, convert);
            Ok(())
        }
        CaptureDestination::CloudLive(feed) => {
            let mono: Vec<f32> = if channels == 1 {
                data.iter().map(|&sample| convert(sample)).collect()
            } else {
                data.chunks(channels)
                    .map(|frame| {
                        frame.iter().map(|&sample| convert(sample)).sum::<f32>()
                            / frame.len() as f32
                    })
                    .collect()
            };
            feed.push(mono).map_err(CaptureFailureKind::from)
        }
    }
}

/// Build a typed input stream that converts to f32 and downmixes to mono in
/// the CPAL callback. Failure handling itself remains outside that callback.
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use parking_lot::Mutex;

    use crate::transcription::local::LocalAudioFeed;

    use super::*;

    fn collecting_local_destination() -> (CaptureDestination, Arc<Mutex<Vec<Vec<f32>>>>) {
        let blocks = Arc::new(Mutex::new(Vec::new()));
        let pushed = Arc::clone(&blocks);
        let feed = LocalAudioFeed::new(
            |_| true,
            move |samples| {
                pushed.lock().push(samples);
                Ok(())
            },
            || true,
            || {},
        );
        (CaptureDestination::LocalLive(feed), blocks)
    }

    #[test]
    fn supported_sample_formats_normalize_to_f32() {
        let (f32_destination, f32_blocks) = collecting_local_destination();
        assert_eq!(
            capture_input_block(&[-1.0f32, 0.25, 1.0], 1, normalize_f32, &f32_destination),
            Ok(())
        );
        assert_eq!(&*f32_blocks.lock(), &[vec![-1.0, 0.25, 1.0]]);

        let (i16_destination, i16_blocks) = collecting_local_destination();
        assert_eq!(
            capture_input_block(&[i16::MIN, 0, i16::MAX], 1, normalize_i16, &i16_destination),
            Ok(())
        );
        let i16_values = &i16_blocks.lock()[0];
        assert_eq!(i16_values[0], -1.0);
        assert_eq!(i16_values[1], 0.0);
        assert!((i16_values[2] - (32767.0 / 32768.0)).abs() < f32::EPSILON);

        let (u16_destination, u16_blocks) = collecting_local_destination();
        assert_eq!(
            capture_input_block(
                &[u16::MIN, 32768, u16::MAX],
                1,
                normalize_u16,
                &u16_destination
            ),
            Ok(())
        );
        let u16_values = &u16_blocks.lock()[0];
        assert_eq!(u16_values[0], -1.0);
        assert_eq!(u16_values[1], 0.0);
        assert!((u16_values[2] - (32767.0 / 32768.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn multichannel_frames_are_averaged_before_local_handoff() {
        let (destination, blocks) = collecting_local_destination();
        assert_eq!(
            capture_input_block(&[1.0f32, -1.0, 0.5, 0.5], 2, normalize_f32, &destination),
            Ok(())
        );
        assert_eq!(&*blocks.lock(), &[vec![0.0, 0.5]]);
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    destination: CaptureDestination,
    capture_failure_tx: &Sender<CaptureFailure>,
    generation: u64,
    failure_state: CaptureFailureState,
    convert: fn(T) -> f32,
) -> Option<Box<dyn CaptureStream>>
where
    T: cpal::SizedSample + 'static,
{
    let data_failure_tx = capture_failure_tx.clone();
    let data_failure_state = failure_state.clone();
    let stream_error_tx = capture_failure_tx.clone();
    let stream_error_state = failure_state;
    let stream = device
        .build_input_stream(
            *config,
            move |data: &[T], _| {
                if let Err(kind) = capture_input_block(data, channels, convert, &destination) {
                    data_failure_state.record(kind);
                    let _ = data_failure_tx.send(CaptureFailure { generation, kind });
                }
            },
            move |err: cpal::Error| {
                match err.kind() {
                    cpal::ErrorKind::Xrun
                    | cpal::ErrorKind::RealtimeDenied
                    | cpal::ErrorKind::DeviceChanged => {
                        // Transient hardware buffer overrun/underrun or dynamic routing change.
                        // The stream remains active; do not abort the recording session.
                    }
                    _ => {
                        stream_error_state.record(CaptureFailureKind::Stream);
                        let _ = stream_error_tx.send(CaptureFailure {
                            generation,
                            kind: CaptureFailureKind::Stream,
                        });
                    }
                }
            },
            None,
        )
        .ok()?;
    Some(Box::new(CpalCaptureStream(stream)))
}
