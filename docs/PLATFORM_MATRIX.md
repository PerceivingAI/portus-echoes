# Platform Functionality Matrix

This document defines the operational parity and platform-specific behaviors of PortusEchoes across Windows and Linux.

---

## 1. Feature Parity Matrix

| Feature | Windows | Linux (X11) | Linux (Wayland) | Notes |
|---|---|---|---|---|
| **Audio Capture** | WASAPI | ALSA / PipeWire | ALSA / PipeWire | Native sample rates downmixed to 16 kHz mono. |
| **Silero VAD** | Embedded v6.2.0 | Embedded v6.2.0 | Embedded v6.2.0 | 512-sample (32 ms) frame evaluation. |
| **Local Whisper** | Vulkan / CPU | Vulkan / CPU | Vulkan / CPU | Pinned whisper.cpp v1.9.1 with AVX2/FMA fallback. |
| **OpenAI Cloud** | Full Parity | Full Parity | Full Parity | SSE completed streaming + Realtime WebSocket. |
| **Groq Cloud** | Full Parity | Full Parity | Full Parity | Sub-second multipart transcription. |
| **Credential Storage** | Windows Credential Manager | Secret Service API / DBus | Secret Service API / DBus | OS-native keychain persistence. |
| **Overlay HUD** | Native Click-Through Pill | WebKitGTK Click-Through Pill | WebKitGTK Click-Through Pill | Bottom-center work-area positioning. |
| **Global Shortcuts** | Low-Level Hook (`WH_KEYBOARD_LL`) | X11 Key Grab (`XGrabKey`) | GlobalShortcuts Portal / Hook | Background push-to-talk activation. |
| **Text Delivery** | Direct Caret Injection | Persistent Clipboard Delivery | Persistent Clipboard Delivery | See Section 2 below. |

---

## 2. Text Delivery Architecture & Linux Release Scope

### 2.1. Windows: Direct Caret Injection
On Windows, PortusEchoes delivers text directly to the active control under your mouse pointer without touching the clipboard or requiring you to click on the window to focus it.

How text arrives depends on the provider and model:
- **OpenAI Live (`gpt-live-transcribe`):** Words stream out in real time as you speak. If you move your mouse across multiple windows while speaking, words appear in each window on the fly as your cursor crosses into it.
- **Groq & Standard Cloud Models:** Audio is recorded during hold, uploaded on release, and the complete transcript is typed into the window under your mouse cursor in a single operation once processing finishes.
- **Local Whisper:** Speech segments are decoded and typed into the window under the mouse cursor as each segment finishes.
### 2.2. Linux: Persistent Clipboard Delivery
On Linux, PortusEchoes uses a process-lifetime clipboard manager (`arboard`):
- **Permanent Selection Window:** The application maintains ownership of the X11 selection window (`CLIPBOARD` atom) and Wayland data device.
- **Ambient User Feedback:** When dictation finishes, the overlay pill and Settings banner show `"Processing..."` during transcription, followed by `"On Clipboard!"` in accent color for 1,700 ms.
- **Manual Paste:** Pressing `Ctrl+V` (or terminal paste `Ctrl+Shift+V`) pastes the completed transcript into any target application.

### 2.3. Post-Launch Linux Parity Work
Simulated keystroke injection (`xdotool --clearmodifiers ctrl+v`) was evaluated and deliberately rejected for production release because synthetic modifier releases inherently collide with the user holding the push-to-talk shortcut keys, aborting recording sessions prematurely.

Direct caret injection on Linux without clipboard reliance is actively scheduled for post-release development, evaluating:
1. Virtual keyboard emulation via kernel `/dev/uinput` with non-root user group permissions.
2. Wayland-native virtual keyboard protocols (`zwp_virtual_keyboard_v1`).
3. AT-SPI accessibility text editing interfaces (`org.a11y.atspi.EditableText`).

---

## 3. Provider Independence & Modularity

All transcription providers in PortusEchoes are completely decoupled:
- **Zero Shared Dependencies:** Using Groq does not require downloading local models or having an OpenAI account. Using Local Whisper runs fully offline without entering cloud API keys.
- **On-Demand Configuration:** Users only need to configure the specific provider they intend to use.
- **Safe Switching:** Switching active providers immediately cancels any active transcription in progress, ensuring the previous provider's text does not overwrite your current work.
