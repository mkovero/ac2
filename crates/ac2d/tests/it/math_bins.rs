//! A transfer math over live transfers at the `bins` resolution digs down as they do: its
//! `tf` lists their common layout whether or not anyone looks, its stages arrive once
//! subscribed, and each bin is the operands' own bins of that stage combined. Any operand
//! that has no such bins (stored, at a grid resolution, another LF window, left out, stopped)
//! keeps the math on the grid.
#![allow(clippy::unwrap_used)]

use crate::bin_streams::{bins, column, covering, layout, stage, tf_topic};
use crate::common;
use crate::transfer::tf;

use std::collections::HashMap;
use std::time::Duration;

use ac2_audio::fake::FakePath;
use ac2_audio::{FakeBackend, FakeConfig, FakeDriver};
use ac2_core::average::coherence_weight;
use ac2_proto::frame::{BinsFrame, Frame, OperandStatus, ValidityMask};
use ac2_proto::model::{
    AverageMethod, GeneratorDesired, GeneratorSettings, LfWindow, MathConfig, MathDomain, MathExpr,
    MathOp, MeasConfig, MeasKind, Operand, Resolution, Signal, TransferResolution,
};
use ac2_proto::topic::Topic;
use ac2_proto::units::{Dbfs, MeasId, Seconds};
use ac2_proto::{Command, Subscription};
use ac2d::{Daemon, Handle};
use common::*;
use num_complex::Complex64;

/// Input 1 hears the speaker +3 dB one acoustic delay late, input 2 −3 dB two late, input 3
/// nothing, input 4 −3 dB five late.
fn backend() -> FakeBackend {
    let db = |g: f32| 10f32.powf(g / 20.0);
    FakeBackend::new(FakeConfig {
        sample_rate: FS,
        block_frames: BLOCK,
        inputs: 5,
        outputs: 2,
        drive: ac2_audio::fake::FakeDrive::Manual,
        seed: 5,
        paths: vec![
            FakePath::loopback(0, 0, LOOP_DELAY),
            FakePath::acoustic(0, 1, LOOP_DELAY + ACOUSTIC_DELAY, vec![db(3.0)], 1e-5),
            FakePath::acoustic(0, 2, LOOP_DELAY + 2 * ACOUSTIC_DELAY, vec![db(-3.0)], 1e-5),
            FakePath::acoustic(0, 4, LOOP_DELAY + 5 * ACOUSTIC_DELAY, vec![db(-3.0)], 1e-5),
        ],
        ..FakeConfig::default()
    })
    .unwrap()
}

fn seat(name: &str, input: u16, resolution: TransferResolution, lf: LfWindow) -> MeasConfig {
    let mut m = transfer(name);
    if let MeasKind::Transfer { config } = &mut m.kind {
        config.measurement_input = input;
        config.resolution = resolution;
        config.lf_window = lf;
    }
    m
}

fn live(m: u32) -> Operand {
    Operand::Meas { meas: MeasId(m) }
}

fn math(name: &str, expr: MathExpr) -> MeasConfig {
    MeasConfig {
        scope: ac2_proto::model::MeasScope::Session,
        name: name.into(),
        kind: MeasKind::Math {
            config: MathConfig::of(
                ac2_proto::model::TraceOwner::Imported,
                MathDomain::Transfer,
                expr,
            ),
        },
    }
}

fn minus(a: Operand, b: Operand) -> MathExpr {
    MathExpr::Binary {
        a,
        op: MathOp::Subtract,
        b,
    }
}

struct Rig {
    h: Handle,
    c: Client,
    sub: Sub,
    d: FakeDriver,
}

/// Seats as `(name, input, resolution, LF window, delay in acoustic delays)`, measurements
/// 1, 2, … in order, started, with pink noise playing.
fn rig(name: &str, seats: &[(u16, TransferResolution, LfWindow, u32)], prefixes: &[&[u8]]) -> Rig {
    init_log();
    let b = backend();
    let mut cfg = config(b.clone(), inproc(name));
    cfg.lease_expiry = Duration::from_secs(60);
    let h = Daemon::start(cfg).unwrap();
    let (mut c, sub) = connect(&h, prefixes);
    let mut s = session(false);
    s.input_channels = vec![0, 1, 2, 3, 4];
    c.ok(Command::SessionOpen { config: s });
    for (i, (input, res, lf, n)) in seats.iter().enumerate() {
        let m = MeasId(i as u32 + 1);
        c.ok(Command::MeasCreate {
            config: seat(&format!("Seat {}", i + 1), *input, *res, *lf),
        });
        c.ok(Command::DelaySet {
            meas: m,
            delay: Seconds(f64::from(n * ACOUSTIC_DELAY) / f64::from(FS)),
        });
        c.ok(Command::MeasStart { meas: m });
    }
    let tok = c.acquire();
    c.ok(Command::GenSet {
        lease_token: tok,
        desired: GeneratorDesired {
            settings: GeneratorSettings {
                signal: Signal::Pink,
                level: Dbfs(-20.0),
                band: None,
                outputs: vec![0],
            },
            armed: true,
            firing: true,
        },
    });
    let d = driver(&b);
    Rig { h, c, sub, d }
}

fn create(r: &mut Rig, config: MeasConfig) -> MeasId {
    let id = r.c.measurement(Command::MeasCreate { config }).id;
    r.c.ok(Command::MeasStart { meas: id });
    id
}

const STD: LfWindow = LfWindow::Standard;
const BINS: TransferResolution = TransferResolution::Bins;

/// A bin of a wire frame as a complex response.
fn z(f: &BinsFrame, i: usize) -> Complex64 {
    Complex64::from_polar(
        10f64.powf(f64::from(f.mag[i]) / 20.0),
        f64::from(f.phase[i]).to_radians(),
    )
}

/// The math `A − B` at bin `i` from the operands' own stage frames, each referred from its
/// own inserted delay to the math's `to` at frequency `f`, as the grid result is.
fn difference(a: &BinsFrame, b: &BinsFrame, i: usize, to: f64, f: f64) -> Complex64 {
    let rot = |d: f64| Complex64::from_polar(1.0, -std::f64::consts::TAU * f * (d - to));
    z(a, i) * rot(a.meta.delay.0) - z(b, i) * rot(b.meta.delay.0)
}

fn bin_hz(f: &BinsFrame, i: usize) -> f64 {
    f64::from(f.meta.first_bin + i as u32) * f.meta.bin_hz.0
}

/// An eligible math lists the layout unviewed and forms no stage; a subscribed stage is the
/// per-bin difference of the operands' own stages, and a bin alone in its 1/96 column equals
/// that column; a stopped operand drops the layout.
#[test]
fn a_difference_of_live_bins_digs_down() {
    let mut r = rig(
        "math-bins",
        &[(1, BINS, STD, 1), (2, BINS, STD, 2)],
        &[b"d/3/tf"],
    );
    let m = create(&mut r, math("A - B", minus(live(1), live(2))));
    assert_eq!(m, MeasId(3));
    run(&mut r.d, 1.5);
    let end = end_sample(&r.d);
    let got = covering(&r.sub, end, 0, &[tf_topic(3)]);
    let l = layout(&got[&tf_topic(3)]);
    assert_eq!(l.stages.len(), 3);

    for t in [0, 1]
        .map(|s| [stage(3, s), stage(1, s), stage(2, s)])
        .concat()
    {
        r.sub.subscribe(Subscription::Topic(t));
    }
    // Subscriptions land asynchronously: audio runs until the math's stage frames come.
    let first = (0..100)
        .find_map(|_| {
            run(&mut r.d, 0.1);
            r.sub
                .frame(Duration::from_millis(100), |f| f.topic() == stage(3, 0))
        })
        .expect("a math stage frame");
    // The first frame its topic ever carried: none was formed while nobody looked.
    assert_eq!(first.stamp.seq, 1);
    assert!(bins(&first).meta.blocks > 0);
    run(&mut r.d, 1.0);
    let end = end_sample(&r.d);
    let topics = [
        tf_topic(3),
        stage(3, 0),
        stage(3, 1),
        stage(1, 0),
        stage(1, 1),
        stage(2, 0),
        stage(2, 1),
    ];
    // The newest of each: frames the math formed before its operands took the last audio
    // are followed by those formed after.
    let mut got = covering(&r.sub, end, 0, &topics);
    while let Some(f) = r
        .sub
        .frame(Duration::from_millis(500), |f| topics.contains(&f.topic()))
    {
        got.insert(f.topic(), f);
    }
    let t = &got[&tf_topic(3)];
    let to = tf(t).meta.delay.0;
    let mut checked = 0;
    for s in 0..2u8 {
        let (mf, af, bf) = (&got[&stage(3, s)], &got[&stage(1, s)], &got[&stage(2, s)]);
        let (m, a, b) = (bins(mf), bins(af), bins(bf));
        assert_eq!(mf.stamp.audio_sample, t.stamp.audio_sample);
        assert_eq!(mf.stamp.config_rev, t.stamp.config_rev);
        assert_eq!(mf.stamp.grid_id, None);
        assert_eq!(m.meta.delay, tf(t).meta.delay);
        assert_eq!(m.meta.nudged, tf(t).meta.nudged);
        assert_eq!(m.meta.blocks, a.meta.blocks.min(b.meta.blocks));
        assert_eq!(m.meta.hop, a.meta.hop.max(b.meta.hop));
        assert_eq!(
            (m.meta.first_bin, m.mag.len()),
            (a.meta.first_bin, a.mag.len())
        );
        assert_ne!(a.meta.delay, b.meta.delay);
        // A difference has no coherence, in its columns or its bins.
        assert!(m.coh.iter().all(|v| v.is_nan()), "stage {s}");
        for i in 0..m.mag.len() {
            if a.mag[i].is_nan() || b.mag[i].is_nan() {
                assert!(m.mag[i].is_nan(), "stage {s} bin {i}");
                continue;
            }
            let want = difference(a, b, i, to, bin_hz(a, i));
            let got = Complex64::from_polar(
                10f64.powf(f64::from(m.mag[i]) / 20.0),
                f64::from(m.phase[i]).to_radians(),
            );
            // Each operand's bin is quantised to 0.01 dB and 0.01°, and so is the result:
            // about 1.2e-3 of each magnitude involved, with margin.
            let scale = z(a, i).norm() + z(b, i).norm();
            assert!(
                (got - want).norm() <= 2e-3 * scale,
                "stage {s} bin {}: {got} {want}",
                m.meta.first_bin as usize + i
            );
            checked += 1;
        }
    }
    assert!(checked > 1500, "{checked}");
    assert!(tf(t).coh.iter().all(|v| v.is_nan()));

    // Stage 0 bin 100 (1 171.9 Hz) and stage 1 bin 110 (322 Hz) are alone in their 1/96
    // columns, where the operands' columns are their bins: the math's column is the same
    // difference, the delays referred at the bin's frequency as the column's estimate is.
    for (s, bin) in [(0u8, 100u32), (1, 110)] {
        let (m, a, b) = (
            bins(&got[&stage(3, s)]),
            bins(&got[&stage(1, s)]),
            bins(&got[&stage(2, s)]),
        );
        let i = (bin - m.meta.first_bin) as usize;
        let c = column(bin_hz(m, i));
        let tt = tf(t);
        assert_eq!(tt.validity[c], ValidityMask::NONE);
        let want = difference(a, b, i, to, bin_hz(m, i));
        let col = Complex64::from_polar(
            10f64.powf(f64::from(tt.mag[c]) / 20.0),
            f64::from(tt.phase[c]).to_radians(),
        );
        let scale = z(a, i).norm() + z(b, i).norm();
        assert!(
            (col - want).norm() <= 2e-3 * scale,
            "stage {s}: {col} {want}"
        );
    }

    // An operand stopping takes the layout off the math's next result.
    r.c.ok(Command::MeasStop { meas: MeasId(2) });
    let f = (0..100)
        .find_map(|_| {
            run(&mut r.d, 0.1);
            r.sub.frame(Duration::from_millis(100), |f| {
                f.topic() == tf_topic(3) && tf(f).meta.bins.is_none()
            })
        })
        .expect("a result without the layout");
    let st = &tf(&f).meta.math.as_ref().unwrap().operands;
    assert_eq!(st[1].status, OperandStatus::Stopped);
    r.h.shutdown();
}

/// Only operands that all have the same stage bins let a math dig down: a stored one, one at
/// 1/96, another LF window, or one left out keeps it on the grid; an average of three live
/// positions at bins lists the layout and keeps its range band on the grid.
#[test]
fn only_operands_with_one_layout_dig_down() {
    let ninety_sixth: TransferResolution = Resolution::NinetySixth.into();
    let mut r = rig(
        "math-bins-not",
        &[
            (1, BINS, STD, 1),
            (2, BINS, STD, 2),
            (3, BINS, STD, 0),
            (2, ninety_sixth, STD, 2),
            (2, BINS, LfWindow::Long, 2),
            (1, BINS, STD, 0),
        ],
        &[b"d/"],
    );
    run(&mut r.d, 1.0);
    let stored = r.c.trace(Command::TraceCapture {
        meas: MeasId(2),
        name: "Seat 2 then".into(),
        slot: None,
    });
    let average = |of: &[u32]| MathExpr::Average {
        of: of.iter().map(|m| live(*m)).collect(),
        method: AverageMethod::Complex,
    };
    let maths = [
        create(&mut r, math("avg", average(&[1, 2, 6]))),
        create(&mut r, math("at 1/96", minus(live(1), live(4)))),
        create(&mut r, math("long", minus(live(1), live(5)))),
        create(&mut r, math("silent", average(&[1, 2, 3]))),
        create(
            &mut r,
            math(
                "stored",
                minus(live(1), Operand::Trace { trace: stored.id }),
            ),
        ),
    ];
    assert_eq!(maths, [7, 8, 9, 10, 11].map(MeasId));
    run(&mut r.d, 1.5);
    let end = end_sample(&r.d);
    let got = covering(&r.sub, end, 0, &[7, 8, 9, 10, 11].map(tf_topic));
    let avg = &got[&tf_topic(7)];
    let l = layout(avg);
    assert_eq!(l.stages.len(), 3);
    assert!(tf(avg).range.is_some(), "the band stays on the grid");
    for m in 8..=11 {
        let t = tf(&got[&tf_topic(m)]);
        assert_eq!(t.meta.bins, None, "math {m}");
        assert!(
            t.validity.contains(&ValidityMask::NONE),
            "math {m} has a result"
        );
    }
    let st = &tf(&got[&tf_topic(10)]).meta.math.as_ref().unwrap().operands;
    assert!(!matches!(st[2].status, OperandStatus::Included), "{st:?}");
    r.h.shutdown();
}

/// Subscribes to stages 0 and 1 of every one of `meas`, runs audio until math `math`'s first
/// stage frame comes and a second more, and returns the newest frame of each stage and of
/// the math's `tf`, all from after the last audio.
fn newest(r: &mut Rig, math: u32, meas: &[u32]) -> HashMap<Topic, Frame> {
    let topics: Vec<Topic> = std::iter::once(tf_topic(math))
        .chain(meas.iter().flat_map(|m| [0, 1].map(|s| stage(*m, s))))
        .collect();
    for t in &topics[1..] {
        r.sub.subscribe(Subscription::Topic(*t));
    }
    // Subscriptions land asynchronously: audio runs until the math's stage frames come.
    (0..100)
        .find_map(|_| {
            run(&mut r.d, 0.1);
            r.sub
                .frame(Duration::from_millis(100), |f| f.topic() == stage(math, 0))
        })
        .expect("a math stage frame");
    run(&mut r.d, 1.0);
    let end = end_sample(&r.d);
    let mut got = covering(&r.sub, end, 0, &topics);
    while let Some(f) = r
        .sub
        .frame(Duration::from_millis(500), |f| topics.contains(&f.topic()))
    {
        got.insert(f.topic(), f);
    }
    got
}

/// A power average (the dialog's default) and a coherence-weighted one of two live positions
/// at bins: each bin is the average of the operands' own bins of its stage, the delays
/// referred at the bin's frequency, and its coherence their mean; a bin an operand has no
/// value at has none.
#[test]
fn an_average_of_live_bins_is_formed_per_bin() {
    let mut r = rig(
        "math-bins-avg",
        &[(1, BINS, STD, 1), (2, BINS, STD, 2)],
        &[b"d/"],
    );
    let average = |method| MathExpr::Average {
        of: vec![live(1), live(2)],
        method,
    };
    for (m, method) in [
        (3, AverageMethod::Power),
        (4, AverageMethod::CoherenceWeighted),
    ] {
        assert_eq!(create(&mut r, math("avg", average(method))), MeasId(m));
    }
    run(&mut r.d, 1.5);
    for (m, method) in [
        (3u32, AverageMethod::Power),
        (4, AverageMethod::CoherenceWeighted),
    ] {
        let got = newest(&mut r, m, &[m, 1, 2]);
        let t = &got[&tf_topic(m)];
        let to = tf(t).meta.delay.0;
        let mut checked = 0;
        for s in 0..2u8 {
            let (mf, a, b) = (
                bins(&got[&stage(m, s)]),
                bins(&got[&stage(1, s)]),
                bins(&got[&stage(2, s)]),
            );
            assert_eq!(got[&stage(m, s)].stamp.audio_sample, t.stamp.audio_sample);
            assert_eq!(mf.mag.len(), a.mag.len());
            for i in 0..mf.mag.len() {
                if a.mag[i].is_nan() || b.mag[i].is_nan() {
                    assert!(mf.mag[i].is_nan(), "{method:?} stage {s} bin {i}");
                    continue;
                }
                let f = bin_hz(a, i);
                let rot =
                    |d: f64| Complex64::from_polar(1.0, -std::f64::consts::TAU * f * (d - to));
                let (za, zb) = (z(a, i) * rot(a.meta.delay.0), z(b, i) * rot(b.meta.delay.0));
                let want = match method {
                    AverageMethod::Power => Complex64::from_polar(
                        (0.5 * (za.norm_sqr() + zb.norm_sqr())).sqrt(),
                        (za + zb).arg(),
                    ),
                    _ => {
                        let w = |c: f32| coherence_weight(f64::from(c));
                        let (wa, wb) = (w(a.coh[i]), w(b.coh[i]));
                        (za * wa + zb * wb) / (wa + wb)
                    }
                };
                // Quantisation as for the difference: 0.01 dB and 0.01° of each operand
                // and of the result.
                let scale = za.norm() + zb.norm();
                assert!(
                    (z(mf, i) - want).norm() <= 2e-3 * scale,
                    "{method:?} stage {s} bin {i}: {} {want}",
                    z(mf, i)
                );
                let coh = 0.5 * (a.coh[i] + b.coh[i]);
                assert!(
                    (mf.coh[i] - coh).abs() <= 2e-3,
                    "{method:?} stage {s} bin {i}: γ² {} {coh}",
                    mf.coh[i]
                );
                checked += 1;
            }
        }
        assert!(checked > 1500, "{method:?}: {checked}");
    }
    r.h.shutdown();
}

/// The math `A − B` at its column `c`, as a complex response.
fn column_z(t: &Frame, c: usize) -> Complex64 {
    let t = tf(t);
    Complex64::from_polar(
        10f64.powf(f64::from(t.mag[c]) / 20.0),
        f64::from(t.phase[c]).to_radians(),
    )
}

/// A difference's grid column is the difference of its operands' bins, each referred at its
/// bin's frequency, for operands 2.5 ms and 10 ms apart: a column holding one bin equals that
/// bin's difference, and one of several lies among their differences (which only checks the
/// indexing; a centre-referred column would lie there too). Referred at the column
/// centre instead, a column would miss by `2π·(centre − bin)·Δd`, which the test prints.
#[test]
fn a_math_column_is_its_bins_math() {
    let mut r = rig(
        "math-bins-columns",
        &[(1, BINS, STD, 1), (2, BINS, STD, 2), (4, BINS, STD, 5)],
        &[b"d/"],
    );
    for (m, other) in [(4u32, 2u32), (5, 3)] {
        assert_eq!(
            create(&mut r, math("A - B", minus(live(1), live(other)))),
            MeasId(m)
        );
    }
    run(&mut r.d, 1.5);
    for (m, other) in [(4u32, 2u32), (5, 3)] {
        let got = newest(&mut r, m, &[1, other, m]);
        let t = &got[&tf_topic(m)];
        let to = tf(t).meta.delay.0;
        let (mut alone, mut several) = (0, 0);
        let (mut miss, mut centre_miss) = (0f64, 0f64);
        for s in 0..2u8 {
            let (a, b) = (bins(&got[&stage(1, s)]), bins(&got[&stage(other, s)]));
            let delays = (a.meta.delay.0 - b.meta.delay.0).abs();
            assert!(
                (delays - [0.0025, 0.01][(m - 4) as usize]).abs() < 1e-9,
                "{delays}"
            );
            // Bins by the 1/96 column holding them, the stage's outermost columns left out:
            // a column there may blend with the next stage.
            let mut columns: Vec<(usize, Vec<usize>)> = Vec::new();
            for i in 0..a.mag.len() {
                let c = column(bin_hz(a, i));
                match columns.last_mut() {
                    Some((last, v)) if *last == c => v.push(i),
                    _ => columns.push((c, vec![i])),
                }
            }
            let inner = &columns[1..columns.len().saturating_sub(1)];
            for (c, held) in inner {
                if tf(t).validity[*c] != ValidityMask::NONE
                    || held.iter().any(|&i| a.mag[i].is_nan() || b.mag[i].is_nan())
                {
                    continue;
                }
                let col = column_z(t, *c);
                let each: Vec<Complex64> = held
                    .iter()
                    .map(|&i| difference(a, b, i, to, bin_hz(a, i)))
                    .collect();
                let scale = held
                    .iter()
                    .map(|&i| z(a, i).norm() + z(b, i).norm())
                    .fold(0.0, f64::max);
                if let [i] = held[..] {
                    let want = each[0];
                    assert!(
                        (col - want).norm() <= 2e-3 * scale,
                        "math {m} stage {s} column {c}: {col} {want}"
                    );
                    let centre = 1000.0 * 2f64.powf((*c as f64 - 672.0) / 96.0);
                    let off = difference(a, b, i, to, centre);
                    centre_miss = centre_miss.max((off - want).norm() / scale);
                    miss = miss.max((col - want).norm() / scale);
                    alone += 1;
                } else {
                    // Inside the box the bins' differences span, widened by the bulge of the
                    // curve between neighbouring bins (at most half their distance). A sanity
                    // check that the column and its bins are indexed alike, not evidence for
                    // the frequency a column is referred at: the centre lies among the bins,
                    // so a column referred there passes too; the columns of one bin above are.
                    let step = each
                        .windows(2)
                        .map(|w| (w[1] - w[0]).norm())
                        .fold(0.0, f64::max);
                    let tol = 0.5 * step + 2e-3 * scale;
                    let inside = |part: fn(&Complex64) -> f64| {
                        let (lo, hi) = each
                            .iter()
                            .map(part)
                            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
                                (lo.min(v), hi.max(v))
                            });
                        (lo - tol..=hi + tol).contains(&part(&col))
                    };
                    assert!(
                        inside(|z| z.re) && inside(|z| z.im),
                        "math {m} stage {s} column {c}: {col} among {each:?}"
                    );
                    several += 1;
                }
            }
        }
        eprintln!(
            "math {m}: {alone} columns of one bin, {several} of several; a column of one bin \
             misses its bin by up to {miss:.5} of |A| + |B|, at the centre it would by \
             {centre_miss:.4}"
        );
        assert!(alone > 100 && several > 100, "math {m}: {alone} {several}");
    }
    r.h.shutdown();
}
