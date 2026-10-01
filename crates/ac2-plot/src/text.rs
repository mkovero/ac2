//! Text through glyphon with one bundled font, so layout and glyph rasterization do not
//! depend on the fonts installed on the machine and labels can be in golden images.

use glyphon::{
    Attrs, Buffer, Cache, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache, TextArea,
    TextAtlas, TextBounds, TextRenderer, fontdb,
};

use crate::geometry::{ClipPx, Xform};
use crate::scene::{HAlign, Label, VAlign};

/// Inter Regular 3.19 (SIL Open Font License 1.1, see `fonts/Inter-OFL.txt`).
pub const FONT_DATA: &[u8] = include_bytes!("../fonts/Inter-Regular.ttf");
pub const FONT_FAMILY: &str = "Inter";

/// Line box height as a multiple of the font size.
const LINE_HEIGHT: f32 = 1.25;

/// A shaped label kept across frames; reshaped only when its text or size changes.
struct Slot {
    text: String,
    size_px: f32,
    buffer: Buffer,
    /// Widest line and total height of the laid-out text, physical pixels.
    width: f32,
    height: f32,
    /// Baseline of the first line below the top of the box.
    baseline: f32,
    /// Top-left of the text box this frame, physical pixels.
    left: f32,
    top: f32,
    bounds: TextBounds,
    color: glyphon::Color,
}

pub(crate) struct TextLayer {
    font_system: FontSystem,
    swash: SwashCache,
    viewport: glyphon::Viewport,
    atlas: TextAtlas,
    /// One per scene layer that has labels, so text can sit between layers.
    renderers: Vec<TextRenderer>,
    slots: Vec<Slot>,
    used_slots: usize,
    multisample: wgpu::MultisampleState,
}

impl std::fmt::Debug for TextLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextLayer")
            .field("renderers", &self.renderers.len())
            .field("slots", &self.slots.len())
            .finish_non_exhaustive()
    }
}

impl TextLayer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        multisample: wgpu::MultisampleState,
    ) -> Self {
        let mut db = fontdb::Database::new();
        db.load_font_data(FONT_DATA.to_vec());
        db.set_sans_serif_family(FONT_FAMILY);
        // A fixed locale keeps shaping independent of the machine's environment.
        let font_system = FontSystem::new_with_locale_and_db("en-US".into(), db);
        let cache = Cache::new(device);
        let viewport = glyphon::Viewport::new(device, &cache);
        // glyphon's "accurate" mode decodes colours to linear for sRGB targets; on a non-sRGB
        // target (egui's, and ours offscreen) colours are written as encoded, like the other
        // pipelines.
        let mode = if format.is_srgb() {
            glyphon::ColorMode::Accurate
        } else {
            glyphon::ColorMode::Web
        };
        let atlas = TextAtlas::with_color_mode(device, queue, &cache, format, mode);
        Self {
            font_system,
            swash: SwashCache::new(),
            viewport,
            atlas,
            renderers: Vec::new(),
            slots: Vec::new(),
            used_slots: 0,
            multisample,
        }
    }

    /// Starts a frame: releases glyphs unused by the previous frame.
    pub fn begin(&mut self, queue: &wgpu::Queue, target: [u32; 2]) {
        self.atlas.trim();
        self.used_slots = 0;
        self.viewport.update(
            queue,
            Resolution {
                width: target[0],
                height: target[1],
            },
        );
    }

    /// Lays out `labels` and uploads them as text renderer `index`.
    pub fn prepare_layer<'s>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        index: usize,
        labels: impl Iterator<Item = (&'s Label, ClipPx)>,
        xf: &Xform,
    ) -> Result<(), glyphon::PrepareError> {
        let first = self.used_slots;
        for (label, clip) in labels {
            let i = self.used_slots;
            self.used_slots += 1;
            self.fill_slot(i, label, clip, xf);
        }
        while self.renderers.len() <= index {
            self.renderers.push(TextRenderer::new(
                &mut self.atlas,
                device,
                self.multisample,
                None,
            ));
        }
        let areas = self.slots[first..self.used_slots].iter().map(|s| TextArea {
            buffer: &s.buffer,
            left: s.left,
            top: s.top,
            scale: 1.0,
            bounds: s.bounds,
            default_color: s.color,
            custom_glyphs: &[],
        });
        self.renderers[index].prepare(
            device,
            queue,
            &mut self.font_system,
            &mut self.atlas,
            &self.viewport,
            areas,
            &mut self.swash,
        )
    }

    fn fill_slot(&mut self, i: usize, label: &Label, clip: ClipPx, xf: &Xform) {
        let size_px = label.size * xf.scale;
        if i == self.slots.len() {
            let buffer = Buffer::new(&mut self.font_system, Metrics::new(size_px, size_px));
            self.slots.push(Slot {
                text: String::new(),
                size_px: f32::NAN,
                buffer,
                width: 0.0,
                height: 0.0,
                baseline: 0.0,
                left: 0.0,
                top: 0.0,
                bounds: TextBounds::default(),
                color: glyphon::Color::rgba(0, 0, 0, 0),
            });
        }
        let slot = &mut self.slots[i];
        if slot.text != label.text || slot.size_px != size_px {
            slot.text.clear();
            slot.text.push_str(&label.text);
            slot.size_px = size_px;
            let fs = &mut self.font_system;
            slot.buffer.set_metrics_and_size(
                Metrics::new(size_px, size_px * LINE_HEIGHT),
                None,
                None,
            );
            slot.buffer.set_text(
                &label.text,
                &Attrs::new().family(Family::Name(FONT_FAMILY)),
                Shaping::Advanced,
                None,
            );
            slot.buffer.shape_until_scroll(fs, false);
            let mut width: f32 = 0.0;
            let mut height: f32 = 0.0;
            let mut baseline = None;
            for run in slot.buffer.layout_runs() {
                width = width.max(run.line_w);
                height = height.max(run.line_top + run.line_height);
                baseline.get_or_insert(run.line_y);
            }
            slot.width = width;
            slot.height = height;
            slot.baseline = baseline.unwrap_or(0.0);
        }
        let [x, y] = xf.point(label.pos);
        let left = match label.anchor.h {
            HAlign::Left => x,
            HAlign::Center => x - slot.width * 0.5,
            HAlign::Right => x - slot.width,
        };
        let top = match label.anchor.v {
            VAlign::Top => y,
            VAlign::Center => y - slot.height * 0.5,
            VAlign::Baseline => y - slot.baseline,
            VAlign::Bottom => y - slot.height,
        };
        // Whole-pixel origin: glyphs are rasterized once per subpixel bin, and an integer
        // origin keeps a label's look from shimmering as its anchor moves by fractions.
        slot.left = left.round();
        slot.top = top.round();
        slot.bounds = TextBounds {
            left: clip[0].floor() as i32,
            top: clip[1].floor() as i32,
            right: clip[2].ceil() as i32,
            bottom: clip[3].ceil() as i32,
        };
        let [r, g, b, a] = label.color.to_rgba8();
        slot.color = glyphon::Color::rgba(r, g, b, a);
    }

    pub fn paint(
        &self,
        index: usize,
        pass: &mut wgpu::RenderPass<'_>,
    ) -> Result<(), glyphon::RenderError> {
        match self.renderers.get(index) {
            Some(r) => r.render(&self.atlas, &self.viewport, pass),
            None => Ok(()),
        }
    }
}
