//! GPU side of the 3D spectrograph ([`Skyline3d`]): an R32Float ring of level rows per
//! [`SkylineId`], uploaded row by row by `Arc` identity, the Rg8 spike target the spike
//! pass draws into before the composite reads it, and the quarter-resolution bloom targets
//! (source, blur ping-pong) for the tiers that glow.

use std::collections::HashMap;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};

use crate::PrepareError;
use crate::geometry::ClipPx;
use crate::heatmap::Luts;
use crate::scene::{HeatmapColumn, Skyline3d, SkylineId, SkylineTier};

/// The spike target: level in R, the square root of brightness in G. Rg8Unorm is
/// renderable, blendable and filterable on every backend, GL included; 256 level steps
/// match the 256-entry LUT.
pub(crate) const SPIKE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg8Unorm;

/// The bloom targets. Float formats would keep the far, dim rows the 8 bits drop, but GL
/// renders them only with extensions the downlevel limits do not promise; Rgba8Unorm is
/// renderable, blendable (the additive source) and filterable everywhere.
pub(crate) const BLOOM_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Bloom target pixels per host pixel.
const BLOOM_SCALE: f32 = 0.25;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct Uniform {
    view_proj: [[f32; 4]; 4],
    rect: [f32; 4],
    clip: [f32; 4],
    eye: [f32; 4],
    world: [f32; 4],
    range: [f32; 2],
    phase: f32,
    cols: u32,
    rows: u32,
    newest: u32,
    target_size: [f32; 2],
    foot: f32,
    body_pow: f32,
    tip_px: f32,
    tip_gain: f32,
    level_floor: f32,
    age_fade: f32,
    eye_fade: f32,
    width_share: f32,
    width_min: f32,
    width_max: f32,
    down: f32,
    height_cap: f32,
    bloom_size: [f32; 2],
    bloom_threshold: f32,
    bloom_gain: f32,
    glow_gain: f32,
    taps: u32,
    _pad2: [u32; 2],
}

#[derive(Debug)]
struct Gpu {
    cols: u32,
    rows: u32,
    ring: wgpu::Texture,
    uniform: wgpu::Buffer,
    spike_bg: wgpu::BindGroup,
    /// The spike target, the composite's bind group over it and the bloom; remade with the
    /// target.
    target: Option<Target>,
    colormap: Option<crate::scene::Colormap>,
    /// The row last uploaded at each ring position, held so the allocation cannot be
    /// reused for other values while compared by pointer.
    uploaded: Vec<Option<HeatmapColumn>>,
    seen: u64,
    /// Spikes to draw this frame (`cols · rows`), 0 when the target is empty.
    instances: u32,
}

#[derive(Debug)]
struct Target {
    spikes: wgpu::Texture,
    composite_bg: wgpu::BindGroup,
    bloom: Option<Bloom>,
}

/// The bloom source is drawn into `a`, blurred across into `b` and down back into `a`,
/// which the glow composite reads.
#[derive(Debug)]
struct Bloom {
    a: wgpu::Texture,
    b: wgpu::Texture,
    source_bg: wgpu::BindGroup,
    /// Reads `a`: the horizontal blur's input and the glow's.
    a_bg: wgpu::BindGroup,
    b_bg: wgpu::BindGroup,
}

/// What the skyline pipelines bind besides the globals.
#[derive(Debug)]
pub(crate) struct Layouts {
    pub spike: wgpu::BindGroupLayout,
    pub composite: wgpu::BindGroupLayout,
    /// The bloom source: the spikes' ring and the LUT, both read by the vertex stage.
    pub bloom_source: wgpu::BindGroupLayout,
    /// A bloom target read by the blur passes and the glow composite.
    pub bloom: wgpu::BindGroupLayout,
}

/// The skyline's pipelines and the globals bind group, for [`Skylines::prepare`].
#[derive(Clone, Copy)]
pub(crate) struct Passes<'a> {
    pub layouts: &'a Layouts,
    pub spikes: &'a wgpu::RenderPipeline,
    pub bloom_source: &'a wgpu::RenderPipeline,
    pub blur_h: &'a wgpu::RenderPipeline,
    pub blur_v: &'a wgpu::RenderPipeline,
    pub globals: &'a wgpu::BindGroup,
}

pub(crate) fn layouts(device: &wgpu::Device) -> Layouts {
    let uniform = wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let tex = |binding, visibility, filterable| wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    let spike = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ac2-plot skyline spikes"),
        // R32Float is not filterable without an optional feature; the vertex shader only
        // loads it.
        entries: &[uniform, tex(1, wgpu::ShaderStages::VERTEX, false)],
    });
    let composite = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ac2-plot skyline composite"),
        entries: &[
            uniform,
            tex(2, wgpu::ShaderStages::FRAGMENT, true),
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            tex(4, wgpu::ShaderStages::FRAGMENT, false),
        ],
    });
    let bloom_source = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ac2-plot skyline bloom source"),
        entries: &[
            uniform,
            tex(1, wgpu::ShaderStages::VERTEX, false),
            tex(4, wgpu::ShaderStages::VERTEX, false),
        ],
    });
    let bloom = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ac2-plot skyline bloom"),
        entries: &[
            uniform,
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            tex(5, wgpu::ShaderStages::FRAGMENT, true),
        ],
    });
    Layouts {
        spike,
        composite,
        bloom_source,
        bloom,
    }
}

#[derive(Debug)]
pub(crate) struct Skylines {
    sampler: wgpu::Sampler,
    gpus: HashMap<SkylineId, Gpu>,
    frame: u64,
}

impl Skylines {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("ac2-plot skyline"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                ..Default::default()
            }),
            gpus: HashMap::new(),
            frame: 0,
        }
    }

    pub fn begin(&mut self) {
        self.frame += 1;
    }

    /// Releases skylines not referenced since [`Self::begin`].
    pub fn end(&mut self) {
        let frame = self.frame;
        self.gpus.retain(|_, g| g.seen == frame);
    }

    pub fn composite_bind_group(&self, id: SkylineId) -> Option<&wgpu::BindGroup> {
        self.gpus
            .get(&id)
            .filter(|g| g.instances > 0)
            .and_then(|g| g.target.as_ref().map(|t| &t.composite_bg))
    }

    /// The glow composite's bind group, `None` when there is nothing to add.
    pub fn glow_bind_group(&self, id: SkylineId) -> Option<&wgpu::BindGroup> {
        self.gpus
            .get(&id)
            .filter(|g| g.instances > 0)
            .and_then(|g| g.target.as_ref()?.bloom.as_ref().map(|b| &b.a_bg))
    }

    /// Sets the gain the glow of skyline `id`, prepared this frame, is added at. Refused
    /// for a skyline not in this scene or drawn without bloom.
    pub fn set_glow(
        &self,
        queue: &wgpu::Queue,
        id: SkylineId,
        gain: f32,
    ) -> Result<(), PrepareError> {
        let bad = |reason: &str| PrepareError::Skyline {
            id,
            reason: reason.into(),
        };
        let g = self
            .gpus
            .get(&id)
            .filter(|g| g.seen == self.frame)
            .ok_or_else(|| bad("glow without its skyline earlier in the scene"))?;
        if g.target.as_ref().is_none_or(|t| t.bloom.is_none()) {
            return Err(bad("glow of a tier without bloom"));
        }
        if !(gain.is_finite() && gain >= 0.0) {
            return Err(bad("glow gain must be finite and not negative"));
        }
        queue.write_buffer(
            &g.uniform,
            std::mem::offset_of!(Uniform, glow_gain) as u64,
            bytemuck::bytes_of(&gain),
        );
        Ok(())
    }

    /// Validates `s`, uploads the ring rows that changed, writes the uniform and records
    /// the spike pass into `encoder`. `rect` and `clip` are in the host target's physical
    /// pixels.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        luts: &mut Luts,
        passes: Passes<'_>,
        s: &Skyline3d,
        rect: [f32; 4],
        clip: ClipPx,
    ) -> Result<(), PrepareError> {
        let layouts = passes.layouts;
        let max = device.limits().max_texture_dimension_2d;
        let bad = |reason: String| PrepareError::Skyline { id: s.id, reason };
        if s.cols == 0 || s.rows == 0 || s.cols > max || s.rows > max {
            return Err(bad(format!(
                "{} columns x {} rows; each must be 1..={max}",
                s.cols, s.rows
            )));
        }
        if !(s.range[0].is_finite() && s.range[1].is_finite() && s.range[0] != s.range[1]) {
            return Err(bad(format!(
                "range {:?} must be finite and non-empty",
                s.range
            )));
        }
        if s.data.len() != s.rows as usize {
            return Err(bad(format!(
                "{} data rows for a ring of {}",
                s.data.len(),
                s.rows
            )));
        }
        let cols = s.cols as usize;
        if let Some((i, r)) = s
            .data
            .iter()
            .enumerate()
            .find_map(|(i, r)| r.as_ref().filter(|r| r.len() != cols).map(|r| (i, r)))
        {
            return Err(bad(format!("row {i} has {} values, not {cols}", r.len())));
        }
        if s.newest >= s.rows {
            return Err(bad(format!("newest row {} of {}", s.newest, s.rows)));
        }
        if self.gpus.get(&s.id).is_some_and(|g| g.seen == self.frame) {
            return Err(bad("id used twice in one scene".into()));
        }

        if !self
            .gpus
            .get(&s.id)
            .is_some_and(|g| g.cols == s.cols && g.rows == s.rows)
        {
            let ring = new_ring(device, queue, s.cols, s.rows);
            let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ac2-plot skyline uniform"),
                size: std::mem::size_of::<Uniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let view = ring.create_view(&wgpu::TextureViewDescriptor::default());
            let spike_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ac2-plot skyline spikes"),
                layout: &layouts.spike,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                ],
            });
            self.gpus.insert(
                s.id,
                Gpu {
                    cols: s.cols,
                    rows: s.rows,
                    ring,
                    uniform,
                    spike_bg,
                    target: None,
                    colormap: None,
                    uploaded: vec![None; s.rows as usize],
                    seen: 0,
                    instances: 0,
                },
            );
        }
        let Some(g) = self.gpus.get_mut(&s.id) else {
            unreachable!("inserted above")
        };
        g.seen = self.frame;
        upload_changed(queue, g, &s.data);

        let down = match s.tier {
            SkylineTier::Full => 1.0,
            SkylineTier::Lite | SkylineTier::Bare => 0.5,
        };
        let size = [(rect[2] - rect[0]) * down, (rect[3] - rect[1]) * down];
        let px = [
            (size[0].ceil() as u32).clamp(1, max),
            (size[1].ceil() as u32).clamp(1, max),
        ];
        let taps = s.tier.bloom_taps();
        let bloom_size = [
            (rect[2] - rect[0]) * BLOOM_SCALE,
            (rect[3] - rect[1]) * BLOOM_SCALE,
        ];
        let bloom_px = [
            (bloom_size[0].ceil() as u32).clamp(1, max),
            (bloom_size[1].ceil() as u32).clamp(1, max),
        ];
        let target_fits = g.target.as_ref().is_some_and(|t| {
            t.spikes.width() == px[0]
                && t.spikes.height() == px[1]
                && g.colormap == Some(s.colormap)
                && match &t.bloom {
                    Some(b) => {
                        taps.is_some() && b.a.width() == bloom_px[0] && b.a.height() == bloom_px[1]
                    }
                    None => taps.is_none(),
                }
        });
        if !target_fits {
            let spikes = texture(device, "ac2-plot skyline target", px, SPIKE_FORMAT);
            let view = spikes.create_view(&wgpu::TextureViewDescriptor::default());
            let lut = luts.view(device, queue, s.colormap);
            let composite_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ac2-plot skyline composite"),
                layout: &layouts.composite,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: g.uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&lut),
                    },
                ],
            });
            let bloom = taps.map(|_| {
                let a = texture(device, "ac2-plot skyline bloom a", bloom_px, BLOOM_FORMAT);
                let b = texture(device, "ac2-plot skyline bloom b", bloom_px, BLOOM_FORMAT);
                let ring = g.ring.create_view(&wgpu::TextureViewDescriptor::default());
                let source_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("ac2-plot skyline bloom source"),
                    layout: &layouts.bloom_source,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: g.uniform.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&ring),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: wgpu::BindingResource::TextureView(&lut),
                        },
                    ],
                });
                let reads = |t: &wgpu::Texture| {
                    let view = t.create_view(&wgpu::TextureViewDescriptor::default());
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("ac2-plot skyline bloom"),
                        layout: &layouts.bloom,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: g.uniform.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 3,
                                resource: wgpu::BindingResource::Sampler(&self.sampler),
                            },
                            wgpu::BindGroupEntry {
                                binding: 5,
                                resource: wgpu::BindingResource::TextureView(&view),
                            },
                        ],
                    })
                };
                Bloom {
                    a_bg: reads(&a),
                    b_bg: reads(&b),
                    a,
                    b,
                    source_bg,
                }
            });
            g.target = Some(Target {
                spikes,
                composite_bg,
                bloom,
            });
            g.colormap = Some(s.colormap);
        }

        let look = &s.look;
        let uniform = Uniform {
            view_proj: s.view_proj,
            rect,
            clip,
            eye: [
                s.eye[0],
                s.eye[1],
                s.eye[2],
                nearest_depth(&s.view_proj, s.world),
            ],
            world: [s.world[0], s.world[1], s.world[2], 0.0],
            range: s.range,
            phase: s
                .phase
                .clamp(0.0, ac2_scene::spectrograph_3d::MAX_PHASE as f32),
            cols: s.cols,
            rows: s.rows,
            newest: s.newest,
            target_size: size,
            foot: look.foot,
            body_pow: look.body_pow,
            tip_px: look.tip_px,
            tip_gain: look.tip_gain,
            level_floor: look.level_floor,
            age_fade: look.age_fade,
            eye_fade: look.eye_fade,
            width_share: look.width_share,
            width_min: look.width_px[0],
            width_max: look.width_px[1],
            down,
            height_cap: ac2_scene::spectrograph_3d::HEIGHT_CAP as f32,
            bloom_size,
            bloom_threshold: look.bloom_threshold,
            bloom_gain: look.bloom_gain,
            // Set by the glow primitive, later in the scene; nothing glows without one.
            glow_gain: 0.0,
            taps: taps.unwrap_or(0),
            _pad2: [0; 2],
        };
        queue.write_buffer(&g.uniform, 0, bytemuck::bytes_of(&uniform));
        g.instances = if s.data.iter().any(Option::is_some) {
            s.cols * s.rows
        } else {
            0
        };

        let Some(t) = &g.target else {
            unreachable!("made above")
        };
        let draw = g.instances;
        let instanced = |pass: &mut wgpu::RenderPass<'_>,
                         pipeline: &wgpu::RenderPipeline,
                         bg: &wgpu::BindGroup| {
            if draw > 0 {
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, passes.globals, &[]);
                pass.set_bind_group(1, bg, &[]);
                pass.draw(0..4, 0..draw);
            }
        };
        let mut pass = begin(encoder, "ac2-plot skyline spikes", &t.spikes);
        instanced(&mut pass, passes.spikes, &g.spike_bg);
        drop(pass);
        if let Some(b) = &t.bloom
            && draw > 0
        {
            let mut pass = begin(encoder, "ac2-plot skyline bloom source", &b.a);
            instanced(&mut pass, passes.bloom_source, &b.source_bg);
            drop(pass);
            for (label, pipeline, from, to) in [
                ("ac2-plot skyline blur across", passes.blur_h, &b.a_bg, &b.b),
                ("ac2-plot skyline blur down", passes.blur_v, &b.b_bg, &b.a),
            ] {
                let mut pass = begin(encoder, label, to);
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, passes.globals, &[]);
                pass.set_bind_group(1, from, &[]);
                pass.draw(0..4, 0..1);
            }
        }
        Ok(())
    }
}

fn texture(
    device: &wgpu::Device,
    label: &str,
    px: [u32; 2],
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: px[0],
            height: px[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

/// A pass over all of `target`, cleared to 0.
fn begin<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    label: &str,
    target: &wgpu::Texture,
) -> wgpu::RenderPass<'e> {
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

/// Uploads the ring positions whose row differs from the one uploaded there, one write per
/// run of adjacent changed positions.
fn upload_changed(queue: &wgpu::Queue, g: &mut Gpu, data: &[Option<HeatmapColumn>]) {
    let same = |a: &Option<HeatmapColumn>, b: &Option<HeatmapColumn>| match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        _ => false,
    };
    let cols = g.cols as usize;
    let mut run: Vec<f32> = Vec::new();
    let mut i = 0;
    while i < data.len() {
        if same(&data[i], &g.uploaded[i]) {
            i += 1;
            continue;
        }
        let first = i;
        run.clear();
        while i < data.len() && !same(&data[i], &g.uploaded[i]) {
            match &data[i] {
                Some(r) => run.extend_from_slice(r),
                None => run.extend(std::iter::repeat_n(f32::NAN, cols)),
            }
            g.uploaded[i] = data[i].clone();
            i += 1;
        }
        write_rows(queue, &g.ring, g.cols, first as u32, &run);
    }
}

/// One ring row is one texture row, so a row upload is one contiguous write. Starts all
/// NaN: no data.
fn new_ring(device: &wgpu::Device, queue: &wgpu::Queue, cols: u32, rows: u32) -> wgpu::Texture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ac2-plot skyline ring"),
        size: wgpu::Extent3d {
            width: cols,
            height: rows,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    write_rows(
        queue,
        &texture,
        cols,
        0,
        &vec![f32::NAN; cols as usize * rows as usize],
    );
    texture
}

fn write_rows(queue: &wgpu::Queue, texture: &wgpu::Texture, cols: u32, first: u32, v: &[f32]) {
    let n = (v.len() / cols as usize) as u32;
    if n == 0 {
        return;
    }
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: 0,
                y: first,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(v),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(cols * 4),
            rows_per_image: None,
        },
        wgpu::Extent3d {
            width: cols,
            height: n,
            depth_or_array_layers: 1,
        },
    );
}

/// The floor's nearest depth along the view: the least clip `w` of its corners (`w` is linear
/// in the point, so a corner holds the floor's least).
fn nearest_depth(vp: &[[f32; 4]; 4], world: [f32; 3]) -> f32 {
    let w = |x: f32, z: f32| vp[0][3] * x + vp[2][3] * z + vp[3][3];
    let (hw, d) = (world[0], world[1]);
    [w(-hw, 0.0), w(hw, 0.0), w(-hw, d), w(hw, d)]
        .into_iter()
        .fold(f32::INFINITY, f32::min)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_uniform_matches_the_shader_layout() {
        // The WGSL struct: a mat4, four vec4s, then 28 four-byte scalars (vec2s among them),
        // the last two padding to a multiple of 16.
        assert_eq!(std::mem::size_of::<Uniform>(), 64 + 4 * 16 + 28 * 4);
        assert_eq!(std::mem::offset_of!(Uniform, bloom_size), 208);
    }
}
