#![allow(clippy::unwrap_used)]

use super::*;
use crate::grid::column_frequencies;
use ac2_proto::GridDef;
use ac2_proto::model::*;
use ac2_proto::units::*;

const SIZE: Viewport = Viewport {
    width: 900.0,
    height: 500.0,
};

fn grid() -> GridDef {
    GridDef::Log {
        ppo: 12,
        k_min: -60,
        k_max: 50,
    }
}

/// H2 at −40 dB (valid) up to 2 kHz, H3 within the noise, nothing above f2/k.
fn data() -> TraceData {
    let f = column_frequencies(&grid());
    let n = f.len();
    let curve = |level: f32, floor: f32, top: f64| DistortionCurve {
        level_db: f
            .iter()
            .map(|x| if *x <= top { level } else { f32::NAN })
            .collect(),
        floor_db: f
            .iter()
            .map(|x| if *x <= top { floor } else { f32::NAN })
            .collect(),
    };
    TraceData {
        meta: TraceMeta {
            id: TraceId(3),
            edit: TraceEdit {
                name: "1083 sweep".into(),
                color: Rgb { r: 1, g: 2, b: 3 },
                visible: true,
                locked: false,
                order: 3,
                offset: Db(0.0),
                polarity: Polarity::Normal,
                delay_nudge: Seconds(0.0),
                slot: None,
                smoothing: None,
            },
            kind: TraceKind::Sweep,
            source: TraceSource::IrCapture {
                run: SweepId(1),
                epoch: SessionEpoch(1),
                sweep: EssSpec::with_fades(Hz(20.0), Hz(4000.0), Seconds(3.0)),
                level: Dbfs(-50.0),
                repeats: 2,
                reference_input: 1,
                measurement_input: 0,
            },
            grid_id: grid().id(),
            delay: Seconds(0.00332),
            depth: None,
            cal: CalState::Uncalibrated,
            mic: None,
            created_at: WallNs(0),
        },
        mag_db: vec![-6.0; n],
        phase_deg: Some(vec![0.0; n]),
        coherence: None,
        sweep: Some(SweepData {
            harmonics: vec![
                HarmonicCurve {
                    order: 2,
                    curve: curve(-40.0, -70.0, 2000.0),
                },
                HarmonicCurve {
                    order: 3,
                    curve: curve(-68.0, -70.0, 1333.0),
                },
            ],
            thd: curve(-39.9, -67.0, 2000.0),
            ir: SweepIr {
                t0: Seconds(-0.75),
                dt: Seconds(0.001),
                linear: (0..1000)
                    .map(|i| if i == 750 { 1.0 } else { 0.0 })
                    .collect(),
                etc_db: (0..1000)
                    .map(|i| if i == 750 { 0.0 } else { -90.0 })
                    .collect(),
            },
            info: SweepInfo {
                sample_rate: Hz(48_000.0),
                rate: Seconds(0.4),
                duration: Seconds(3.1),
                repeats: 2,
                arrival: Seconds(0.00332),
                reference_level: Db(2.3),
                window_pre: Seconds(0.008),
                window_post: Seconds(0.087),
                gate_pre: Seconds(0.03),
                gate: Seconds(0.88),
                floor_margin: Db(6.0),
                clipped: false,
            },
        }),
    }
}

fn status() -> Status {
    Status {
        daemon_silence_s: 0.0,
        protection: ac2_proto::frame::ProtectionFlags::NONE,
        frame_age_s: None,
        timing: None,
        no_delay_estimate: None,
    }
}

#[test]
fn readings_and_their_text() {
    let d = data();
    let s = d.sweep.as_ref().unwrap();
    let f = column_frequencies(&grid());
    let at1k = nearest_column(&f, 1000.0).unwrap();
    assert_eq!(
        reading(&s.harmonics[0].curve, at1k, 6.0),
        Reading::Level(-40.0)
    );
    assert_eq!(
        reading(&s.harmonics[1].curve, at1k, 6.0),
        Reading::BelowFloor(-70.0)
    );
    let at3k = nearest_column(&f, 3000.0).unwrap();
    assert_eq!(
        reading(&s.harmonics[0].curve, at3k, 6.0),
        Reading::NotMeasured
    );
    assert_eq!(
        reading_text(Reading::Level(-40.0), DistortionUnit::Db),
        "−40.0 dB"
    );
    assert_eq!(
        reading_text(Reading::Level(-40.0), DistortionUnit::Percent),
        "1.00 %"
    );
    assert_eq!(
        reading_text(Reading::BelowFloor(-70.0), DistortionUnit::Db),
        "< −70.0 dB"
    );
    assert_eq!(
        reading_text(Reading::BelowFloor(-70.0), DistortionUnit::Percent),
        "< 0.0316 %"
    );
    assert_eq!(reading_text(Reading::NotMeasured, DistortionUnit::Db), "—");
    assert_eq!(percent_text(-20.0), "10.0 %");
    assert_eq!(percent_text(0.0), "100 %");

    let sum = summary(s, &f);
    assert_eq!(sum.thd[1], (1000.0, Reading::Level(-39.9_f32 as f64)));
    // 10 kHz is above the sweep: not measured.
    assert_eq!(sum.thd[2].1, Reading::NotMeasured);
    assert_eq!(sum.peaks[0].0, 2);
    assert!((sum.peaks[0].1.unwrap().1 + 40.0).abs() < 1e-9);
    // H3 never leaves the noise: no peak.
    assert_eq!(sum.peaks[1], (3, None));
}

#[test]
fn distortion_view_draws_valid_points_and_shades_the_floor() {
    let d = data();
    let f = column_frequencies(&grid());
    let mut view = ViewState {
        cursor_hz: Some(1000.0),
        ..ViewState::default()
    };
    let sc = distortion_scene(
        Some(SweepView {
            data: &d,
            freqs: &f,
        }),
        &status(),
        &view,
        &Theme::dark(),
        SIZE,
    );
    assert!(sc.note.is_none());
    assert!(
        sc.fundamental.bottom() < sc.plot.y,
        "fundamental above distortion"
    );
    let names: Vec<&str> = sc.legend.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["H2", "H3", "THD", "< floor", "noise"]);
    assert_eq!(sc.legend[3].mark, LegendMark::Dashed);
    assert_eq!(sc.legend[4].mark, LegendMark::Shade);
    assert!(
        sc.info
            .starts_with("1083 sweep · arrival 3.32 ms · 2 × 3.10 s"),
        "{}",
        sc.info
    );
    assert_eq!(sc.caption, sc.info, "room for all of it");
    // Wide: the legend is one row beside the axis title.
    let legend_ys: Vec<f32> = legend_labels(&sc).iter().map(|l| l.pos[1]).collect();
    assert!(
        legend_ys.iter().all(|y| *y == legend_ys[0]),
        "{legend_ys:?}"
    );
    // H2 is drawn at −40 dB up to 2 kHz, solid; H3 (all within the noise) at its floor,
    // dashed, in its own colour.
    let data_layer = &sc.scene.layers[1];
    let at = |db: f64| sc.y_axis.mapping.to_px(db);
    let on = |p: &Polyline, y: f32| p.points.iter().all(|q| (q[1] - y).abs() < 0.01);
    let h2 = data_layer
        .polylines
        .iter()
        .find(|p| p.clip == Some(sc.plot) && on(p, at(-40.0)))
        .expect("H2 line");
    assert!(h2.stroke.dash.is_none());
    let x2k = sc.x_axis.mapping.to_px(2000.0);
    assert!(h2.points.iter().all(|q| q[0] <= x2k + 0.5));
    let h3 = data_layer
        .polylines
        .iter()
        .find(|p| p.clip == Some(sc.plot) && on(p, at(-70.0)))
        .expect("H3 at its floor");
    assert!(h3.stroke.dash.is_some());
    assert_eq!(
        h3.stroke.color,
        order_color(&Theme::dark(), 3).with_alpha(0.6)
    );
    let drawn_in_plot = data_layer
        .polylines
        .iter()
        .filter(|p| p.clip == Some(sc.plot))
        .count();
    assert_eq!(drawn_in_plot, 3, "H2, THD solid; H3 dashed at its floor");
    assert_eq!(data_layer.bands.len(), 1, "noise shaded");
    let cur = sc.cursor.expect("cursor");
    assert_eq!(cur.freq, "1.00 kHz");
    let row = |n: &str| cur.rows.iter().find(|r| r.0 == n).unwrap().1.clone();
    assert_eq!(row("H2"), "−40.0 dB");
    assert_eq!(row("H3"), "< −70.0 dB");
    assert_eq!(row("fund"), "−6.0 dB");

    // Percent: a log axis over the same ratios (0.001 … 100 %), decade labels; the curves
    // land where they did in dB, the readouts in percent.
    view.distortion.unit = DistortionUnit::Percent;
    let pc = distortion_scene(
        Some(SweepView {
            data: &d,
            freqs: &f,
        }),
        &status(),
        &view,
        &Theme::dark(),
        SIZE,
    );
    let m = pc.y_axis.mapping;
    assert_eq!(m.scale, crate::axis::Scale::Log);
    assert!((m.range.lo - 0.001).abs() < 1e-12 && (m.range.hi - 100.0).abs() < 1e-9);
    let labels = pc.y_axis.labels();
    for l in ["0.01", "0.1", "1", "10", "100"] {
        assert!(labels.contains(&l), "{labels:?}");
    }
    assert!((m.to_px(1.0) - sc.y_axis.mapping.to_px(-40.0)).abs() < 1e-3);
    let pc_lines = pc.scene.layers[1].polylines.clone();
    let h2_pc = pc_lines
        .iter()
        .find(|p| p.clip == Some(pc.plot) && on(p, m.to_px(1.0)))
        .expect("H2 at 1 %");
    assert_eq!(h2_pc.points.len(), h2.points.len());
    let cur = pc.cursor.expect("cursor");
    let row = |n: &str| cur.rows.iter().find(|r| r.0 == n).unwrap().1.clone();
    assert_eq!(row("H2"), "1.00 %");
    assert_eq!(row("H3"), "< 0.0316 %");
    assert_eq!(row("THD"), "1.01 %");
    assert_eq!(row("fund"), "−6.0 dB", "the response stays in dB");
}

/// The legend's names (overlay labels in the legend's colours along the plot's top).
fn legend_labels(sc: &DistortionScene) -> Vec<crate::primitives::Label> {
    let names: Vec<&str> = sc.legend.iter().map(|e| e.name.as_str()).collect();
    sc.scene.layers[2]
        .labels
        .iter()
        .filter(|l| names.contains(&l.text.as_str()))
        .cloned()
        .collect()
}

#[test]
fn caption_and_legend_never_overlap_in_small_panes() {
    use crate::canvas::tests::{intersects, label_box};
    let mut d = data();
    d.meta.edit.name = "Main L 1083 on stand".into();
    d.sweep.as_mut().unwrap().info.clipped = true;
    let f = column_frequencies(&grid());
    for unit in [DistortionUnit::Db, DistortionUnit::Percent] {
        let mut view = ViewState {
            cursor_hz: Some(1000.0),
            ..ViewState::default()
        };
        view.distortion.unit = unit;
        // A quarter of a 1290 px window (≈ 520 × 330) and smaller, up to maximised.
        for (w, h) in [
            (300.0, 220.0),
            (400.0, 260.0),
            (520.0, 330.0),
            (640.0, 400.0),
            (900.0, 500.0),
            (1290.0, 780.0),
        ] {
            let size = Viewport {
                width: w,
                height: h,
            };
            let sc = distortion_scene(
                Some(SweepView {
                    data: &d,
                    freqs: &f,
                }),
                &status(),
                &view,
                &Theme::dark(),
                size,
            );
            // Every text placed inside the two plots: axis titles, caption, legend, cursor.
            let inside = |p: [f32; 2]| {
                [sc.fundamental, sc.plot]
                    .iter()
                    .any(|r| p[0] >= r.x && p[0] <= r.right() && p[1] >= r.y && p[1] <= r.bottom())
            };
            let labels: Vec<_> = sc
                .scene
                .layers
                .iter()
                .flat_map(|l| &l.labels)
                .filter(|l| inside(l.pos))
                .collect();
            assert!(labels.len() >= 2 + sc.legend.len(), "{w}×{h}");
            for (i, a) in labels.iter().enumerate() {
                let ba = label_box(a);
                assert!(
                    ba.x >= sc.plot.x && ba.right() <= sc.plot.right() + 0.5,
                    "{w}×{h} {unit:?}: {:?} leaves the plot",
                    a.text
                );
                for b in &labels[i + 1..] {
                    assert!(
                        !intersects(ba, label_box(b)),
                        "{w}×{h} {unit:?}: {:?} over {:?}",
                        a.text,
                        b.text
                    );
                }
            }
            // The arrival is kept as long as it fits; CLIPPED always.
            assert!(sc.caption.contains("CLIPPED"), "{w}×{h}: {:?}", sc.caption);
            if w >= 400.0 {
                assert!(sc.caption.starts_with("arrival") || sc.caption == sc.info);
            }
            if w >= 900.0 {
                assert_eq!(sc.caption, sc.info);
            }
        }
    }
}

#[test]
fn lone_valid_points_are_short_lines_and_noise_is_dashed() {
    // H2 alternates: valid at even columns, within the noise at odd ones (a curve hovering
    // at its floor), as on a speaker measured near the noise.
    let mut d = data();
    let s = d.sweep.as_mut().unwrap();
    let h2 = &mut s.harmonics[0].curve;
    for (i, (l, fl)) in h2.level_db.iter_mut().zip(&mut h2.floor_db).enumerate() {
        if l.is_finite() {
            *fl = -70.0;
            *l = if i % 2 == 0 { -50.0 } else { -66.0 };
        }
    }
    let f = column_frequencies(&grid());
    // Columns closer than the pixels of a 1/4 pane would make dots.
    let size = Viewport {
        width: 420.0,
        height: 300.0,
    };
    let sc = distortion_scene(
        Some(SweepView {
            data: &d,
            freqs: &f,
        }),
        &status(),
        &ViewState::default(),
        &Theme::dark(),
        size,
    );
    let y50 = sc.y_axis.mapping.to_px(-50.0);
    let y70 = sc.y_axis.mapping.to_px(-70.0);
    let lines = &sc.scene.layers[1].polylines;
    let solid = lines
        .iter()
        .find(|p| p.stroke.dash.is_none() && p.points.iter().any(|q| (q[1] - y50).abs() < 0.01))
        .expect("H2 valid points");
    let segs = crate::canvas::tests::segments(&solid.points);
    assert!(segs.len() > 10);
    for s in &segs {
        assert_eq!(s.len(), 2, "each lone point is a level across its cell");
        assert!(s[1][0] - s[0][0] > 0.5, "visible length: {s:?}");
        assert_eq!(s[0][1], s[1][1]);
    }
    // The odd columns are the order's floor, dashed: distinguishable from the level.
    let dashed = lines
        .iter()
        .find(|p| {
            p.stroke.dash.is_some()
                && p.stroke.color == order_color(&Theme::dark(), 2).with_alpha(0.6)
        })
        .expect("H2 floor where within the noise");
    assert!(
        dashed
            .points
            .iter()
            .all(|q| !q[1].is_finite() || (q[1] - y70).abs() < 0.01)
    );
    // Cells meet: a lone level and its neighbours' floor share their edges.
    let first = segs[0][1][0];
    assert!(
        dashed.points.iter().any(|q| (q[0] - first).abs() < 0.01),
        "floor cell starts where the level's ends"
    );
}

#[test]
fn runs_join_neighbours_and_widen_lone_points() {
    let xs = [0.0, 10.0, 20.0, 30.0, 40.0];
    let nan = f32::NAN;
    let r = runs(&xs, &[1.0, 2.0, nan, 4.0, nan]);
    assert_eq!(r.len(), 5);
    assert_eq!(r[..2], [[0.0, 1.0], [10.0, 2.0]]);
    assert!(r[2][0].is_nan());
    // A lone point spans its cell, half-way to each neighbour.
    assert_eq!(r[3..], [[25.0, 4.0], [35.0, 4.0]]);
    // At the edge: as wide on the open side as on the other.
    assert_eq!(
        runs(&xs, &[1.0, nan, nan, nan, nan]),
        [[-5.0, 1.0], [5.0, 1.0]]
    );
    assert!(runs(&xs, &[nan; 5]).is_empty());
}

#[test]
fn without_a_sweep_the_view_says_so() {
    let sc = distortion_scene(None, &status(), &ViewState::default(), &Theme::dark(), SIZE);
    assert_eq!(sc.note.as_deref(), Some("no sweep result"));
    assert!(sc.legend.is_empty());
}

#[test]
fn the_sweep_ir_marks_the_harmonics() {
    let d = data();
    let sc = sweep_ir_scene(
        &d,
        Color::from_rgba8([255, 0, 0, 255]),
        &status(),
        &ViewState::default(),
        &Theme::dark(),
        SIZE,
    )
    .expect("ir");
    assert!(
        sc.origin.starts_with("t = 0 at the arrival 3.32 ms"),
        "{}",
        sc.origin
    );
    let marks: Vec<&str> = sc.scene.layers[2]
        .labels
        .iter()
        .map(|l| l.text.as_str())
        .filter(|t| t.starts_with('H'))
        .collect();
    assert_eq!(marks, ["H2", "H3"]);
    // H2 at −L·ln 2.
    assert!((harmonic_marks(d.sweep.as_ref().unwrap())[0].0 + 400.0 * 2f64.ln()).abs() < 1e-9);
}
