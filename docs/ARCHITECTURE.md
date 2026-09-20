# Architecture Overview

PortusEchoes is a desktop speech-to-text utility built with a Rust backend (Tauri v2) and a React/TypeScript frontend. It enables hold-to-talk dictation directly at the active cursor position using local inference or cloud endpoints.

---

## 1. System Architecture

```text
[Global Hotkey / Tray Start]
        │
        ▼
[RecordingCoordinator] ──(Monotonic RecordingIdentity)
        │
        ├── Freeze Provider & Settings Snapshot
        ├── Request-scoped Cancellation Token
        │
        ▼
[AudioEngine] ──(CPAL Stream)
        │
        ├── Local: StreamingResampler (16 kHz) ──► Silero VAD (32 ms frames)
        │                                                │
        │                                                ▼
        │                                  SegmentInferenceWorker
        │                                                │
        │                                                ▼
        │                                  whisper.cpp (Vulkan / CPU)
        │
        └── Cloud: In-Memory Audio Buffer ──► HTTP Multipart / WebSocket
                                                         │
                                                         ▼
                                                OpenAI / Groq API
        │
        ▼
[Delivery Engine]
        ├── Windows: Direct Caret Injection (EM_REPLACESEL / WM_CHAR)
        └── Linux:   Persistent Clipboard Delivery (arboard)
```

---

## 2. Core Subsystems

### 2.1. Recording Coordinator (`src-tauri/src/recording/`)
- **Authority Boundary:** Allocates a strictly monotonic `RecordingIdentity` for each push-to-talk press.
- **Supersession:** Pressing the shortcut while a session is active supersedes and cancels the prior session immediately.
- **Linearized Effects:** Transcript delivery and error reporting require matching the active `RecordingIdentity`. Stale or canceled background jobs cannot deliver text or overwrite clipboard state.

### 2.2. Audio Capture (`src-tauri/src/audio/`)
- Interfaced via CPAL (Cross-Platform Audio Library) using WASAPI on Windows and ALSA/PipeWire on Linux.
- Captures raw PCM input, downmixes multichannel audio to mono, and normalizes samples to 32-bit floating point (`f32`).
- Routes samples to `LocalLive` (streamed to VAD) or `CloudCompleted` (buffered in RAM).

### 2.3. Local Transcription Pipeline (`src-tauri/src/transcription/local/`)
- **Streaming Resampler:** Real-time multi-rate conversion using Rubato to enforce 16 kHz mono.
- **Silero VAD `v6.2.0`:** Evaluates sequential 512-sample (32 ms) frames to segment continuous speech and discard silence.
- **Whisper Inference:** Executes single-segment greedy decoding using repository-owned bindings to pinned `whisper.cpp v1.9.1`. Employs `no_context = true` across segments to prevent cross-segment hallucination loops during long sessions.

### 2.4. Cloud Transcription Pipeline (`src-tauri/src/transcription/cloud/`)
- **OpenAI:** Completed file transport with Server-Sent Events (`stream=true`) streaming and Realtime WebSocket audio streaming.
- **Groq:** High-speed cloud inference with sub-second response times.
- **Security:** Credentials reside solely in OS-native secure keychains (Windows Credential Manager / Linux Secret Service) and are loaded request-scoped into memory.

### 2.5. Text Delivery (`src-tauri/src/output/`)
- **Windows:** Resolves the control under the cursor and sends Win32 `EM_REPLACESEL` / `WM_CHAR` messages directly, avoiding clipboard overwrite.
- **Linux:** Manages a process-lifetime clipboard instance (`PERSISTENT_CLIPBOARD`) maintaining X11 selection ownership.

### 2.6. User Interface (`src/`)
- **Overlay HUD (`src/overlay.tsx`):** Independent 180×48 borderless click-through pill window providing visual feedback (`Preparing`, `Recording`, `Processing...`, `On Clipboard!`).
- **Settings Panel (`src/settings/`):** Configuration for active provider, local model management, cloud API keys, diagnostics, and keyboard shortcut binding.
