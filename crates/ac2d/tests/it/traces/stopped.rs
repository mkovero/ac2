//! A stopped measurement keeps its final result: a transfer sends its final `tf` and IR at
//! the stop whether or not anyone subscribes, so a view opened or a client connected later
//! gets them.

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

/// A 1/48 transfer whose IR nobody watched leaves its final `tf` and IR at the stop, under
/// one stamp.
#[test]
fn a_stopped_transfer_leaves_its_curve_and_ir() {
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
    r.h.shutdown();
}
