//! Microphone availability and OS-level mute state detection.
//!
//! Evaluates the default input device state on demand without persistent caching.
//! See `docs/AUDIO.md` and `MIC_OFF.md`.

/// Authoritative microphone state snapshot evaluated at recording admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicrophoneStatus {
    /// Device exists, supported config is valid, and volume is unmuted.
    Available,
    /// Device exists but is muted at the OS level or volume is 0%.
    Muted,
    /// No default input device exists, device is disconnected, or access is denied.
    Unavailable,
}

#[cfg(target_os = "windows")]
pub fn check_microphone_status() -> MicrophoneStatus {
    unsafe {
        use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
        use windows::Win32::Media::Audio::{
            eCapture, eConsole, IMMDeviceEnumerator, MMDeviceEnumerator,
        };
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
        };

        // CoInitializeEx may return S_OK, S_FALSE, or RPC_E_CHANGED_MODE if already initialized.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let enumerator: IMMDeviceEnumerator =
            match CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) {
                Ok(enumerator) => enumerator,
                Err(_) => return MicrophoneStatus::Unavailable,
            };

        let device = match enumerator.GetDefaultAudioEndpoint(eCapture, eConsole) {
            Ok(device) => device,
            Err(_) => return MicrophoneStatus::Unavailable,
        };

        let endpoint_volume: IAudioEndpointVolume = match device.Activate(CLSCTX_ALL, None) {
            Ok(endpoint_volume) => endpoint_volume,
            Err(_) => return MicrophoneStatus::Unavailable,
        };

        let is_muted = match endpoint_volume.GetMute() {
            Ok(muted) => muted.as_bool(),
            Err(_) => return MicrophoneStatus::Unavailable,
        };
        if is_muted {
            return MicrophoneStatus::Muted;
        }

        if let Ok(volume) = endpoint_volume.GetMasterVolumeLevelScalar() {
            if volume == 0.0 {
                return MicrophoneStatus::Muted;
            }
        }
        MicrophoneStatus::Available
    }
}

#[cfg(target_os = "linux")]
pub fn check_microphone_status() -> MicrophoneStatus {
    // Tier 1: Probe PipeWire / WirePlumber
    if let Ok(output) = std::process::Command::new("wpctl")
        .args(["get-volume", "@DEFAULT_AUDIO_SOURCE@"])
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            if text.contains("[MUTED]") {
                return MicrophoneStatus::Muted;
            }
            if let Some(vol_str) = text.trim().strip_prefix("Volume: ") {
                let vol_val = vol_str.split_whitespace().next().unwrap_or("");
                if let Ok(val) = vol_val.parse::<f32>() {
                    if val <= 0.0001 {
                        return MicrophoneStatus::Muted;
                    }
                } else if vol_val == "0.00" || vol_val == "0" {
                    return MicrophoneStatus::Muted;
                }
            }
        }
    }

    // Tier 2: Probe PulseAudio / PipeWire-Pulse
    if let Ok(output) = std::process::Command::new("pactl")
        .args(["get-source-mute", "@DEFAULT_SOURCE@"])
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            if text.contains("yes") {
                return MicrophoneStatus::Muted;
            }
        }
    }

    if let Ok(output) = std::process::Command::new("pactl")
        .args(["get-source-volume", "@DEFAULT_SOURCE@"])
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            if text.contains(" 0% ") || text.contains("/ 0% /") {
                return MicrophoneStatus::Muted;
            }
        }
    }

    // Tier 3: Check default input device existence via CPAL
    use cpal::traits::HostTrait;
    let host = cpal::default_host();
    if host.default_input_device().is_none() {
        return MicrophoneStatus::Unavailable;
    }

    MicrophoneStatus::Available
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub fn check_microphone_status() -> MicrophoneStatus {
    use cpal::traits::HostTrait;
    let host = cpal::default_host();
    if host.default_input_device().is_none() {
        MicrophoneStatus::Unavailable
    } else {
        MicrophoneStatus::Available
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn microphone_status_variants_are_distinct() {
        assert_ne!(MicrophoneStatus::Available, MicrophoneStatus::Muted);
        assert_ne!(MicrophoneStatus::Available, MicrophoneStatus::Unavailable);
        assert_ne!(MicrophoneStatus::Muted, MicrophoneStatus::Unavailable);
    }
}
