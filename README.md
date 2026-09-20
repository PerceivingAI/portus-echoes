# PortusEchoes

Hold-to-talk speech transcription at your cursor. Transcribe locally on your device with GPU acceleration or through cloud transcription providers. For Windows and Linux.

---

## Features

- **Type without clicking (Windows):** Hover your mouse over any window and speak. Text is typed directly into the window under your cursor without clicking. With the OpenAI Live model on Windows, words appear in real time as you speak, allowing you to move your mouse across windows to write across them mid-sentence.
- **Local Inference:** Fully offline, private speech recognition powered by Whisper.
- **Hardware Acceleration:** Automatic GPU acceleration with multi-threaded CPU fallback.
- **Cloud Providers:** BYOK for OpenAI (streaming & Realtime) and Groq.
- **Push-to-Talk Indicator:** Lightweight, borderless, click-through overlay showing real time recording and transcription status.
- **30 Minute Continuous Recording:** Dictate uninterrupted for up to 30 minutes in a single session across all modes and models (offline Local, OpenAI, and Groq). Developers compiling from source can expand this limit even further.
- **Security & Privacy:** API keys persist exclusively in OS-native secure storage (Windows Credential Manager / Linux Secret Service). No audio is written to disk.
- **Cross-Platform:** Windows and Linux support.

---

## Choose Your Provider

Every provider in PortusEchoes is completely independent. You only need to configure what you want to use:

- **Offline Local Only:** Transcribe completely privately on your device using GPU or CPU. Never requires an API key or an internet connection.
- **Groq Only:** For fast transcriptions without having to manage local models. Just add a Groq API key and transcribe!
- **OpenAI Only:** Connect your OpenAI API key for real-time live streaming or high accuracy cloud models.

You never have to download local models if you only want cloud transcription, and you never have to provide API keys if you only want offline local dictation.

---

## Platform Support & Delivery

| Capability | Windows | Linux |
|---|---|---|
| **Audio Capture** | WASAPI | ALSA / PipeWire |
| **Local Inference** | GPU / CPU | GPU / CPU |
| **Voice Activity Detection** | Integrated Silero VAD | Integrated Silero VAD |
| **Text Delivery** | Direct Caret Injection | Clipboard Delivery |

*For complete platform details and upcoming Linux caret injection work, see [docs/PLATFORM_MATRIX.md](docs/PLATFORM_MATRIX.md).*

---

## Installation & Releases

Pre-built releases for Windows and Linux are available under [GitHub Releases](https://github.com/PerceivingAI/portus-echoes/releases).

### Windows
1. Download `PortusEchoes_<version>_x64-setup.exe` from the latest release.
2. Run the installer and follow the setup wizard. On the finish page, you can optionally check the box to create a desktop shortcut (left unchecked by default).
3. Once installed, PortusEchoes will launch and sit in your system tray.

### Linux
Choose the package format best suited for your distribution:
- **AppImage (Universal standalone):**
  1. Download `portus-echoes_<version>_amd64.AppImage`.
  2. Make it executable:
     ```bash
     chmod +x portus-echoes_*.AppImage
     ```
  3. Run it directly:
     ```bash
     ./portus-echoes_*.AppImage
     ```
- **Debian / Ubuntu Package (`.deb`):**
  1. Download `portus-echoes_<version>_amd64.deb`.
  2. Install it with `dpkg`:
     ```bash
     sudo dpkg -i portus-echoes_*_amd64.deb
     ```
  3. Launch PortusEchoes from your application menu or terminal.

---

## Building from Source (Quickstart)

### Prerequisites

- **Node.js:** 18+ and `npm`
- **Rust:** 1.88+ and `cargo`
- **CMake:** 3.16+
- **C/C++ Compiler:** MSVC (Windows) or GCC/Clang (Linux)

#### Linux System Packages

```bash
# Arch Linux
sudo pacman -S alsa-lib pipewire vulkan-loader libx11 libxtst libxkbcommon

# Ubuntu / Debian
sudo apt-get install libasound2-dev libvulkan-dev libx11-dev libxtst-dev libxkbcommon-dev

# Fedora
sudo dnf install alsa-lib-devel vulkan-loader-devel libX11-devel libXtst-devel libxkbcommon-devel
```

### Setup & Run

1. Clone the repository and configure the environment:
   ```bash
   cp .env.example .env
   ```

2. Install dependencies:
   ```bash
   npm install
   ```

3. Launch development mode:
   ```bash
   npm run tauri dev
   ```

4. Build for production:
   ```bash
   npm run tauri build
   ```

---

## How It Works

PortusEchoes lives in your system tray and works across every application on your desktop:

- **Hover and speak:** On Windows, put your mouse pointer over any text box, editor, or chat app without clicking. Hold your shortcut key (`Ctrl+Alt+Space` by default), speak naturally, and release when you are done.
- **Direct typing:** On Windows, text is typed directly into whichever window sits under your mouse cursor without touching your clipboard.
  - **OpenAI Live:** Words stream out in real time while you speak. Moving your mouse across different windows routes words into each window as your cursor moves over it.
  - **Groq:** Audio is processed after release and the full transcript is typed into the window under your mouse.
  - **Local Whisper:** Speech is transcribed locally and typed into the window under your mouse as each segment completes.
- **Clipboard delivery on Linux:** Linux currently uses clipboard delivery across all providers and models. Your completed transcript is automatically copied to your clipboard. The overlay indicator will let you know when the text is ready to paste (`Ctrl+V`) into any target window.
- **Clear feedback:** A small, click-through pill at the bottom of your screen shows when your microphone is ready and listening.
- **Sustained Dictation:** Speak freely for a quick phrase or for up to 30 minutes without artificial time limits or mid-sentence drops.
- **Full control:** Rebind your shortcut to any key combination, switch between local offline models and cloud providers, or control recordings from the system tray menu.

---

## Configuration & Settings Guide

Access the Settings window from the system tray icon or during initial onboarding to configure providers, models, and shortcuts.

### 1. Active Provider & Shortcut
- In the **Settings** tab under **Active Provider**, select which engine handles your dictation: **Local**, **OpenAI**, or **Groq**. (You can also toggle active providers directly from the system tray menu).
- Under **Shortcut**, click the input box and press any key combination to rebind your push-to-talk shortcut.

### 2. Local Whisper Models
- **Standard Models:** In the **Local** tab, click the download icon next to a listed model. The file will download directly into your local application data directory.
- **Custom Models:** You can use any Whisper GGML model by downloading the `.bin` file from the official Hugging Face repository:
  **[https://huggingface.co/ggerganov/whisper.cpp/tree/main](https://huggingface.co/ggerganov/whisper.cpp/tree/main)**
  Select **Custom Path** in the Local tab and browse to your downloaded `.bin` file.

### 3. OpenAI Setup
- In the **OpenAI** tab, enter your API key (stored securely in your OS credential store).
- Select a standard model slot (`GPT Live Transcribe` or `GPT Transcribe`), or select **Custom Model ID** and enter any compatible model (e.g. `gpt-4o-transcribe`, `gpt-4o-mini-transcribe`).

### 4. Groq Setup
- In the **Groq** tab, enter your Groq API key.
- Select a standard model (`Whisper Large V3 Turbo`) or select **Custom Model ID** and enter a custom Groq model ID.

---

## Documentation

- [Architecture Overview](docs/ARCHITECTURE.md)
- [Platform Matrix](docs/PLATFORM_MATRIX.md)
- [Local Inference](docs/LOCAL_INFERENCE.md)
- [Cloud Providers](docs/CLOUD_PROVIDERS.md)
- [Audio Pipeline](docs/AUDIO_PIPELINE.md)
- [Shortcuts & Overlay](docs/SHORTCUTS_AND_OVERLAY.md)
- [Build & Packaging](docs/BUILD_AND_PACKAGING.md)
- [Third-Party Licenses & Attributions](docs/THIRD_PARTY.md)

---

## License

PortusEchoes is licensed under the [Apache License, Version 2.0](LICENSE).
