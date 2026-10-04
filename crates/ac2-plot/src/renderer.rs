//! The renderer: `prepare` uploads a scene into reusable buffers, `paint` records draws into
//! a caller-owned render pass (an egui-wgpu paint callback or the offscreen path).

use std::fmt;
use std::sync::{Arc, Mutex};

use bytemuck::{Pod, Zeroable};

use crate::geometry::{
    self, ClipPx, FillInstance, LineScratch, SegmentInstance, Xform, intersect, is_empty,
    scissor_of,
};
use crate::heatmap::{self, Heatmaps};
use crate::scene::{HeatmapId, Rect, Scene};
use crate::text::{Fonts, TextLayer};

/// Where and how a scene lands in the render target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameTarget {
    /// Render target size in physical pixels.
    pub size_px: [u32; 2],
    /// Physical pixel position of the scene's logical origin (the widget's top-left).
    pub origin_px: [f32; 2],
    /// Physical pixels per logical pixel (egui's `pixels_per_point`).
    pub scale: f32,
    /// Outer clip in physical pixels `[x0, y0, x1, y1]` (egui's clip rect); nothing is drawn
    /// outside it or outside the scene viewport.
    pub clip_px: [f32; 4],
}

impl FrameTarget {
    /// The scene fills a `size_px` target at `scale`.
    pub fn full(size_px: [u32; 2], scale: f32) -> Self {
        Self {
            size_px,
            origin_px: [0.0, 0.0],
            scale,
            clip_px: [0.0, 0.0, size_px[0] as f32, size_px[1] as f32],
        }
    }
}

#[derive(Debug)]
pub enum PrepareError {
    /// `Polyline::alpha` is neither empty nor one value per point.
    AlphaLength {
        layer: usize,
        polyline: usize,
        points: usize,
        alpha: usize,
    },
    Heatmap {
        id: HeatmapId,
        reason: String,
    },
    InvalidTarget(FrameTarget),
    Text(glyphon::PrepareError),
}

impl fmt::Display for PrepareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PrepareError::AlphaLength {
                layer,
                polyline,
                points,
                alpha,
            } => write!(
                f,
                "layer {layer} polyline {polyline}: {alpha} alpha values for {points} points"
            ),
            PrepareError::Heatmap { id, reason } => write!(f, "heatmap {}: {reason}", id.0),
            PrepareError::InvalidTarget(t) => write!(f, "invalid frame target {t:?}"),
            PrepareError::Text(e) => write!(f, "text: {e}"),
        }
    }
}

impl std::error::Error for PrepareError {}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct Globals {
    target_size: [f32; 2],
    srgb_target: u32,
    _pad: u32,
}

/// One draw in paint order.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Draw {
    Fill { first: u32, end: u32 },
    Lines { first: u32, end: u32 },
    Heatmap(HeatmapId),
    Text(usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Cmd {
    draw: Draw,
    scissor: [u32; 4],
}

/// Vertex buffer that grows to the next power of two and is otherwise reused.
#[derive(Debug)]
struct GrowBuffer {
    label: &'static str,
    buffer: Option<wgpu::Buffer>,
}

impl GrowBuffer {
    fn write(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let need = bytes.len() as u64;
        if self.buffer.as_ref().is_none_or(|b| b.size() < need) {
            self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(self.label),
                size: need.next_power_of_two().max(4096),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        if let Some(b) = &self.buffer {
            queue.write_buffer(b, 0, bytes);
        }
    }
}

/// What every renderer of one target format shares: pipelines (compiled once — on a
/// GLES driver each is a WGSL→GLSL translation and a link), the glyph pipelines, and the
/// font and glyph rasterization caches. Per-renderer state (instance buffers, heatmaps,
/// glyph atlas, laid-out text) stays with each [`Renderer`], so a renderer whose scene did
/// not change keeps what it drew while another prepares.
pub struct RenderShared {
    pub(crate) format: wgpu::TextureFormat,
    pub(crate) multisample: wgpu::MultisampleState,
    globals_layout: wgpu::BindGroupLayout,
    heatmap_layout: wgpu::BindGroupLayout,
    lines: wgpu::RenderPipeline,
    fill: wgpu::RenderPipeline,
    heatmap: wgpu::RenderPipeline,
    pub(crate) glyphs: glyphon::Cache,
    pub(crate) fonts: Mutex<Fonts>,
}

impl fmt::Debug for RenderShared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RenderShared")
            .field("format", &self.format)
            .finish_non_exhaustive()
    }
}

/// Paints [`Scene`]s. Build once per target format; reuse across frames.
pub struct Renderer {
    shared: Arc<RenderShared>,
    globals: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    heatmaps: Heatmaps,
    text: TextLayer,
    segments: Vec<SegmentInstance>,
    fills: Vec<FillInstance>,
    scratch: LineScratch,
    segment_buf: GrowBuffer,
    fill_buf: GrowBuffer,
    cmds: Vec<Cmd>,
    target: [u32; 2],
}

impl fmt::Debug for Renderer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Renderer")
            .field("format", &self.shared.format)
            .field("segments", &self.segments.len())
            .field("fills", &self.fills.len())
            .field("cmds", &self.cmds.len())
            .finish_non_exhaustive()
    }
}

fn shader(device: &wgpu::Device, label: &str, body: &str) -> wgpu::ShaderModule {
    let src = format!("{}\n{body}", include_str!("shaders/common.wgsl"));
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(src.into()),
    })
}

struct PipelineSpec<'a> {
    label: &'a str,
    module: &'a wgpu::ShaderModule,
    layout: &'a wgpu::PipelineLayout,
    instance: Option<wgpu::VertexBufferLayout<'a>>,
}

fn pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    multisample: wgpu::MultisampleState,
    spec: PipelineSpec<'_>,
) -> wgpu::RenderPipeline {
    let buffers = [spec.instance];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(spec.label),
        layout: Some(spec.layout),
        vertex: wgpu::VertexState {
            module: spec.module,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: if buffers[0].is_some() { &buffers } else { &[] },
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            ..Default::default()
        },
        depth_stencil: None,
        multisample,
        fragment: Some(wgpu::FragmentState {
            module: spec.module,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

const SEGMENT_ATTRS: [wgpu::VertexAttribute; 10] = wgpu::vertex_attr_array![
    0 => Float32x2, 1 => Float32x2, 2 => Float32x2, 3 => Float32x2, 4 => Float32x4,
    5 => Float32x2, 6 => Float32x3, 7 => Float32, 8 => Unorm8x4, 9 => Uint32
];
const FILL_ATTRS: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
    0 => Float32x2, 1 => Float32x2, 2 => Float32x2, 3 => Float32x4, 4 => Unorm8x4, 5 => Uint32
];

impl RenderShared {
    /// Pipelines for a `format` target with `multisample` (1 sample unless the host UI
    /// renders with MSAA; the line and fill shaders anti-alias analytically).
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        multisample: wgpu::MultisampleState,
    ) -> Arc<Self> {
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ac2-plot globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let heatmap_layout = heatmap::bind_group_layout(device);
        let simple_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ac2-plot"),
            bind_group_layouts: &[Some(&globals_layout)],
            immediate_size: 0,
        });
        let heatmap_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ac2-plot heatmap"),
            bind_group_layouts: &[Some(&globals_layout), Some(&heatmap_layout)],
            immediate_size: 0,
        });

        let lines_mod = shader(
            device,
            "ac2-plot lines.wgsl",
            include_str!("shaders/lines.wgsl"),
        );
        let fill_mod = shader(
            device,
            "ac2-plot fill.wgsl",
            include_str!("shaders/fill.wgsl"),
        );
        let heat_mod = shader(
            device,
            "ac2-plot heatmap.wgsl",
            include_str!("shaders/heatmap.wgsl"),
        );
        let lines = pipeline(
            device,
            format,
            multisample,
            PipelineSpec {
                label: "ac2-plot lines",
                module: &lines_mod,
                layout: &simple_layout,
                instance: Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<SegmentInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &SEGMENT_ATTRS,
                }),
            },
        );
        let fill = pipeline(
            device,
            format,
            multisample,
            PipelineSpec {
                label: "ac2-plot fill",
                module: &fill_mod,
                layout: &simple_layout,
                instance: Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<FillInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &FILL_ATTRS,
                }),
            },
        );
        let heatmap = pipeline(
            device,
            format,
            multisample,
            PipelineSpec {
                label: "ac2-plot heatmap",
                module: &heat_mod,
                layout: &heatmap_pl,
                instance: None,
            },
        );
        Arc::new(Self {
            format,
            multisample,
            globals_layout,
            heatmap_layout,
            lines,
            fill,
            heatmap,
            glyphs: glyphon::Cache::new(device),
            fonts: Mutex::new(Fonts::new()),
        })
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }
}

impl Renderer {
    /// A renderer drawing with `shared`'s pipelines into its own buffers.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, shared: &Arc<RenderShared>) -> Self {
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ac2-plot globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ac2-plot globals"),
            layout: &shared.globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });
        Self {
            shared: Arc::clone(shared),
            globals,
            globals_bg,
            heatmaps: Heatmaps::new(shared.heatmap_layout.clone()),
            text: TextLayer::new(device, queue, shared),
            segments: Vec::new(),
            fills: Vec::new(),
            scratch: LineScratch::default(),
            segment_buf: GrowBuffer {
                label: "ac2-plot segments",
                buffer: None,
            },
            fill_buf: GrowBuffer {
                label: "ac2-plot fills",
                buffer: None,
            },
            cmds: Vec::new(),
            target: [0, 0],
        }
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.shared.format
    }

    /// The pipelines and caches this renderer draws with.
    pub fn shared(&self) -> &Arc<RenderShared> {
        &self.shared
    }

    /// Converts `scene` to instances and uploads them, heatmap columns and text. Steady
    /// state reuses every CPU and GPU buffer; only growth or new heatmaps allocate.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &Scene,
        target: &FrameTarget,
    ) -> Result<(), PrepareError> {
        if target.size_px[0] == 0 || target.size_px[1] == 0 || !geometry::positive(target.scale) {
            return Err(PrepareError::InvalidTarget(*target));
        }
        for (li, layer) in scene.layers.iter().enumerate() {
            for (pi, p) in layer.polylines.iter().enumerate() {
                if !p.alpha.is_empty() && p.alpha.len() != p.points.len() {
                    return Err(PrepareError::AlphaLength {
                        layer: li,
                        polyline: pi,
                        points: p.points.len(),
                        alpha: p.alpha.len(),
                    });
                }
            }
        }

        self.target = target.size_px;
        self.segments.clear();
        self.fills.clear();
        self.cmds.clear();
        self.heatmaps.begin();
        self.text.begin(queue, target.size_px);

        let xf = Xform {
            scale: target.scale,
            origin: target.origin_px,
        };
        let full = [0.0, 0.0, target.size_px[0] as f32, target.size_px[1] as f32];
        let scene_rect = xf.rect(&Rect::new(
            0.0,
            0.0,
            scene.viewport.width,
            scene.viewport.height,
        ));
        let base_clip = intersect(intersect(full, target.clip_px), scene_rect);
        let clip_of = |c: &Option<Rect>| match c {
            Some(r) => intersect(base_clip, xf.rect(r)),
            None => base_clip,
        };
        let size = target.size_px;

        let mut text_layers = 0;
        for layer in &scene.layers {
            for r in &layer.rects {
                let clip = clip_of(&r.clip);
                let first = self.fills.len();
                geometry::push_rect(&mut self.fills, r, &xf, clip);
                push_cmd(
                    &mut self.cmds,
                    Kind::Fill,
                    first,
                    self.fills.len(),
                    &clip,
                    size,
                );
            }
            for h in &layer.heatmaps {
                let clip = clip_of(&h.clip);
                // Uploads apply even when the heatmap is clipped away, so the ring stays
                // complete while it is scrolled out of view.
                self.heatmaps
                    .prepare(device, queue, h, xf.rect(&h.rect), clip)?;
                if let Some(scissor) = scissor_of(&clip, size) {
                    self.cmds.push(Cmd {
                        draw: Draw::Heatmap(h.id),
                        scissor,
                    });
                }
            }
            for b in &layer.bands {
                let clip = clip_of(&b.clip);
                let first = self.fills.len();
                if !is_empty(&clip) {
                    geometry::push_band(&mut self.fills, b, &xf, clip);
                }
                push_cmd(
                    &mut self.cmds,
                    Kind::Fill,
                    first,
                    self.fills.len(),
                    &clip,
                    size,
                );
            }
            for g in &layer.grids {
                let first_fill = self.fills.len();
                let first = self.segments.len();
                let clip =
                    geometry::push_grid(&mut self.fills, &mut self.segments, g, &xf, base_clip);
                push_cmd(
                    &mut self.cmds,
                    Kind::Fill,
                    first_fill,
                    self.fills.len(),
                    &clip,
                    size,
                );
                push_cmd(
                    &mut self.cmds,
                    Kind::Lines,
                    first,
                    self.segments.len(),
                    &clip,
                    size,
                );
            }
            for p in &layer.polylines {
                let clip = clip_of(&p.clip);
                let first = self.segments.len();
                if !is_empty(&clip) {
                    geometry::push_polyline(&mut self.segments, &mut self.scratch, p, &xf, clip);
                }
                push_cmd(
                    &mut self.cmds,
                    Kind::Lines,
                    first,
                    self.segments.len(),
                    &clip,
                    size,
                );
            }
            let visible = |l: &&crate::scene::Label| {
                !l.text.is_empty()
                    && l.size > 0.0
                    && l.color.a > 0.0
                    && !is_empty(&clip_of(&l.clip))
            };
            if layer.labels.iter().any(|l| visible(&l)) {
                let labels = layer
                    .labels
                    .iter()
                    .filter(visible)
                    .map(|l| (l, clip_of(&l.clip)));
                self.text
                    .prepare_layer(&self.shared, device, queue, text_layers, labels, &xf)
                    .map_err(PrepareError::Text)?;
                if let Some(scissor) = scissor_of(&base_clip, size) {
                    self.cmds.push(Cmd {
                        draw: Draw::Text(text_layers),
                        scissor,
                    });
                }
                text_layers += 1;
            }
        }
        self.heatmaps.end();

        let globals = Globals {
            target_size: [size[0] as f32, size[1] as f32],
            srgb_target: u32::from(self.shared.format.is_srgb()),
            _pad: 0,
        };
        queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));
        self.segment_buf
            .write(device, queue, bytemuck::cast_slice(&self.segments));
        self.fill_buf
            .write(device, queue, bytemuck::cast_slice(&self.fills));
        Ok(())
    }

    /// Records the prepared scene into `pass`, whose colour attachment must be the target
    /// described by the last [`FrameTarget`]. Sets its own viewport (the whole target) and
    /// scissors, so the caller should restore its state afterwards (egui-wgpu does).
    pub fn paint(&self, pass: &mut wgpu::RenderPass<'_>) -> Result<(), glyphon::RenderError> {
        let [w, h] = self.target;
        if self.cmds.is_empty() || w == 0 || h == 0 {
            return Ok(());
        }
        pass.set_viewport(0.0, 0.0, w as f32, h as f32, 0.0, 1.0);
        for cmd in &self.cmds {
            let [x, y, sw, sh] = cmd.scissor;
            pass.set_scissor_rect(x, y, sw, sh);
            match cmd.draw {
                Draw::Fill { first, end } => {
                    let Some(buf) = &self.fill_buf.buffer else {
                        continue;
                    };
                    pass.set_pipeline(&self.shared.fill);
                    pass.set_bind_group(0, &self.globals_bg, &[]);
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..4, first..end);
                }
                Draw::Lines { first, end } => {
                    let Some(buf) = &self.segment_buf.buffer else {
                        continue;
                    };
                    pass.set_pipeline(&self.shared.lines);
                    pass.set_bind_group(0, &self.globals_bg, &[]);
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..4, first..end);
                }
                Draw::Heatmap(id) => {
                    let Some(bg) = self.heatmaps.bind_group(id) else {
                        continue;
                    };
                    pass.set_pipeline(&self.shared.heatmap);
                    pass.set_bind_group(0, &self.globals_bg, &[]);
                    pass.set_bind_group(1, bg, &[]);
                    pass.draw(0..4, 0..1);
                }
                Draw::Text(i) => self.text.paint(i, pass)?,
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Fill,
    Lines,
}

/// Records instances `first..end` of one pipeline, merging with the previous draw when it is
/// the same pipeline, contiguous and under the same scissor. The shaders clip each instance
/// exactly, so merging only needs equal scissors.
fn push_cmd(
    cmds: &mut Vec<Cmd>,
    kind: Kind,
    first: usize,
    end: usize,
    clip: &ClipPx,
    size: [u32; 2],
) {
    if end <= first {
        return;
    }
    let Some(scissor) = scissor_of(clip, size) else {
        return;
    };
    let (first, end) = (first as u32, end as u32);
    if let Some(last) = cmds.last_mut()
        && last.scissor == scissor
    {
        match (&mut last.draw, kind) {
            (Draw::Fill { end: e, .. }, Kind::Fill) | (Draw::Lines { end: e, .. }, Kind::Lines)
                if *e == first =>
            {
                *e = end;
                return;
            }
            _ => {}
        }
    }
    let draw = match kind {
        Kind::Fill => Draw::Fill { first, end },
        Kind::Lines => Draw::Lines { first, end },
    };
    cmds.push(Cmd { draw, scissor });
}
