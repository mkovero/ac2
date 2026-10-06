//! Math channels on a fake rig with three mic inputs (+3 dB, −3 dB and silent paths, the
//! second arriving `ACOUSTIC_DELAY` samples after the first):
//!
//! - the live average of three positions reads the analytic power average, the silent
//!   position is left out and said to be, and its capture names the positions;
//! - `Seat 1 ÷ Seat 2` reads +6 dB with the second position's later arrival in its phase,
//!   follows an edit to `×` (0 dB), and freezes into a trace on capture;
//! - a capture of Seat 1 plus live Seat 2 predicts their acoustic sum (the comb of two
//!   arrivals), and the stored operand cannot be deleted under it;
//! - `Spectrum 1 − Spectrum 2` reads 6 dB on the spectrum stream;
//! - mixed kinds, levels with ÷ and an imported operand in a sum are refused; operands keep
//!   their kind and grid while named.
#![allow(clippy::unwrap_used)]

mod common;

use std::time::Duration;

use ac2_audio::fake::FakePath;
use ac2_audio::{FakeBackend, FakeConfig, FakeDriver};
use ac2_proto::frame::{Frame, FrameData, OperandStatus, ProtectionFlags, TfFrame, ValidityMask};
use ac2_proto::model::{
    AverageMethod, GeneratorDesired, GeneratorSettings, ImportFormat, ImportRole, MathConfig,
    MathDomain, MathExpr, MathOp, MeasConfig, MeasKind, Operand, PhaseBasis, Signal,
    SpectrumConfig, TraceKind, TraceSource,
};
use ac2_proto::units::{Blob, Dbfs, LeaseToken, MeasId, Seconds, TraceId};
use ac2_proto::{Command, ErrorCode, ReplyBody};
use ac2d::Daemon;
use common::*;

const T: Duration = Duration::from_secs(10);

fn rig3() -> FakeBackend {
    let db = |g: f32| 10f32.powf(g / 20.0);
    FakeBackend::new(FakeConfig {
        sample_rate: FS,
        block_frames: BLOCK,
        inputs: 4,
        outputs: 2,
        drive: ac2_audio::fake::FakeDrive::Manual,
        seed: 11,
        paths: vec![
            FakePath::loopback(0, 0, LOOP_DELAY),
            FakePath::acoustic(0, 1, LOOP_DELAY + ACOUSTIC_DELAY, vec![db(3.0)], 1e-5),
            FakePath::acoustic(0, 2, LOOP_DELAY + 2 * ACOUSTIC_DELAY, vec![db(-3.0)], 1e-5),
            // Input 3 hears nothing: its measurement has NO SIGNAL.
        ],
        ..FakeConfig::default()
    })
    .unwrap()
}

fn tf_on(name: &str, input: u16) -> MeasConfig {
    let mut m = transfer(name);
    if let MeasKind::Transfer { config } = &mut m.kind {
        config.measurement_input = input;
    }
    m
}

fn meas(m: u32) -> Operand {
    Operand::Meas { meas: MeasId(m) }
}

fn math(name: &str, domain: MathDomain, expr: MathExpr) -> MeasConfig {
    MeasConfig {
        name: name.into(),
        kind: MeasKind::Math {
            config: MathConfig::of(domain, expr),
        },
    }
}

fn average(name: &str, members: &[u32]) -> MeasConfig {
    MeasConfig {
        name: name.into(),
        kind: MeasKind::Math {
            config: MathConfig::power_average(
                MathDomain::Transfer,
                members.iter().map(|m| meas(*m)).collect(),
            ),
        },
    }
}

fn binary(name: &str, domain: MathDomain, a: Operand, op: MathOp, b: Operand) -> MeasConfig {
    math(name, domain, MathExpr::Binary { a, op, b })
}

fn grid_freq(i: usize) -> f64 {
    1000.0 * 2f64.powf((i as f64 - 240.0) / 48.0)
}

fn tf(f: &Frame) -> &TfFrame {
    match &f.data {
        FrameData::Tf(t) => t,
        _ => unreachable!(),
    }
}

fn created(r: ReplyBody) -> MeasId {
    match r {
        ReplyBody::Measurement(m) => m.id,
        other => panic!("{other:?}"),
    }
}

/// Plays pink noise for `seconds`, keeping the lease.
fn play(d: &mut FakeDriver, c: &mut Client, tok: LeaseToken, seconds: f64) {
    let mut left = seconds;
    while left > 0.0 {
        let s = left.min(0.5);
        run(d, s);
        c.ok(Command::GenRefresh { lease_token: tok });
        left -= s;
    }
}

/// The newest frame of the stream `pred` picks that covers the audio played so far.
fn newest(sub: &Sub, d: &FakeDriver, pred: impl Fn(&Frame) -> bool) -> Frame {
    let end = end_sample(d);
    sub.frame(T, |f| pred(f) && f.stamp.audio_sample.0 + 1 >= end)
        .expect("frame")
}

fn of_meas(f: &Frame, m: u32) -> bool {
    match &f.data {
        FrameData::Tf(t) => t.meas == MeasId(m),
        FrameData::Spec(s) => s.meas == MeasId(m),
        _ => false,
    }
}

/// Columns of the 48 ppo grid from 300 Hz to 5 kHz with a value.
fn band(t: &TfFrame) -> Vec<usize> {
    (0..t.mag.len())
        .filter(|&i| (300.0..=5000.0).contains(&grid_freq(i)))
        .filter(|&i| t.validity[i] == ValidityMask::NONE)
        .collect()
}

struct Rig {
    h: ac2d::Handle,
    c: Client,
    sub: Sub,
    d: FakeDriver,
    tok: LeaseToken,
}

/// Three seats (transfer measurements 1–3 on inputs 1–3, each aligned to its arrival) and
/// pink noise playing; `prefixes` subscribed.
fn rig(name: &str, prefixes: &[&[u8]]) -> Rig {
    init_log();
    let backend = rig3();
    let mut cfg = config(backend.clone(), inproc(name));
    cfg.lease_expiry = Duration::from_secs(60);
    let h = Daemon::start(cfg).unwrap();
    let (mut c, sub) = connect(&h, prefixes);
    let mut s = session(false);
    s.input_channels = vec![0, 1, 2, 3];
    c.ok(Command::SessionOpen { config: s });
    for (i, input) in [1u16, 2, 3].into_iter().enumerate() {
        c.ok(Command::MeasCreate {
            config: tf_on(&format!("Seat {}", i + 1), input),
        });
    }
    for (meas, n) in [(1u32, 1u32), (2, 2), (3, 1)] {
        c.ok(Command::DelaySet {
            meas: MeasId(meas),
            delay: Seconds(f64::from(n * ACOUSTIC_DELAY) / f64::from(FS)),
        });
        c.ok(Command::MeasStart { meas: MeasId(meas) });
    }
    let tok = match c.ok(Command::GenAcquire { force: false }) {
        ReplyBody::Lease(l) => l.lease_token,
        other => panic!("{other:?}"),
    };
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
    let d = driver(&backend);
    Rig { h, c, sub, d, tok }
}

#[test]
fn power_average_of_three_positions() {
    let mut r = rig("math-avg", &[b"d/4/tf"]);
    let c = &mut r.c;

    // Refused shapes: one operand, a duplicate, an unknown operand.
    for bad in [&[1u32][..], &[1, 1], &[1, 9]] {
        let e = c
            .call(Command::MeasCreate {
                config: average("bad", bad),
            })
            .unwrap_err();
        assert!(
            matches!(e.code, ErrorCode::Invalid | ErrorCode::NotFound),
            "{bad:?}: {e:?}"
        );
    }

    match c.ok(Command::MeasCreate {
        config: average("Audience", &[1, 2, 3]),
    }) {
        ReplyBody::Measurement(m) => {
            assert_eq!(m.id, MeasId(4));
            // Its grid is its operands', known before it runs.
            assert!(m.grid_id.is_some());
            assert!(m.delay.is_none());
        }
        other => panic!("{other:?}"),
    }
    c.ok(Command::MeasStart { meas: MeasId(4) });
    play(&mut r.d, c, r.tok, 3.5);
    let f = newest(&r.sub, &r.d, |f| of_meas(f, 4));
    let t = tf(&f);
    assert_eq!(f.stamp.protection, ProtectionFlags::NONE);
    let a = t.meta.math.as_ref().expect("math metadata");
    let st: Vec<OperandStatus> = a.operands.iter().map(|m| m.status).collect();
    assert_eq!(st[..2], [OperandStatus::Included, OperandStatus::Included]);
    assert!(
        matches!(st[2], OperandStatus::Refused { protection } if protection.contains(ProtectionFlags::NO_SIGNAL)),
        "{st:?}"
    );
    assert_eq!(a.included(), 2);
    // The phase is referred to the first operand's delay.
    assert_eq!(
        t.meta.delay,
        Seconds(f64::from(ACOUSTIC_DELAY) / f64::from(FS))
    );
    let want = 10.0 * ((10f64.powf(0.3) + 10f64.powf(-0.3)) / 2.0).log10();
    let cols = band(t);
    assert!(cols.len() > 100, "valid columns {}", cols.len());
    for &i in &cols {
        assert!(
            (f64::from(t.mag[i]) - want).abs() < 0.3,
            "{:.0} Hz: {} vs {want}",
            grid_freq(i),
            t.mag[i]
        );
    }

    // Captured: a transfer trace on the average's grid naming the two positions averaged.
    let trace = match c.ok(Command::TraceCapture {
        meas: MeasId(4),
        name: "audience".into(),
        slot: None,
    }) {
        ReplyBody::Trace(t) => t,
        other => panic!("{other:?}"),
    };
    match &trace.source {
        TraceSource::Math {
            meas,
            expr,
            operands,
            phase,
            ..
        } => {
            assert_eq!(*meas, MeasId(4));
            assert!(matches!(
                expr,
                MathExpr::Average {
                    method: AverageMethod::Power,
                    ..
                }
            ));
            let names: Vec<&str> = operands.iter().map(|m| m.name.as_str()).collect();
            assert_eq!(names, ["Seat 1", "Seat 2"]);
            assert_eq!(*phase, PhaseBasis::SharedTimeBase);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(trace.id, TraceId(1));
    assert!(
        trace.source.shared_epoch().is_some(),
        "an average keeps the epoch"
    );

    // Operands stay transfers on the channel's grid, and are not deleted under it.
    let e = c.call(Command::MeasDelete { meas: MeasId(2) }).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");
    let mut other_grid = tf_on("Seat 2", 2);
    if let MeasKind::Transfer { config } = &mut other_grid.kind {
        config.grid.ppo = 24;
        config.grid.k_min = -120;
        config.grid.k_max = 119;
    }
    let e = c
        .call(Command::MeasUpdate {
            meas: MeasId(2),
            config: other_grid,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");
    let e = c.call(Command::MeasReset { meas: MeasId(4) }).unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid, "{e:?}");

    // An operand stopped is left out as stopped; one usable operand is no average.
    c.ok(Command::MeasStop { meas: MeasId(2) });
    play(&mut r.d, c, r.tok, 0.5);
    let f = newest(&r.sub, &r.d, |f| of_meas(f, 4));
    let t = tf(&f);
    let st: Vec<OperandStatus> = t
        .meta
        .math
        .as_ref()
        .unwrap()
        .operands
        .iter()
        .map(|m| m.status)
        .collect();
    assert_eq!(st[1], OperandStatus::Stopped);
    assert!(t.mag.iter().all(|m| m.is_nan()));
    assert!(t.validity.iter().all(|v| *v == ValidityMask::FEW_OPERANDS));
    let e = c
        .call(Command::TraceCapture {
            meas: MeasId(4),
            name: "one".into(),
            slot: None,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");

    // Deleting the channel frees its operands.
    c.ok(Command::MeasDelete { meas: MeasId(4) });
    c.ok(Command::MeasDelete { meas: MeasId(2) });
    r.h.shutdown();
}

#[test]
fn ratio_edit_capture_and_summation() {
    let mut r = rig("math-ratio", &[b"d/4/tf", b"d/5/tf"]);
    let c = &mut r.c;
    let q = created(c.ok(Command::MeasCreate {
        config: binary(
            "Seat 1 ÷ Seat 2",
            MathDomain::Transfer,
            meas(1),
            MathOp::Divide,
            meas(2),
        ),
    }));
    assert_eq!(q, MeasId(4));
    c.ok(Command::MeasStart { meas: q });
    play(&mut r.d, c, r.tok, 3.5);
    let f = newest(&r.sub, &r.d, |f| of_meas(f, 4));
    let t = tf(&f);
    assert_eq!(
        t.meta.delay,
        Seconds(0.0),
        "a ratio states its relative arrival"
    );
    // Seat 2 arrives ACOUSTIC_DELAY after Seat 1: the ratio's phase leads by that much.
    let dt = f64::from(ACOUSTIC_DELAY) / f64::from(FS);
    let cols = band(t);
    assert!(cols.len() > 100, "valid columns {}", cols.len());
    for &i in &cols {
        assert!((t.mag[i] - 6.0).abs() < 0.3, "{} dB", t.mag[i]);
        let off = ac2_traces::columns::wrap_deg(f64::from(t.phase[i]) - 360.0 * grid_freq(i) * dt);
        assert!(off.abs() < 5.0, "{:.0} Hz: {}° off", grid_freq(i), off);
    }

    // Capture freezes it: the trace keeps +6 dB while the channel is edited to ×.
    let frozen = match c.ok(Command::TraceCapture {
        meas: q,
        name: "ratio".into(),
        slot: Some(1),
    }) {
        ReplyBody::Trace(t) => t,
        other => panic!("{other:?}"),
    };
    assert_eq!(frozen.kind, TraceKind::Transfer);
    match &frozen.source {
        TraceSource::Math {
            expr: MathExpr::Binary {
                op: MathOp::Divide, ..
            },
            operands,
            phase: PhaseBasis::SharedTimeBase,
            ..
        } => {
            let names: Vec<&str> = operands.iter().map(|o| o.name.as_str()).collect();
            assert_eq!(names, ["Seat 1", "Seat 2"]);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(frozen.source.shared_epoch(), None, "a ratio is relative");
    c.ok(Command::MeasUpdate {
        meas: q,
        config: binary(
            "Seat 1 × Seat 2",
            MathDomain::Transfer,
            meas(1),
            MathOp::Multiply,
            meas(2),
        ),
    });
    play(&mut r.d, c, r.tok, 1.0);
    let f = newest(&r.sub, &r.d, |f| of_meas(f, 4));
    let t = tf(&f);
    for &i in &band(t) {
        assert!(t.mag[i].abs() < 0.3, "{} dB", t.mag[i]);
    }
    let data = match c.ok(Command::TraceGet { trace: frozen.id }) {
        ReplyBody::TraceData(d) => d,
        other => panic!("{other:?}"),
    };
    for &i in &band(t) {
        assert!((data.mag_db[i] - 6.0).abs() < 0.3, "{}", data.mag_db[i]);
    }

    // Summation prediction: Seat 1 as captured plus Seat 2 live, in one epoch, is the sum of
    // the two arrivals, |g₁ + g₂·e^{−j2πfΔ}|.
    let seat1 = match c.ok(Command::TraceCapture {
        meas: MeasId(1),
        name: "Seat 1 alone".into(),
        slot: Some(2),
    }) {
        ReplyBody::Trace(t) => t,
        other => panic!("{other:?}"),
    };
    let sum = created(c.ok(Command::MeasCreate {
        config: binary(
            "Seat 1 + Seat 2",
            MathDomain::Transfer,
            Operand::Trace { trace: seat1.id },
            MathOp::Add,
            meas(2),
        ),
    }));
    assert_eq!(sum, MeasId(5));
    c.ok(Command::MeasStart { meas: sum });
    play(&mut r.d, c, r.tok, 1.0);
    let f = newest(&r.sub, &r.d, |f| of_meas(f, 5));
    let t = tf(&f);
    assert_eq!(t.meta.delay, Seconds(dt), "referred to the first operand");
    assert!(t.coh.iter().all(|c| c.is_nan()), "a sum has no coherence");
    let (g1, g2) = (10f64.powf(3.0 / 20.0), 10f64.powf(-3.0 / 20.0));
    let mut checked = 0;
    for &i in &band(t) {
        let w = 2.0 * std::f64::consts::PI * grid_freq(i) * dt;
        let want = (g1 * g1 + g2 * g2 + 2.0 * g1 * g2 * w.cos()).sqrt();
        // Away from the shallow dips, where the dB value is steep.
        if want > 0.9 {
            assert!(
                (f64::from(t.mag[i]) - 20.0 * want.log10()).abs() < 0.5,
                "{:.0} Hz: {} vs {}",
                grid_freq(i),
                t.mag[i],
                20.0 * want.log10()
            );
            checked += 1;
        }
    }
    assert!(checked > 50, "{checked}");
    // The stored operand is not deleted under it.
    let e = c
        .call(Command::TraceDelete { trace: seat1.id })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");

    // Refused: an imported operand in a sum (no shared time base), a spectrum operand in
    // transfer math, a math channel as an operand.
    let mut text = String::new();
    for k in 0..=30 {
        let f = 20.0 * 1000f64.powf(f64::from(k) / 30.0);
        text.push_str(&format!("{f} 0.0 0.0\n"));
    }
    let imp = match c.ok(Command::TraceImport {
        file_name: "flat.txt".into(),
        format: ImportFormat::AnalyzerText,
        role: ImportRole::Trace,
        content: Blob(text.into_bytes()),
    }) {
        ReplyBody::Trace(t) => t,
        other => panic!("{other:?}"),
    };
    let e = c
        .call(Command::MeasCreate {
            config: binary(
                "x",
                MathDomain::Transfer,
                meas(1),
                MathOp::Add,
                Operand::Trace { trace: imp.id },
            ),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid, "{e:?}");
    assert!(e.msg.contains("time base"), "{}", e.msg);
    // ÷ with it is fine, and says its phase is of own alignments.
    let own = created(c.ok(Command::MeasCreate {
        config: binary(
            "Seat 1 ÷ flat",
            MathDomain::Transfer,
            meas(1),
            MathOp::Divide,
            Operand::Trace { trace: imp.id },
        ),
    }));
    c.ok(Command::MeasDelete { meas: own });
    let e = c
        .call(Command::MeasCreate {
            config: binary("x", MathDomain::Transfer, meas(1), MathOp::Divide, meas(4)),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid, "{e:?}");
    r.h.shutdown();
}

#[test]
fn spectrum_difference_on_the_spectrum_stream() {
    let mut r = rig("math-spec", &[b"d/6/spec"]);
    let c = &mut r.c;
    for input in [1u16, 2] {
        c.ok(Command::MeasCreate {
            config: MeasConfig {
                name: format!("Spectrum {input}"),
                kind: MeasKind::Spectrum {
                    config: SpectrumConfig::on_input(input),
                },
            },
        });
    }
    // Spectra take − and +, not ÷; spectrum math takes no transfer operand.
    let e = c
        .call(Command::MeasCreate {
            config: binary("x", MathDomain::Spectrum, meas(4), MathOp::Divide, meas(5)),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid, "{e:?}");
    let e = c
        .call(Command::MeasCreate {
            config: binary(
                "x",
                MathDomain::Spectrum,
                meas(4),
                MathOp::Subtract,
                meas(1),
            ),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid, "{e:?}");
    let id = created(c.ok(Command::MeasCreate {
        config: binary(
            "Spectrum 1 − Spectrum 2",
            MathDomain::Spectrum,
            meas(4),
            MathOp::Subtract,
            meas(5),
        ),
    }));
    assert_eq!(id, MeasId(6));
    for m in 4..=6 {
        c.ok(Command::MeasStart { meas: MeasId(m) });
    }
    play(&mut r.d, c, r.tok, 4.0);
    let f = newest(&r.sub, &r.d, |f| of_meas(f, 6));
    let FrameData::Spec(s) = &f.data else {
        panic!("{f:?}");
    };
    let m = s.meta.math.as_ref().expect("math metadata");
    assert_eq!(m.included(), 2);
    assert_eq!(m.phase, PhaseBasis::NoPhase);
    // The display columns between 300 Hz and 5 kHz: the same noise 6 dB apart.
    let grid = ac2_proto::BinColumns::new(f64::from(FS), 65_536, 96);
    let mut diffs: Vec<f32> = grid
        .centres
        .iter()
        .zip(&s.level)
        .filter(|(f, v)| (300.0..=5000.0).contains(*f) && v.is_finite())
        .map(|(_, v)| *v)
        .collect();
    assert!(diffs.len() > 100, "{}", diffs.len());
    diffs.sort_by(f32::total_cmp);
    let median = diffs[diffs.len() / 2];
    assert!((median - 6.0).abs() < 0.5, "median {median} dB");
    // Its capture is a spectrum trace on every bin.
    let t = match c.ok(Command::TraceCapture {
        meas: id,
        name: "diff".into(),
        slot: None,
    }) {
        ReplyBody::Trace(t) => t,
        other => panic!("{other:?}"),
    };
    assert!(matches!(t.kind, TraceKind::Spectrum { .. }), "{:?}", t.kind);
    // An operand keeps its FFT length while named.
    let e = c
        .call(Command::MeasUpdate {
            meas: MeasId(4),
            config: MeasConfig {
                name: "Spectrum 1".into(),
                kind: MeasKind::Spectrum {
                    config: SpectrumConfig {
                        fft_len: 8192,
                        ..SpectrumConfig::on_input(1)
                    },
                },
            },
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");
    r.h.shutdown();
}
