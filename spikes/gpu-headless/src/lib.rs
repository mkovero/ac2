//! Phase 0 spike: gpu-headless. Throwaway code; findings live in docs/design/spike-gpu-headless.md.
//!
//! Headless wgpu device, offscreen plot rendering (instanced SDF line segments), pixel
//! readback and tolerance comparison against a reference PNG.

use std::fmt;
use std::path::Path;
use std::time::{Duration, Instant};

use bytemuck::{Pod, Zeroable};

/// Set to `1` to request only a software (CPU) adapter: lavapipe/llvmpipe, WARP.
pub const FALLBACK_ENV: &str = "AC2_GPU_FALLBACK";
/// Set to `1` to make a missing adapter a test failure instead of a skip (CI).
pub const REQUIRE_ENV: &str = "AC2_REQUIRE_GPU";
/// Set to `1` to regenerate reference images from the test run.
pub const BLESS_ENV: &str = "AC2_BLESS";

/// Offscreen target format. Non-sRGB so blending happens on encoded values, as egui does;
/// the readback bytes are then exactly what is blended, with no conversion step that a
/// software rasterizer might round differently.
pub const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

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

#[derive(Debug)]
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub info: wgpu::AdapterInfo,
    /// All adapters the instance saw, for logging.
    pub seen: Vec<wgpu::AdapterInfo>,
    pub create_time: Duration,
}

impl Gpu {
    /// Creates a device without any window or surface.
    ///
    /// Backends follow `WGPU_BACKEND` (e.g. `vulkan`, `gl`, `dx12`, `metal`); software
    /// adapters are forced with `AC2_GPU_FALLBACK=1`.
    pub fn new() -> Result<Self, GpuError> {
        pollster::block_on(Self::new_async())
    }

    async fn new_async() -> Result<Self, GpuError> {
        let t0 = Instant::now();
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let forced_fallback = std::env::var(FALLBACK_ENV).is_ok_and(|v| v == "1");
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::from_env().unwrap_or_default(),
                force_fallback_adapter: forced_fallback,
                compatible_surface: None,
                ..Default::default()
            })
            .await
            .map_err(|e| GpuError::NoAdapter {
                reason: e.to_string(),
                forced_fallback,
            })?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("spike-gpu-headless"),
                // Downlevel limits keep the renderer valid on GL/llvmpipe and WARP too.
                required_limits: wgpu::Limits::downlevel_defaults(),
                ..Default::default()
            })
            .await
            .map_err(GpuError::Device)?;
        let create_time = t0.elapsed();
        let seen = instance
            .enumerate_adapters(wgpu::Backends::all())
            .await
            .iter()
            .map(wgpu::Adapter::get_info)
            .collect();
        Ok(Self {
            device,
            queue,
            info: adapter.get_info(),
            seen,
            create_time,
        })
    }

    pub fn describe(&self) -> String {
        describe(&self.info)
    }
}

pub fn describe(i: &wgpu::AdapterInfo) -> String {
    format!(
        "{} [{:?}, {:?}, driver: {} {}]",
        i.name, i.backend, i.device_type, i.driver, i.driver_info
    )
}

/// Host-side RGBA8 image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn load_png(path: &Path) -> Result<Self, String> {
        let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let decoder = png::Decoder::new(std::io::BufReader::new(file));
        let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
        let size = reader
            .output_buffer_size()
            .ok_or_else(|| "png too large".to_string())?;
        let mut buf = vec![0; size];
        let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
        if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
            return Err(format!(
                "{}: expected RGBA8, got {:?}/{:?}",
                path.display(),
                info.color_type,
                info.bit_depth
            ));
        }
        buf.truncate(info.buffer_size());
        Ok(Self {
            width: info.width,
            height: info.height,
            rgba: buf,
        })
    }

    pub fn save_png(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), self.width, self.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::High);
        let mut w = enc.write_header().map_err(|e| e.to_string())?;
        w.write_image_data(&self.rgba).map_err(|e| e.to_string())
    }
}

/// Software rasterizers differ from each other (and from hardware) in edge rounding and
/// shader float precision; a pixel passes if every channel is within `channel`, and the
/// image passes if at most `max_bad_fraction` of pixels fail.
#[derive(Clone, Copy, Debug)]
pub struct Tolerance {
    pub channel: u8,
    pub max_bad_fraction: f64,
}

#[derive(Clone, Debug)]
pub struct Diff {
    pub bad_pixels: usize,
    pub total_pixels: usize,
    pub max_channel_diff: u8,
    /// Pixels differing at all (any channel > 0).
    pub nonzero_pixels: usize,
    /// Visualisation: red where outside tolerance, dim where within.
    pub image: Image,
}

impl Diff {
    pub fn bad_fraction(&self) -> f64 {
        self.bad_pixels as f64 / self.total_pixels as f64
    }

    pub fn passes(&self, tol: Tolerance) -> bool {
        self.bad_fraction() <= tol.max_bad_fraction
    }
}

pub fn compare(actual: &Image, expected: &Image, tol: Tolerance) -> Result<Diff, String> {
    if (actual.width, actual.height) != (expected.width, expected.height) {
        return Err(format!(
            "size mismatch: actual {}x{}, expected {}x{}",
            actual.width, actual.height, expected.width, expected.height
        ));
    }
    let mut bad = 0;
    let mut nonzero = 0;
    let mut max = 0u8;
    let mut vis = Vec::with_capacity(actual.rgba.len());
    for (a, e) in actual
        .rgba
        .chunks_exact(4)
        .zip(expected.rgba.chunks_exact(4))
    {
        let d = a
            .iter()
            .zip(e)
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap_or(0);
        max = max.max(d);
        if d > 0 {
            nonzero += 1;
        }
        if d > tol.channel {
            bad += 1;
            vis.extend_from_slice(&[255, 0, 0, 255]);
        } else {
            let g = e[0] / 4;
            vis.extend_from_slice(&[g, g.saturating_add(d * 8), g, 255]);
        }
    }
    Ok(Diff {
        bad_pixels: bad,
        total_pixels: (actual.width * actual.height) as usize,
        max_channel_diff: max,
        nonzero_pixels: nonzero,
        image: Image {
            width: actual.width,
            height: actual.height,
            rgba: vis,
        },
    })
}

// ---------------------------------------------------------------------------------------
// Scene

#[derive(Clone, Debug)]
pub struct Polyline {
    /// Pixel coordinates, origin top-left, y down.
    pub points: Vec<[f32; 2]>,
    /// Per-vertex opacity multiplier (coherence blanking).
    pub alpha: Vec<f32>,
    /// Straight (non-premultiplied) RGBA.
    pub color: [f32; 4],
    pub width: f32,
}

#[derive(Clone, Debug)]
pub struct Label {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub color: [u8; 4],
}

#[derive(Clone, Debug)]
pub struct Scene {
    pub width: u32,
    pub height: u32,
    pub background: [f64; 4],
    pub lines: Vec<Polyline>,
    pub labels: Vec<Label>,
}

const F_LO: f64 = 20.0;
const F_HI: f64 = 20_000.0;

/// Plot-like test scene: log-frequency grid, `curves` traces of `points` vertices each.
/// Values come from closed-form expressions, so the scene is identical on every machine.
pub fn demo_scene(width: u32, height: u32, curves: usize, points: usize) -> Scene {
    let (w, h) = (f64::from(width), f64::from(height));
    let margin = 0.06 * w.min(h);
    let (x0, x1, y0, y1) = (margin, w - margin, margin, h - margin);
    let fx = |f: f64| x0 + (f / F_LO).log10() / (F_HI / F_LO).log10() * (x1 - x0);
    let db_lo = -30.0;
    let db_hi = 18.0;
    let dby = |db: f64| y1 - (db - db_lo) / (db_hi - db_lo) * (y1 - y0);
    // Grid lines sit on pixel centres so a 1 px stroke covers exactly one pixel column.
    let snap = |v: f64| (v.floor() + 0.5) as f32;

    let mut lines = Vec::new();
    let grid = |major: bool| {
        if major {
            [0.55, 0.58, 0.62, 0.55]
        } else {
            [0.40, 0.42, 0.46, 0.30]
        }
    };
    for decade in [10.0, 100.0, 1000.0, 10_000.0] {
        for m in 1..10 {
            let f = decade * f64::from(m);
            if !(F_LO..=F_HI).contains(&f) {
                continue;
            }
            let x = snap(fx(f));
            lines.push(Polyline {
                points: vec![[x, y0 as f32], [x, y1 as f32]],
                alpha: vec![1.0, 1.0],
                color: grid(m == 1),
                width: 1.0,
            });
        }
    }
    let mut db = db_lo;
    while db <= db_hi {
        let y = snap(dby(db));
        lines.push(Polyline {
            points: vec![[x0 as f32, y], [x1 as f32, y]],
            alpha: vec![1.0, 1.0],
            color: grid(db == 0.0),
            width: 1.0,
        });
        db += 6.0;
    }

    let palette = [
        [0.00, 0.62, 0.95, 1.0],
        [1.00, 0.55, 0.10, 1.0],
        [0.30, 0.80, 0.35, 1.0],
        [0.90, 0.30, 0.45, 1.0],
    ];
    for c in 0..curves {
        let k = c as f64;
        let mut pts = Vec::with_capacity(points);
        let mut alpha = Vec::with_capacity(points);
        for i in 0..points {
            let u = i as f64 / (points - 1) as f64;
            let f = F_LO * (F_HI / F_LO).powf(u);
            let lf = f.log10();
            // A few resonances and a slope, offset per curve.
            let bump = |fc: f64, q: f64, g: f64| g / (1.0 + ((lf - fc.log10()) * q).powi(2));
            let mag = bump(60.0 * (1.0 + 0.3 * k), 6.0, 9.0) - bump(250.0, 8.0, 12.0)
                + bump(3000.0 / (1.0 + 0.2 * k), 4.0, 6.0)
                - 4.0 * (lf - 3.0).max(0.0).powi(2)
                + 3.0 * (lf * (3.0 + k)).sin()
                - 2.5 * k;
            pts.push([fx(f) as f32, dby(mag) as f32]);
            // Coherence: low below ~40 Hz and in a dip around 250 Hz.
            let coh_lf = ((lf - 1.4) / 0.25).clamp(0.0, 1.0);
            let coh_dip = 1.0 - 0.85 * (-((lf - 2.4) / 0.08).powi(2)).exp();
            alpha.push((0.15 + 0.85 * coh_lf * coh_dip) as f32);
        }
        lines.push(Polyline {
            points: pts,
            alpha,
            color: palette[c % palette.len()],
            width: 2.5 + (c % 2) as f32,
        });
    }

    Scene {
        width,
        height,
        background: [0.07, 0.08, 0.10, 1.0],
        lines,
        labels: vec![Label {
            text: "1 kHz  +3.2 dB  coh 0.98".to_string(),
            x: (x0 + 8.0) as f32,
            y: (y0 + 6.0) as f32,
            size: 16.0,
            color: [230, 230, 230, 255],
        }],
    }
}

// ---------------------------------------------------------------------------------------
// Renderer

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct Segment {
    prev: [f32; 2],
    p0: [f32; 2],
    p1: [f32; 2],
    next: [f32; 2],
    color: [f32; 4],
    alpha: [f32; 2],
    half_width: f32,
    flags: u32,
}

const HAS_PREV: u32 = 1;
const HAS_NEXT: u32 = 2;

fn segments(scene: &Scene) -> Vec<Segment> {
    let mut out = Vec::new();
    for l in &scene.lines {
        let n = l.points.len();
        for i in 0..n.saturating_sub(1) {
            let mut flags = 0;
            if i > 0 {
                flags |= HAS_PREV;
            }
            if i + 2 < n {
                flags |= HAS_NEXT;
            }
            out.push(Segment {
                prev: l.points[i.saturating_sub(1)],
                p0: l.points[i],
                p1: l.points[i + 1],
                next: l.points[(i + 2).min(n - 1)],
                color: l.color,
                alpha: [l.alpha[i], l.alpha[i + 1]],
                half_width: l.width * 0.5,
                flags,
            });
        }
    }
    out
}

/// Optional text path (glyphon / cosmic-text).
pub struct TextLayer {
    font_system: glyphon::FontSystem,
    swash: glyphon::SwashCache,
    viewport: glyphon::Viewport,
    atlas: glyphon::TextAtlas,
    renderer: glyphon::TextRenderer,
}

impl fmt::Debug for TextLayer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextLayer").finish_non_exhaustive()
    }
}

impl TextLayer {
    /// Uses system fonts. Returns `None` when the system has none; goldens must not depend
    /// on this path (font sets differ per machine), see the findings doc.
    pub fn new(gpu: &Gpu) -> Option<Self> {
        let font_system = glyphon::FontSystem::new();
        font_system.db().faces().next()?;
        let cache = glyphon::Cache::new(&gpu.device);
        let viewport = glyphon::Viewport::new(&gpu.device, &cache);
        let mut atlas = glyphon::TextAtlas::new(&gpu.device, &gpu.queue, &cache, TARGET_FORMAT);
        let renderer = glyphon::TextRenderer::new(
            &mut atlas,
            &gpu.device,
            wgpu::MultisampleState::default(),
            None,
        );
        Some(Self {
            font_system,
            swash: glyphon::SwashCache::new(),
            viewport,
            atlas,
            renderer,
        })
    }

    fn prepare(&mut self, gpu: &Gpu, scene: &Scene) -> Result<(), glyphon::PrepareError> {
        self.viewport.update(
            &gpu.queue,
            glyphon::Resolution {
                width: scene.width,
                height: scene.height,
            },
        );
        let buffers: Vec<glyphon::Buffer> = scene
            .labels
            .iter()
            .map(|l| {
                let mut b = glyphon::Buffer::new(
                    &mut self.font_system,
                    glyphon::Metrics::new(l.size, l.size * 1.25),
                );
                b.set_size(None, None);
                b.set_text(
                    &l.text,
                    &glyphon::Attrs::new().family(glyphon::Family::SansSerif),
                    glyphon::Shaping::Advanced,
                    None,
                );
                b.shape_until_scroll(&mut self.font_system, false);
                b
            })
            .collect();
        let areas = scene
            .labels
            .iter()
            .zip(&buffers)
            .map(|(l, b)| glyphon::TextArea {
                buffer: b,
                left: l.x,
                top: l.y,
                scale: 1.0,
                bounds: glyphon::TextBounds {
                    left: 0,
                    top: 0,
                    right: scene.width as i32,
                    bottom: scene.height as i32,
                },
                default_color: glyphon::Color::rgba(l.color[0], l.color[1], l.color[2], l.color[3]),
                custom_glyphs: &[],
            });
        self.renderer.prepare(
            &gpu.device,
            &gpu.queue,
            &mut self.font_system,
            &mut self.atlas,
            &self.viewport,
            areas,
            &mut self.swash,
        )
    }
}

#[derive(Debug)]
struct Target {
    width: u32,
    height: u32,
    texture: wgpu::Texture,
    readback: wgpu::Buffer,
    padded_row: u32,
}

#[derive(Debug)]
pub struct Renderer {
    pipeline: wgpu::RenderPipeline,
    globals: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    instances: Option<(wgpu::Buffer, u64)>,
    target: Option<Target>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FrameTiming {
    /// CPU: build instances, upload, encode, submit.
    pub encode: Duration,
    /// Submit to readback mapped (GPU work + copy + map).
    pub gpu_and_map: Duration,
    /// Copy mapped rows out (un-padding).
    pub unpack: Duration,
    pub total: Duration,
}

impl Renderer {
    pub fn new(gpu: &Gpu) -> Self {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("plot.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("plot.wgsl").into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("plot"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let attrs = wgpu::vertex_attr_array![
            0 => Float32x2, 1 => Float32x2, 2 => Float32x2, 3 => Float32x2,
            4 => Float32x4, 5 => Float32x2, 6 => Float32, 7 => Uint32
        ];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("plot lines"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Segment>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &attrs,
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: TARGET_FORMAT,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });
        Self {
            pipeline,
            globals,
            bind_group,
            instances: None,
            target: None,
        }
    }

    fn target(&mut self, gpu: &Gpu, width: u32, height: u32) -> &Target {
        let fits = self
            .target
            .as_ref()
            .is_some_and(|t| t.width == width && t.height == height);
        if !fits {
            let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("offscreen"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: TARGET_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
            let padded_row = (width * 4).div_ceil(align) * align;
            let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("readback"),
                size: u64::from(padded_row) * u64::from(height),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.target = Some(Target {
                width,
                height,
                texture,
                readback,
                padded_row,
            });
        }
        self.target.as_ref().unwrap_or_else(|| unreachable!())
    }

    /// Renders `scene` offscreen and reads it back.
    pub fn render(
        &mut self,
        gpu: &Gpu,
        scene: &Scene,
        mut text: Option<&mut TextLayer>,
    ) -> Result<(Image, FrameTiming), String> {
        let t0 = Instant::now();
        let segs = segments(scene);
        let bytes: &[u8] = bytemuck::cast_slice(&segs);
        let need = (bytes.len() as u64).max(256);
        if self.instances.as_ref().is_none_or(|(_, cap)| *cap < need) {
            let buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("segments"),
                size: need.next_power_of_two(),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instances = Some((buf, need.next_power_of_two()));
        }
        let (inst, _) = self.instances.as_ref().ok_or("no instance buffer")?;
        if !bytes.is_empty() {
            gpu.queue.write_buffer(inst, 0, bytes);
        }
        let vp = [scene.width as f32, scene.height as f32, 0.0, 0.0];
        gpu.queue
            .write_buffer(&self.globals, 0, bytemuck::cast_slice(&vp));
        if let Some(t) = text.as_deref_mut() {
            t.prepare(gpu, scene).map_err(|e| e.to_string())?;
        }

        // Re-borrow pieces after the mutable target() call.
        self.target(gpu, scene.width, scene.height);
        let target = self.target.as_ref().ok_or("no target")?;
        let (inst, _) = self.instances.as_ref().ok_or("no instance buffer")?;
        let view = target
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut enc = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let bg = scene.background;
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("plot"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: bg[0],
                            g: bg[1],
                            b: bg[2],
                            a: bg[3],
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if !segs.is_empty() {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, inst.slice(..bytes.len() as u64));
                pass.draw(0..4, 0..segs.len() as u32);
            }
            if let Some(t) = text.as_deref() {
                t.renderer
                    .render(&t.atlas, &t.viewport, &mut pass)
                    .map_err(|e| e.to_string())?;
            }
        }
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &target.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(target.padded_row),
                    rows_per_image: Some(scene.height),
                },
            },
            wgpu::Extent3d {
                width: scene.width,
                height: scene.height,
                depth_or_array_layers: 1,
            },
        );
        gpu.queue.submit([enc.finish()]);
        let t1 = Instant::now();

        let slice = target.readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        gpu.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| e.to_string())?;
        rx.recv()
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let t2 = Instant::now();

        let row = (scene.width * 4) as usize;
        let mut rgba = Vec::with_capacity(row * scene.height as usize);
        {
            let data = slice.get_mapped_range().map_err(|e| e.to_string())?;
            for r in data.chunks(target.padded_row as usize) {
                rgba.extend_from_slice(&r[..row]);
            }
        }
        target.readback.unmap();
        let t3 = Instant::now();
        Ok((
            Image {
                width: scene.width,
                height: scene.height,
                rgba,
            },
            FrameTiming {
                encode: t1 - t0,
                gpu_and_map: t2 - t1,
                unpack: t3 - t2,
                total: t3 - t0,
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_layout_matches_shader() {
        assert_eq!(std::mem::size_of::<Segment>(), 64);
    }

    #[test]
    fn compare_counts_out_of_tolerance_pixels() {
        let a = Image {
            width: 2,
            height: 1,
            rgba: vec![10, 10, 10, 255, 0, 0, 0, 255],
        };
        let mut b = a.clone();
        b.rgba[0] = 13;
        b.rgba[4] = 100;
        let tol = Tolerance {
            channel: 4,
            max_bad_fraction: 0.4,
        };
        let d = compare(&a, &b, tol).expect("same size");
        assert_eq!(d.bad_pixels, 1);
        assert_eq!(d.nonzero_pixels, 2);
        assert_eq!(d.max_channel_diff, 100);
        assert!(!d.passes(tol));
    }

    #[test]
    fn joins_are_flagged_only_inside_polyline() {
        let s = demo_scene(200, 100, 1, 4);
        let segs = segments(&s);
        let curve = &segs[segs.len() - 3..];
        assert_eq!(curve[0].flags, HAS_NEXT);
        assert_eq!(curve[1].flags, HAS_PREV | HAS_NEXT);
        assert_eq!(curve[2].flags, HAS_PREV);
    }
}
