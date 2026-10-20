//! The sweep's impulse response view and the harmonic marks on it.

use super::*;

/// The sweep's impulse response as an IR frame for [`crate::ir::ir_scene`]: time zero at
/// the arrival, the harmonics' impulses before it.
pub fn ir_frame(d: &TraceData) -> Option<ac2_proto::frame::IrFrame> {
    let s = d.sweep.as_ref()?;
    Some(ac2_proto::frame::IrFrame {
        meas: ac2_proto::units::MeasId(0),
        meta: ac2_proto::frame::IrMeta {
            sample_rate: s.info.sample_rate,
            t0: s.ir.t0,
            dt: s.ir.dt,
            inserted_delay: s.info.arrival,
        },
        linear: s.ir.linear.clone(),
        etc: Some(s.ir.etc_db.clone()),
    })
}

/// Shortest IR plot left above the room table, logical pixels; a shorter pane shows the
/// IR alone.
const MIN_IR_HEIGHT: f32 = 140.0;

/// The sweep's impulse response in the IR view, with each harmonic's impulse marked at
/// `−L·ln k` and time zero named as the arrival; below it, when the pane is tall enough,
/// the room parameters of its octave bands ([`crate::room`]).
pub fn sweep_ir_scene(
    d: &TraceData,
    color: Color,
    status: &Status,
    view: &ViewState,
    theme: &Theme,
    size: Viewport,
) -> Option<crate::ir::IrScene> {
    let s = d.sweep.as_ref()?;
    let frame = ir_frame(d)?;
    let table = s
        .room
        .as_ref()
        .map(|r| crate::room::room_table(r, crate::room::BandSet::Octave))
        .filter(|t| size.height - crate::room::table_height(t, theme) >= MIN_IR_HEIGHT);
    let ir_size = Viewport {
        height: size.height
            - table
                .as_ref()
                .map_or(0.0, |t| crate::room::table_height(t, theme)),
        ..size
    };
    let mut sc = crate::ir::ir_scene(
        &frame,
        color,
        None,
        &crate::ir::IrCtx {
            status,
            view,
            axes: &view.distortion.ir,
            style: crate::view::Style {
                chrome: view.chrome,
                theme,
            },
            size: ir_size,
        },
    );
    sc.room = table.clone();
    if let Some(t) = &table {
        let rect = Rect::new(
            MARGINS.left,
            ir_size.height,
            (size.width - MARGINS.left - MARGINS.right).max(1.0),
            size.height - ir_size.height,
        );
        sc.scene.viewport = size;
        sc.scene
            .layers
            .push(crate::room::table_layer(t, rect, theme));
    }
    let origin = format!(
        "t = 0 at the arrival {}",
        crate::readout::delay_readout(s.info.arrival.0, view.temperature_c)
    );
    for layer in &mut sc.scene.layers {
        for l in &mut layer.labels {
            if l.text == sc.origin {
                l.text.clone_from(&origin);
            }
        }
    }
    sc.origin = origin;
    let xm = sc.x_axis.mapping;
    let plot = sc.plot;
    if let Some(overlay) = sc.scene.layers.get_mut(2) {
        for (t_ms, name) in harmonic_marks(s) {
            let x = xm.to_px(t_ms);
            if !(x >= plot.x && x <= plot.right()) {
                continue;
            }
            overlay.polylines.push(Polyline {
                points: vec![[x, plot.y], [x, plot.bottom() - 16.0]],
                alpha: vec![],
                stroke: Stroke {
                    color: theme.text_dim,
                    width: 1.0,
                    dash: Some(crate::primitives::Dash {
                        on: 3.0,
                        off: 3.0,
                        offset: 0.0,
                    }),
                },
                clip: Some(plot),
            });
            overlay.labels.push(label(
                name,
                [x + 3.0, plot.bottom() - 4.0],
                anchor(HAlign::Left, VAlign::Bottom),
                theme.small_font_size,
                theme.text_dim,
            ));
        }
    }
    Some(sc)
}

/// Times (ms re the arrival) and names of the harmonic impulses: `(−L·ln k, "H k")`.
pub fn harmonic_marks(s: &SweepData) -> Vec<(f64, String)> {
    s.harmonics
        .iter()
        .map(|h| {
            (
                -s.info.rate.0 * f64::from(h.order).ln() * 1000.0,
                order_name(h.order),
            )
        })
        .collect()
}
