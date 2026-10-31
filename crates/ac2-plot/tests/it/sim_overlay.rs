//! End to end: simulated reflections over a measured impulse response (`ac2-scene`'s
//! `room::sim_marks`), rendered and compared with a golden image. Faint dashed lines, the
//! strong reflections named in plan terms in rows that do not overlap, the legend at the
//! plot's foot.

use crate::common;

use ac2_plot::Viewport;
use ac2_proto::frame::{IrFrame, IrMeta};
use ac2_proto::model::{
    Marker, MarkerKind, PlanSpec, SimArrival, SimState, Surface, TracePosition,
};
use ac2_proto::units::{Hz, MeasId, Seconds, WalkId};
use ac2_scene::banner::Status;
use ac2_scene::room::sim_marks::{draw_marks, reflection_marks};
use ac2_scene::theme::Theme;
use ac2_scene::view::{Style, ViewState};
use ac2_testkit::image::ImageTolerance;
use common::{golden, gpu, render_on, renderer};

const TEXT: ImageTolerance = ImageTolerance {
    channel: 32,
    max_bad_fraction: 0.005,
};

/// A 20 × 30 m room, FOH at (10, 20) facing the stage at y = 0.
fn room() -> PlanSpec {
    PlanSpec {
        outline: vec![[0.0, 0.0], [20.0, 0.0], [20.0, 30.0], [0.0, 30.0]],
        markers: vec![Marker {
            name: "FOH".into(),
            kind: MarkerKind::Foh,
            at: [10.0, 20.0, 0.0],
            heading_deg: Some(-90.0),
        }],
        ..ac2_proto::samples::plan()
    }
}

/// Direct sound at 37.5 ms, then reflections `(ms after it, dB, path)`.
const REFLECTIONS: [(f64, f64, &[Surface]); 7] = [
    (4.2, -2.0, &[Surface::Floor]),
    (9.8, -5.0, &[Surface::Wall { edge: 2 }]),
    (14.5, -8.0, &[Surface::Wall { edge: 3 }]),
    (16.0, -12.0, &[Surface::Ceiling]),
    (23.0, -14.0, &[Surface::Wall { edge: 1 }]),
    (31.0, -22.0, &[Surface::Wall { edge: 2 }, Surface::Ceiling]),
    (44.0, -27.0, &[Surface::Wall { edge: 0 }, Surface::Floor]),
];

fn sim() -> SimState {
    let mut s = ac2_proto::samples::sim_state();
    let arrival = |ms: f64, db: f64, path: &[Surface]| {
        let e = 0.01 * 10f64.powf(db / 10.0) as f32;
        SimArrival {
            time: Seconds(0.0375 + ms / 1000.0),
            energy: vec![e, e],
            path: path.to_vec(),
        }
    };
    let r = &mut s.sources[0].receivers[0];
    r.arrivals = std::iter::once(arrival(0.0, 0.0, &[]))
        .chain(REFLECTIONS.iter().map(|&(ms, db, p)| arrival(ms, db, p)))
        .collect();
    s
}

/// A measured response: the direct sound at 0, the reflections near where the simulation
/// puts them (a little late, as a real room's are), and a decaying tail under them.
fn frame() -> IrFrame {
    let dt = 1e-4;
    let n = 800;
    let mut noise = 0x2545_f491_u32;
    let linear = (0..n)
        .map(|i| {
            let t = -0.005 + i as f64 * dt;
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            let r = (f64::from(noise) / f64::from(u32::MAX)) * 2.0 - 1.0;
            let mut v = if t >= 0.0 {
                0.05 * r * (-t / 0.03).exp()
            } else {
                0.002 * r
            };
            if t.abs() < dt / 2.0 {
                v = 1.0;
            }
            for &(ms, db, _) in &REFLECTIONS {
                if (t - (ms + 0.4) / 1000.0).abs() < dt / 2.0 {
                    v += 10f64.powf(db / 20.0) * if ms > 20.0 { -1.0 } else { 1.0 };
                }
            }
            v as f32
        })
        .collect();
    IrFrame {
        meas: MeasId(1),
        meta: IrMeta {
            sample_rate: Hz(48_000.0),
            t0: Seconds(-0.005),
            dt: Seconds(dt),
            inserted_delay: Seconds(0.0),
        },
        linear,
        etc: None,
    }
}

#[test]
fn ir_sim_overlay() {
    let name = "ir_sim_overlay";
    let Some(gpu) = gpu(name) else {
        return;
    };
    let theme = Theme::dark();
    let view = ViewState::default();
    let size = Viewport {
        width: 900.0,
        height: 380.0,
    };
    let mut sc = ac2_scene::ir::ir_scene(
        &frame(),
        theme.traces[0],
        None,
        &ac2_scene::ir::IrCtx {
            status: &Status::default(),
            view: &view,
            axes: &view.ir.axes,
            style: Style {
                chrome: view.chrome,
                theme: &theme,
            },
            size,
        },
    );
    let pos = TracePosition {
        label: "P1".into(),
        at: [5.0, 12.0, 1.2],
        plan: "Hall".into(),
        walk: WalkId(2),
    };
    let p = room();
    let marks = reflection_marks(&sim(), Some(&p), &pos).expect("heard at P1");
    assert_eq!(marks.marks.len(), REFLECTIONS.len());
    draw_marks(&mut sc, marks, &theme);
    let mut r = renderer(gpu);
    let img = render_on(gpu, &mut r, &sc.scene, 1.0, theme.background);
    golden(name, &img, TEXT);
}
