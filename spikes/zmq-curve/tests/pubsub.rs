//! XPUB (daemon) / SUB (client): multipart frames and subscription visibility.

mod common;

use common::*;
use spike_zmq_curve::ctrl::{ClientSecurity, ServerSecurity};
use spike_zmq_curve::data::{SubEvent, bind_xpub};
use spike_zmq_curve::proto::{decode_frame, encode_frame};
use spike_zmq_curve::zmq::Context;

#[test]
fn xpub_sees_subscriptions_and_frames_roundtrip() -> TestResult {
    let ctx = Context::new()?;
    let xpub = bind_xpub(&ctx, "tcp://127.0.0.1:*", &ServerSecurity::Null, 1000)?;
    let ep = xpub.last_endpoint()?;

    let s1 = sub(&ctx, &ep, &ClientSecurity::Null, &["d/m1/tf", "evt"])?;
    let mut seen = wait_sub_event(&xpub, SubEvent::Subscribe, "d/m1/tf")?;
    if !seen.contains(&SubEvent::Subscribe(b"evt".to_vec())) {
        seen.extend(wait_sub_event(&xpub, SubEvent::Subscribe, "evt")?);
    }

    // A frame on a topic nobody subscribed is filtered at the publisher. Single-publisher order
    // is preserved, so if it had been delivered it would arrive before the next frame.
    xpub.send_multipart(&encode_frame("d/m2/tf", &header(0, 4), &[0.0; 4]), 0)?;
    let values = tf_values(1);
    let parts = encode_frame("d/m1/tf", &header(1, TF_N), &values);
    assert_eq!(parts[2].len(), 6144);
    xpub.send_multipart(&parts, 0)?;

    let m = s1.recv_timeout(TIMEOUT)?.ok_or("no frame")?;
    let f = decode_frame(&m.frames)?;
    assert_eq!(f.topic, "d/m1/tf");
    assert_eq!(f.header.seq, 1);
    assert_eq!(f.values, values);

    // XPUB_VERBOSE: a second subscriber to the same topic is reported again.
    let s2 = sub(&ctx, &ep, &ClientSecurity::Null, &["d/m1/tf"])?;
    wait_sub_event(&xpub, SubEvent::Subscribe, "d/m1/tf")?;

    // Explicit unsubscribe while another subscriber remains is not reported (non-verbose
    // unsubscribe), but the last one leaving is — here by disconnecting.
    s1.unsubscribe(b"d/m1/tf")?;
    drop(s1); // also drops "evt": its last subscriber leaves
    let events = wait_sub_event(&xpub, SubEvent::Unsubscribe, "evt")?;
    assert!(
        !events.contains(&SubEvent::Unsubscribe(b"d/m1/tf".to_vec())),
        "unsubscribe reported while another subscriber remains: {events:?}"
    );
    drop(s2);
    wait_sub_event(&xpub, SubEvent::Unsubscribe, "d/m1/tf")?;
    Ok(())
}
