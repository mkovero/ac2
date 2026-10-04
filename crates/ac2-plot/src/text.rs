//! Text through glyphon with one bundled font, so layout and glyph rasterization do not
//! depend on the fonts installed on the machine and labels can be in golden images.

use std::collections::HashMap;

use glyphon::{
    Attrs, Buffer, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache, TextArea,
    TextAtlas, TextBounds, TextRenderer, fontdb,
};

use crate::geometry::{ClipPx, Xform};
use crate::renderer::RenderShared;
use crate::scene::{HAlign, Label, VAlign};

/// Inter Regular 3.19 (SIL Open Font License 1.1, see `fonts/Inter-OFL.txt`).
pub const FONT_DATA: &[u8] = include_bytes!("../fonts/Inter-Regular.ttf");
pub const FONT_FAMILY: &str = "Inter";

/// Line box height as a multiple of the font size.
const LINE_HEIGHT: f32 = 1.25;

/// A label's shaped text, kept while some frame uses it. Keyed by text and size, so a
/// label added or removed early in a layer does not reshape every label after it.
struct Shaped {
    buffer: Buffer,
    /// Widest line and total height of the laid-out text, physical pixels.
    width: f32,
    height: f32,
    /// Baseline of the first line below the top of the box.
    baseline: f32,
    /// The frame that last used it.
    frame: u64,
}

/// One label placed this frame.
struct Placed {
    shaped: usize,
    /// Top-left of the text box, physical pixels.
    left: f32,
    top: f32,
    bounds: TextBounds,
    color: glyphon::Color,
}

/// The bundled font and the glyph rasterization cache, shared by every renderer.
pub(crate) struct Fonts {
    system: FontSystem,
    swash: SwashCache,
}

impl Fonts {
    pub fn new() -> Self {
        let mut db = fontdb::Database::new();
        db.load_font_data(FONT_DATA.to_vec());
        db.set_sans_serif_family(FONT_FAMILY);
        // A fixed locale keeps shaping independent of the machine's environment.
        Self {
            system: FontSystem::new_with_locale_and_db("en-US".into(), db),
            swash: SwashCache::new(),
        }
    }
}

pub(crate) struct TextLayer {
    viewport: glyphon::Viewport,
    atlas: TextAtlas,
    /// One per scene layer that has labels, so text can sit between layers.
    renderers: Vec<TextRenderer>,
    /// Shaped texts; `None` marks a free entry.
    shaped: Vec<Option<Shaped>>,
    free: Vec<usize>,
    /// Index into `shaped` by size (f32 bits) and text.
    index: HashMap<u32, HashMap<String, usize>>,
    placed: Vec<Placed>,
    frame: u64,
}

impl std::fmt::Debug for TextLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextLayer")
            .field("renderers", &self.renderers.len())
            .field("shaped", &self.shaped_len())
            .finish_non_exhaustive()
    }
}

impl TextLayer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, shared: &RenderShared) -> Self {
        let format = shared.format;
        let viewport = glyphon::Viewport::new(device, &shared.glyphs);
        // glyphon's "accurate" mode decodes colours to linear for sRGB targets; on a non-sRGB
        // target (egui's, and ours offscreen) colours are written as encoded, like the other
        // pipelines.
        let mode = if format.is_srgb() {
            glyphon::ColorMode::Accurate
        } else {
            glyphon::ColorMode::Web
        };
        let atlas = TextAtlas::with_color_mode(device, queue, &shared.glyphs, format, mode);
        Self {
            viewport,
            atlas,
            renderers: Vec::new(),
            shaped: Vec::new(),
            free: Vec::new(),
            index: HashMap::new(),
            placed: Vec::new(),
            frame: 0,
        }
    }

    /// Shaped texts kept.
    pub fn shaped_len(&self) -> usize {
        self.shaped.len() - self.free.len()
    }

    /// Starts a frame: forgets texts the previous frame did not use and releases glyphs
    /// unused by it.
    pub fn begin(&mut self, queue: &wgpu::Queue, target: [u32; 2]) {
        let last = self.frame;
        for by_text in self.index.values_mut() {
            by_text.retain(|_, i| {
                let keep = self.shaped[*i].as_ref().is_some_and(|s| s.frame == last);
                if !keep {
                    self.shaped[*i] = None;
                    self.free.push(*i);
                }
                keep
            });
        }
        self.index.retain(|_, by_text| !by_text.is_empty());
        self.frame += 1;
        self.placed.clear();
        self.atlas.trim();
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
        shared: &RenderShared,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        index: usize,
        labels: impl Iterator<Item = (&'s Label, ClipPx)>,
        xf: &Xform,
    ) -> Result<(), glyphon::PrepareError> {
        let mut fonts = shared.fonts.lock().unwrap_or_else(|e| e.into_inner());
        let fonts = &mut *fonts;
        let first = self.placed.len();
        for (label, clip) in labels {
            let p = self.place(&mut fonts.system, label, clip, xf);
            self.placed.push(p);
        }
        while self.renderers.len() <= index {
            self.renderers.push(TextRenderer::new(
                &mut self.atlas,
                device,
                shared.multisample,
                None,
            ));
        }
        let shaped = &self.shaped;
        let areas = self.placed[first..].iter().filter_map(|p| {
            let s = shaped[p.shaped].as_ref()?;
            Some(TextArea {
                buffer: &s.buffer,
                left: p.left,
                top: p.top,
                scale: 1.0,
                bounds: p.bounds,
                default_color: p.color,
                custom_glyphs: &[],
            })
        });
        self.renderers[index].prepare(
            device,
            queue,
            &mut fonts.system,
            &mut self.atlas,
            &self.viewport,
            areas,
            &mut fonts.swash,
        )
    }

    /// The entry of `text` at `size_px`, shaped now if no recent frame used it.
    fn shaped(&mut self, fs: &mut FontSystem, text: &str, size_px: f32) -> usize {
        let frame = self.frame;
        if let Some(&i) = self.index.get(&size_px.to_bits()).and_then(|m| m.get(text))
            && let Some(s) = self.shaped[i].as_mut()
        {
            s.frame = frame;
            return i;
        }
        let mut buffer = Buffer::new(fs, Metrics::new(size_px, size_px * LINE_HEIGHT));
        buffer.set_text(
            text,
            &Attrs::new().family(Family::Name(FONT_FAMILY)),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(fs, false);
        let mut width: f32 = 0.0;
        let mut height: f32 = 0.0;
        let mut baseline = None;
        for run in buffer.layout_runs() {
            width = width.max(run.line_w);
            height = height.max(run.line_top + run.line_height);
            baseline.get_or_insert(run.line_y);
        }
        let s = Shaped {
            buffer,
            width,
            height,
            baseline: baseline.unwrap_or(0.0),
            frame,
        };
        let i = match self.free.pop() {
            Some(i) => {
                self.shaped[i] = Some(s);
                i
            }
            None => {
                self.shaped.push(Some(s));
                self.shaped.len() - 1
            }
        };
        self.index
            .entry(size_px.to_bits())
            .or_default()
            .insert(text.to_owned(), i);
        i
    }

    fn place(&mut self, fs: &mut FontSystem, label: &Label, clip: ClipPx, xf: &Xform) -> Placed {
        let i = self.shaped(fs, &label.text, label.size * xf.scale);
        let (width, height, baseline) = self.shaped[i]
            .as_ref()
            .map_or((0.0, 0.0, 0.0), |s| (s.width, s.height, s.baseline));
        let [x, y] = xf.point(label.pos);
        let left = match label.anchor.h {
            HAlign::Left => x,
            HAlign::Center => x - width * 0.5,
            HAlign::Right => x - width,
        };
        let top = match label.anchor.v {
            VAlign::Top => y,
            VAlign::Center => y - height * 0.5,
            VAlign::Baseline => y - baseline,
            VAlign::Bottom => y - height,
        };
        let [r, g, b, a] = label.color.to_rgba8();
        Placed {
            shaped: i,
            // Whole-pixel origin: glyphs are rasterized once per subpixel bin, and an
            // integer origin keeps a label's look from shimmering as its anchor moves by
            // fractions.
            left: left.round(),
            top: top.round(),
            bounds: TextBounds {
                left: clip[0].floor() as i32,
                top: clip[1].floor() as i32,
                right: clip[2].ceil() as i32,
                bottom: clip[3].ceil() as i32,
            },
            color: glyphon::Color::rgba(r, g, b, a),
        }
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
