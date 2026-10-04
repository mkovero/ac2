//! Headless device and offscreen rendering with pixel readback, for tests, screenshots and
//! benchmarks. The interactive path is [`Renderer::prepare`] + [`Renderer::paint`] inside
//! the UI toolkit's own render pass.

use std::fmt;
use std::time::{Duration, Instant};

use crate::renderer::{FrameTarget, PrepareError, Renderer};
use crate::scene::{Color, Scene};

/// Set to `1` to request only a software (CPU) adapter: lavapipe/llvmpipe, WARP.
pub const FALLBACK_ENV: &str = "AC2_GPU_FALLBACK";

/// Offscreen format. Not sRGB, so blending happens on display-encoded values as in egui,
/// and the bytes read back are exactly what was blended.
pub const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

#[derive(Debug)]
pub enum GpuError {
    NoAdapter {
        reason: String,
        forced_fallback: bool,
    },
    Device(wgpu::RequestDeviceError),
}

impl fmt::Display for GpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GpuError::NoAdapter {
                reason,
                forced_fallback,
            } => write!(
                f,
                "no wgpu adapter (force_fallback_adapter={forced_fallback}): {reason}. \
                 Linux needs a Vulkan ICD (mesa-vulkan-drivers -> lavapipe) or EGL/GL \
                 (libegl-mesa0 -> llvmpipe, select with WGPU_BACKEND=gl)"
            ),
            GpuError::Device(e) => write!(f, "request_device failed: {e}"),
        }
    }
}

impl std::error::Error for GpuError {}

/// A device without window or surface.
#[derive(Debug)]
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub info: wgpu::AdapterInfo,
}

impl Gpu {
    /// Backends follow `WGPU_BACKEND` (`vulkan`, `gl`, `dx12`, `metal`); a software adapter
    /// is forced with `AC2_GPU_FALLBACK=1`. Limits are downlevel defaults so the renderer is
    /// exercised under the same limits as GL/llvmpipe and WARP.
    pub fn new() -> Result<Self, GpuError> {
        pollster::block_on(Self::new_async())
    }

    async fn new_async() -> Result<Self, GpuError> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let forced_fallback = std::env::var(FALLBACK_ENV).is_ok_and(|v| v == "1");
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                // The integrated GPU of a two-GPU laptop, as the app's window uses.
                power_preference: wgpu::PowerPreference::from_env()
                    .unwrap_or(wgpu::PowerPreference::LowPower),
                force_fallback_adapter: forced_fallback,
                compatible_surface: None,
                ..Default::default()
            })
            .await
            .map_err(|e| GpuError::NoAdapter {
                reason: e.to_string(),
                forced_fallback,
            })?;
        // GLES (a Raspberry Pi's V3D) grants only the WebGL2-class limits, which the
        // renderer stays within.
        let limits = if adapter.get_info().backend == wgpu::Backend::Gl {
            wgpu::Limits::downlevel_webgl2_defaults()
        } else {
            wgpu::Limits::downlevel_defaults()
        };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("ac2-plot headless"),
                required_limits: limits,
                ..Default::default()
            })
            .await
            .map_err(GpuError::Device)?;
        Ok(Self {
            device,
            queue,
            info: adapter.get_info(),
        })
    }

    /// Adapter name, backend, type and driver: golden failures need this to triage.
    pub fn describe(&self) -> String {
        let i = &self.info;
        format!(
            "{} [{:?}, {:?}, driver: {} {}]",
            i.name, i.backend, i.device_type, i.driver, i.driver_info
        )
    }
}

/// RGBA8 pixels, rows top to bottom, no padding.
#[derive(Clone, PartialEq, Eq)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl fmt::Debug for Pixels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Pixels({}x{})", self.width, self.height)
    }
}

#[derive(Debug)]
pub enum RenderError {
    Prepare(PrepareError),
    Paint(glyphon::RenderError),
    Readback(String),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenderError::Prepare(e) => write!(f, "prepare: {e}"),
            RenderError::Paint(e) => write!(f, "paint: {e}"),
            RenderError::Readback(e) => write!(f, "readback: {e}"),
        }
    }
}

impl std::error::Error for RenderError {}

/// Wall time of one offscreen frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameTiming {
    /// CPU: [`Renderer::prepare`].
    pub prepare: Duration,
    /// CPU: encode the pass and submit.
    pub encode: Duration,
    /// Submit to readback mapped (GPU work, copy, map).
    pub gpu: Duration,
}

/// Reusable offscreen colour target and readback buffer.
#[derive(Debug)]
pub struct Offscreen {
    size: [u32; 2],
    texture: wgpu::Texture,
    readback: wgpu::Buffer,
    padded_row: u32,
}

impl Offscreen {
    pub fn new(device: &wgpu::Device, size: [u32; 2]) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ac2-plot offscreen"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_row = (size[0] * 4).div_ceil(align) * align;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ac2-plot readback"),
            size: u64::from(padded_row) * u64::from(size[1]),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            size,
            texture,
            readback,
            padded_row,
        }
    }

    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    /// Clears to `background`, paints `scene` at `scale` physical pixels per logical pixel
    /// over the whole target and reads the frame back.
    pub fn render(
        &self,
        gpu: &Gpu,
        renderer: &mut Renderer,
        scene: &Scene,
        scale: f32,
        background: Color,
    ) -> Result<(Pixels, FrameTiming), RenderError> {
        let t0 = Instant::now();
        renderer
            .prepare(
                &gpu.device,
                &gpu.queue,
                scene,
                &FrameTarget::full(self.size, scale),
            )
            .map_err(RenderError::Prepare)?;
        let prepare = t0.elapsed();
        let (px, timing) = self.paint_prepared(gpu, renderer, background)?;
        Ok((px, FrameTiming { prepare, ..timing }))
    }

    /// Clears to `background`, paints whatever `renderer` last prepared (for a
    /// [`FrameTarget`] of this target's size) and reads the frame back.
    pub fn paint_prepared(
        &self,
        gpu: &Gpu,
        renderer: &Renderer,
        background: Color,
    ) -> Result<(Pixels, FrameTiming), RenderError> {
        let t1 = Instant::now();
        let view = self
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut enc = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ac2-plot offscreen"),
            });
        {
            let bg = |v: f32| f64::from(v);
            // The clear colour is the premultiplied value the blend stage would have left.
            let a = background.a;
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ac2-plot offscreen"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: bg(background.r * a),
                            g: bg(background.g * a),
                            b: bg(background.b * a),
                            a: bg(a),
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            renderer.paint(&mut pass).map_err(RenderError::Paint)?;
        }
        enc.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_row),
                    rows_per_image: Some(self.size[1]),
                },
            },
            wgpu::Extent3d {
                width: self.size[0],
                height: self.size[1],
                depth_or_array_layers: 1,
            },
        );
        gpu.queue.submit([enc.finish()]);
        let t2 = Instant::now();

        let slice = self.readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let rb = |e: &dyn fmt::Display| RenderError::Readback(e.to_string());
        gpu.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| rb(&e))?;
        rx.recv().map_err(|e| rb(&e))?.map_err(|e| rb(&e))?;
        let row = self.size[0] as usize * 4;
        let mut rgba = Vec::with_capacity(row * self.size[1] as usize);
        {
            let data = slice.get_mapped_range().map_err(|e| rb(&e))?;
            for r in data.chunks(self.padded_row as usize) {
                rgba.extend_from_slice(&r[..row]);
            }
        }
        self.readback.unmap();
        let t3 = Instant::now();
        Ok((
            Pixels {
                width: self.size[0],
                height: self.size[1],
                rgba,
            },
            FrameTiming {
                prepare: Duration::ZERO,
                encode: t2 - t1,
                gpu: t3 - t2,
            },
        ))
    }
}

/// One-shot convenience: a fresh offscreen target of `size` physical pixels.
pub fn render_to_image(
    gpu: &Gpu,
    renderer: &mut Renderer,
    scene: &Scene,
    size: [u32; 2],
    scale: f32,
    background: Color,
) -> Result<Pixels, RenderError> {
    Offscreen::new(&gpu.device, size)
        .render(gpu, renderer, scene, scale, background)
        .map(|(p, _)| p)
}
