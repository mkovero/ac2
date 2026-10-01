//! Live impulse-response pane: linear, log (|h| in dB re peak) or ETC, on a time axis whose
//! origin is the inserted delay.
//!
//! The IR frame's point `i` is at `t0 + i·dt` seconds relative to the inserted delay, so
//! t = 0 is where the delay finder put the arrival; the absolute time is `t + inserted`.
//! The log and ETC views are normalised to their own peak (0 dB = strongest point), which
//! is display scaling, not a level claim.

use ac2_proto::frame::IrFrame;

use crate::axis::{self, Axis, Range};
use crate::banner::{BannerRow, Status};
use crate::canvas::{self, Canvas, MARGINS, anchor, gapped, label};
use crate::primitives::{Color, HAlign, Polyline, Rect, Scene, Stroke, VAlign, Viewport};
use crate::readout;
use crate::theme::Theme;
use crate::time::Freshness;
use crate::view::{IrMode, ViewState};

#[derive(Clone, Debug, PartialEq)]
pub struct IrScene {
    pub scene: Scene,
    pub plot: Rect,
    pub x_axis: Axis,
    pub y_axis: Axis,
    pub title: String,
    /// `t = 0 at inserted delay 12.34 ms · 4.24 m @ 20 °C`.
    pub origin: String,
    /// Why nothing is drawn, when that is the case.
    pub note: Option<String>,
    /// Banner strip above the plot; zero height when no banner is up.
    pub strip: Rect,
    pub banners: Vec<BannerRow>,
}

/// Time of every point, ms re the inserted delay.
pub fn times_ms(frame: &IrFrame) -> Vec<f64> {
    let (t0, dt) = (frame.meta.t0.0, frame.meta.dt.0);
    (0..frame.linear.len())
        .map(|i| (t0 + i as f64 * dt) * 1000.0)
        .collect()
}

/// Values drawn for `mode` and the y range they need. `None` when the mode has no data.
pub fn ir_values(
    frame: &IrFrame,
    mode: IrMode,
    depth_db: f64,
) -> Option<(Vec<f64>, Range, String)> {
    let finite_max = |v: &mut dyn Iterator<Item = f64>| {
        v.filter(|x| x.is_finite())
            .fold(f64::NEG_INFINITY, f64::max)
    };
    match mode {
        IrMode::Linear => {
            let peak = finite_max(&mut frame.linear.iter().map(|x| f64::from(x.abs())));
            let lim = if peak > 0.0 { peak * 1.1 } else { 1.0 };
            let v = frame.linear.iter().map(|x| f64::from(*x)).collect();
            Some((v, Range::new(-lim, lim), "Amplitude (FS)".into()))
        }
        IrMode::Log => {
            let peak = finite_max(&mut frame.linear.iter().map(|x| f64::from(x.abs())));
            if peak.is_nan() || peak <= 0.0 {
                return None;
            }
            let v = frame
                .linear
                .iter()
                .map(|x| {
                    let a = f64::from(x.abs());
                    if a > 0.0 && a.is_finite() {
                        20.0 * (a / peak).log10()
                    } else {
                        f64::NAN
                    }
                })
                .collect();
            Some((v, Range::new(-depth_db, 3.0), "dB re peak".into()))
        }
        IrMode::Etc => {
            let etc = frame.etc.as_ref()?;
            let peak = finite_max(&mut etc.iter().map(|x| f64::from(*x)));
            if !peak.is_finite() {
                return None;
            }
            let v = etc.iter().map(|x| f64::from(*x) - peak).collect();
            Some((v, Range::new(-depth_db, 3.0), "ETC dB re peak".into()))
        }
    }
}

pub fn ir_scene(
    frame: &IrFrame,
    color: Color,
    freshness: Option<Freshness>,
    status: &Status,
    view: &ViewState,
    theme: &Theme,
    size: Viewport,
) -> IrScene {
    let mut c = Canvas::new(size, theme);
    let plot_w = (size.width - MARGINS.left - MARGINS.right).max(1.0);
    let strip = canvas::banner_strip(&mut c, status, MARGINS.left, plot_w, size, theme);
    let plot = canvas::plot_area(size, strip.rect.bottom(), MARGINS.right);
    let t = times_ms(frame);
    let full = match (t.first(), t.last()) {
        (Some(a), Some(b)) if b > a => Range::new(*a, *b),
        _ => Range::new(-1.0, 1.0),
    };
    let trange = view.ir.time_ms.filter(Range::is_valid).unwrap_or(full);
    let x_axis = axis::linear_axis(trange, plot.x, plot.right(), "ms");
    let values = ir_values(frame, view.ir.mode, view.ir.log_depth_db);
    let (yrange, title) = values
        .as_ref()
        .map_or((Range::new(-1.0, 1.0), String::new()), |(_, r, t)| {
            (*r, t.clone())
        });
    let title = if title.is_empty() {
        match view.ir.mode {
            IrMode::Linear => "Amplitude (FS)",
            IrMode::Log => "dB re peak",
            IrMode::Etc => "ETC dB re peak",
        }
        .to_string()
    } else {
        title
    };
    let y_unit = if view.ir.mode == IrMode::Linear {
        "FS"
    } else {
        "dB"
    };
    let y_axis = axis::linear_axis(yrange, plot.bottom(), plot.y, y_unit);
    canvas::pane_frame(&mut c, plot, &x_axis, &y_axis, true, &title, theme);
    let (xm, ym) = (x_axis.mapping, y_axis.mapping);
    if view.ir.mode == IrMode::Linear {
        canvas::hline(&mut c, plot, ym.to_px(0.0), theme.zero_line);
    }
    // t = 0: where the inserted delay puts the arrival.
    canvas::vline(&mut c.base, plot, xm.to_px(0.0), theme.zero_line);
    let origin = format!(
        "t = 0 at inserted delay {}",
        readout::delay_readout(frame.meta.inserted_delay.0, view.temperature_c)
    );
    c.base.labels.push(label(
        origin.clone(),
        [plot.right() - 6.0, plot.y + 4.0],
        anchor(HAlign::Right, VAlign::Top),
        theme.small_font_size,
        theme.text_dim,
    ));

    let note = match &values {
        None if view.ir.mode == IrMode::Etc && frame.etc.is_none() => {
            Some("ETC not published for this measurement".to_string())
        }
        None => Some("no impulse energy".to_string()),
        Some(_) => None,
    };
    if let Some((v, _, _)) = &values {
        let xs: Vec<f32> = t.iter().map(|x| xm.to_px(*x)).collect();
        // Log views: points below the floor sit on the floor instead of vanishing, so the
        // decay stays a connected line.
        let floor = yrange.lo;
        let ys: Vec<f32> = v
            .iter()
            .map(|y| {
                if view.ir.mode != IrMode::Linear && y.is_nan() {
                    ym.to_px(floor)
                } else {
                    ym.to_px(y.max(floor))
                }
            })
            .collect();
        let (points, _) = gapped(&xs, &ys, None, |_, _| false);
        let dim = if freshness.is_some_and(|f| f.is_stale()) {
            theme.stale_alpha
        } else {
            1.0
        };
        c.data.polylines.push(Polyline {
            points,
            alpha: vec![],
            stroke: Stroke::solid(color.with_alpha(dim), (theme.trace_width * 0.75).max(1.0)),
            clip: Some(plot),
        });
    }
    if let Some(n) = &note {
        c.overlay.labels.push(label(
            n.clone(),
            [plot.x + plot.w / 2.0, plot.y + plot.h / 2.0],
            anchor(HAlign::Center, VAlign::Center),
            theme.font_size,
            theme.text_dim,
        ));
    }
    IrScene {
        scene: c.into_scene(size),
        plot,
        x_axis,
        y_axis,
        title,
        origin,
        note,
        strip: strip.rect,
        banners: strip.rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::IrView;
    use ac2_proto::frame::IrMeta;
    use ac2_proto::units::{Hz, MeasId, Seconds};

    const SIZE: Viewport = Viewport {
        width: 800.0,
        height: 300.0,
    };

    fn frame() -> IrFrame {
        // 48 kHz, 21 points from −0.5 ms; impulse 0.5 at t = 0, a −0.25 reflection at 2 ms.
        let mut linear = vec![0.0f32; 21];
        linear[5] = 0.5;
        linear[9] = 0.05;
        linear[20] = -0.25;
        IrFrame {
            meas: MeasId(1),
            meta: IrMeta {
                sample_rate: Hz(48_000.0),
                t0: Seconds(-0.0005),
                dt: Seconds(0.0001),
                inserted_delay: Seconds(0.012_34),
            },
            linear,
            etc: None,
        }
    }

    fn view(mode: IrMode) -> ViewState {
        ViewState {
            ir: IrView {
                mode,
                ..IrView::default()
            },
            ..ViewState::default()
        }
    }

    #[test]
    fn time_axis_origin_is_the_inserted_delay() {
        let f = frame();
        let t = times_ms(&f);
        assert!((t[5]).abs() < 1e-12);
        assert!((t[20] - 1.5).abs() < 1e-9);
        let s = ir_scene(
            &f,
            Color::WHITE,
            None,
            &Status::default(),
            &view(IrMode::Linear),
            &Theme::dark(),
            SIZE,
        );
        assert_eq!(
            s.origin,
            "t = 0 at inserted delay 12.34 ms · 4.24 m @ 20 °C"
        );
        assert_eq!(s.x_axis.title, "ms");
        assert!(
            s.x_axis.labels().contains(&"0.0"),
            "{:?}",
            s.x_axis.labels()
        );
        // Linear: symmetric, 10 % headroom over the peak.
        assert!((s.y_axis.mapping.range.hi - 0.55).abs() < 1e-6);
        assert!((s.y_axis.mapping.range.lo + 0.55).abs() < 1e-6);
        assert_eq!(s.title, "Amplitude (FS)");
        // The impulse point is drawn at t = 0.
        let line = &s.scene.layers[1].polylines[0];
        assert!((line.points[5][0] - s.x_axis.mapping.to_px(0.0)).abs() < 1e-3);
        assert!(s.note.is_none());
    }

    #[test]
    fn log_view_is_re_peak() {
        let f = frame();
        let (v, r, title) = ir_values(&f, IrMode::Log, 60.0).expect("values");
        assert_eq!(title, "dB re peak");
        assert!((v[5] - 0.0).abs() < 1e-9);
        assert!((v[9] + 20.0).abs() < 1e-6);
        assert!((v[20] + 6.0206).abs() < 1e-3);
        assert!(v[0].is_nan());
        assert_eq!(r, Range::new(-60.0, 3.0));
        // Silent samples sit on the floor in the drawing.
        let s = ir_scene(
            &f,
            Color::WHITE,
            None,
            &Status::default(),
            &view(IrMode::Log),
            &Theme::dark(),
            SIZE,
        );
        let line = &s.scene.layers[1].polylines[0];
        assert_eq!(line.points.len(), 21);
        assert!((line.points[0][1] - s.plot.bottom()).abs() < 1e-3);
    }

    #[test]
    fn etc_missing_or_present() {
        let mut f = frame();
        let s = ir_scene(
            &f,
            Color::WHITE,
            None,
            &Status::default(),
            &view(IrMode::Etc),
            &Theme::dark(),
            SIZE,
        );
        assert_eq!(
            s.note.as_deref(),
            Some("ETC not published for this measurement")
        );
        assert!(s.scene.layers[1].polylines.is_empty());
        f.etc = Some((0..21).map(|i| -10.0 - i as f32).collect());
        let (v, _, _) = ir_values(&f, IrMode::Etc, 60.0).expect("etc");
        assert_eq!(v[0], 0.0);
        assert_eq!(v[20], -20.0);
    }

    #[test]
    fn zoomed_time_range_and_stale() {
        let f = frame();
        let mut v = view(IrMode::Linear);
        v.ir.time_ms = Some(Range::new(-0.2, 0.2));
        let s = ir_scene(
            &f,
            Color::WHITE,
            Some(Freshness::from_age(2.0)),
            &Status::default(),
            &v,
            &Theme::dark(),
            SIZE,
        );
        assert_eq!(s.x_axis.mapping.range, Range::new(-0.2, 0.2));
        assert_eq!(
            s.x_axis.labels(),
            [
                "−0.20", "−0.15", "−0.10", "−0.05", "0.00", "0.05", "0.10", "0.15", "0.20"
            ]
        );
        let stroke = s.scene.layers[1].polylines[0].stroke;
        assert!((stroke.color.a - Theme::dark().stale_alpha).abs() < 1e-6);
    }

    #[test]
    fn banners_sit_above_the_plot() {
        let f = frame();
        let calm = ir_scene(
            &f,
            Color::WHITE,
            None,
            &Status::default(),
            &view(IrMode::Log),
            &Theme::dark(),
            SIZE,
        );
        assert_eq!(calm.strip.h, 0.0);
        assert_eq!(calm.plot.y, MARGINS.top);
        let s = ir_scene(
            &f,
            Color::WHITE,
            None,
            &crate::banner::tests::everything(),
            &view(IrMode::Log),
            &Theme::dark(),
            SIZE,
        );
        assert_eq!(s.plot.y, s.strip.bottom() + MARGINS.top);
        assert_eq!(s.plot.bottom(), calm.plot.bottom());
        crate::canvas::tests::assert_banners_clear(&s.scene, &s.banners, &[s.plot]);
    }
}
