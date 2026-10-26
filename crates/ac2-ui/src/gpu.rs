//! How the window's wgpu device is chosen.
//!
//! - The low-power adapter: on a laptop with two GPUs the integrated one draws these plots
//!   at a fraction of the discrete one's power, and waking the discrete GPU costs battery
//!   for nothing. `WGPU_POWER_PREF=high` still picks the other.
//! - Downlevel limits sized to the adapter's textures: the plots need only vertex
//!   instancing, uniforms and sampled textures (no storage buffers, no compute), so asking
//!   for more only rules out GPUs that would draw them fine — a Raspberry Pi 4 (V3D, 4096 px
//!   textures, GLES 3.1 or v3dv Vulkan) refuses wgpu's desktop defaults.
//! - A present mode that never waits for vblank: with FIFO, acquiring the next surface
//!   texture blocks until the display takes a frame, and a display that is off or unplugged
//!   (an NVIDIA X11 output with the TV switched off) gives none, so the acquire times out
//!   after a second and the whole UI thread — picture, keys, the stimulus's stop key — runs
//!   at 1 Hz. The app paces its own frames instead ([`frame_interval`], [`crate::pace`]).

use std::sync::Arc;
use std::time::Duration;

use ac2_plot::{Rasterizer, wgpu};

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

/// The fastest a CPU rasteriser is asked to draw: 30 frames a second. A dense 1080p plot
/// (16 curves, grid, labels) takes about 20 ms to rasterise on llvmpipe, so 60 frames a
/// second cannot be drawn at all and keeps the rasteriser's threads busy all the time,
/// taking cores from the analyzer and the daemon; at 30 they draw about 60 % of the time,
/// and a level or spectrum updating 30 times a second still reads as continuous motion.
pub const SOFTWARE_FRAME_INTERVAL: Duration = Duration::from_micros(33_333);

/// The fastest a GPU is asked to draw: 60 frames a second. The surface no longer waits for
/// vblank, so without a cap a stream of data frames or an animation would draw as fast as
/// the GPU can; 60 is the common display rate (egui does not report the monitor's), and
/// levels and spectra gain nothing readable above it.
pub const GPU_FRAME_INTERVAL: Duration = Duration::from_micros(16_667);

/// Least time between frame starts asked for by data or animation:
/// [`GPU_FRAME_INTERVAL`] on a GPU, [`SOFTWARE_FRAME_INTERVAL`] on a CPU rasteriser. Input
/// still repaints at once, so the pointer and keys stay responsive.
pub fn frame_interval(r: Rasterizer) -> Duration {
    match r {
        Rasterizer::Gpu => GPU_FRAME_INTERVAL,
        Rasterizer::Software => SOFTWARE_FRAME_INTERVAL,
    }
}

/// The surface's present mode: `AutoNoVsync`, which wgpu resolves against the surface's
/// capabilities to Immediate, else Mailbox, else FIFO. Mailbox would never tear, but egui
/// configures the surface itself and gives no hook to read its capabilities first, and
/// asking for a mode the surface lacks is an error rather than a fallback. Immediate may
/// tear on an uncomposited X11 screen; a compositor (xfwm4's, Wayland) usually shows whole frames.
/// GLES on Linux offers only FIFO, so a GL adapter (V3D on GLES) keeps vblank pacing.
pub const PRESENT_MODE: wgpu::PresentMode = wgpu::PresentMode::AutoNoVsync;

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
        surface: egui_wgpu::SurfaceConfig {
            present_mode: PRESENT_MODE,
            // Two frames in flight: a slow GPU (V3D) records the next frame while the last
            // one draws; the pacing, not the queue, bounds how far ahead the app runs.
            desired_maximum_frame_latency: Some(2),
        },
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_rasterisers_are_paced() {
        let gpu = frame_interval(Rasterizer::Gpu);
        assert!((1.0 / gpu.as_secs_f64() - 60.0).abs() < 0.01, "{gpu:?}");
        let sw = frame_interval(Rasterizer::Software);
        assert!((1.0 / sw.as_secs_f64() - 30.0).abs() < 0.01, "{sw:?}");
    }

    /// The surface never blocks on vblank where the surface can avoid it, and the latency
    /// is set rather than left to egui's default.
    #[test]
    fn present_mode_does_not_wait_for_vblank() {
        let o = wgpu_options();
        assert_eq!(o.surface.present_mode, wgpu::PresentMode::AutoNoVsync);
        assert_eq!(o.surface.desired_maximum_frame_latency, Some(2));
    }

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
