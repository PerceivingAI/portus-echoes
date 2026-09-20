//! PCM format conversion for the transcription layer — fully in-memory.
//!
//! Local sessions use `StreamingResampler` incrementally before stateful VAD.
//! The `CompletedAudio` Cloud branch uses the one-shot helpers to resample to
//! 16 kHz mono and encode WAV bytes (`hound`) after stop. `LiveAudio` does not
//! use these complete-buffer helpers. Audio never touches disk. See `docs/AUDIO.md`.

use rubato::{Fft, FixedSync, Indexing, Resampler};

/// Sample rate required by Local whisper processing and the current
/// completed-audio Cloud WAV branch. Live Cloud transport owns its own later
/// provider-specific audio contract.
pub const TARGET_SAMPLE_RATE: u32 = 16_000;

/// Structured failures from in-memory audio conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioConversionError {
    InvalidSampleRate,
    ResamplerSetup,
    InvalidInput,
    Resampling,
    StreamFinalized,
    WavStart,
    WavWrite,
    WavFinalize,
    Mp3Encode,
}

/// Resample a mono f32 buffer from `input_rate` Hz to 16 kHz mono.
///
/// Returns a clone of the input when already at the target rate; empty
/// input yields empty output. `input_rate` must be > 0.
#[allow(dead_code)] // complete-buffer helper used by CompletedAudio and tests
pub fn resample_to_16k_mono(
    samples: &[f32],
    input_rate: u32,
) -> Result<Vec<f32>, AudioConversionError> {
    if input_rate == 0 {
        return Err(AudioConversionError::InvalidSampleRate);
    }
    if samples.is_empty() || input_rate == TARGET_SAMPLE_RATE {
        return Ok(samples.to_vec());
    }
    let mut resampler = Fft::<f32>::new(
        input_rate as usize,
        TARGET_SAMPLE_RATE as usize,
        1024,
        1,
        FixedSync::Input,
    )
    .map_err(|_| AudioConversionError::ResamplerSetup)?;
    let channels: [&[f32]; 1] = [samples];
    let input = rubato::audioadapter_buffers::direct::SequentialSliceOfSlices::new(
        &channels,
        1,
        samples.len(),
    )
    .map_err(|_| AudioConversionError::InvalidInput)?;
    // `process_all` resets state, chunks internally, and trims the startup
    // delay, so the output length is `input_len * rate_ratio`.
    let output = resampler
        .process_all(&input, samples.len(), None)
        .map_err(|_| AudioConversionError::Resampling)?;
    Ok(output.take_data())
}

/// Stateful Local-only converter from mono device-rate f32 PCM to continuous
/// 16 kHz mono PCM.
///
/// Unlike `resample_to_16k_mono`, this never resets the resampler between
/// `feed` calls. It mirrors rubato's whole-clip alignment rules by trimming the
/// startup delay exactly once and, on `flush`, pumping zero-input chunks until
/// the expected `ceil(input_frames * ratio)` output length is complete.
pub struct StreamingResampler {
    input_rate: u32,
    resampler: Option<Fft<f32>>,
    pending_input: Vec<f32>,
    delay_remaining: usize,
    total_input_frames: usize,
    emitted_output_frames: usize,
    finalized: bool,
}

impl StreamingResampler {
    pub fn new(input_rate: u32) -> Result<Self, AudioConversionError> {
        Self::to_rate(input_rate, TARGET_SAMPLE_RATE)
    }

    pub fn to_rate(input_rate: u32, target_rate: u32) -> Result<Self, AudioConversionError> {
        if input_rate == 0 || target_rate == 0 {
            return Err(AudioConversionError::InvalidSampleRate);
        }

        if input_rate == target_rate {
            return Ok(Self {
                input_rate,
                resampler: None,
                pending_input: Vec::new(),
                delay_remaining: 0,
                total_input_frames: 0,
                emitted_output_frames: 0,
                finalized: false,
            });
        }

        let resampler = Fft::<f32>::new(
            input_rate as usize,
            target_rate as usize,
            1024,
            1,
            FixedSync::Input,
        )
        .map_err(|_| AudioConversionError::ResamplerSetup)?;
        let delay_remaining = resampler.output_delay();

        Ok(Self {
            input_rate,
            resampler: Some(resampler),
            pending_input: Vec::new(),
            delay_remaining,
            total_input_frames: 0,
            emitted_output_frames: 0,
            finalized: false,
        })
    }

    /// Feed the next sequential mono device-rate PCM block.
    ///
    /// Output contains only aligned 16 kHz samples that are already available;
    /// any incomplete resampler input chunk remains buffered for the next call.
    pub fn feed(&mut self, samples: &[f32]) -> Result<Vec<f32>, AudioConversionError> {
        if self.finalized {
            return Err(AudioConversionError::StreamFinalized);
        }
        if samples.is_empty() {
            return Ok(Vec::new());
        }

        self.total_input_frames += samples.len();

        if self.resampler.is_none() {
            self.emitted_output_frames += samples.len();
            return Ok(samples.to_vec());
        }

        self.pending_input.extend_from_slice(samples);
        let mut output = Vec::new();

        loop {
            let needed = self
                .resampler
                .as_ref()
                .expect("non-target sample rate owns a resampler")
                .input_frames_next();
            if self.pending_input.len() < needed {
                break;
            }

            let chunk: Vec<f32> = self.pending_input.drain(..needed).collect();
            let produced = self.process_chunk(&chunk, None)?;
            self.append_aligned(produced, &mut output, None);
        }

        Ok(output)
    }

    /// Finish the stream and return all remaining aligned 16 kHz PCM.
    ///
    /// A final partial chunk is zero-padded through rubato's `partial_len`
    /// contract, then pure-silence chunks flush the filter delay. Extra padding
    /// is truncated so total output length exactly matches the one-shot helper.
    pub fn flush(&mut self) -> Result<Vec<f32>, AudioConversionError> {
        if self.finalized {
            return Err(AudioConversionError::StreamFinalized);
        }
        self.finalized = true;

        if self.resampler.is_none() || self.total_input_frames == 0 {
            return Ok(Vec::new());
        }

        let expected_output_frames = ((self.total_input_frames as f64 * TARGET_SAMPLE_RATE as f64)
            / self.input_rate as f64)
            .ceil() as usize;
        let mut output = Vec::new();

        if !self.pending_input.is_empty() {
            let needed = self
                .resampler
                .as_ref()
                .expect("non-target sample rate owns a resampler")
                .input_frames_next();
            let pending = std::mem::take(&mut self.pending_input);
            let valid = pending.len();
            let mut padded = vec![0.0f32; needed];
            padded[..valid].copy_from_slice(&pending);
            let produced = self.process_chunk(&padded, Some(valid))?;
            let remaining = expected_output_frames.saturating_sub(self.emitted_output_frames);
            self.append_aligned(produced, &mut output, Some(remaining));
        }

        while self.emitted_output_frames < expected_output_frames {
            let needed = self
                .resampler
                .as_ref()
                .expect("non-target sample rate owns a resampler")
                .input_frames_next();
            let silence = vec![0.0f32; needed];
            let produced = self.process_chunk(&silence, Some(0))?;
            let remaining = expected_output_frames - self.emitted_output_frames;
            self.append_aligned(produced, &mut output, Some(remaining));
        }

        Ok(output)
    }

    fn process_chunk(
        &mut self,
        samples: &[f32],
        partial_len: Option<usize>,
    ) -> Result<Vec<f32>, AudioConversionError> {
        let channels: [&[f32]; 1] = [samples];
        let input = rubato::audioadapter_buffers::direct::SequentialSliceOfSlices::new(
            &channels,
            1,
            samples.len(),
        )
        .map_err(|_| AudioConversionError::InvalidInput)?;
        let indexing = partial_len.map(|len| Indexing::new().partial_len(len));
        self.resampler
            .as_mut()
            .expect("non-target sample rate owns a resampler")
            .process(&input, indexing.as_ref())
            .map(|output| output.take_data())
            .map_err(|_| AudioConversionError::Resampling)
    }

    fn append_aligned(
        &mut self,
        mut produced: Vec<f32>,
        output: &mut Vec<f32>,
        max_frames: Option<usize>,
    ) {
        if self.delay_remaining > 0 {
            let trim = self.delay_remaining.min(produced.len());
            produced.drain(..trim);
            self.delay_remaining -= trim;
        }
        if let Some(max_frames) = max_frames {
            produced.truncate(max_frames);
        }
        self.emitted_output_frames += produced.len();
        output.extend(produced);
    }
}

/// Encode f32 samples (already at 16 kHz mono) as an in-memory 16-bit PCM
/// WAV for the cloud upload path. Out-of-range samples are clamped.
#[allow(dead_code)] // consumed by the current Cloud upload path
pub fn encode_wav_16bit_mono(samples: &[f32]) -> Result<Vec<u8>, AudioConversionError> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: TARGET_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = std::io::Cursor::new(Vec::with_capacity(64 + samples.len() * 2));
    {
        let mut writer =
            hound::WavWriter::new(&mut cursor, spec).map_err(|_| AudioConversionError::WavStart)?;
        for &sample in samples {
            let clamped = sample.clamp(-1.0, 1.0);
            let value = (clamped * i16::MAX as f32).round() as i16;
            writer
                .write_sample(value)
                .map_err(|_| AudioConversionError::WavWrite)?;
        }
        writer
            .finalize()
            .map_err(|_| AudioConversionError::WavFinalize)?;
    }
    Ok(cursor.into_inner())
}

/// One-shot helper for the completed-audio Cloud branch: resample from
/// `input_rate` and encode to in-memory WAV bytes.
#[allow(dead_code)] // consumed by the current CompletedAudio upload path
pub fn to_wav_bytes(samples: &[f32], input_rate: u32) -> Result<Vec<u8>, AudioConversionError> {
    let resampled = resample_to_16k_mono(samples, input_rate)?;
    encode_wav_16bit_mono(&resampled)
}

/// Encode f32 samples (already at 16 kHz mono) as an in-memory MP3
/// for the cloud upload path. Out-of-range samples are clamped.
pub fn encode_mp3_16k_mono(samples: &[f32]) -> Result<Vec<u8>, AudioConversionError> {
    use mp3lame_encoder::{Bitrate, Builder, FlushNoGap, MonoPcm, Quality};

    let mut encoder = Builder::new()
        .ok_or(AudioConversionError::Mp3Encode)?
        .with_num_channels(1)
        .map_err(|_| AudioConversionError::Mp3Encode)?
        .with_sample_rate(TARGET_SAMPLE_RATE)
        .map_err(|_| AudioConversionError::Mp3Encode)?
        .with_brate(Bitrate::Kbps64)
        .map_err(|_| AudioConversionError::Mp3Encode)?
        .with_quality(Quality::Best)
        .map_err(|_| AudioConversionError::Mp3Encode)?
        .build()
        .map_err(|_| AudioConversionError::Mp3Encode)?;

    let i16_samples: Vec<i16> = samples
        .iter()
        .map(|&sample| (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16)
        .collect();

    let input = MonoPcm(&i16_samples);
    let mut mp3_out = Vec::new();
    mp3_out.reserve(mp3lame_encoder::max_required_buffer_size(i16_samples.len()));
    encoder
        .encode_to_vec(input, &mut mp3_out)
        .map_err(|_| AudioConversionError::Mp3Encode)?;
    mp3_out.reserve(7200);
    encoder
        .flush_to_vec::<FlushNoGap>(&mut mp3_out)
        .map_err(|_| AudioConversionError::Mp3Encode)?;
    Ok(mp3_out)
}

/// One-shot helper for the completed-audio Cloud branch: resample from
/// `input_rate` and encode to in-memory MP3 bytes.
pub fn to_mp3_bytes(samples: &[f32], input_rate: u32) -> Result<Vec<u8>, AudioConversionError> {
    let resampled = resample_to_16k_mono(samples, input_rate)?;
    encode_mp3_16k_mono(&resampled)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, rate: u32, seconds: f32) -> Vec<f32> {
        let n = (rate as f32 * seconds) as usize;
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin() * 0.5)
            .collect()
    }

    #[test]
    fn passthrough_at_target_rate() {
        let input = vec![0.1, -0.2, 0.3];
        let out = resample_to_16k_mono(&input, TARGET_SAMPLE_RATE).unwrap();
        assert_eq!(out, input);
    }

    #[test]
    fn empty_input_yields_empty_output() {
        assert!(resample_to_16k_mono(&[], 44_100).unwrap().is_empty());
    }

    #[test]
    fn zero_rate_is_rejected() {
        assert_eq!(
            resample_to_16k_mono(&[0.0], 0),
            Err(AudioConversionError::InvalidSampleRate)
        );
    }

    #[test]
    fn downsample_44100_to_16000() {
        let input = sine(440.0, 44_100, 1.0);
        let out = resample_to_16k_mono(&input, 44_100).unwrap();
        // One second of audio at 16 kHz — allow a few frames of slack.
        assert!(
            (out.len() as i64 - 16_000).abs() <= 8,
            "expected ~16000 frames, got {}",
            out.len()
        );
        assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
        // The signal must survive resampling (not collapse to silence).
        let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.3, "peak {peak} too low — signal lost");
    }

    #[test]
    fn upsample_8000_to_16000() {
        let input = sine(440.0, 8_000, 0.5);
        let out = resample_to_16k_mono(&input, 8_000).unwrap();
        assert!((out.len() as i64 - 8_000).abs() <= 8);
        assert!(out.iter().all(|s| s.is_finite()));
    }

    fn stream_in_chunks(
        input: &[f32],
        input_rate: u32,
        chunk_sizes: &[usize],
    ) -> Result<Vec<f32>, AudioConversionError> {
        let mut stream = StreamingResampler::new(input_rate)?;
        let mut output = Vec::new();
        let mut offset = 0usize;
        let mut chunk_index = 0usize;
        while offset < input.len() {
            let requested = chunk_sizes[chunk_index % chunk_sizes.len()];
            let end = (offset + requested).min(input.len());
            output.extend(stream.feed(&input[offset..end])?);
            offset = end;
            chunk_index += 1;
        }
        output.extend(stream.flush()?);
        Ok(output)
    }

    #[test]
    fn streaming_is_invariant_across_arbitrary_chunk_boundaries() {
        let input = sine(440.0, 44_100, 1.37);
        let one_shot = resample_to_16k_mono(&input, 44_100).unwrap();
        let fragmented = stream_in_chunks(&input, 44_100, &[1, 7, 313, 2_048, 97, 511]).unwrap();
        let coarse = stream_in_chunks(&input, 44_100, &[8_191, 17_003]).unwrap();

        // Streaming and the existing one-shot path agree on the exact duration.
        assert_eq!(fragmented.len(), one_shot.len());
        assert_eq!(coarse.len(), one_shot.len());

        // Device callback boundaries must have zero effect on the resampled PCM.
        assert_eq!(fragmented.len(), coarse.len());
        let max_error = fragmented
            .iter()
            .zip(coarse.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_error < 1e-7,
            "streaming output depends on feed chunk boundaries: max error {max_error}"
        );

        // A 440 Hz, 0.5-amplitude sine at 16 kHz cannot legitimately jump by
        // anything close to full scale between adjacent samples. This catches
        // resets/discontinuities at internal processing boundaries.
        let max_step = fragmented
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_step < 0.15,
            "streaming discontinuity detected: step {max_step}"
        );
    }

    #[test]
    fn streaming_output_has_exact_ceil_ratio_length_and_preserves_signal() {
        let input = sine(997.0, 48_000, 0.731);
        let streamed = stream_in_chunks(&input, 48_000, &[17, 1_001, 64, 333]).unwrap();
        let expected =
            ((input.len() as f64 * TARGET_SAMPLE_RATE as f64) / 48_000.0).ceil() as usize;
        assert_eq!(streamed.len(), expected);
        assert!(streamed.iter().all(|sample| sample.is_finite()));
        let peak = streamed
            .iter()
            .fold(0.0f32, |max, sample| max.max(sample.abs()));
        assert!(
            peak > 0.3,
            "streaming signal peak {peak} is unexpectedly low"
        );
    }

    #[test]
    fn streaming_short_final_chunk_is_emitted_only_by_flush() {
        let input = sine(440.0, 44_100, 0.01);
        let one_shot = resample_to_16k_mono(&input, 44_100).unwrap();
        let mut stream = StreamingResampler::new(44_100).unwrap();

        let before_flush = stream.feed(&input).unwrap();
        assert!(before_flush.is_empty());
        let tail = stream.flush().unwrap();

        assert_eq!(tail.len(), one_shot.len());
        let split = stream_in_chunks(&input, 44_100, &[1, 13, 29, 71]).unwrap();
        assert_eq!(tail, split);
        assert!(tail.iter().all(|sample| sample.is_finite()));
        assert!(
            tail.iter()
                .fold(0.0f32, |max, sample| max.max(sample.abs()))
                > 0.3,
            "final flush lost the input signal"
        );
    }

    #[test]
    fn streaming_empty_input_flushes_empty() {
        let mut stream = StreamingResampler::new(44_100).unwrap();
        assert!(stream.feed(&[]).unwrap().is_empty());
        assert!(stream.flush().unwrap().is_empty());
    }

    #[test]
    fn streaming_target_rate_is_exact_passthrough() {
        let input = sine(440.0, TARGET_SAMPLE_RATE, 0.1);
        let streamed = stream_in_chunks(&input, TARGET_SAMPLE_RATE, &[3, 29, 101]).unwrap();
        assert_eq!(streamed, input);
    }

    #[test]
    fn streaming_zero_rate_is_rejected() {
        assert!(matches!(
            StreamingResampler::new(0),
            Err(AudioConversionError::InvalidSampleRate)
        ));
    }

    #[test]
    fn streaming_rejects_feed_or_second_flush_after_finalization() {
        let mut stream = StreamingResampler::new(44_100).unwrap();
        stream.feed(&[0.1; 200]).unwrap();
        stream.flush().unwrap();
        assert_eq!(
            stream.feed(&[0.2]),
            Err(AudioConversionError::StreamFinalized)
        );
        assert_eq!(stream.flush(), Err(AudioConversionError::StreamFinalized));
    }

    #[test]
    fn wav_roundtrip_preserves_samples() {
        let input: Vec<f32> = vec![0.0, 0.5, -0.5, 1.0, -1.0, 0.25];
        let bytes = encode_wav_16bit_mono(&input).unwrap();
        let mut reader = hound::WavReader::new(std::io::Cursor::new(bytes)).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, TARGET_SAMPLE_RATE);
        assert_eq!(spec.bits_per_sample, 16);
        assert_eq!(spec.sample_format, hound::SampleFormat::Int);
        let decoded: Vec<f32> = reader
            .samples::<i16>()
            .map(|s| s.unwrap() as f32 / i16::MAX as f32)
            .collect();
        assert_eq!(decoded.len(), input.len());
        for (got, want) in decoded.iter().zip(input.iter()) {
            assert!(
                (got - want).abs() < 1.0 / i16::MAX as f32 + 1e-6,
                "got {got}, want {want}"
            );
        }
    }

    #[test]
    fn wav_clamps_out_of_range_samples() {
        let bytes = encode_wav_16bit_mono(&[2.0, -3.0]).unwrap();
        let mut reader = hound::WavReader::new(std::io::Cursor::new(bytes)).unwrap();
        let samples: Vec<i16> = reader.samples::<i16>().map(|s| s.unwrap()).collect();
        assert_eq!(samples, vec![i16::MAX, -i16::MAX]);
    }

    #[test]
    fn wav_empty_input_is_valid_empty_file() {
        let bytes = encode_wav_16bit_mono(&[]).unwrap();
        let mut reader = hound::WavReader::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(reader.samples::<i16>().count(), 0);
    }

    #[test]
    fn to_wav_bytes_resamples_then_encodes() {
        let input = sine(440.0, 44_100, 0.25);
        let bytes = to_wav_bytes(&input, 44_100).unwrap();
        let reader = hound::WavReader::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(reader.spec().sample_rate, TARGET_SAMPLE_RATE);
        assert_eq!(reader.duration(), input.len() as u32 * 16_000 / 44_100);
    }

    #[test]
    fn to_mp3_bytes_resamples_then_encodes() {
        let input = sine(440.0, 44_100, 1.0);
        let bytes = to_mp3_bytes(&input, 44_100).unwrap();
        assert!(!bytes.is_empty());
        // MP3 stream starts with either an ID3v2 header "ID3" or MPEG sync bytes (0xFF)
        let has_id3 = bytes.starts_with(b"ID3");
        let has_sync = bytes.iter().any(|&b| b == 0xFF);
        assert!(has_id3 || has_sync, "output must contain MP3 frame sync or ID3");
    }
}
