//! What rasterises the frames: a GPU, or the CPU behind a GPU API (a VM without GPU
//! passthrough, a desktop without drivers).
//!
//! A software rasteriser shades every pixel on the CPU cores the analyzer also needs, so
//! the window must ask it for fewer frames.

use wgpu::{AdapterInfo, DeviceType};

/// Who rasterises: decides the window's frame-rate cap and its optional effects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rasterizer {
    Gpu,
    /// A CPU implementation of the graphics API (WARP, llvmpipe, lavapipe, SwiftShader).
    Software,
}

/// Lower-case fragments of the adapter or driver names of known CPU rasterisers. Some
/// report themselves as an integrated or "other" device (WARP through Direct3D, llvmpipe
/// through GL on older Mesa), so the name is checked as well as the type.
const SOFTWARE_NAMES: [&str; 7] = [
    "llvmpipe",
    "lavapipe",
    "softpipe",
    "swiftshader",
    "warp",
    "microsoft basic render driver",
    "software rasterizer",
];

/// Classifies an adapter from its reported type, name and driver.
pub fn classify(device_type: DeviceType, name: &str, driver: &str) -> Rasterizer {
    if device_type == DeviceType::Cpu {
        return Rasterizer::Software;
    }
    let software = [name, driver].iter().any(|s| {
        let s = s.to_lowercase();
        SOFTWARE_NAMES.iter().any(|n| s.contains(n))
    });
    if software {
        Rasterizer::Software
    } else {
        Rasterizer::Gpu
    }
}

/// [`classify`] over what wgpu reports for an adapter.
pub fn classify_info(info: &AdapterInfo) -> Rasterizer {
    classify(info.device_type, &info.name, &info.driver)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_type_is_software_whatever_the_name() {
        assert_eq!(
            classify(DeviceType::Cpu, "Some Adapter", ""),
            Rasterizer::Software
        );
    }

    #[test]
    fn known_software_names_are_software_under_any_type() {
        for (name, driver) in [
            ("llvmpipe (LLVM 17.0.6, 256 bits)", "llvmpipe"),
            ("lavapipe", ""),
            ("Microsoft Basic Render Driver", ""),
            ("WARP", ""),
            ("SwiftShader Device (Subzero)", "SwiftShader driver"),
            ("Mesa X11", "softpipe"),
        ] {
            for t in [
                DeviceType::Other,
                DeviceType::IntegratedGpu,
                DeviceType::VirtualGpu,
            ] {
                assert_eq!(
                    classify(t, name, driver),
                    Rasterizer::Software,
                    "{name} / {driver} as {t:?}"
                );
            }
        }
    }

    #[test]
    fn real_gpus_are_gpus() {
        for (t, name, driver) in [
            (
                DeviceType::DiscreteGpu,
                "AMD Radeon RX 6600 (RADV NAVI23)",
                "radv",
            ),
            (
                DeviceType::IntegratedGpu,
                "Intel(R) UHD Graphics 620",
                "Intel open-source Mesa driver",
            ),
            (DeviceType::IntegratedGpu, "V3D 4.2", "V3DV Mesa"),
            (DeviceType::IntegratedGpu, "Apple M2", ""),
            // A paravirtual GPU passes the host's GPU through: hardware rasterises.
            (DeviceType::VirtualGpu, "Virtio-GPU Venus (AMD)", "venus"),
        ] {
            assert_eq!(classify(t, name, driver), Rasterizer::Gpu, "{name}");
        }
    }
}
