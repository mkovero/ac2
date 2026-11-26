//! A stopped measurement keeps its final result: a transfer sends its final `tf` and IR at
//! the stop whether or not anyone subscribes, so a view opened or a client connected later
//! gets them, and `trace.capture` of the stopped measurement stores that same result until
//! it starts again or changes.

use super::*;

use ac2_proto::Frame;

/// The frames a fresh subscriber to `prefixes` is served from the daemon's slots: what came
/// before the first keepalive.
fn late_frames(r: &Rig, prefixes: &[&[u8]]) -> Vec<Frame> {
    let mut p = prefixes.to_vec();
    p.push(b"ka");
    let late = Sub::connect(r.h.context(), r.h.data_endpoint(), &p);
    let mut got = Vec::new();
    loop {
        let f = late.frame(T, |_| true).expect("the keepalive");
        if matches!(f.data, FrameData::Ka(_)) {
            return got;
        }
        got.push(f);
    }
}

fn capture(c: &mut Client, name: &str) -> Result<ac2_proto::ReplyBody, ac2_proto::ProtoError> {
    c.call(Command::TraceCapture {
        meas: MeasId(1),
        name: name.into(),
        slot: None,
    })
}

const NOTHING_KEPT: &str =
    "main: no result to capture (it has not measured with its current settings)";

/// Measurement `meas`'s frames a late subscriber gets from the daemon's slots.
fn meas_frames(r: &Rig, meas: MeasId) -> Vec<Frame> {
    let d = format!("d/{}/", meas.0);
    late_frames(r, &[d.as_bytes()])
}

/// Measurement 1 run on, then stopped with a result kept.
fn run_and_stop(r: &mut Rig) {
    r.c.ok(Command::MeasStart { meas: MeasId(1) });
    settle(&mut r.d, &mut r.c, &r.sub, r.tok, 0);
    r.c.ok(Command::MeasStop { meas: MeasId(1) });
    assert!(!meas_frames(r, MeasId(1)).is_empty());
}

/// Its kept result gone: a capture is refused and no frame of it is left to draw.
fn nothing_kept(r: &mut Rig) {
    assert_eq!(capture(&mut r.c, "gone").unwrap_err().msg, NOTHING_KEPT);
    assert!(meas_frames(r, MeasId(1)).is_empty());
}

/// A 1/48 transfer whose IR nobody watched leaves its final `tf` and IR at the stop, under
/// one stamp; a capture after the stop stores that result, IR included.
#[test]
fn a_stopped_transfer_keeps_its_curve_and_ir() {
    let mut r = rig("stopped-keeps");
    // Nobody subscribes to anything of measurement 1 from here on; its IR never had one.
    r.sub
        .unsubscribe(ac2_proto::Subscription::Topic(ac2_proto::Topic::Data {
            meas: MeasId(1),
            stream: ac2_proto::Stream::Tf,
        }));
    run(&mut r.d, 0.5);
    r.c.ok(Command::MeasStop { meas: MeasId(1) });

    let got = late_frames(&r, &[b"d/1/tf", b"d/1/ir"]);
    let tf = got
        .iter()
        .find(|f| matches!(f.data, FrameData::Tf(_)))
        .expect("the final tf");
    let ir = got
        .iter()
        .find_map(|f| match &f.data {
            FrameData::Ir(i) => Some((f, i)),
            _ => None,
        })
        .expect("the final IR");
    assert_eq!(ir.0.stamp.audio_sample, tf.stamp.audio_sample);
    assert_eq!(ir.0.stamp.config_rev, tf.stamp.config_rev);
    assert_eq!(ir.0.stamp.grid_id, None);
    let FrameData::Tf(tf_data) = &tf.data else {
        unreachable!()
    };

    let t = r.c.trace(Command::TraceCapture {
        meas: MeasId(1),
        name: "after stop".into(),
        slot: Some(1),
    });
    match t.source {
        TraceSource::Captured { at_sample, .. } => assert_eq!(at_sample, tf.stamp.audio_sample),
        other => panic!("{other:?}"),
    }
    let d = data(&mut r.c, t.id);
    for i in band() {
        assert!(
            (d.mag_db[i] - tf_data.mag[i]).abs() < 0.05,
            "{} Hz: {} vs {}",
            grid_freq(i),
            d.mag_db[i],
            tf_data.mag[i]
        );
    }
    let kept = d.ir.expect("captured with its IR");
    let (_, live) = ir;
    assert_eq!(kept.ir.linear.len(), live.linear.len());
    // The frame's IR goes as 16-bit mantissas of one power of two per frame; the trace
    // keeps every f32 of the same impulse response.
    let peak = live.linear.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    let step = 2f32.powf((peak / 32_765.0).log2().ceil());
    for (a, b) in kept.ir.linear.iter().zip(&live.linear) {
        assert!((a - b).abs() <= step, "{a} vs {b}");
    }
    r.h.shutdown();
}

/// The kept result goes when the measurement starts again (its averages start over) or
/// is reset; a measurement never run has nothing to capture.
#[test]
fn a_restart_or_reset_drops_the_kept_result() {
    let mut r = rig("stopped-restart");
    r.c.ok(Command::MeasStop { meas: MeasId(1) });
    capture(&mut r.c, "kept").unwrap();

    r.c.ok(Command::MeasReset { meas: MeasId(1) });
    let e = capture(&mut r.c, "reset").unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);
    assert_eq!(
        e.msg,
        "main: no result to capture (it has not measured with its current settings)"
    );

    r.c.ok(Command::MeasStart { meas: MeasId(1) });
    settle(&mut r.d, &mut r.c, &r.sub, r.tok, 0);
    r.c.ok(Command::MeasStop { meas: MeasId(1) });
    capture(&mut r.c, "kept again").unwrap();
    // Started again and stopped before any audio: nothing measured since the start.
    r.c.ok(Command::MeasStart { meas: MeasId(1) });
    r.c.ok(Command::MeasStop { meas: MeasId(1) });
    assert!(capture(&mut r.c, "restarted").is_err());

    // A display smoothing change keeps it.
    r.c.ok(Command::MeasStart { meas: MeasId(1) });
    settle(&mut r.d, &mut r.c, &r.sub, r.tok, 0);
    r.c.ok(Command::MeasStop { meas: MeasId(1) });
    let mut cfg = r.c.state().measurements[0].config.clone();
    if let MeasKind::Transfer { config } = &mut cfg.kind {
        config.smoothing = Some(Smoothing {
            fraction: SmoothingFraction::Third,
            mode: SmoothingMode::Magnitude,
        });
    }
    r.c.update_ok(MeasId(1), cfg);
    let t = r.c.trace(Command::TraceCapture {
        meas: MeasId(1),
        name: "smoothed".into(),
        slot: None,
    });
    assert_eq!(
        t.edit.smoothing.map(|s| s.fraction),
        Some(SmoothingFraction::Third)
    );

    // Never run.
    r.c.ok(Command::MeasCreate {
        config: transfer("idle"),
    });
    let idle = r.c.state().measurements.last().unwrap().id;
    let e =
        r.c.call(Command::TraceCapture {
            meas: idle,
            name: "idle".into(),
            slot: None,
        })
        .unwrap_err();
    assert_eq!(
        e.msg,
        "idle: no result to capture (it has not measured with its current settings)"
    );
    r.h.shutdown();
}

/// Slots are per measurement: two measurements captured into slot 1 each keep theirs, and a
/// second capture of one takes only that one's slot.
#[test]
fn each_measurement_has_its_own_slots() {
    let mut r = rig("slots-per-meas");
    r.c.ok(Command::MeasCreate {
        config: transfer("second"),
    });
    let second = r.c.state().measurements.last().unwrap().id;
    r.c.ok(Command::MeasStart { meas: second });
    settle(&mut r.d, &mut r.c, &r.sub, r.tok, 0);
    let into_1 = |c: &mut Client, meas: MeasId, name: &str| {
        c.trace(Command::TraceCapture {
            meas,
            name: name.into(),
            slot: Some(1),
        })
    };
    let a = into_1(&mut r.c, MeasId(1), "main S1");
    let b = into_1(&mut r.c, second, "second S1");
    let a2 = into_1(&mut r.c, MeasId(1), "main S1 again");
    let all = traces(&mut r.c);
    let slot_of = |id| all.iter().find(|t| t.id == id).unwrap().edit.slot;
    assert_eq!(slot_of(a.id), None);
    assert_eq!(slot_of(b.id), Some(1));
    assert_eq!(slot_of(a2.id), Some(1));
    r.h.shutdown();
}

/// A delay set to the value it holds moves nothing: its rev stays and a stopped transfer
/// keeps its result.
#[test]
fn a_delay_set_to_its_own_value_keeps_the_result() {
    let mut r = rig("stopped-same-delay");
    r.c.ok(Command::MeasStop { meas: MeasId(1) });
    let d = r.c.delay_now(MeasId(1));
    let ac2_proto::ReplyBody::Delay(after) = r.c.ok(Command::DelaySet {
        meas: MeasId(1),
        delay: d.applied,
    }) else {
        panic!("delay.set answers with the delay")
    };
    assert_eq!((after.rev, after.applied), (d.rev, d.applied));
    capture(&mut r.c, "same delay").unwrap();
    assert!(!meas_frames(&r, MeasId(1)).is_empty());
    r.h.shutdown();
}

/// A second stop keeps the result; a new delay, a configuration that restarts the averages
/// and the session closing each drop it with its frames; a delete drops the frames.
#[test]
fn slow_what_keeps_and_what_drops_the_kept_result() {
    let mut r = rig("stopped-drops");
    r.c.ok(Command::MeasStop { meas: MeasId(1) });
    r.c.ok(Command::MeasStop { meas: MeasId(1) });
    capture(&mut r.c, "twice stopped").unwrap();
    assert!(!meas_frames(&r, MeasId(1)).is_empty());

    r.c.ok(Command::DelaySet {
        meas: MeasId(1),
        delay: Seconds(0.001),
    });
    nothing_kept(&mut r);

    run_and_stop(&mut r);
    let mut cfg = r.c.state().measurements[0].config.clone();
    if let MeasKind::Transfer { config } = &mut cfg.kind {
        config.lf_window = LfWindow::Long;
    }
    r.c.update_ok(MeasId(1), cfg);
    nothing_kept(&mut r);

    // Reset while stopped: a new config rev says the frames held are not its result.
    run_and_stop(&mut r);
    let rev = r.c.state().measurements[0].config_rev;
    r.c.ok(Command::MeasReset { meas: MeasId(1) });
    assert!(r.c.state().measurements[0].config_rev > rev);
    nothing_kept(&mut r);

    r.c.ok(Command::MeasCreate {
        config: transfer("second"),
    });
    let second = r.c.state().measurements.last().unwrap().id;
    r.c.ok(Command::MeasStart { meas: second });
    for _ in 0..5 {
        run(&mut r.d, 0.5);
        r.c.ok(Command::GenRefresh { lease_token: r.tok });
    }
    r.c.ok(Command::MeasStop { meas: second });
    assert!(!meas_frames(&r, second).is_empty());
    r.c.ok(Command::MeasDelete {
        meas: second,
        traces: OwnedTraces::Delete,
    });
    assert!(meas_frames(&r, second).is_empty());

    // The audio closing ends every job and what they kept.
    run_and_stop(&mut r);
    r.c.ok(Command::StreamClose);
    assert!(meas_frames(&r, MeasId(1)).is_empty());
    r.c.ok(Command::StreamOpen {
        config: stream(true),
    });
    assert!(r.c.state().measurements.iter().any(|m| m.id == MeasId(1)));
    nothing_kept(&mut r);
    r.h.shutdown();
}

/// Mean squared second difference of `v` over every three neighbouring valid columns: how
/// rough a curve is.
fn roughness(v: &[f32]) -> f64 {
    let d: Vec<f64> = v
        .windows(3)
        .filter(|w| w.iter().all(|x| x.is_finite()))
        .map(|w| {
            let d = f64::from(w[0]) - 2.0 * f64::from(w[1]) + f64::from(w[2]);
            d * d
        })
        .collect();
    d.iter().sum::<f64>() / d.len() as f64
}

/// A display smoothing change of a stopped transfer: its final `tf` goes out again under the
/// new config rev, smoothed as a running job would send it, and a capture stores the
/// unsmoothed columns with the new smoothing as its display edit.
#[test]
fn a_smoothing_change_while_stopped_relabels_the_kept_result() {
    let mut r = rig("stopped-smoothing");
    r.c.ok(Command::MeasStop { meas: MeasId(1) });
    let plain = r.c.trace(Command::TraceCapture {
        meas: MeasId(1),
        name: "plain".into(),
        slot: None,
    });
    let plain = data(&mut r.c, plain.id);
    let mut cfg = r.c.state().measurements[0].config.clone();
    if let MeasKind::Transfer { config } = &mut cfg.kind {
        config.smoothing = Some(Smoothing {
            fraction: SmoothingFraction::Third,
            mode: SmoothingMode::Magnitude,
        });
    }
    r.c.update_ok(MeasId(1), cfg);
    let rev = r.c.state().measurements[0].config_rev;
    let got = late_frames(&r, &[b"d/1/tf"]);
    let tf = got
        .iter()
        .rev()
        .find_map(|f| match &f.data {
            FrameData::Tf(t) => Some((f, t)),
            _ => None,
        })
        .expect("the tf again");
    assert_eq!(tf.0.stamp.config_rev, rev);
    assert_eq!(
        tf.1.meta.smoothing.map(|s| s.fraction),
        Some(SmoothingFraction::Third)
    );
    // The frame is the kept result smoothed: the unsmoothed columns a capture stores, put
    // through ac2-core's smoother, to the hundredths of a dB a frame carries.
    let smoothing = tf.1.meta.smoothing.expect("smoothing");
    let (want, _) = core_smoothed(&plain, smoothing);
    let mut compared = 0;
    for (i, want) in want.iter().enumerate() {
        if !(tf.1.mag[i].is_finite() && plain.mag_db[i].is_finite()) {
            continue;
        }
        compared += 1;
        assert!(
            (f64::from(tf.1.mag[i]) - want).abs() < 0.006,
            "{} Hz: drawn {}, ac2-core {want}",
            grid_freq(i),
            tf.1.mag[i]
        );
    }
    assert!(compared > 300, "{compared} columns compared");
    // …and not the kept result as it was: a third of an octave averages some sixteen columns,
    // so whatever the measurement noise is, the curve is far less rough than the raw one. A
    // ratio of roughness does not depend on how large the noise of this run happens to be.
    let (drawn, raw) = (roughness(&tf.1.mag), roughness(&plain.mag_db));
    assert!(
        raw > 100.0 * drawn,
        "drawn smoothed: roughness {drawn}, raw {raw}"
    );

    let t = r.c.trace(Command::TraceCapture {
        meas: MeasId(1),
        name: "smoothed".into(),
        slot: None,
    });
    assert_eq!(
        t.edit.smoothing.map(|s| s.fraction),
        Some(SmoothingFraction::Third)
    );
    // Shown as the frame draws it; stored unsmoothed, as its smoothing taken off shows.
    let shown = data(&mut r.c, t.id);
    let t = set_smoothing(&mut r.c, &t, None);
    let stored = data(&mut r.c, t.id);
    for i in band() {
        assert!(
            (shown.mag_db[i] - tf.1.mag[i]).abs() < 0.05,
            "{} Hz: {} shown, {} drawn",
            grid_freq(i),
            shown.mag_db[i],
            tf.1.mag[i]
        );
        assert!(
            (stored.mag_db[i] - plain.mag_db[i]).abs() < 1e-4,
            "{} Hz: stored unsmoothed",
            grid_freq(i)
        );
    }
    r.h.shutdown();
}

/// A spectrum's capture depends on the smoothing its job forms, so a smoothing change while
/// stopped drops its kept result and its frames.
#[test]
fn a_spectrum_smoothing_change_while_stopped_drops_the_kept_result() {
    let mut r = rig("stopped-spec-smoothing");
    let spec = r.c.measurement(Command::MeasCreate {
        config: MeasConfig {
            scope: MeasScope::Session,
            name: "spec".into(),
            kind: MeasKind::Spectrum {
                config: SpectrumConfig::on_input(1),
            },
        },
    });
    r.c.ok(Command::MeasStart { meas: spec.id });
    settle(&mut r.d, &mut r.c, &r.sub, r.tok, 0);
    r.c.ok(Command::MeasStop { meas: spec.id });
    let into = |c: &mut Client| {
        c.call(Command::TraceCapture {
            meas: spec.id,
            name: "spec".into(),
            slot: None,
        })
    };
    into(&mut r.c).unwrap();
    let mut cfg = spec.config.clone();
    if let MeasKind::Spectrum { config } = &mut cfg.kind {
        config.smoothing = Some(SmoothingFraction::Third);
    }
    r.c.update_ok(spec.id, cfg);
    assert!(into(&mut r.c).is_err());
    assert!(meas_frames(&r, spec.id).is_empty());
    r.h.shutdown();
}

/// A capture after the stop is of the result as it stopped: dated at the stop, whenever it
/// is taken.
#[test]
fn a_capture_after_the_stop_is_dated_at_the_stop() {
    let mut r = rig("stopped-dated");
    let now = || {
        let d = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("after 1970");
        WallNs(u64::try_from(d.as_nanos()).expect("in range"))
    };
    let before = now();
    r.c.ok(Command::MeasStop { meas: MeasId(1) });
    let stopped = now();
    std::thread::sleep(Duration::from_millis(50));
    let a = r.c.trace(Command::TraceCapture {
        meas: MeasId(1),
        name: "a".into(),
        slot: None,
    });
    std::thread::sleep(Duration::from_millis(50));
    let b = r.c.trace(Command::TraceCapture {
        meas: MeasId(1),
        name: "b".into(),
        slot: None,
    });
    assert_eq!(a.created_at, b.created_at);
    assert!(a.created_at >= before && a.created_at <= stopped, "{a:?}");
    r.h.shutdown();
}

/// A capture's slot is its source curve's, wherever it is filed: moved under another
/// measurement, it still gives up slot 1 to the next capture of its own curve. Kept into
/// the imported group by deleting their measurements, two traces in slot 1 leave it to one.
#[test]
fn a_slot_is_the_source_curves() {
    let mut r = rig("slots-per-source");
    r.c.ok(Command::MeasCreate {
        config: transfer("second"),
    });
    let second = r.c.state().measurements.last().unwrap().id;
    r.c.ok(Command::MeasStart { meas: second });
    settle(&mut r.d, &mut r.c, &r.sub, r.tok, 0);
    let into_1 = |c: &mut Client, meas: MeasId, name: &str| {
        c.trace(Command::TraceCapture {
            meas,
            name: name.into(),
            slot: Some(1),
        })
    };
    let a = into_1(&mut r.c, MeasId(1), "main S1");
    let mut e = a.edit.clone();
    e.owner = TraceOwner::Meas { meas: second };
    r.c.ok(Command::TraceUpdate {
        trace: a.id,
        edit: e,
    });
    let b = into_1(&mut r.c, second, "second S1");
    let slot_of = |c: &mut Client, id| traces(c).iter().find(|t| t.id == id).unwrap().edit.slot;
    assert_eq!(slot_of(&mut r.c, a.id), Some(1), "another curve's slot 1");
    let a2 = into_1(&mut r.c, MeasId(1), "main S1 again");
    assert_eq!(slot_of(&mut r.c, a.id), None);
    assert_eq!(slot_of(&mut r.c, a2.id), Some(1));

    for m in [MeasId(1), second] {
        r.c.ok(Command::MeasDelete {
            meas: m,
            traces: OwnedTraces::Keep,
        });
    }
    let all = traces(&mut r.c);
    let in_1: Vec<_> = all.iter().filter(|t| t.edit.slot == Some(1)).collect();
    assert_eq!(in_1.len(), 1, "{all:?}");
    assert!(all.iter().all(|t| t.edit.owner == TraceOwner::Imported));
    assert!([a2.id, b.id].contains(&in_1[0].id));
    r.h.shutdown();
}

/// Main's newest `tf` frame a late subscriber gets.
fn late_tf(r: &Rig) -> (Rev, ac2_proto::frame::TfFrame) {
    late_frames(r, &[b"d/1/tf"])
        .into_iter()
        .rev()
        .find_map(|f| match f.data {
            FrameData::Tf(t) => Some((f.stamp.config_rev, t)),
            _ => None,
        })
        .expect("a tf frame")
}

fn main_smoothing(c: &mut Client, s: Option<Smoothing>) {
    let mut cfg = c.state().measurements[0].config.clone();
    if let MeasKind::Transfer { config } = &mut cfg.kind {
        config.smoothing = s;
    }
    c.update_ok(MeasId(1), cfg);
}

/// The result re-sent at a smoothing changed while stopped is the frame the running job
/// sends at that smoothing for the same average, masked columns and all: a column the
/// job leaves out of the smoothing (unresolved, low coherence) is left out again.
#[test]
fn a_resmoothed_result_is_what_the_job_sends() {
    let mut r = rig("stopped-resmooth");
    let third = Some(Smoothing {
        fraction: SmoothingFraction::Third,
        mode: SmoothingMode::Magnitude,
    });
    main_smoothing(&mut r.c, third);
    settle(&mut r.d, &mut r.c, &r.sub, r.tok, 0);
    r.c.ok(Command::MeasStop { meas: MeasId(1) });
    let (_, sent) = late_tf(&r);
    assert!(
        sent.validity
            .iter()
            .any(|v| *v != ac2_proto::frame::ValidityMask::NONE),
        "some columns masked"
    );
    main_smoothing(&mut r.c, None);
    main_smoothing(&mut r.c, third);
    let (rev, again) = late_tf(&r);
    assert_eq!(rev, r.c.state().measurements[0].config_rev);
    assert_eq!(again.validity, sent.validity);
    let same = |a: f32, b: f32, tol: f32| (a.is_nan() && b.is_nan()) || (a - b).abs() <= tol;
    for (i, (a, b)) in sent.mag.iter().zip(&again.mag).enumerate() {
        assert!(
            same(*a, *b, 1e-3),
            "column {i}: {a} dB sent, {b} dB re-sent"
        );
    }
    for (i, (a, b)) in sent.phase.iter().zip(&again.phase).enumerate() {
        assert!(
            same(*a, *b, 1e-3),
            "column {i}: phase {a} sent, {b} re-sent"
        );
    }
    r.h.shutdown();
}

/// A stopped `bins` transfer re-sends every final frame at a smoothing change: its `tf`,
/// each stage's bins and its IR, all at the new configuration rev, so a view that drops a
/// stopped measurement's frames of an older rev keeps the whole result.
#[test]
fn a_smoothing_change_while_stopped_resends_every_final_frame() {
    let mut r = rig("stopped-bins-resend");
    let mut m = transfer("bins");
    if let MeasKind::Transfer { config } = &mut m.kind {
        config.resolution = TransferResolution::Bins;
    }
    let m = r.c.measurement(Command::MeasCreate { config: m });
    r.c.ok(Command::MeasStart { meas: m.id });
    settle(&mut r.d, &mut r.c, &r.sub, r.tok, 0);
    r.c.ok(Command::MeasStop { meas: m.id });
    let mut cfg = m.config.clone();
    if let MeasKind::Transfer { config } = &mut cfg.kind {
        config.smoothing = Some(Smoothing {
            fraction: SmoothingFraction::Sixth,
            mode: SmoothingMode::Magnitude,
        });
    }
    r.c.update_ok(m.id, cfg);
    let rev =
        r.c.state()
            .measurements
            .iter()
            .find(|x| x.id == m.id)
            .expect("listed")
            .config_rev;
    let topics: Vec<String> = std::iter::once(format!("d/{}/", m.id.0))
        .chain((0..4).map(|s| format!("bins/{}/{s}", m.id.0)))
        .collect();
    let p: Vec<&[u8]> = topics.iter().map(|t| t.as_bytes()).collect();
    let got = late_frames(&r, &p);
    let has = |k: fn(&FrameData) -> bool| got.iter().any(|f| k(&f.data));
    assert!(has(|d| matches!(d, FrameData::Tf(_))), "tf");
    assert!(has(|d| matches!(d, FrameData::Ir(_))), "ir");
    assert!(has(|d| matches!(d, FrameData::Bins(_))), "bins");
    for f in &got {
        assert_eq!(f.stamp.config_rev, rev, "{:?}", f.data.topic());
    }
    r.h.shutdown();
}

/// A stopped spectrum's capture says what it was measured with at the stop: its input's
/// mic curve changed afterwards is not what its result has subtracted.
#[test]
fn a_stopped_spectrum_captures_its_mic_of_the_stop() {
    const CURVE: &str = "* test mic\n20 0\n1000 0\n4000 0\n8000 6\n24000 6\n";
    let mut r = rig("stopped-spec-mic");
    let spec = r.c.measurement(Command::MeasCreate {
        config: MeasConfig {
            scope: MeasScope::Session,
            name: "spec".into(),
            kind: MeasKind::Spectrum {
                config: SpectrumConfig::on_input(1),
            },
        },
    });
    r.c.ok(Command::MeasStart { meas: spec.id });
    settle(&mut r.d, &mut r.c, &r.sub, r.tok, 0);
    r.c.ok(Command::MeasStop { meas: spec.id });
    let capture = |c: &mut Client, name: &str| {
        c.trace(Command::TraceCapture {
            meas: spec.id,
            name: name.into(),
            slot: None,
        })
    };
    let before = capture(&mut r.c, "before");
    r.c.ok(Command::CalCurveImport(ac2_proto::CalCurveImportArgs {
        mic: "M30".into(),
        label: None,
        file_name: "M30.frd".into(),
        content: Blob(CURVE.as_bytes().to_vec()),
        channel: Some(1),
    }));
    let after = capture(&mut r.c, "after");
    assert_ne!(
        before.mic.as_ref().map(|m| m.name.as_str()),
        Some("M30"),
        "the curve came after the stop"
    );
    assert_eq!(after.mic, before.mic);
    assert_eq!(after.cal, before.cal);
    r.h.shutdown();
}

/// A stopped math trace's capture names its operands and the EQ it applies as they were
/// at the stop: an operand renamed or its EQ set edited afterwards changes neither.
#[test]
fn a_stopped_math_captures_its_operands_of_the_stop() {
    let mut r = rig("stopped-math-names");
    let section = |gain: f64| EqSection {
        enabled: true,
        shape: EqShape::Peaking {
            f: Hz(1000.0),
            gain: Db(gain),
            bw: Bandwidth::Q { value: 2.0 },
            q_measured: None,
        },
    };
    let m = match r.c.call(Command::MeasEq {
        meas: MeasId(1),
        edit: EqEdit::New {
            name: None,
            realisation: None,
            peak_q: None,
            sections: vec![section(-6.0)],
        },
    }) {
        Ok(ac2_proto::ReplyBody::Measurement(m)) => m,
        other => panic!("{other:?}"),
    };
    let set = m.eq[0].id;
    let q = r.c.measurement(Command::MeasCreate {
        config: MeasConfig {
            scope: MeasScope::Session,
            name: "main × EQ".into(),
            kind: MeasKind::Math {
                config: MathConfig::of(
                    TraceOwner::Meas { meas: MeasId(1) },
                    MathDomain::Transfer,
                    MathExpr::Binary {
                        a: Operand::Meas { meas: MeasId(1) },
                        op: MathOp::Multiply,
                        b: Operand::Eq {
                            meas: MeasId(1),
                            set,
                        },
                    },
                ),
            },
        },
    });
    r.c.ok(Command::MeasStart { meas: q.id });
    settle(&mut r.d, &mut r.c, &r.sub, r.tok, 0);
    r.c.ok(Command::MeasStop { meas: q.id });
    let capture = |c: &mut Client, name: &str| {
        c.trace(Command::TraceCapture {
            meas: q.id,
            name: name.into(),
            slot: None,
        })
    };
    let before = capture(&mut r.c, "before");
    let names = |t: &TraceMeta| match &t.source {
        TraceSource::Math { operands, .. } => {
            operands.iter().map(|o| o.name.clone()).collect::<Vec<_>>()
        }
        other => panic!("{other:?}"),
    };
    assert!(names(&before).contains(&"main".to_owned()), "{before:?}");
    assert!(!before.eq_in.is_empty(), "the set it applies");

    let mut cfg = r.c.state().measurements[0].config.clone();
    cfg.name = "renamed".into();
    r.c.update_ok(MeasId(1), cfg);
    let mut body = m.eq[0].body.clone();
    body.sections = vec![section(-3.0)];
    let base = r.c.state().measurements[0].eq[0].rev;
    r.c.ok(Command::MeasEq {
        meas: MeasId(1),
        edit: EqEdit::Put { set, base, body },
    });
    let after = capture(&mut r.c, "after");
    assert_eq!(names(&after), names(&before));
    assert_eq!(after.eq_in, before.eq_in);
    r.h.shutdown();
}

/// A reset while stopped invalidates the result kept, once: with none kept it changes
/// nothing, so a view holds no frame it would have to drop again.
#[test]
fn a_reset_with_nothing_kept_changes_nothing() {
    let mut r = rig("stopped-reset-twice");
    r.c.ok(Command::MeasStop { meas: MeasId(1) });
    let rev = |c: &mut Client| c.state().measurements[0].config_rev;
    let stopped = rev(&mut r.c);
    r.c.ok(Command::MeasReset { meas: MeasId(1) });
    let reset = rev(&mut r.c);
    assert!(reset > stopped);
    nothing_kept(&mut r);
    r.c.ok(Command::MeasReset { meas: MeasId(1) });
    assert_eq!(rev(&mut r.c), reset);
    r.h.shutdown();
}

/// A running spectrum on input 2, `d/2/spec` heard by a subscriber that then goes; the
/// rig settled with it.
fn spectrum_heard(r: &mut Rig) -> Measurement {
    let spec = r.c.measurement(Command::MeasCreate {
        config: MeasConfig {
            scope: MeasScope::Session,
            name: "spec".into(),
            kind: MeasKind::Spectrum {
                config: SpectrumConfig::on_input(1),
            },
        },
    });
    r.c.ok(Command::MeasStart { meas: spec.id });
    let heard = Sub::connect(r.h.context(), r.h.data_endpoint(), &[b"d/2/spec"]);
    settle(&mut r.d, &mut r.c, &r.sub, r.tok, 0);
    heard
        .frame(T, |f| matches!(f.data, FrameData::Spec(_)))
        .expect("a spec frame");
    spec
}

/// A job that sends no final frames leaves its last ones on view, and what is on view
/// agrees with what it keeps: a topic last sent at the result's rev stays, and the
/// result is kept.
#[test]
fn a_stopped_spectrum_keeps_its_frames_and_result_at_one_rev() {
    let mut r = rig("stopped-spec-agree");
    let spec = spectrum_heard(&mut r);
    r.c.ok(Command::MeasStop { meas: spec.id });
    let rev = r.c.state().measurements[1].config_rev;
    let got = meas_frames(&r, spec.id);
    assert!(got.iter().any(|f| matches!(f.data, FrameData::Spec(_))));
    assert!(got.iter().all(|f| f.stamp.config_rev == rev), "{got:?}");
    r.c.ok(Command::TraceCapture {
        meas: spec.id,
        name: "kept".into(),
        slot: None,
    });
    r.h.shutdown();
}

/// A topic not sent since an in-place change (nobody subscribed when the smoothing moved)
/// holds a frame of the older rev, which every view drops: it does not cost the result,
/// and it goes from the slots, so nothing of another rev than the result stays.
#[test]
fn a_topic_not_sent_since_a_change_does_not_cost_the_result() {
    let mut r = rig("stopped-spec-stale");
    let spec = spectrum_heard(&mut r);
    let mut cfg = spec.config.clone();
    if let MeasKind::Spectrum { config } = &mut cfg.kind {
        config.smoothing = Some(SmoothingFraction::Third);
    }
    r.c.update_ok(spec.id, cfg);
    r.c.ok(Command::MeasStop { meas: spec.id });
    let rev = r.c.state().measurements[1].config_rev;
    let got = meas_frames(&r, spec.id);
    assert!(got.iter().all(|f| f.stamp.config_rev == rev), "{got:?}");
    r.c.ok(Command::TraceCapture {
        meas: spec.id,
        name: "kept".into(),
        slot: None,
    });
    r.h.shutdown();
}

/// The audio closing ends every kept result: a stopped measurement that kept one gets a
/// new config rev, so the views drop its frames as a capture now refuses it.
#[test]
fn a_session_close_ends_the_kept_result_on_view() {
    let mut r = rig("stopped-close");
    r.c.ok(Command::MeasStop { meas: MeasId(1) });
    let rev = |c: &mut Client| c.state().measurements[0].config_rev;
    let stopped = rev(&mut r.c);
    r.c.ok(Command::StreamClose);
    assert!(rev(&mut r.c) > stopped);
    assert!(meas_frames(&r, MeasId(1)).is_empty());
    r.h.shutdown();
}
