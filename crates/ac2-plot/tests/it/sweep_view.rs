//! End to end: the sweep's response and distortion view built from a synthetic run,
//! rendered and compared with a golden image. Its texts are asserted headless in
//! `ac2-scene`; this checks that the plates it shares with the transfer and spectrum views
//! are drawn as theirs are (the legend with its samples, the cursor values, the mark).

use crate::common;

use ac2_plot::Viewport;
use ac2_proto::GridDef;
use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_scene::banner::Status;
use ac2_scene::distortion::{SweepView, distortion_scene};
use ac2_scene::plate::PlateKind;
use ac2_scene::theme::Theme;
use ac2_scene::view::ViewState;
use ac2_testkit::image::ImageTolerance;
use common::{golden, gpu, render_on, renderer};

const TEXT: ImageTolerance = ImageTolerance {
    channel: 32,
    max_bad_fraction: 0.005,
};

fn grid() -> GridDef {
    GridDef::Log {
        ppo: 12,
        k_min: -60,
        k_max: 50,
    }
}

/// A run to 4 kHz: a response with a resonance at 1 kHz and a notch at 250 Hz, H2 at
/// −40 dB up to 2 kHz, H3 within the noise, the reference input's own H2 4 dB under the
/// measured one (so the pane carries the note that says the two cannot be told apart).
fn run() -> TraceData {
    let f = grid().centres();
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
    let bump = |x: f64, at: f64, db: f64| db * (-((x / at).log2() * 4.0).powi(2)).exp();
    TraceData {
        meta: TraceMeta {
            id: TraceId(3),
            edit: TraceEdit {
                owner: TraceOwner::Imported,
                name: "Run 1".into(),
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
            source: TraceSource::Sweep {
                meas: MeasId(1),
                meas_name: "Sweep".into(),
                number: 1,
                run: SweepId(1),
                epoch: SessionEpoch(1),
                sweep: EssSpec::with_fades(Hz(20.0), Hz(4000.0), Seconds(3.0)),
                level: Dbfs(-50.0),
                repeats: 2,
                lf_harmonics: LfHarmonics::Standard,
                reference_input: 1,
                measurement_input: 0,
            },
            grid_id: grid().id(),
            delay: Seconds(0.00332),
            depth: None,
            lf_window: None,
            cal: CalState::Uncalibrated,
            mic: None,
            mic_curve: None,
            created_at: WallNs(0),
            eq_in: Vec::new(),
            position: None,
            captured_unsettled: false,
            superseded: false,
        },
        mag_db: f
            .iter()
            .map(|x| (-6.0 + bump(*x, 1000.0, 9.0) - bump(*x, 250.0, 14.0)) as f32)
            .collect(),
        phase_deg: None,
        coherence: None,
        ir: None,
        dynamics: None,
        sweep: Some(SweepData {
            unresolved: None,
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
            reference_harmonics: Some(vec![HarmonicCurve {
                order: 2,
                curve: curve(-44.0, -110.0, 2000.0),
            }]),
            ir: TraceIr {
                t0: Seconds(0.0),
                dt: Seconds(0.001),
                linear: vec![1.0, 0.0],
                etc_db: vec![0.0, -90.0],
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
            room: None,
        }),
    }
}

/// The cursor on the resonance read against its mark on the notch: the legend on its plate
/// in the distortion plot (the run, its orders, the dotted, dashed and shaded samples), the
/// cursor values on theirs at the right, the mark's plate in a bottom corner of the
/// fundamental's plot and its dashed line in both plots; the near-reference note along the
/// bottom, uncut.
#[test]
fn sweep_view_cursor_mark() {
    let name = "sweep_view_cursor_mark";
    let Some(gpu) = gpu(name) else {
        return;
    };
    let d = run();
    let f = grid().centres();
    let theme = Theme::dark();
    let view = ViewState {
        cursor_hz: Some(1000.0),
        cursor_mark_hz: Some(250.0),
        ..ViewState::default()
    };
    let s = distortion_scene(
        Some(SweepView {
            data: &d,
            freqs: &f,
            color: theme.trace_color(0),
            selected: true,
        }),
        &[],
        "",
        &Status::default(),
        &view,
        &theme,
        Viewport::new(720.0, 520.0),
    );
    let kinds: Vec<PlateKind> = s.plates().iter().map(|p| p.kind).collect();
    assert_eq!(
        kinds,
        [PlateKind::Legend, PlateKind::Readout, PlateKind::Mark]
    );
    assert_eq!(s.near_reference.len(), 1);
    let mut r = renderer(gpu);
    let img = render_on(gpu, &mut r, &s.scene, 1.0, theme.background);
    golden(name, &img, TEXT);
}
