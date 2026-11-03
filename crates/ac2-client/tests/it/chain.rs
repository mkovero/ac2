//! Delta chains on the data socket: the client applies each topic's frames in order, never
//! draws a frame built on the wrong base, keeps the last one it applied (STALE dims it once
//! no new frame comes), asks for a keyframe at most once per `KEYFRAME_ASK_GAP` when a chain
//! it held breaks, and waits without asking while it holds no frame of the topic yet.

use std::time::{Duration, Instant};

use ac2_client::Client;
use ac2_client::data::KEYFRAME_ASK_GAP;
use ac2_client::fake::FakeDaemon;
use ac2_proto::frame::{Frame, FrameData};
use ac2_proto::units::MeasId;
use ac2_proto::{
    CodedFrame, Stream, Subscription, Topic, encode_delta, encode_frame, encode_key, samples,
};

use crate::client::{R, connect, fake, until};

const TF1: Topic = Topic::Data {
    meas: MeasId(1),
    stream: Stream::Tf,
};
const IR1: Topic = Topic::Data {
    meas: MeasId(1),
    stream: Stream::Ir,
};

/// The `name` sample chain on measurement 1, stamped as the fake's frames (its `seq`
/// gaps kept).
fn chain(f: &FakeDaemon, name: &str) -> R<Vec<CodedFrame>> {
    let (_, frames) = samples::chains()
        .into_iter()
        .find(|(n, _)| *n == name)
        .ok_or("no such chain")?;
    let mut out = Vec::new();
    for mut fr in frames {
        fr.stamp = f.lock().stamp(fr.stamp.seq, None);
        match &mut fr.data {
            FrameData::Tf(t) => t.meas = MeasId(1),
            FrameData::Ir(r) => r.meas = MeasId(1),
            _ => {}
        }
        out.push(CodedFrame::from_frame(&fr)?);
    }
    Ok(out)
}

fn delta(x: &CodedFrame, base: &CodedFrame) -> R<Vec<Vec<u8>>> {
    Ok(encode_delta(x, base)?.ok_or("the base fits")?)
}

/// What `c` shows of `topic`: its `seq`, and whether it is stale.
fn shown(c: &Client, topic: &Topic) -> Option<(u64, bool)> {
    let l = c.latest().ok()?;
    let t = l.get(topic)?;
    Some((t.frame.stamp.seq, t.stale))
}

/// The values drawn are exactly those of a keyframe of `want`.
fn same_values(c: &Client, topic: &Topic, want: &CodedFrame) -> R {
    let l = c.latest()?;
    let got: &Frame = &l.get(topic).ok_or("nothing shown")?.frame;
    assert_eq!(encode_frame(got)?, encode_frame(&want.to_frame()?)?);
    Ok(())
}

/// Subscribes to measurement 1 and waits until its `tf` keyframe `key` is shown.
async fn subscribed(c: &Client, f: &FakeDaemon, key: &CodedFrame) -> R {
    c.wait_synced(crate::client::DEADLINE).await?;
    c.subscribe(Subscription::Meas(MeasId(1)))?;
    let end = Instant::now() + crate::client::DEADLINE;
    while Instant::now() < end {
        f.lock().publish_parts(encode_key(key)?);
        tokio::time::sleep(Duration::from_millis(20)).await;
        if shown(c, &TF1).is_some() {
            return Ok(());
        }
    }
    Err("subscription never delivered".into())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_broken_chain_keeps_the_last_frame_and_asks_once() -> R {
    let f = fake()?;
    let c = connect(&f).await?;
    let tf = chain(&f, "tf")?;
    subscribed(&c, &f, &tf[0]).await?;

    // In order: each delta applies on the frame before it.
    f.lock().publish_parts(delta(&tf[1], &tf[0])?);
    until("delta applied", || async {
        shown(&c, &TF1).is_some_and(|(s, _)| s == tf[1].stamp.seq)
    })
    .await?;
    same_values(&c, &TF1, &tf[1])?;
    assert_eq!(f.lock().keyframe_asks, 0);

    // `tf[2]` is lost: the delta on it is refused, the frame before stays, a keyframe is
    // asked for.
    f.lock().publish_parts(delta(&tf[3], &tf[2])?);
    until("refused", || async {
        c.latest().is_ok_and(|l| l.refused_total == 1)
    })
    .await?;
    let first_refusal = Instant::now();
    assert_eq!(shown(&c, &TF1).map(|(s, _)| s), Some(tf[1].stamp.seq));
    same_values(&c, &TF1, &tf[1])?;
    until("keyframe asked for", || async {
        f.lock().keyframe_asks == 1
    })
    .await?;

    // Refused again at once: no second request within the gap.
    f.lock().publish_parts(delta(&tf[3], &tf[2])?);
    until("refused again", || async {
        c.latest().is_ok_and(|l| l.refused_total == 2)
    })
    .await?;
    if first_refusal.elapsed() < KEYFRAME_ASK_GAP - Duration::from_millis(50) {
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(f.lock().keyframe_asks, 1, "asked twice within the gap");
    }

    // No new frame: the kept frame goes STALE, never replaced by a wrong one.
    until("stale", || async {
        shown(&c, &TF1).is_some_and(|(s, stale)| stale && s == tf[1].stamp.seq)
    })
    .await?;

    // The keyframe brings it back, and the chain goes on from it (captured now: an old
    // capture time reads as stale too).
    let mut tf = tf;
    for x in &mut tf[2..] {
        x.stamp.capture_wall_ns = f.lock().stamp(0, None).capture_wall_ns;
    }
    f.lock().publish_parts(encode_key(&tf[2])?);
    f.lock().publish_parts(delta(&tf[3], &tf[2])?);
    until("resynced", || async {
        shown(&c, &TF1).is_some_and(|(s, stale)| !stale && s == tf[3].stamp.seq)
    })
    .await?;
    same_values(&c, &TF1, &tf[3])?;
    assert_eq!(c.latest()?.malformed_total, 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_delta_before_any_keyframe_waits_for_it() -> R {
    let f = fake()?;
    let c = connect(&f).await?;
    let tf = chain(&f, "tf")?;
    let ir = chain(&f, "ir")?;
    subscribed(&c, &f, &tf[0]).await?;

    // A new subscription may see deltas before its keyframe: nothing held, nothing asked.
    f.lock().publish_parts(delta(&ir[1], &ir[0])?);
    f.lock().publish_parts(delta(&tf[1], &tf[0])?);
    until("tf delta", || async {
        shown(&c, &TF1).is_some_and(|(s, _)| s == tf[1].stamp.seq)
    })
    .await?;
    let l = c.latest()?;
    assert!(l.get(&IR1).is_none());
    assert_eq!(l.refused_total, 0);
    assert_eq!(f.lock().keyframe_asks, 0);

    // Its keyframe, then a delta whose exponent changed.
    f.lock().publish_parts(encode_key(&ir[1])?);
    f.lock().publish_parts(delta(&ir[2], &ir[1])?);
    until("ir chain", || async {
        shown(&c, &IR1).is_some_and(|(s, _)| s == ir[2].stamp.seq)
    })
    .await?;
    same_values(&c, &IR1, &ir[2])?;
    Ok(())
}
