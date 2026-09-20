use std::ffi::CStr;

use whisper_rs_sys::{
    ggml_backend_dev_count, ggml_backend_dev_description, ggml_backend_dev_get,
    ggml_backend_dev_memory, ggml_backend_dev_name, ggml_backend_dev_type,
    ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_GPU,
    ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_IGPU,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VulkanDeviceKind {
    Discrete,
    Integrated,
}

/// Minimal Vulkan device metadata needed by PortusEchoes runtime selection.
///
/// `whisper_gpu_ordinal` uses the exact GPU/IGPU counting order that
/// whisper.cpp applies to `whisper_context_params::gpu_device`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VulkanDeviceInfo {
    pub whisper_gpu_ordinal: i32,
    pub name: String,
    pub description: String,
    pub memory_free: usize,
    pub memory_total: usize,
    pub kind: VulkanDeviceKind,
}

unsafe fn string_from_ptr(value: *const std::os::raw::c_char) -> String {
    if value.is_null() {
        String::new()
    } else {
        CStr::from_ptr(value).to_string_lossy().into_owned()
    }
}

/// Enumerate usable GPU/IGPU devices in GGML's stable registry order.
///
/// PortusEchoes builds only the Vulkan and CPU backends, so every GPU/IGPU
/// returned by the registry is a Vulkan device. The ordinal is assigned by
/// counting GPU/IGPU entries exactly as whisper.cpp does when resolving
/// `gpu_device`.
pub fn list_devices() -> Vec<VulkanDeviceInfo> {
    unsafe {
        let mut ordinal = 0i32;
        let mut devices = Vec::new();

        for index in 0..ggml_backend_dev_count() {
            let device = ggml_backend_dev_get(index);
            if device.is_null() {
                continue;
            }

            let device_type = ggml_backend_dev_type(device);
            let kind = if device_type == ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_GPU {
                VulkanDeviceKind::Discrete
            } else if device_type == ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_IGPU {
                VulkanDeviceKind::Integrated
            } else {
                continue;
            };

            let mut memory_free = 0usize;
            let mut memory_total = 0usize;
            ggml_backend_dev_memory(device, &mut memory_free, &mut memory_total);

            devices.push(VulkanDeviceInfo {
                whisper_gpu_ordinal: ordinal,
                name: string_from_ptr(ggml_backend_dev_name(device)),
                description: string_from_ptr(ggml_backend_dev_description(device)),
                memory_free,
                memory_total,
                kind,
            });
            ordinal += 1;
        }

        devices
    }
}

#[cfg(test)]
mod vulkan_tests {
    use super::*;

    #[test]
    fn enumerate_must_not_panic() {
        let _ = list_devices();
    }

    #[test]
    fn device_metadata_matches_whisper_gpu_ordinals() {
        let devices = list_devices();

        for (expected_ordinal, device) in devices.iter().enumerate() {
            assert_eq!(device.whisper_gpu_ordinal, expected_ordinal as i32);
            assert!(!device.name.trim().is_empty());
            assert!(device.memory_total >= device.memory_free);
        }
    }
}
