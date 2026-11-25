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
