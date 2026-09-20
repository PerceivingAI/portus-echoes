# Cloud Transcription Providers

PortusEchoes integrates with OpenAI and Groq for high-accuracy or low-latency cloud speech recognition.

---

## 1. Supported Providers & Models

### 1.1. OpenAI
- **Endpoint:** `POST https://api.openai.com/v1/audio/transcriptions`
- **Standard Models:**
  - `gpt-live-transcribe` (Realtime WebSocket audio streaming). On Windows, words appear live as you speak, so moving your mouse between windows writes across them in real time.
  - `gpt-transcribe` (Completed audio with Server-Sent Events streaming).
- **Verified Custom Models:**
  - `gpt-4o-transcribe`
  - `gpt-4o-mini-transcribe`
- **Audio Format:** 16 kHz mono WAV or MP3 multipart upload.

### 1.2. Groq
- **Endpoint:** `POST https://api.groq.com/openai/v1/audio/transcriptions`
- **Standard Model:** `whisper-large-v3-turbo`
- **Delivery Mode:** Processes the entire recording after release and returns the full transcript at once (~300–600 ms turnaround time). On Windows, the complete text is typed directly into whichever window sits under your mouse when the result arrives.
- **Custom Models:** Supports any valid Groq speech recognition model ID.

---

## 2. Security & Credential Storage

PortusEchoes does not store API keys in plain-text configuration files or logs.

- **Storage Location:** Keys are stored directly in the host operating system's native keychain:
  - **Windows:** Windows Credential Manager (`Windows Vault`)
  - **Linux:** Secret Service API via DBus (GNOME Keyring, KWallet, or keepassxc-service)
- **Memory Isolation:** API keys are read from the credential store on-demand for individual requests and are never written to disk or included in telemetry.

---

## 3. Provider Switching Behavior

- Selecting an active provider in Settings or the System Tray menu immediately updates the application state.
- If a recording or finalization is currently in progress when you switch providers, the active session is immediately canceled to prevent outdated transcripts from overwriting the clipboard.
- The next push-to-talk activation uses the newly selected provider.

---

## 4. Provider Independence

OpenAI and Groq operate completely independently from each other and from the local offline engine:
- You can configure and use Groq without setting up an OpenAI key or downloading any local Whisper models.
- You can use OpenAI without configuring Groq.
- API keys, models, and custom drafts are stored in isolated provider slots with zero cross-talk.
