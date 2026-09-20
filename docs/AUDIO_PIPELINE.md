# Audio Pipeline Specification

This document details the audio ingestion, conversion, voice activity detection, and streaming architecture of PortusEchoes.

---

## 1. Pipeline Overview

```text
Microphone (Native Hardware Rate, e.g. 48 kHz / 44.1 kHz)
   │
   ▼
[CPAL Capture Stream] (Downmix to Mono f32)
   │
   ├── Local Mode:
   │      │
   │      ▼
   │   [Streaming Resampler] (Rubato FFT ──► 16 kHz Mono f32)
   │      │
   │      ▼
   │   [Silero VAD Segmenter] (512-sample / 32 ms evaluation frames)
   │      │
   │      ▼
   │   [Speech Segments] (Zero-padded, silence trimmed)
   │      │
   │      ▼
   │   [SegmentInferenceWorker] ──► whisper.cpp
   │
   └── Cloud Mode:
          │
          ▼
       [In-Memory RAM Buffer] ──► 16 kHz WAV / MP3 Encoding ──► HTTP / WS
```

---

## 2. Capture & Resampling

- **Hardware Interfacing:** Managed by CPAL (Cross-Platform Audio Library) using default system input devices.
- **Normalization:** Converts integer PCM samples (`i16`, `u16`, `f32`) into standardized normalized 32-bit floating point (`[-1.0, 1.0]`).
- **Channel Downmixing:** Multichannel microphone inputs (stereo, quad-mic arrays) are averaged down to a single mono stream.
- **Resampling:** When native hardware operates at rates other than 16 kHz (e.g. 44.1 kHz, 48 kHz, 96 kHz), Rubato performs band-limited asynchronous resampling with anti-aliasing filtering.

---

## 3. Voice Activity Detection (Silero VAD)

- **Model:** Silero VAD `v6.2.0` embedded as a static asset (`ggml-silero-v6.2.0.bin`).
- **Frame Geometry:** Exactly 512 samples per frame (32 ms @ 16 kHz).
- **Speech Thresholding:** Frames with speech probability $> 0.5$ are marked as active speech.
- **Padding:** Automatically adds speech padding before and after detected utterances to prevent clipping initial consonants or trailing word endings.
- **Segment Bounds:** Enforces a maximum single-utterance limit (~8 seconds) to ensure low-latency FIFO chunk emission to Whisper inference.

---

## 4. Stability & Overflow Invariants

- **Transient Error Tolerance:** The Linux CPAL input stream distinguishes non-fatal ALSA/PipeWire buffer overruns (`ErrorKind::Xrun`, `ErrorKind::RealtimeDenied`, `ErrorKind::DeviceChanged`) from genuine hardware disconnects, preventing false recording drops under heavy CPU inference load.
- **PCM Queue Budget:** Sized for up to 30 minutes of continuous audio backlog.
- **Memory Safety:** Materialized speech frames are systematically drained from memory upon segmentation, maintaining a flat memory footprint (<50 MB) over extended dictation sessions.

---

## 5. Unified Recording Capacity & Customization

PortusEchoes ships configured out of the box for up to **30 minutes (1,800 seconds)** of uninterrupted recording across all modes and models, governed by a unified build-time configuration in `.env`:
- **Offline Local:** Buffer queues maintain capacity with continuous segment memory drainage, preventing memory growth during long sessions.
- **Cloud Completed (OpenAI / Groq):** Audio is encoded to compressed MP3 before upload so that a full 30-minute session stays well under the 25 MB API request ceiling (~14.4 MB).
- **Cloud Live (OpenAI Realtime):** Streams audio continuously with the unified session limit.

### Build-Time Limit Customization
Developers compiling their own binaries can customize or expand this limit:
- In `.env`, set `RECORDING_LIMIT_SECS` (default `1800`):
  - `0` = Unlimited / untimed (runs continuously until the shortcut is released).
  - `> 0` = Maximum hold duration in seconds across all modes before the recording automatically commits and finishes.
- If increasing the limit beyond 30 minutes for Local Whisper, also adjust `LOCAL_PCM_BUDGET_SECONDS` in `src-tauri/src/transcription/local/feed.rs` and `INFERENCE_SAMPLE_BUDGET` in `inference.rs`.
