//! GPU side of heatmaps: an R32Float ring texture per [`HeatmapId`], updated column-wise,
//! plus one colormap LUT texture per [`Colormap`].

use std::collections::HashMap;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};

use crate::PrepareError;
use crate::colormap::{LUT_SIZE, lut};
use crate::geometry::ClipPx;
use crate::scene::{Colormap, Heatmap, HeatmapAxes, HeatmapColumn, HeatmapId};

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct Uniform {
    rect: [f32; 4],
    clip: [f32; 4],
    range: [f32; 2],
    opacity: f32,
    columns: u32,
    rows: u32,
    scroll: u32,
    /// 1 when time runs bottom to top ([`HeatmapAxes::TimeUp`]).
    time_up: u32,
    _pad: u32,
}

#[derive(Debug)]
struct Ring {
    columns: u32,
    rows: u32,
    colormap: Colormap,
    texture: wgpu::Texture,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    /// The column last uploaded at each ring position (`None`: empty). Held, so the
    /// allocation cannot be reused for other values while compared by pointer.
    uploaded: Vec<Option<HeatmapColumn>>,
    /// Frame counter of the last scene that referenced it.
    seen: u64,
}

#[derive(Debug)]
pub(crate) struct Heatmaps {
    layout: wgpu::BindGroupLayout,
    luts: Vec<(Colormap, wgpu::TextureView)>,
    rings: HashMap<HeatmapId, Ring>,
    frame: u64,
}

pub(crate) fn bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let tex = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            // R32Float is not filterable without an optional feature; the shader only uses
            // textureLoad, so it does not need to be.
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ac2-plot heatmap"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            tex(1),
            tex(2),
        ],
    })
}

impl Heatmaps {
    pub fn new(layout: wgpu::BindGroupLayout) -> Self {
        Self {
            layout,
            luts: Vec::new(),
            rings: HashMap::new(),
            frame: 0,
        }
    }

    pub fn begin(&mut self) {
        self.frame += 1;
    }

    /// Releases rings not referenced since [`Self::begin`].
    pub fn end(&mut self) {
        let frame = self.frame;
        self.rings.retain(|_, r| r.seen == frame);
    }

    pub fn bind_group(&self, id: HeatmapId) -> Option<&wgpu::BindGroup> {
        self.rings.get(&id).map(|r| &r.bind_group)
    }

    fn lut_view(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        map: Colormap,
    ) -> wgpu::TextureView {
        if let Some((_, v)) = self.luts.iter().find(|(m, _)| *m == map) {
            return v.clone();
        }
        let size = wgpu::Extent3d {
            width: LUT_SIZE as u32,
            height: 1,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ac2-plot colormap"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // Not sRGB: the LUT holds display-encoded values like every other scene colour.
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let data = lut(map);
        queue.write_texture(
            texture.as_image_copy(),
            bytemuck::cast_slice(&data),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(LUT_SIZE as u32 * 4),
                rows_per_image: None,
            },
            size,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.luts.push((map, view.clone()));
        view
    }

    /// Validates `h`, (re)creates its ring when new or resized, uploads the columns that
    /// changed and writes its uniform.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        h: &Heatmap,
        rect: [f32; 4],
        clip: ClipPx,
    ) -> Result<(), PrepareError> {
        let max = device.limits().max_texture_dimension_2d;
        let bad = |reason: String| PrepareError::Heatmap { id: h.id, reason };
        if h.columns == 0 || h.rows == 0 || h.columns > max || h.rows > max {
            return Err(bad(format!(
                "{} columns x {} rows; each must be 1..={max}",
                h.columns, h.rows
            )));
        }
        if !(h.range[0].is_finite() && h.range[1].is_finite() && h.range[0] != h.range[1]) {
            return Err(bad(format!(
                "range {:?} must be finite and non-empty",
                h.range
            )));
        }
        if self.rings.get(&h.id).is_some_and(|r| r.seen == self.frame) {
            return Err(bad("id used twice in one scene".into()));
        }
        let rows = h.rows as usize;
        if h.data.len() != h.columns as usize {
            return Err(bad(format!(
                "{} data columns for a ring of {}",
                h.data.len(),
                h.columns
            )));
        }
        if let Some((i, c)) = h
            .data
            .iter()
            .enumerate()
            .find_map(|(i, c)| c.as_ref().filter(|c| c.len() != rows).map(|c| (i, c)))
        {
            return Err(bad(format!(
                "column {i} has {} values, not {rows}",
                c.len()
            )));
        }

        let fits = self.rings.get(&h.id).is_some_and(|r| {
            r.columns == h.columns && r.rows == h.rows && r.colormap == h.colormap
        });
        if !fits {
            let lut = self.lut_view(device, queue, h.colormap);
            let reuse = self
                .rings
                .remove(&h.id)
                .filter(|r| r.columns == h.columns && r.rows == h.rows)
                .map(|r| (r.texture, r.uniform, r.uploaded));
            // A reused texture keeps its contents, so it keeps the record of them too.
            let (texture, uniform, uploaded) = match reuse {
                Some(t) => t,
                None => (
                    new_ring_texture(device, queue, h.columns, h.rows),
                    device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("ac2-plot heatmap uniform"),
                        size: std::mem::size_of::<Uniform>() as u64,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    }),
                    vec![None; h.columns as usize],
                ),
            };
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ac2-plot heatmap"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&lut),
                    },
                ],
            });
            self.rings.insert(
                h.id,
                Ring {
                    columns: h.columns,
                    rows: h.rows,
                    colormap: h.colormap,
                    texture,
                    uniform,
                    bind_group,
                    uploaded,
                    seen: 0,
                },
            );
        }
        let Some(ring) = self.rings.get_mut(&h.id) else {
            unreachable!("ring inserted above")
        };
        ring.seen = self.frame;
        upload_changed(queue, ring, &h.data, rows);
        let uniform = Uniform {
            rect,
            clip,
            range: h.range,
            opacity: h.opacity.clamp(0.0, 1.0),
            columns: h.columns,
            rows: h.rows,
            scroll: h.scroll % h.columns,
            time_up: u32::from(h.axes == HeatmapAxes::TimeUp),
            _pad: 0,
        };
        queue.write_buffer(&ring.uniform, 0, bytemuck::bytes_of(&uniform));
        Ok(())
    }
}

/// Uploads the ring positions whose column differs from the one uploaded there, one write
/// per run of adjacent changed positions.
fn upload_changed(
    queue: &wgpu::Queue,
    ring: &mut Ring,
    data: &[Option<HeatmapColumn>],
    rows: usize,
) {
    let same = |a: &Option<HeatmapColumn>, b: &Option<HeatmapColumn>| match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        _ => false,
    };
    let mut run: Vec<f32> = Vec::new();
    let mut i = 0;
    while i < data.len() {
        if same(&data[i], &ring.uploaded[i]) {
            i += 1;
            continue;
        }
        let first = i;
        run.clear();
        while i < data.len() && !same(&data[i], &ring.uploaded[i]) {
            match &data[i] {
                Some(c) => run.extend_from_slice(c),
                None => run.extend(std::iter::repeat_n(f32::NAN, rows)),
            }
            ring.uploaded[i] = data[i].clone();
            i += 1;
        }
        write_columns(queue, &ring.texture, ring.rows, first as u32, &run);
    }
}

/// Ring stored transposed (width = rows, height = columns) so a time column is one
/// contiguous texture row. Starts all NaN: no data.
fn new_ring_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    columns: u32,
    rows: u32,
) -> wgpu::Texture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ac2-plot heatmap ring"),
        size: wgpu::Extent3d {
            width: rows,
            height: columns,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let empty = vec![f32::NAN; columns as usize * rows as usize];
    write_columns(queue, &texture, rows, 0, &empty);
    texture
}

fn write_columns(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    rows: u32,
    first: u32,
    values: &[f32],
) {
    let n = (values.len() / rows as usize) as u32;
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
        bytemuck::cast_slice(values),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(rows * 4),
            rows_per_image: None,
        },
        wgpu::Extent3d {
            width: rows,
            height: n,
            depth_or_array_layers: 1,
        },
    );
}
