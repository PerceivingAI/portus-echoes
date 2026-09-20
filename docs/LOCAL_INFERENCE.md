# Local Transcription Engine

PortusEchoes provides fully offline speech transcription using embedded GGML models. All local processing runs directly on your machine without transmitting audio to any external service.

---

## 1. Engine Specifications

- **Inference Core:** Pinned `whisper.cpp v1.9.1` via repository-owned bindings (`vendor/whisper-rs`).
- **Voice Activity Detection:** Embedded Silero VAD `v6.2.0` (`src-tauri/assets/ggml-silero-v6.2.0.bin`).
- **Processing Architecture:** Single-worker FIFO queue with stateful audio streaming.
- **Decoding Parameters:** Greedy sampling (`best_of = 1`) with `no_context = true` across speech segments to prevent token accumulation and hallucination loops during long sessions.

---

## 2. Hardware Acceleration

PortusEchoes automatically determines the optimal computation backend at startup:

1. **Vulkan Compute (Primary):**
   - Probes available Vulkan physical devices.
   - Automatically selects discrete GPUs over integrated GPUs, prioritizing dedicated VRAM capacity.
   - Offloads matrix multiplication and neural network layers to the GPU.
2. **CPU Fallback:**
   - Automatically engages if no compatible Vulkan device or driver is detected.
   - Utilizes multi-threaded AVX2 and FMA vector instructions.
   - Configures thread count dynamically based on physical core counts.

---

## 3. Model Catalog & Configuration

### 3.1. Standard Models & In-App Downloads
PortusEchoes ships with two curated standard models configured in `.env.example`:
1. **Whisper Small Q8:** `ggml-small-q8_0.bin` (Fast, high-efficiency quantized model)
2. **Whisper Medium Q8:** `ggml-medium-q8_0.bin` (Higher accuracy quantized model)

Within the application's Settings panel, clicking the download icon on either model downloads the model file directly from the official **Hugging Face** whisper.cpp repository:
**[https://huggingface.co/ggerganov/whisper.cpp/tree/main](https://huggingface.co/ggerganov/whisper.cpp/tree/main)**

When compiling from source, developers can configure different default models by updating the `LOCAL_MODELS` JSON array in `.env`:
```env
LOCAL_MODELS='[{"downloadPath":"https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q8_0.bin","label":"Whisper Small Q8"},{"downloadPath":"https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium-q8_0.bin","label":"Whisper Medium Q8"}]'
```

### 3.2. Additional Models via Custom Path
To use any other supported Whisper model (such as `tiny`, `base`, `large`, or multilingual models):
1. Download the desired `.bin` file directly from the official repository:
   **[https://huggingface.co/ggerganov/whisper.cpp/tree/main](https://huggingface.co/ggerganov/whisper.cpp/tree/main)**
2. In Settings under the Local provider, select **Custom Path** and browse to the downloaded `.bin` model file.

### 3.3. Application Storage Locations
Managed standard models are saved to:
- **Linux:** `~/.local/share/portusechoes/models/`
- **Windows:** `%LOCALAPPDATA%\portusechoes\models\`
---

## 4. Sustained Recording Resilience

- **Queue Headroom:** The internal audio buffer supports up to 30 minutes of continuous speech without queue overflow.
- **Buffer Overrun Protection:** The audio feed filters transient OS audio glitches (XRUNs) under heavy CPU load, ensuring recording streams remain active.
- **Graceful Saturation:** If the inference queue reaches maximum buffer capacity under extreme load, capture auto-stops cleanly and finalizes all accumulated speech rather than dropping data.
