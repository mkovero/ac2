//! A pane written as PNG: the scene the pane last drew, painted by the same `ac2-plot`
//! renderer as on screen into an offscreen target of the window's GPU, read back and
//! encoded. Only the plot is in the picture; the pane's title strip and key-hint line are
//! window chrome drawn by egui around it.

use std::path::Path;

use ac2_plot::offscreen::OFFSCREEN_FORMAT;
use ac2_plot::{Color, Gpu, RenderShared, Renderer, Scene, render_to_image, wgpu};

/// Physical pixels in the PNG per pixel on screen. Print and slides are viewed closer and
/// larger than a monitor at its native density: twice the screen's pixels keeps 1 px grid
/// lines and small axis labels sharp there, at four times the file's pixel count.
pub const EXPORT_SCALE: f32 = 2.0;

/// The PNG's size for a plot of `size` logical points shown at `ppp` pixels per point,
/// and the pixels per point it is painted at: [`EXPORT_SCALE`] times the screen's, less
/// when the larger side would pass `max_px` (the device's texture limit).
pub fn export_size(size: [f32; 2], ppp: f32, max_px: u32) -> ([u32; 2], f32) {
    let longest = size[0].max(size[1]).max(1.0);
    let scale = (ppp * EXPORT_SCALE).min(max_px as f32 / longest);
    let px = |v: f32| ((v * scale).round() as u32).clamp(1, max_px);
    ([px(size[0]), px(size[1])], scale)
}

/// Paints `scene` (laid out for `size` points, on `background`) at [`export_size`] and
/// writes it to `path` as RGBA PNG, its folder made first; the pixel size written.
pub fn write_png(
    rs: &egui_wgpu::RenderState,
    scene: &Scene,
    size: [f32; 2],
    ppp: f32,
    background: Color,
    path: &Path,
) -> Result<[u32; 2], String> {
    let gpu = Gpu {
        device: rs.device.clone(),
        queue: rs.queue.clone(),
        info: rs.adapter.get_info(),
    };
    let max_px = gpu.device.limits().max_texture_dimension_2d;
    let (px, scale) = export_size(size, ppp, max_px);
    let shared = RenderShared::new(
        &gpu.device,
        OFFSCREEN_FORMAT,
        wgpu::MultisampleState::default(),
    );
    let mut renderer = Renderer::new(&gpu.device, &gpu.queue, &shared);
    let mut pixels = render_to_image(&gpu, &mut renderer, scene, px, scale, background)
        .map_err(|e| e.to_string())?;
    // The pixels come back premultiplied; the themes' plot backgrounds are opaque, so every
    // pixel already is and forcing alpha only keeps the PNG's straight alpha honest.
    for a in pixels.rgba.iter_mut().skip(3).step_by(4) {
        *a = 255;
    }
    let png = encode(&pixels.rgba, px).map_err(|e| e.to_string())?;
    ac2_paths::write_atomic(path, &png).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(px)
}

impl crate::App {
    /// The logical size pane `id`'s plot was last drawn at: what an export of it scales.
    pub fn pane_plot_size(&self, id: crate::state::PaneId) -> Option<[f32; 2]> {
        self.scenes.get(&id).map(|c| [c.size.x, c.size.y])
    }

    /// Writes pane `req.pane` as last drawn in this pass; the toast's text, or why not.
    pub(crate) fn export_pane(
        &self,
        rs: Option<&egui_wgpu::RenderState>,
        ppp: f32,
        req: &crate::state::PaneExport,
    ) -> Result<String, String> {
        let rs = rs.ok_or("plots need the wgpu renderer")?;
        if !self.pane_rects.iter().any(|(id, _)| *id == req.pane) {
            return Err("that pane is not on screen".into());
        }
        let cached = self
            .scenes
            .get(&req.pane)
            .ok_or("that pane has drawn no plot yet")?;
        let background = self.theme().plot_background;
        let size = [cached.size.x, cached.size.y];
        let [w, h] = write_png(rs, &cached.scene, size, ppp, background, &req.path)?;
        Ok(format!(
            "{} pane exported to {} ({w}×{h} px)",
            cached.view.title(),
            req.path.display()
        ))
    }
}

fn encode(rgba: &[u8], [w, h]: [u32; 2]) -> Result<Vec<u8>, png::EncodingError> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header()?;
        writer.write_image_data(rgba)?;
        writer.finish()?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_export_is_twice_the_screen_pixels() {
        assert_eq!(export_size([600.0, 400.0], 1.0, 8192), ([1200, 800], 2.0));
        assert_eq!(export_size([600.0, 400.0], 1.5, 8192), ([1800, 1200], 3.0));
    }

    #[test]
    fn a_large_pane_stays_within_the_texture_limit() {
        let (px, scale) = export_size([1900.0, 1000.0], 2.0, 4096);
        assert_eq!(px[0], 4096);
        assert!(px[1] <= 4096 && px[1] > 2000, "{px:?}");
        assert!(scale < 4.0);
    }
}
