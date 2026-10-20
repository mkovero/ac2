//! EQ what-if sets from an empty daemon on the fake rig (`docs/design/eq-overlay.md`): input
//! 1 hears a flat path, input 2 the same path through the cookbook peaking −6 dB at 1 kHz,
//! Q 2 (as if the console had it inserted).
//!
//! - a new set predicts the live curve: −6 dB at 1 kHz and 0 dB at 125 Hz against the
//!   curve, its phase the cookbook biquad's; its own curve is the filter exactly;
//! - section and set bypass take the prediction back to the curve;
//! - a capture predicts on its own (the whole set: it recorded none of it);
//! - double-apply: mark, move the measurement to the EQ'd input, capture: the capture records
//!   the set, the live and the captured predictions equal their curves (`no change`); an
//!   edit to −8 dB predicts −2 dB more on both, the whole −8 dB on the unmarked capture;
//! - a capture of a prediction keeps its values and records the set; a session reload
//!   recomputes the curves; refusals by message.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::{Duration, Instant};

use ac2_audio::fake::FakePath;
use ac2_audio::{FakeBackend, FakeConfig, FakeDriver};
use ac2_proto::frame::{Frame, FrameData, TfFrame};
use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Command, ErrorCode, ReplyBody};
use ac2d::Daemon;
use common::*;
use num_complex::Complex64;

const T: Duration = Duration::from_secs(20);
const F0: f64 = 1000.0;
const Q: f64 = 2.0;

/// The cookbook peaking biquad at 48 kHz, `(b, a)` with `a0 = 1`.
fn cookbook(gain_db: f64) -> ([f64; 3], [f64; 2]) {
    let a = 10f64.powf(gain_db / 40.0);
    let w0 = 2.0 * std::f64::consts::PI * F0 / f64::from(FS);
    let al = w0.sin() / (2.0 * Q);
    let c = w0.cos();
    let a0 = 1.0 + al / a;
    (
        [(1.0 + al * a) / a0, -2.0 * c / a0, (1.0 - al * a) / a0],
        [-2.0 * c / a0, (1.0 - al / a) / a0],
    )
}

/// The biquad's response at `f`.
fn h(gain_db: f64, f: f64) -> Complex64 {
    let (b, a) = cookbook(gain_db);
    let z1 = Complex64::from_polar(1.0, -2.0 * std::f64::consts::PI * f / f64::from(FS));
    (b[0] + b[1] * z1 + b[2] * z1 * z1) / (1.0 + a[0] * z1 + a[1] * z1 * z1)
}

/// Its impulse response, long enough to have decayed below the noise.
fn impulse(gain_db: f64, scale: f32) -> Vec<f32> {
    let (b, a) = cookbook(gain_db);
    let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
    (0..4096)
        .map(|n| {
            let x0 = if n == 0 { 1.0 } else { 0.0 };
            let y0 = b[0] * x0 + b[1] * x1 + b[2] * x2 - a[0] * y1 - a[1] * y2;
            (x2, x1, y2, y1) = (x1, x0, y1, y0);
            (y0 as f32) * scale
        })
        .collect()
}

fn backend() -> FakeBackend {
    FakeBackend::new(FakeConfig {
        sample_rate: FS,
        block_frames: BLOCK,
        inputs: 3,
        outputs: 2,
        drive: ac2_audio::fake::FakeDrive::Manual,
        seed: 5,
        paths: vec![
            FakePath::loopback(0, 0, LOOP_DELAY),
            FakePath::acoustic(0, 1, LOOP_DELAY + ACOUSTIC_DELAY, vec![ACOUSTIC_GAIN], 1e-5),
            FakePath::acoustic(
                0,
                2,
                LOOP_DELAY + ACOUSTIC_DELAY,
                impulse(-6.0, ACOUSTIC_GAIN),
                1e-5,
            ),
        ],
        ..FakeConfig::default()
    })
    .unwrap()
}

fn index(f: f64) -> usize {
    (240.0 + 48.0 * (f / 1000.0).log2()).round() as usize
}

fn freq(i: usize) -> f64 {
    1000.0 * 2f64.powf((i as f64 - 240.0) / 48.0)
}

fn peaking(gain: f64) -> EqSection {
    EqSection {
        enabled: true,
        shape: EqShape::Peaking {
            f: Hz(F0),
            gain: Db(gain),
            bw: Bandwidth::Q { value: Q },
            q_measured: None,
        },
    }
}

struct Rig {
    h: ac2d::Handle,
    c: Client,
    sub: Sub,
    d: FakeDriver,
    tok: LeaseToken,
    _dir: tempfile::TempDir,
}

fn rig() -> Rig {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let backend = backend();
    let mut cfg = config(backend.clone(), inproc("eq"));
    cfg.lease_expiry = Duration::from_secs(60);
    cfg.session_dir = dir.path().join("sessions");
    let h = Daemon::start(cfg).unwrap();
    let (mut c, sub) = connect(&h, &[b"d/"]);
    let mut s = session(false);
    s.input_channels = vec![0, 1, 2];
    c.ok(Command::SessionOpen { config: s });
    c.ok(Command::MeasCreate {
        config: transfer("Main L"),
    });
    c.ok(Command::DelaySet {
        meas: MeasId(1),
        delay: Seconds(f64::from(ACOUSTIC_DELAY) / f64::from(FS)),
    });
    c.ok(Command::MeasStart { meas: MeasId(1) });
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
    Rig {
        h,
        c,
        sub,
        d,
        tok,
        _dir: dir,
    }
}

impl Rig {
    fn play(&mut self, seconds: f64) {
        let mut left = seconds;
        while left > 0.0 {
            let s = left.min(0.5);
            run(&mut self.d, s);
            self.c.ok(Command::GenRefresh {
                lease_token: self.tok,
            });
            left -= s;
        }
    }

    /// The newest frame of each measurement in `metas`, covering the audio played so far.
    fn newest<const N: usize>(&self, metas: [MeasId; N]) -> [TfFrame; N] {
        let end = end_sample(&self.d);
        let mut got: [Option<TfFrame>; N] = std::array::from_fn(|_| None);
        let deadline = Instant::now() + T;
        while got.iter().any(Option::is_none) {
            let left = deadline.checked_duration_since(Instant::now());
            let Some(f): Option<Frame> =
                left.and_then(|l| self.sub.frame(l, |f| f.stamp.audio_sample.0 + 1 >= end))
            else {
                let missing: Vec<MeasId> = (0..N)
                    .filter(|&i| got[i].is_none())
                    .map(|i| metas[i])
                    .collect();
                panic!("no frame up to sample {end} of {missing:?}");
            };
            if let FrameData::Tf(t) = f.data
                && let Some(i) = metas.iter().position(|&m| m == t.meas)
            {
                got[i].get_or_insert(t);
            }
        }
        got.map(|f| f.expect("every one"))
    }

    fn eq(&mut self, edit: EqEdit) -> Measurement {
        match self.c.ok(Command::MeasEq {
            meas: MeasId(1),
            edit,
        }) {
            ReplyBody::Measurement(m) => m,
            other => panic!("{other:?}"),
        }
    }

    fn eq_err(&mut self, edit: EqEdit) -> ac2_proto::ProtoError {
        self.c
            .call(Command::MeasEq {
                meas: MeasId(1),
                edit,
            })
            .unwrap_err()
    }

    fn state(&mut self) -> State {
        match self.c.ok(Command::StateSnapshot) {
            ReplyBody::Snapshot(s) => s.state,
            other => panic!("{other:?}"),
        }
    }

    /// The math channel named `name`.
    fn channel(&mut self, name: &str) -> MeasId {
        let s = self.state();
        s.measurements
            .iter()
            .find(|m| m.config.name == name)
            .unwrap_or_else(|| {
                panic!(
                    "no {name:?} in {:?}",
                    s.measurements
                        .iter()
                        .map(|m| &m.config.name)
                        .collect::<Vec<_>>()
                )
            })
            .id
    }

    fn data(&mut self, id: TraceId) -> TraceData {
        match self.c.ok(Command::TraceGet { trace: id }) {
            ReplyBody::TraceData(d) => *d,
            other => panic!("{other:?}"),
        }
    }

    fn capture(&mut self, meas: MeasId, name: &str) -> TraceMeta {
        match self.c.ok(Command::TraceCapture {
            meas,
            name: name.into(),
            slot: None,
        }) {
            ReplyBody::Trace(t) => t,
            other => panic!("{other:?}"),
        }
    }
}

fn mag(t: &TfFrame, f: f64) -> f64 {
    f64::from(t.mag[index(f)])
}

/// `a − b` at `f` is `want` dB within `tol`.
fn diff_is(a: &TfFrame, b: &TfFrame, f: f64, want: f64, tol: f64, what: &str) {
    let d = mag(a, f) - mag(b, f);
    assert!(
        (d - want).abs() < tol,
        "{what} at {f} Hz: {d:.2} dB, want {want}"
    );
}

#[test]
fn slow_eq_predicts_without_applying_twice() {
    let mut r = rig();
    let main = MeasId(1);

    // A new set with one peaking section predicts the live curve.
    let m = r.eq(EqEdit::New {
        name: None,
        realisation: None,
        peak_q: None,
        sections: vec![peaking(-6.0)],
    });
    let set = m.eq[0].clone();
    assert_eq!(set.body.name, "EQ 1");
    assert_eq!(
        set.body.realisation,
        EqRealisation::Digital {
            rate: Hz(f64::from(FS))
        }
    );
    let live = r.channel("Main L after EQ 1");
    r.play(3.5);
    let [curve, pred] = r.newest([main, live]);
    diff_is(&pred, &curve, 1000.0, -6.0, 0.2, "prediction");
    diff_is(&pred, &curve, 125.0, 0.0, 0.2, "prediction");
    let i = index(707.0);
    let dphi = (f64::from(pred.phase[i] - curve.phase[i]) + 540.0).rem_euclid(360.0) - 180.0;
    let want = h(-6.0, freq(i)).arg().to_degrees();
    assert!((dphi - want).abs() < 2.0, "phase {dphi:.1}° vs {want:.1}°");

    // Its own curve is the filter, exactly.
    let eq_curve = r.data(set.trace);
    assert!(matches!(
        eq_curve.meta.source,
        TraceSource::Eq { set: s, rev: 1, .. } if s == set.id
    ));
    assert!(eq_curve.meta.edit.locked);
    for f in [125.0, 707.0, 1000.0, 4000.0] {
        let i = index(f);
        let want = 20.0 * h(-6.0, freq(i)).norm().log10();
        assert!(
            (f64::from(eq_curve.mag_db[i]) - want).abs() < 1e-3,
            "{f} Hz: {} vs {want}",
            eq_curve.mag_db[i]
        );
    }

    // Section bypass, then set bypass: the curve as it is.
    let mut body = set.body.clone();
    body.sections[0].enabled = false;
    r.eq(EqEdit::Put { set: set.id, body });
    r.play(0.5);
    let [curve, pred] = r.newest([main, live]);
    diff_is(&pred, &curve, 1000.0, 0.0, 0.2, "section bypassed");
    let mut body = set.body.clone();
    body.enabled = false;
    r.eq(EqEdit::Put { set: set.id, body });
    r.play(0.5);
    let [curve, pred] = r.newest([main, live]);
    diff_is(&pred, &curve, 1000.0, 0.0, 0.2, "set bypassed");
    r.eq(EqEdit::Put {
        set: set.id,
        body: set.body.clone(),
    });

    // A capture before the EQ is in: it recorded none of it, so it gets the whole set.
    let before = r.capture(main, "before");
    assert!(before.eq_in.is_empty());
    r.eq(EqEdit::Predict {
        set: set.id,
        curve: Operand::Trace { trace: before.id },
        on: true,
    });
    let before_pred = r.channel("before after EQ 1");

    // Double-apply: the console now has rev 4; the measurement moves to the EQ'd path.
    let m = r.eq(EqEdit::Mark {
        set: set.id,
        clear: false,
    });
    let marked = m.eq[0].in_console.clone().expect("marked");
    assert_eq!(marked.rev, 4);
    let mut cfg = transfer("Main L");
    if let MeasKind::Transfer { config } = &mut cfg.kind {
        config.measurement_input = 2;
    }
    r.c.update_ok(main, cfg);
    r.play(3.5);
    let [curve, pred, bpred] = r.newest([main, live, before_pred]);
    diff_is(&curve, &curve, 1000.0, 0.0, 1e-9, "self");
    assert!(
        (mag(&curve, 1000.0) - mag(&curve, 125.0) + 6.0).abs() < 0.3,
        "the EQ'd path reads the cut"
    );
    diff_is(&pred, &curve, 1000.0, 0.0, 0.2, "no change (live)");
    let bdata = r.data(before.id);
    let bi = index(1000.0);
    assert!(
        (f64::from(bpred.mag[bi] - bdata.mag_db[bi]) + 6.0).abs() < 0.2,
        "the earlier capture: the whole set"
    );
    let after = r.capture(main, "after");
    assert_eq!(after.eq_in, vec![marked.clone()]);
    r.eq(EqEdit::Predict {
        set: set.id,
        curve: Operand::Trace { trace: after.id },
        on: true,
    });
    let after_pred = r.channel("after after EQ 1");
    r.play(0.5);
    let [apred] = r.newest([after_pred]);
    let adata = r.data(after.id);
    assert!(
        f64::from(apred.mag[bi] - adata.mag_db[bi]).abs() < 0.05,
        "no change (capture)"
    );

    // Edit to −8 dB: −2 dB more on the curves that have −6 dB in, −8 on the one without.
    let mut body = set.body.clone();
    body.sections[0] = peaking(-8.0);
    r.eq(EqEdit::Put { set: set.id, body });
    r.play(0.5);
    let [curve, pred, bpred, apred] = r.newest([main, live, before_pred, after_pred]);
    diff_is(
        &pred,
        &curve,
        1000.0,
        -2.0,
        0.2,
        "change since rev 4 (live)",
    );
    assert!((f64::from(apred.mag[bi] - adata.mag_db[bi]) + 2.0).abs() < 0.05);
    assert!((f64::from(bpred.mag[bi] - bdata.mag_db[bi]) + 8.0).abs() < 0.2);

    // A capture of a prediction keeps its values and records the set as it is now.
    let p = r.capture(live, "predicted");
    assert_eq!(p.eq_in.len(), 1);
    assert_eq!(p.eq_in[0].rev, 5);
    let pdata = r.data(p.id);
    assert!((f64::from(pdata.mag_db[bi]) - mag(&pred, 1000.0)).abs() < 0.3);

    // Refusals.
    let mut bad = set.body.clone();
    bad.sections[0].shape = EqShape::Peaking {
        f: Hz(30_000.0),
        gain: Db(-3.0),
        bw: Bandwidth::Q { value: 1.0 },
        q_measured: None,
    };
    let e = r.eq_err(EqEdit::Put {
        set: set.id,
        body: bad,
    });
    assert_eq!(e.code, ErrorCode::Invalid);
    assert!(e.msg.contains("below fs/2"), "{}", e.msg);
    let mut bad = set.body.clone();
    bad.sections.push(EqSection {
        enabled: true,
        shape: EqShape::HighPass {
            f: Hz(80.0),
            alignment: Alignment::LinkwitzRiley,
            order: 3,
        },
    });
    let e = r.eq_err(EqEdit::Put {
        set: set.id,
        body: bad,
    });
    assert_eq!(e.code, ErrorCode::Invalid);
    assert!(e.msg.starts_with("2 · high-pass"), "{}", e.msg);
    let e =
        r.c.call(Command::TraceDelete { trace: set.trace })
            .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");
    let e = r.c.update(main, spl("Main L", 1)).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");

    // Saved and loaded: the set comes back, its curve recomputed.
    r.c.ok(Command::FileSave {
        session: SessionRef::Name { name: "eq".into() },
        overwrite: false,
    });
    r.c.ok(Command::FileLoad {
        session: SessionRef::Name { name: "eq".into() },
        discard: true,
    });
    let s = r.state();
    let loaded = s.measurements.iter().find(|m| m.id == main).unwrap();
    assert_eq!(loaded.eq.len(), 1);
    assert_eq!(loaded.eq[0].rev, 5);
    assert_eq!(loaded.eq[0].in_console, Some(marked));
    let c = r.data(set.trace);
    let want = 20.0 * h(-8.0, freq(bi)).norm().log10();
    assert!((f64::from(c.mag_db[bi]) - want).abs() < 1e-3);

    // Deleting the set takes its curve and predictions with it.
    r.eq(EqEdit::Delete { set: set.id });
    let s = r.state();
    assert!(
        !s.measurements
            .iter()
            .any(|m| m.config.name.contains("after EQ 1"))
    );
    assert!(!s.traces.iter().any(|t| t.id == set.trace));
    r.h.shutdown();
}

/// Without a session: a set on a transfer measurement, its curve at the 48 kHz default, a
/// set refused on a spectrum, the eighth set's limit, copy and delete.
#[test]
fn eq_sets_without_audio() {
    init_log();
    let backend = backend();
    let h = Daemon::start(config(backend, inproc("eq-quiet"))).unwrap();
    let (mut c, _sub) = connect(&h, &[]);
    c.ok(Command::MeasCreate {
        config: transfer("Main L"),
    });
    c.ok(Command::MeasCreate {
        config: transfer("Main R"),
    });
    let e = c
        .call(Command::MeasEq {
            meas: MeasId(1),
            edit: EqEdit::Predict {
                set: EqSetId(1),
                curve: Operand::Meas { meas: MeasId(1) },
                on: true,
            },
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound, "{e:?}");
    let new = |c: &mut Client, meas: u32| match c.call(Command::MeasEq {
        meas: MeasId(meas),
        edit: EqEdit::New {
            name: None,
            realisation: None,
            peak_q: None,
            sections: vec![peaking(-6.0)],
        },
    }) {
        Ok(ReplyBody::Measurement(m)) => Ok(m),
        Ok(other) => panic!("{other:?}"),
        Err(e) => Err(e),
    };
    let m = new(&mut c, 1).unwrap();
    assert_eq!(
        m.eq[0].body.realisation,
        EqRealisation::Digital { rate: Hz(48_000.0) }
    );
    for n in 2..=8 {
        let m = new(&mut c, 1).unwrap();
        assert_eq!(m.eq.last().unwrap().body.name, format!("EQ {n}"));
    }
    let e = new(&mut c, 1).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");
    // Copied: a set of its own on the other measurement, with its own prediction.
    c.ok(Command::MeasEq {
        meas: MeasId(1),
        edit: EqEdit::Copy {
            set: EqSetId(1),
            to: MeasId(2),
        },
    });
    let state = match c.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => s.state,
        other => panic!("{other:?}"),
    };
    let r = state
        .measurements
        .iter()
        .find(|m| m.id == MeasId(2))
        .unwrap();
    assert_eq!(r.eq.len(), 1);
    assert_eq!(r.eq[0].id, EqSetId(9));
    assert!(
        state
            .measurements
            .iter()
            .any(|m| m.config.name == "Main R after EQ 1")
    );
    // Deleting the measurement takes its sets' curves and predictions, whatever is kept.
    c.ok(Command::MeasDelete {
        meas: MeasId(1),
        traces: OwnedTraces::Keep,
    });
    let state = match c.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => s.state,
        other => panic!("{other:?}"),
    };
    assert_eq!(state.traces.len(), 1, "only Main R's EQ curve");
    assert_eq!(
        state
            .measurements
            .iter()
            .filter(|m| m.config.name.contains("after EQ"))
            .count(),
        1
    );
    // A spectrum holds no EQ.
    c.ok(Command::MeasCreate {
        config: MeasConfig {
            scope: MeasScope::Session,
            name: "RTA".into(),
            kind: MeasKind::Spectrum {
                config: SpectrumConfig::on_input(1),
            },
        },
    });
    let id = state.measurements.iter().map(|m| m.id.0).max().unwrap() + 1;
    let e = new(&mut c, id).unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid, "{e:?}");
    h.shutdown();
}
