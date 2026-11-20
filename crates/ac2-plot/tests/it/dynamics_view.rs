//! End to end: the Dynamics pane's four views built from a synthetic run of a compressor on
//! two inputs, rendered and compared with golden images. Their texts are asserted headless
//! in `ac2-scene`; this checks that the cells, the curves, the shaded tolerance and the
//! table are drawn where the scene puts them, with the readout and legend plates shared
//! with the other panes.

use crate::common;

use ac2_plot::Viewport;
use ac2_proto::model::layout::DynamicsView;
use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_scene::banner::Status;
use ac2_scene::dynamics::{
    CurveId, DynCurve, DynamicsFrame, DynamicsModes, Side, curve_steps, dynamics_scene,
};
use ac2_scene::theme::Theme;
use ac2_scene::view::ViewState;
use ac2_testkit::image::ImageTolerance;
use common::{golden, gpu, render_on, renderer};

const TEXT: ImageTolerance = ImageTolerance {
    channel: 32,
    max_bad_fraction: 0.005,
};
const LEVELS: [f64; 3] = [-20.0, -14.0, -8.0];
const LENGTHS: [f64; 3] = [0.01, 0.1, 1.0];

fn secs(v: f64) -> DynamicsTime {
    DynamicsTime::Seconds { value: Seconds(v) }
}

/// A compressor of threshold −26 dBFS, ratio 4 : 1, attack 5 ms and release 300 ms; the
/// second input (`gr_scale` 0.9) compresses a tenth less.
fn row(level: f64, length: f64, gr_scale: f64) -> DynamicsRow {
    let mut r = ac2_proto::samples::dynamics_data().inputs[0].rows[0].clone();
    let full = (level + 26.0) * 0.75 * gr_scale;
    let reached = full * (1.0 - (-length / 0.005).exp());
    r.gr_end = Db(reached);
    r.gr_settled = Some(Db(full));
    r.attack_tau = secs(0.005);
    r.release_tau = secs(0.3 * gr_scale);
    r.recovery_0_5db = secs(0.3 * gr_scale * (reached / 0.5).ln().max(0.0));
    r.drift = Db(0.0);
    r
}

fn table(gr_scale: f64) -> DynamicsTable {
    let base = ac2_proto::samples::dynamics_data().table();
    let mut info = base.info.clone();
    info.bursts = LEVELS.iter().map(|l| Dbfs(*l)).collect();
    info.durations = LENGTHS.iter().map(|d| Seconds(*d)).collect();
    info.steps = LEVELS
        .iter()
        .flat_map(|l| {
            LENGTHS.iter().map(|d| DynamicsStep {
                level: Dbfs(*l),
                burst: Seconds(*d),
                rest: Seconds(3.0),
                recovered: true,
            })
        })
        .collect();
    let rows = info
        .steps
        .iter()
        .map(|s| row(s.level.0, s.burst.0, gr_scale))
        .collect();
    DynamicsTable {
        info,
        inputs: vec![DynamicsInput {
            input: if gr_scale == 1.0 { 1 } else { 2 },
            name: if gr_scale == 1.0 { "VCA L" } else { "VCA R" }.into(),
            rows,
            ..base.inputs[0].clone()
        }],
    }
}

/// Step `step`'s envelopes: gain reduction rising with the attack from the burst's start,
/// falling with the release from its end; THD high while the gain moves.
fn envelope(t: &DynamicsTable, step: u32, input: u16) -> DynamicsEnvelope {
    let s = &t.info.steps[step as usize];
    let r = &t.inputs[0].rows[step as usize];
    let DynamicsTime::Seconds { value: tau } = r.release_tau else {
        unreachable!()
    };
    let full = r.gr_settled.map_or(0.0, |g| g.0) as f32;
    let edge = |ts: Vec<f32>, gr: &dyn Fn(f32) -> f32| DynamicsEdgeCurve {
        h_db: [
            ts.iter()
                .map(|x| -30.0 - 20.0 * (x.abs() * 20.0).min(1.0))
                .collect(),
            ts.iter().map(|_| f32::NAN).collect(),
            ts.iter()
                .map(|x| -45.0 - 10.0 * (x.abs() * 20.0).min(1.0))
                .collect(),
            ts.iter().map(|_| f32::NAN).collect(),
        ],
        gr_db: ts.iter().map(|x| gr(*x)).collect(),
        t: ts,
    };
    let burst = s.burst.0 as f32;
    let start: Vec<f32> = (0..200)
        .map(|i| -0.005 + i as f32 * (burst + 0.005) / 199.0)
        .collect();
    let end: Vec<f32> = (0..200).map(|i| -0.005 + i as f32 * 1.5 / 199.0).collect();
    let reached = r.gr_end.0 as f32;
    DynamicsEnvelope {
        input,
        step,
        start: edge(start, &|x| {
            if x < 0.0 {
                0.0
            } else {
                full * (1.0 - (-x / 0.005).exp())
            }
        }),
        end: edge(end, &|x| {
            if x < 0.0 {
                reached
            } else {
                reached * (-x / tau.0 as f32).exp()
            }
        }),
    }
}

fn curve<'a>(t: &'a DynamicsTable, trace: u32, color: usize, theme: &Theme) -> DynCurve<'a> {
    DynCurve {
        id: CurveId {
            rig: None,
            trace: TraceId(trace),
            input: t.inputs[0].input,
        },
        name: format!("{} · Run 1", t.inputs[0].name),
        color: theme.trace_color(color),
        table: t,
        envelope: None,
    }
}

fn render(name: &str, view: DynamicsView, size: Viewport) {
    let Some(gpu) = gpu(name) else {
        return;
    };
    let theme = Theme::dark();
    let (l, r) = (table(1.0), table(0.9));
    let mut curves = vec![curve(&l, 1, 0, &theme), curve(&r, 2, 1, &theme)];
    let modes = DynamicsModes {
        view,
        a: Side::Curve(curves[0].id),
        b: Side::Curve(curves[1].id),
        ..DynamicsModes::default()
    };
    let frame = |c: &[DynCurve<'_>]| {
        curve_steps(&DynamicsFrame {
            curves: c,
            modes,
            pointer: None,
            missing: "",
        })
    };
    let steps = frame(&curves);
    let envs: Vec<Option<DynamicsEnvelope>> = curves
        .iter()
        .zip(&steps)
        .map(|(c, s)| s.map(|s| envelope(c.table, s, c.id.input)))
        .collect();
    for (c, e) in curves.iter_mut().zip(&envs) {
        c.envelope = e.as_ref();
    }
    let s = dynamics_scene(
        &DynamicsFrame {
            curves: &curves,
            modes,
            pointer: None,
            missing: "",
        },
        &Status::default(),
        &ViewState::default(),
        &theme,
        size,
    );
    assert_eq!((s.a, s.b), (Some(0), Some(1)), "{name}");
    assert!(s.note.is_none(), "{name}: {:?}", s.note);
    let mut rd = renderer(gpu);
    let img = render_on(gpu, &mut rd, &s.scene, 1.0, theme.background);
    golden(name, &img, TEXT);
}

/// A − B per step: a grid of levels by lengths, each cell's difference in gain reduction.
#[test]
fn dynamics_steps() {
    render(
        "dynamics_steps",
        DynamicsView::Steps,
        Viewport::new(900.0, 520.0),
    );
}

/// The picked step's gain reduction around the burst's start and end, A over B.
#[test]
fn dynamics_envelopes() {
    render(
        "dynamics_envelopes",
        DynamicsView::Envelopes,
        Viewport::new(900.0, 520.0),
    );
}

/// Release time against level, A and B.
#[test]
fn dynamics_release() {
    render(
        "dynamics_release",
        DynamicsView::Release,
        Viewport::new(900.0, 520.0),
    );
}

/// The table of every step: A, B and their difference, as wide as a maximised pane.
#[test]
fn dynamics_table() {
    render(
        "dynamics_table",
        DynamicsView::Table,
        Viewport::new(1400.0, 520.0),
    );
}
