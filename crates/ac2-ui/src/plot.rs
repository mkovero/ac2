//! Plot widgets: an `ac2-scene` [`Scene`] painted by `ac2-plot` inside egui's own render
//! pass through an egui-wgpu paint callback.
//!
//! One [`Renderer`] per plot slot (one per pane), kept in egui-wgpu's `CallbackResources`.
//! A renderer owns its instance buffers and text atlas and prepares one scene per frame, so
//! per-slot renderers keep panes independent (a pane can skip `prepare` when its scene did
//! not change) at the cost of one pipeline set and glyph atlas each — four panes, a few MB.
//! A multi-slot renderer would share pipelines but re-upload every pane whenever one
//! changes.

use std::collections::HashMap;
use std::sync::Arc;

use ac2_plot::{FrameTarget, Renderer, Scene, wgpu};
use eframe::egui;
use egui_wgpu::{CallbackResources, CallbackTrait, ScreenDescriptor};

/// Which renderer a widget uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PlotSlot(pub u32);

/// Renderers by slot, created on first use for egui's target format.
pub struct PlotRenderers {
    format: wgpu::TextureFormat,
    slots: HashMap<PlotSlot, SlotState>,
}

struct SlotState {
    renderer: Renderer,
    /// Scene and target last prepared, to skip identical re-uploads.
    last: Option<(Arc<Scene>, FrameTarget)>,
    error: Option<String>,
}

impl std::fmt::Debug for PlotRenderers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlotRenderers")
            .field("format", &self.format)
            .field("slots", &self.slots.len())
            .finish()
    }
}

/// Registers the renderer map with egui-wgpu. Call once with the app's render state; plots
/// are skipped (not drawn) without it, e.g. on a non-wgpu backend.
pub fn install(rs: &egui_wgpu::RenderState) {
    rs.renderer
        .write()
        .callback_resources
        .insert(PlotRenderers {
            format: rs.target_format,
            slots: HashMap::new(),
        });
}

struct PlotCallback {
    slot: PlotSlot,
    scene: Arc<Scene>,
    /// Widget top-left and clip in logical points.
    origin: egui::Pos2,
    clip: egui::Rect,
}

impl CallbackTrait for PlotCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(map) = resources.get_mut::<PlotRenderers>() else {
            return Vec::new();
        };
        let format = map.format;
        let s = map.slots.entry(self.slot).or_insert_with(|| SlotState {
            renderer: Renderer::new(device, queue, format, wgpu::MultisampleState::default()),
            last: None,
            error: None,
        });
        let ppp = screen.pixels_per_point;
        let target = FrameTarget {
            size_px: screen.size_in_pixels,
            origin_px: [self.origin.x * ppp, self.origin.y * ppp],
            scale: ppp,
            clip_px: [
                self.clip.min.x * ppp,
                self.clip.min.y * ppp,
                self.clip.max.x * ppp,
                self.clip.max.y * ppp,
            ],
        };
        let same = s
            .last
            .as_ref()
            .is_some_and(|(sc, t)| Arc::ptr_eq(sc, &self.scene) && *t == target);
        if !same {
            match s.renderer.prepare(device, queue, &self.scene, &target) {
                Ok(()) => {
                    s.last = Some((self.scene.clone(), target));
                    s.error = None;
                }
                Err(e) => {
                    s.last = None;
                    s.error = Some(e.to_string());
                }
            }
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        let Some(map) = resources.get::<PlotRenderers>() else {
            return;
        };
        if let Some(s) = map.slots.get(&self.slot)
            && s.last.is_some()
        {
            // A glyph atlas failure loses this frame's text only; the next prepare retries.
            let _ = s.renderer.paint(pass);
        }
    }
}

/// Paints `scene` (built for `rect`'s size) at `rect`.
pub fn paint(ui: &egui::Ui, slot: PlotSlot, rect: egui::Rect, scene: Arc<Scene>) {
    let clip = ui.clip_rect().intersect(rect);
    ui.painter().add(egui_wgpu::Callback::new_paint_callback(
        rect,
        PlotCallback {
            slot,
            scene,
            origin: rect.min,
            clip,
        },
    ));
}
