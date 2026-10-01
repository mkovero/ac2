//! XPUB (daemon) / SUB (clients): multipart frames, publisher-side filtering and subscription
//! events tracked with prefix semantics.

mod common;

use ac2_zmq::{Context, SocketType, SubscriptionEvent, SubscriptionTracker};
use common::*;

#[test]
fn xpub_reports_subscriptions_and_multipart_frames_roundtrip() -> TestResult {
    let ctx = Context::new()?;
    let xpub = ctx.socket(SocketType::XPub)?;
    xpub.set_xpub_verbose(true)?;
    xpub.bind("tcp://127.0.0.1:*")?;
    let ep = xpub.last_endpoint()?;
    let mut interest = SubscriptionTracker::new();

    let s1 = sub(&ctx, &ep, None, &["d/m1/", "evt"])?;
    let mut seen = track(&mut interest, wait_sub_event(&xpub, &subscribed("d/m1/"))?);
    if !seen.contains(&subscribed("evt")) {
        seen.extend(track(
            &mut interest,
            wait_sub_event(&xpub, &subscribed("evt"))?,
        ));
    }

    // A topic nobody subscribed is filtered at the publisher. One publisher's order is kept,
    // so had it been delivered it would arrive before the next frame.
    xpub.send(&frame("d/m2/tf", 0, &[0; 16]))?;
    let payload: Vec<u8> = (0..6144u32).map(|i| (i % 251) as u8).collect();
    xpub.send(&frame("d/m1/tf", 1, &payload))?;
    let m = recv(&s1)?;
    assert_eq!(m.frames(), frame("d/m1/tf", 1, &payload).as_slice());
    assert_eq!(m.user_id(), None, "no ZAP on a NULL connection");

    // XPUB_VERBOSE: a second subscriber to the same prefix is reported again.
    let s2 = sub(&ctx, &ep, None, &["d/m1/"])?;
    track(&mut interest, wait_sub_event(&xpub, &subscribed("d/m1/"))?);

    // An unsubscribe while another subscriber remains is not reported; the last one leaving
    // is, also when it leaves by disconnecting.
    s1.unsubscribe(b"d/m1/")?;
    drop(s1); // also drops "evt": its last subscriber leaves
    let events = track(&mut interest, wait_sub_event(&xpub, &unsubscribed("evt"))?);
    assert!(
        !events.contains(&unsubscribed("d/m1/")),
        "unsubscribe reported while another subscriber remains: {events:?}"
    );
    assert!(interest.wants(b"d/m1/tf"));
    assert!(!interest.wants(b"evt"));
    assert!(!interest.wants(b"d/m2/tf"));

    drop(s2);
    track(
        &mut interest,
        wait_sub_event(&xpub, &unsubscribed("d/m1/"))?,
    );
    assert!(interest.is_empty());
    Ok(())
}

fn track(
    interest: &mut SubscriptionTracker,
    events: Vec<SubscriptionEvent>,
) -> Vec<SubscriptionEvent> {
    for e in &events {
        interest.apply_event(e);
    }
    events
}

#[test]
fn tracker_applies_xpub_messages() -> TestResult {
    let ctx = Context::new()?;
    let ep = format!("inproc://{}", unique("tracker"));
    let xpub = ctx.socket(SocketType::XPub)?;
    xpub.set_xpub_verbose(true)?;
    xpub.bind(&ep)?;
    let _s = sub(&ctx, &ep, None, &[""])?;
    let mut t = SubscriptionTracker::new();
    let ev = t.apply(&recv(&xpub)?).ok_or("not a subscription")?;
    assert_eq!(ev, subscribed(""));
    assert!(t.wants(b"anything"));
    Ok(())
}
