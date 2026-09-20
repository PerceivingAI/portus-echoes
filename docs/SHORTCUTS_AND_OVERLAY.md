# Shortcuts & Overlay HUD

PortusEchoes features a system-wide push-to-talk shortcut system and a floating Heads-Up Display (HUD) overlay providing immediate, ambient visual feedback.

---

## 1. Global Shortcut Architecture

### 1.1. Low-Level Hooking
- **Windows:** Installs a passive low-level keyboard hook (`SetWindowsHookExW(WH_KEYBOARD_LL)`) on a dedicated high-priority thread (`THREAD_PRIORITY_HIGHEST`). Includes an active-hold hardware watchdog using `GetAsyncKeyState` to ensure long recordings never get stuck if Windows swallows a key-up message during focus changes. Does not monopolize or alter system key dispatch.
- **Linux (X11):** Uses `x11rb` to grab the configured keycode and modifier mask on the root desktop window (`XGrabKey`).
- **Linux (Wayland):** Interacts through the XDG Desktop Portal GlobalShortcuts interface and libinput event streams.

### 1.2. Shortcut Recorder & Safety Guard
- In Settings, users can bind any custom keyboard shortcut by focusing the input box and pressing the desired combination.
- **Suppression Guard:** When the shortcut recorder input receives focus, global hotkey listening is suspended in the background across all platforms, preventing keypresses from triggering audio recordings while configuring new combinations.
- **Advisories:** Warns if a single key without modifiers is chosen or if a combination is registered by another application.

---

## 2. Overlay Indicator Pill

The HUD overlay is a standalone, borderless, semi-transparent pill window (`180px × 48px`, `rounded-full bg-black/90`).

### 2.1. Window Configuration
- **Dimensions:** Fixed at 180px wide by 48px high.
- **Positioning:** Automatically placed at the **bottom-center of the primary monitor work area**, maintaining a fixed 12px margin above the system taskbar.
- **Click-Through:** Configured with `set_ignore_cursor_events(true)` so mouse clicks pass directly through to whatever application sits beneath it.
- **No Focus Hijacking:** Never takes or steals keyboard focus from your active text editor.

### 2.2. Visual State Machine

| State | Phase | Indicator | Meaning |
|---|---|---|---|
| **Preparing** | `preparing` | Static Slate Dot | Initializing microphone stream and hardware buffers. |
| **Recording** | `recording` | Pulsing Red Dot | Microphone is active and recording speech. |
| **Muted** | `muted` | Grey MicOff Icon | Microphone is muted at OS level or disconnected. |
| **Processing** | `finalizing` | Text: `"Processing..."` | Whisper or Cloud is decoding speech (Clipboard mode). |
| **On Clipboard** | `idle` | Text: `"On Clipboard!"` | Text copied to clipboard; holds for 1.7s then auto-hides. |

*Note: In Windows Direct Caret Injection mode, the overlay hides immediately upon hotkey release because text is inserted live at the cursor.*
