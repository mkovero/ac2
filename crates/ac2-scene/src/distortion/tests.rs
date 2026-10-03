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
    let names: Vec<&str> = sc.legend.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["noise floor (H2)", "H2", "H3", "THD"]);
    assert!(
        sc.info
            .starts_with("1083 sweep · arrival 3.32 ms · 2 × 3.10 s"),
        "{}",
        sc.info
    );
    // H2 is drawn at −40 dB up to 2 kHz; H3 (all within the noise) is not drawn.
    let data_layer = &sc.scene.layers[1];
    let y40 = sc.y_axis.mapping.to_px(-40.0);
    let h2 = data_layer
        .polylines
        .iter()
        .find(|p| p.clip == Some(sc.plot) && p.points.iter().all(|q| (q[1] - y40).abs() < 0.01))
        .expect("H2 line");
    let x2k = sc.x_axis.mapping.to_px(2000.0);
    assert!(h2.points.iter().all(|q| q[0] <= x2k + 0.5));
    let drawn_in_plot = data_layer
        .polylines
        .iter()
        .filter(|p| p.clip == Some(sc.plot))
        .count();
    assert_eq!(drawn_in_plot, 2, "H2 and THD; H3 stays in the noise");
    assert_eq!(data_layer.bands.len(), 1, "floor shaded");
    let cur = sc.cursor.expect("cursor");
    assert_eq!(cur.freq, "1.00 kHz");
    let row = |n: &str| cur.rows.iter().find(|r| r.0 == n).unwrap().1.clone();
    assert_eq!(row("H2"), "−40.0 dB");
    assert_eq!(row("H3"), "< −70.0 dB");
    assert_eq!(row("fund"), "−6.0 dB");

    // Percent: same curves, a 0-based axis that holds the largest valid value.
    view.distortion.unit = DistortionUnit::Percent;
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
    assert_eq!(sc.y_axis.mapping.range.lo, 0.0);
    assert!(sc.y_axis.mapping.range.hi >= 1.0);
    let cur = sc.cursor.expect("cursor");
    assert_eq!(cur.rows.iter().find(|r| r.0 == "H2").unwrap().1, "1.00 %");
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
