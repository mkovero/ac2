//! How the window's wgpu device is chosen.
//!
//! - The low-power adapter: on a laptop with two GPUs the integrated one draws these plots
//!   at a fraction of the discrete one's power, and waking the discrete GPU costs battery
//!   for nothing. `WGPU_POWER_PREF=high` still picks the other.
//! - Downlevel limits sized to the adapter's textures: the plots need only vertex
//!   instancing, uniforms and sampled textures (no storage buffers, no compute), so asking
//!   for more only rules out GPUs that would draw them fine — a Raspberry Pi 4 (V3D, 4096 px
//!   textures, GLES 3.1 or v3dv Vulkan) refuses wgpu's desktop defaults.

use std::sync::Arc;

use ac2_plot::wgpu;

/// The device limits asked of an adapter on `backend` that offers `adapter`.
pub fn required_limits(backend: wgpu::Backend, adapter: &wgpu::Limits) -> wgpu::Limits {
    let base = if backend == wgpu::Backend::Gl {
        wgpu::Limits::downlevel_webgl2_defaults()
    } else {
        wgpu::Limits::downlevel_defaults()
    };
    base.using_resolution(adapter.clone())
}

/// The adapter preference: `WGPU_POWER_PREF` when set, else low power.
pub fn power_preference() -> wgpu::PowerPreference {
    wgpu::PowerPreference::from_env().unwrap_or(wgpu::PowerPreference::LowPower)
}

/// eframe's wgpu options with this module's adapter and device choice.
pub fn wgpu_options() -> egui_wgpu::WgpuConfiguration {
    let mut setup = egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    setup.power_preference = power_preference();
    setup.device_descriptor = Arc::new(|adapter| wgpu::DeviceDescriptor {
        label: Some("ac2-ui"),
        required_limits: required_limits(adapter.get_info().backend, &adapter.limits()),
        ..Default::default()
    });
    egui_wgpu::WgpuConfiguration {
        wgpu_setup: setup.into(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A V3D-class adapter (4096 px textures, GLES 3.1 class) can grant what is asked, on
    /// either backend, and the window may use all of its texture size.
    #[test]
    fn small_gpus_can_grant_the_limits() {
        let v3d = wgpu::Limits {
            max_texture_dimension_1d: 4096,
            max_texture_dimension_2d: 4096,
            max_texture_dimension_3d: 256,
            ..wgpu::Limits::downlevel_webgl2_defaults()
        };
        let gl = required_limits(wgpu::Backend::Gl, &v3d);
        assert!(gl.check_limits(&v3d), "{gl:?}");
        assert_eq!(gl.max_texture_dimension_2d, 4096);
        let vk_v3d = wgpu::Limits {
            max_texture_dimension_1d: 4096,
            max_texture_dimension_2d: 4096,
            max_texture_dimension_3d: 256,
            ..wgpu::Limits::downlevel_defaults()
        };
        let vk = required_limits(wgpu::Backend::Vulkan, &vk_v3d);
        assert!(vk.check_limits(&vk_v3d), "{vk:?}");
        assert!(!wgpu::Limits::default().check_limits(&vk_v3d));
        // A desktop GPU's large textures are used, for 4K and larger windows.
        let desktop = wgpu::Limits::default().using_resolution(wgpu::Limits {
            max_texture_dimension_2d: 16384,
            ..wgpu::Limits::default()
        });
        let r = required_limits(wgpu::Backend::Vulkan, &desktop);
        assert_eq!(r.max_texture_dimension_2d, 16384);
        assert!(r.check_limits(&desktop));
    }
}
