use ac2_proto::model::{
    DynamicsEdgeCurve, DynamicsEnvelope, DynamicsInput, DynamicsStep, DynamicsTable,
};
use ac2_proto::units::{Db, Dbfs, Hz, Seconds};

use super::draw::DynamicsFrame;
use super::*;
use crate::banner::Status;
use crate::primitives::Viewport;
use crate::theme::Theme;
use crate::view::ViewState;

const LEVELS: [f64; 3] = [-20.0, -14.0, -8.0];
const LENGTHS: [f64; 2] = [0.01, 1.0];

fn secs(v: f64) -> DynamicsTime {
    DynamicsTime::Seconds { value: Seconds(v) }
}

/// A run of 3 levels × 2 lengths, inputs 1 and 2 (zero-based), every row from `row`.
fn table(row: impl Fn(usize, &DynamicsStep) -> DynamicsRow) -> DynamicsTable {
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
                rest: Seconds(4.0),
                recovered: true,
            })
        })
        .collect();
    let rows: Vec<DynamicsRow> = info
        .steps
        .iter()
        .enumerate()
        .map(|(k, s)| row(k, s))
        .collect();
    let input = |n: u16, name: &str| DynamicsInput {
        input: n,
        name: name.into(),
        rows: rows.clone(),
        ..base.inputs[0].clone()
    };
    DynamicsTable {
        info,
        inputs: vec![input(1, "VCA L"), input(2, "VCA R")],
    }
}

/// GR rising with the level, release τ 0.1 s, recovery 1 s, no drift.
fn row(_: usize, s: &DynamicsStep) -> DynamicsRow {
    let mut r = ac2_proto::samples::dynamics_data().inputs[0].rows[0].clone();
    r.gr_end = Db(s.level.0 + 26.0);
    r.release_tau = secs(0.1);
    r.recovery_0_5db = secs(1.0);
    r.attack_tau = secs(0.002);
    r.drift = Db(0.0);
    r
}

fn curve<'a>(t: &'a DynamicsTable, trace: u32, input: u16, name: &str) -> DynCurve<'a> {
    DynCurve {
        id: CurveId {
            rig: None,
            trace: TraceId(trace),
            input,
        },
        name: name.into(),
        color: Color::WHITE,
        table: t,
        envelope: None,
    }
}

fn scene(curves: &[DynCurve<'_>], modes: DynamicsModes) -> DynamicsScene {
    let frame = DynamicsFrame {
        curves,
        modes,
        pointer: None,
        missing: "drag a dynamics run here",
    };
    dynamics_scene(
        &frame,
        &Status::default(),
        &ViewState::default(),
        &Theme::dark(),
        Viewport::new(900.0, 500.0),
    )
}

#[test]
fn a_is_the_first_input_and_b_the_same_runs_second() {
    let t = table(row);
    let ids = curve_ids(None, TraceId(1), &t);
    let modes = DynamicsModes::default();
    assert_eq!(resolve(&modes, &ids), (Some(0), Some(1)));
    // One input per trace: B is the next trace's first.
    let other = CurveId {
        trace: TraceId(2),
        ..ids[0]
    };
    let ids = vec![ids[0], other];
    assert_eq!(resolve(&modes, &ids), (Some(0), Some(1)));
    assert_eq!(resolve(&modes, &ids[..1]), (Some(0), None));
    // A curve no longer on the pane falls back; B off is none.
    let gone = DynamicsModes {
        a: Side::Curve(CurveId {
            trace: TraceId(9),
            ..ids[0]
        }),
        b: Side::Off,
        ..modes
    };
    assert_eq!(resolve(&gone, &ids), (Some(0), None));
    // B cycles through none after the last curve; A wraps.
    assert_eq!(cycle(Side::Default, Some(1), &ids, true), Side::Off);
    assert_eq!(cycle(Side::Off, None, &ids, true), Side::Curve(ids[0]));
    assert_eq!(
        cycle(Side::Default, Some(1), &ids, false),
        Side::Curve(ids[0])
    );
}

#[test]
fn programmes_match_rests_differ_or_programmes_differ() {
    let a = table(row);
    let mut b = table(row);
    assert_eq!(programmes(&a.info, &b.info), Match::Same);
    // Within a sample is the same rest.
    b.info.steps[2].rest = Seconds(4.0 + 0.5 / 48_000.0);
    assert_eq!(programmes(&a.info, &b.info), Match::Same);
    b.info.steps[2].rest = Seconds(4.1);
    assert_eq!(programmes(&a.info, &b.info), Match::RestsDiffer);
    let ca = curve(&a, 1, 1, "VCA L · Run 1");
    let cb = curve(&b, 2, 1, "VCA L · Run 2");
    let s = scene(&[ca.clone(), cb], DynamicsModes::default());
    assert_eq!(s.note.as_deref(), Some(RESTS_DIFFER));
    assert_eq!(s.cell_texts[0][1], "0.00 dB");

    b.info.carrier = Hz(2000.0);
    assert_eq!(programmes(&a.info, &b.info), Match::Differ);
    let cb = curve(&b, 2, 1, "VCA L · Run 2");
    let s = scene(&[ca, cb], DynamicsModes::default());
    assert_eq!(s.note.as_deref(), Some(PROGRAMMES_DIFFER));
    // A alone: its own GR, not a difference.
    assert_eq!(s.cell_texts[0][1], "18.0 dB");
    assert!(s.caption.ends_with("programmes differ"), "{}", s.caption);
}

#[test]
fn the_pick_is_values_with_a_default_and_a_tolerance() {
    let t = table(row);
    let p = default_pick(&t.info).expect("a pick");
    assert_eq!((p.level.0, p.burst.0), (-14.0, 1.0));
    assert_eq!(pick_text(p), "−14 dBFS · 1 s");
    assert_eq!(step_of(&t.info, p), Some(3));
    let near = DynamicsStepPick {
        level: Dbfs(-14.4),
        burst: Seconds(1.09),
    };
    assert_eq!(step_of(&t.info, near), Some(3));
    let far = DynamicsStepPick {
        level: Dbfs(-13.4),
        burst: Seconds(1.0),
    };
    assert_eq!(step_of(&t.info, far), None);
    // Z / X step through the grid and stop at its ends.
    let up = step_pick(&t.info, p, 1, -1);
    assert_eq!(pick_text(up), "−8 dBFS · 10 ms");
    assert_eq!(step_pick(&t.info, up, 5, -5), up);
}

#[test]
fn words_never_enter_a_difference_and_tolerances_shade() {
    let period = 1e-3;
    let gr = |v| Value::Num(v, Unit::Db);
    let beyond = |d| matches!(d, Diff::Num(_, _, true));
    assert!(beyond(diff(gr(7.5), gr(7.1), period)));
    assert!(!beyond(diff(gr(7.5), gr(7.3), period)));
    // Times: the larger of 25 % and one period.
    let t = |v| Value::Num(v, Unit::Seconds);
    assert!(!beyond(diff(t(1.0), t(0.8), period)));
    assert!(beyond(diff(t(1.0), t(0.7), period)));
    assert!(!beyond(diff(t(0.002), t(0.0012), period)));
    // THD: 3 dB, and only between numbers above the floor.
    let l = |v| Value::Num(v, Unit::DbRe);
    assert!(!beyond(diff(l(-40.0), l(-42.5), period)));
    assert!(beyond(diff(l(-40.0), l(-43.5), period)));
    let w = Value::Word(BELOW_FLOOR);
    assert_eq!(diff(w, l(-60.0), period), Diff::CantCompare);
    assert_eq!(diff_text(Diff::CantCompare), "can't compare");
    assert_eq!(
        diff_text(Diff::Num(-0.0021, Unit::Seconds, false)),
        "−2.10 ms"
    );
    assert_eq!(diff_text(Diff::Num(0.3, Unit::Db, false)), "+0.30 dB");

    let r = ac2_proto::samples::dynamics_data().inputs[0].rows[0].clone();
    let text = |m| value_text(value(&r, m), m);
    assert_eq!(text(DynamicsMetric::GrEnd), "7.5 dB");
    assert_eq!(text(DynamicsMetric::AttackTau), "1.80 ms");
    assert_eq!(text(DynamicsMetric::Attack2Db), "≤ 1 period");
    assert_eq!(text(DynamicsMetric::Settle05Db), "can't tell");
    assert_eq!(text(DynamicsMetric::Recovery2Db), "no gain reduction");
    assert_eq!(text(DynamicsMetric::ThdSettled), "≤ floor");
    assert_eq!(text(DynamicsMetric::ReleaseTau), "2.75 s");
    assert_eq!(text(DynamicsMetric::Drift), "−0.06 dB");
}

#[test]
fn a_baseline_moves_beyond_twice_the_tails_residual() {
    // Level −14 dBFS: release τ 0.1 s, recovery 1 s from 12 dB GR: τ_tail = 1 / ln 24
    // ≈ 0.31 s, r ≈ 0.016 dB, moved beyond ≈ 0.13 dB.
    let t = table(|k, s| {
        let mut r = row(k, s);
        r.drift = Db(match k {
            2 => -0.12,
            3 => -0.14,
            _ => 0.0,
        });
        r
    });
    let b = baselines(&t.info, &t.inputs[0].rows);
    assert_eq!(b[2], Baseline::Steady);
    assert_eq!(b[3], Baseline::Moved(-0.14));
    assert_eq!(baseline_text(b[3]), "baseline moved −0.14 dB");
    assert_eq!(flagged(&b), vec![false, false, false, true, true, false]);

    // The release τ alone reads the fast stage: a 5 s tail is in the recovery.
    let slow = table(|k, s| {
        let mut r = row(k, s);
        r.recovery_0_5db = secs(15.0);
        r.drift = Db(-0.3);
        r
    });
    let b = baselines(&slow.info, &slow.inputs[0].rows);
    assert!(b.iter().all(|b| *b == Baseline::Steady), "{b:?}");

    // Under 1 dB GR the recovery term is not used; with no release either, can't tell.
    let none = table(|k, s| {
        let mut r = row(k, s);
        r.gr_end = Db(0.8);
        r.release_tau = DynamicsTime::CantTell;
        r
    });
    let b = baselines(&none.info, &none.inputs[0].rows);
    assert!(b.iter().all(|b| *b == Baseline::CantTell), "{b:?}");
    let mut unrecovered = table(row);
    unrecovered.info.steps[0].recovered = false;
    assert_eq!(
        baselines(&unrecovered.info, &unrecovered.inputs[0].rows)[0],
        Baseline::NotRecovered
    );
}

#[test]
fn steps_show_a_minus_b_and_flagged_steps_cant_compare() {
    let mut t = table(|k, s| {
        let mut r = row(k, s);
        r.drift = Db(if k == 3 { -0.85 } else { 0.0 });
        r
    });
    t.inputs[1].rows[1].gr_end = Db(t.inputs[1].rows[1].gr_end.0 - 0.5);
    let a = curve(&t, 1, 1, "VCA L · Run 1");
    let b = curve(&t, 1, 2, "VCA R · Run 1");
    let s = scene(&[a, b], DynamicsModes::default());
    assert_eq!((s.a, s.b), (Some(0), Some(1)));
    assert_eq!(s.note, None);
    assert_eq!(
        s.caption,
        "VCA L · Run 1 − VCA R · Run 1 · GR end · −14 dBFS · 1 s"
    );
    // Rows from the loudest level; at −20 dBFS · 1 s A is 0.5 dB above B: shaded.
    assert_eq!(s.cell_texts.len(), 3);
    assert_eq!(s.cell_texts[2], vec!["0.00 dB", "+0.50 dB"]);
    let shades = s.scene.layers[1]
        .rects
        .iter()
        .filter(|r| r.color == Theme::dark().unresolved_shade)
        .count();
    assert_eq!(shades, 1);
    // −14 dBFS · 1 s moved; it and the next step (−8 dBFS · 10 ms) can't compare.
    assert_eq!(s.cell_texts[1][1], CANT_COMPARE);
    assert_eq!(s.cell_texts[0][0], CANT_COMPARE);
    assert_eq!(s.cells.len(), 6);
    // The readout: the picked step, A and B, every metric, the baselines.
    assert_eq!(s.readout[0], "−14 dBFS · 1 s · step 4 of 6");
    assert_eq!(s.readout[1], "A VCA L · Run 1");
    assert_eq!(s.readout[2], "B VCA R · Run 1");
    assert_eq!(s.readout.len(), 3 + 14 + 2);
    assert_eq!(s.readout[3], "GR end  12.0 dB · 12.0 dB · Δ can't compare");
    assert_eq!(s.readout[17], "A baseline moved −0.85 dB");
}

#[test]
fn the_table_lists_a_b_and_their_difference_per_step() {
    let t = table(row);
    let a = curve(&t, 1, 1, "VCA L · Run 1");
    let b = curve(&t, 1, 2, "VCA R · Run 1");
    let modes = DynamicsModes {
        view: DynamicsView::Table,
        ..DynamicsModes::default()
    };
    let s = scene(&[a, b], modes);
    assert_eq!(s.table.len(), 1 + 6 * 3);
    assert_eq!(s.table[0][2], "GR end");
    assert_eq!(s.table[1][..3], ["−20 dBFS · 10 ms", "A", "6.0 dB"]);
    assert_eq!(s.table[3][..3], ["", "A − B", "0.00 dB"]);
    assert!(s.readout.is_empty());
}

fn envelope(step: u32, gr: f32) -> DynamicsEnvelope {
    let edge = |ts: &[f32]| DynamicsEdgeCurve {
        t: ts.to_vec(),
        gr_db: ts.iter().map(|_| gr).collect(),
        h_db: [
            ts.iter().map(|_| -40.0).collect(),
            ts.iter().map(|_| f32::NAN).collect(),
            ts.iter().map(|_| f32::NAN).collect(),
            ts.iter().map(|_| f32::NAN).collect(),
        ],
    };
    let mut start = edge(&[-0.004, 0.0, 0.0005, 0.1, 0.9]);
    // Blind in the middle: THD is NaN there.
    start.h_db[0][2] = f32::NAN;
    DynamicsEnvelope {
        input: 1,
        step,
        start,
        end: edge(&[0.0005, 0.01, 1.0, 3.0]),
    }
}

#[test]
fn envelopes_share_one_scale_and_thd_has_gaps_where_blind() {
    let t = table(row);
    let (ea, eb) = (envelope(3, 12.0), envelope(3, 4.0));
    let mut a = curve(&t, 1, 1, "VCA L · Run 1");
    let mut b = curve(&t, 1, 2, "VCA R · Run 1");
    a.envelope = Some(&ea);
    let modes = DynamicsModes {
        view: DynamicsView::Envelopes,
        ..DynamicsModes::default()
    };
    let s = scene(&[a.clone(), b.clone()], modes);
    assert_eq!(s.plots.len(), 2);
    assert_eq!(
        s.readout,
        vec!["−14 dBFS · 1 s", "VCA R · Run 1  reading envelopes…"]
    );
    b.envelope = Some(&eb);
    let s = scene(&[a.clone(), b.clone()], modes);
    assert_eq!(s.legend.len(), 2);
    let lines = &s.scene.layers[1].polylines;
    // Both curves on one scale: the louder is higher (smaller y) in both plots.
    let y = |l: &crate::primitives::Polyline| l.points[1][1];
    assert!(y(&lines[0]) < y(&lines[2]) && y(&lines[1]) < y(&lines[3]));
    assert_eq!(thd_db([-40.0, f32::NAN, f32::NAN, f32::NAN]), -40.0);
    assert!(thd_db([f32::NAN; 4]).is_nan());
    let thd = scene(&[a, b], DynamicsModes { thd: true, ..modes });
    let onset = &thd.scene.layers[1].polylines[0];
    assert!(onset.points[2][1].is_nan(), "a gap, never a zero");
    assert!(onset.points[1][1].is_finite());
    assert!(thd.caption.starts_with("THD envelopes"));
}

#[test]
fn release_draws_bounds_hollow_and_every_level_on_a_ramp() {
    let t = table(|k, s| {
        let mut r = row(k, s);
        if k == 0 {
            r.release_tau = DynamicsTime::WithinPeriod;
        }
        r
    });
    let a = curve(&t, 1, 1, "VCA L · Run 1");
    let modes = DynamicsModes {
        view: DynamicsView::Release,
        step: Some(DynamicsStepPick {
            level: Dbfs(-20.0),
            burst: Seconds(0.01),
        }),
        ..DynamicsModes::default()
    };
    let s = scene(std::slice::from_ref(&a), modes);
    assert_eq!(
        s.readout[1],
        "VCA L · Run 1  τ ≤ 1 period · recovery 1.00 s"
    );
    // One hollow ring for the bound (17 points round).
    let rings = s.scene.layers[1]
        .polylines
        .iter()
        .filter(|l| l.points.len() == 17)
        .count();
    assert_eq!(rings, 1);
    let all = scene(
        std::slice::from_ref(&a),
        DynamicsModes {
            all_levels: true,
            ..modes
        },
    );
    let names: Vec<&str> = all.legend.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["−20 dBFS", "−14 dBFS", "−8 dBFS"]);
    assert_ne!(all.legend[0].color, all.legend[2].color);
}

#[test]
fn an_empty_pane_says_what_to_do() {
    let s = scene(&[], DynamicsModes::default());
    assert_eq!(s.note.as_deref(), Some("drag a dynamics run here"));
    assert!(s.cells.is_empty());
}
