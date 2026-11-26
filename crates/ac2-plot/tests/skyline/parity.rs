//! Look work: renders of the look emulator's histories beside its pictures.

use super::*;

/// Renders histories exported from the look emulator (`AC2_SKYLINE_PARITY=<dir>` holding
/// `<case>.f32`: rows × 192 levels, newest first) at 960×540 from the emulator's cameras
/// (default and tilted, eye 2.6 from the target), Full tier with its bloom (without it when
/// `AC2_SKYLINE_PARITY_NO_GLOW` is set), to set beside its pictures. Skipped
/// without the variable: a tool for look work, not a check.
#[test]
fn parity_renders() {
    let Some(dir) = std::env::var_os("AC2_SKYLINE_PARITY") else {
        return;
    };
    let Some(gpu) = gpu("parity_renders") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let glow = std::env::var_os("AC2_SKYLINE_PARITY_NO_GLOW").is_none();
    let (w, h) = (960.0f32, 540.0f32);
    let aspect = f64::from(w / h);
    let cols = 192u32;
    for (case, full, narrow) in [
        ("pink-room", [-100.0, 0.0], [-60.0, 0.0]),
        ("rta-pink", [-100.0, 0.0], [-45.0, -5.0]),
        ("tonal", [-100.0, 0.0], [-100.0, 0.0]),
    ] {
        let bytes = std::fs::read(dir.join(format!("{case}.f32"))).expect("exported history");
        let levels: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        let rows = (levels.len() / cols as usize) as u32 + 1;
        let mut data: Vec<Option<HeatmapColumn>> = levels
            .chunks_exact(cols as usize)
            .map(|r| Some(Arc::from(r)))
            .collect();
        data.push(None);
        for (cname, yaw, pitch) in [("default", 0.0, 8.0), ("tilted", 25.0, 30.0)] {
            let at = Pose {
                yaw_deg: yaw,
                pitch_deg: pitch,
                zoom_step: 0.0,
            };
            let d0 = spectrograph_3d::fitted_distance(at, aspect, Stage::Floor);
            let pose = Pose {
                zoom_step: -spectrograph_3d::ZOOM_STEPS_PER_OCTAVE * (2.6 / d0).log2(),
                ..at
            };
            let vp = spectrograph_3d::view_proj(pose, aspect, Stage::Floor);
            for (vname, colormap, range) in [
                ("ice", Colormap::Ice, full),
                ("viridis", Colormap::Viridis, full),
                ("ice_narrow", Colormap::Ice, narrow),
            ] {
                let rect = Rect::new(0.0, 0.0, w, h);
                let mut l = Layer::default();
                l.rects.push(FillRect::new(rect, FIELD));
                l.skylines.push(Skyline3d {
                    id: SkylineId(1),
                    rect,
                    clip: Some(rect),
                    cols,
                    rows,
                    newest: 0,
                    newest_id: 0,
                    phase: 1e-3,
                    view_proj: vp.gpu(),
                    eye: vp.eye.map(|v| v as f32),
                    world: [FLOOR_HALF_W as f32, FLOOR_DEPTH as f32, MAX_HEIGHT as f32],
                    range,
                    colormap,
                    tier: SkylineTier::Full,
                    look: LOOK,
                    data: data.clone(),
                    mirror: None,
                });
                if glow {
                    l.glows.push(SkylineGlow {
                        id: SkylineId(1),
                        clip: Some(rect),
                        gain: glow_gain(colormap),
                    });
                }
                let img = render_on(
                    gpu,
                    &mut renderer(gpu),
                    &scene(w, h, vec![l]),
                    1.0,
                    Color::BLACK,
                );
                img.save_png(&dir.join(format!("real_{case}_{cname}_{vname}.png")))
                    .expect("written");
            }
        }
    }
}
