//! A transfer's stage bins: the `bins/<meas>/<stage>` topic beside `d/`, its frame and its
//! per-frame delta metadata, and the stage layout a `tf` states.
#![allow(clippy::unwrap_used)]

use ac2_proto::frame::{BinsFrame, FrameData, FrameKind, TfFrame};
use ac2_proto::units::MeasId;
use ac2_proto::*;

fn refs(p: &[Vec<u8>]) -> Vec<&[u8]> {
    p.iter().map(Vec::as_slice).collect()
}

fn bins(f: &Frame) -> &BinsFrame {
    match &f.data {
        FrameData::Bins(b) => b,
        other => panic!("{other:?}"),
    }
}

fn tf(f: &Frame) -> &TfFrame {
    match &f.data {
        FrameData::Tf(t) => t,
        other => panic!("{other:?}"),
    }
}

/// Frames compared by their encoding: NaN-safe.
fn same(a: &Frame, b: &Frame) {
    assert_eq!(encode_frame(a).unwrap(), encode_frame(b).unwrap());
}

#[test]
fn topics_spell_one_way_with_one_digit_stages() {
    for (meas, stage) in [(0, 0), (1, 9), (u32::MAX, 3)] {
        let t = Topic::Bins {
            meas: MeasId(meas),
            stage,
        };
        assert_eq!(t.to_string(), format!("bins/{meas}/{stage}"));
        assert_eq!(Topic::parse(&t.to_bytes()), Ok(t));
        assert!(t.to_bytes().len() <= topic::MAX_TOPIC_BYTES);
        assert_eq!(
            Subscription::parse_prefix(&t.to_bytes()),
            Ok(Subscription::Topic(t))
        );
    }
    for bad in [
        &b"bins/1/10"[..],
        b"bins/1/a",
        b"bins/1/",
        b"bins/01/1",
        b"bins//1",
        b"bins/1/1/",
        b"bins/1",
        b"bins/4294967296/1",
        b"bins/1/-1",
        b"d/1/0",
    ] {
        assert!(Topic::parse(bad).is_err(), "{bad:?}");
    }
    for bad in [&b"bins/"[..], b"bins/1/", b"bins"] {
        assert!(Subscription::parse_prefix(bad).is_err(), "{bad:?}");
    }
    assert_eq!(topic::MAX_BIN_STAGES, 10);
}

/// `bins/` stands beside `d/`: a subscription to every measurement stream, or to one
/// measurement's, takes no stage, and one stage's topic is never a prefix of another's.
#[test]
fn data_subscriptions_take_no_stage() {
    let s1 = Topic::Bins {
        meas: MeasId(1),
        stage: 1,
    };
    for sub in [Subscription::AllData, Subscription::Meas(MeasId(1))] {
        assert!(!sub.matches(&s1), "{sub:?}");
        assert!(!s1.to_bytes().starts_with(&sub.prefix()), "{sub:?}");
    }
    assert!(!Subscription::InputMeters.matches(&s1));
    for other in [
        Topic::Bins {
            meas: MeasId(1),
            stage: 2,
        },
        Topic::Bins {
            meas: MeasId(12),
            stage: 1,
        },
    ] {
        assert!(!other.to_bytes().starts_with(&s1.to_bytes()));
        assert!(!Subscription::Topic(s1).matches(&other));
    }
    assert!(Subscription::Topic(s1).matches(&s1));
}

#[test]
fn a_stage_frame_roundtrips_as_key_and_delta() {
    let f = samples::bins_frame();
    assert_eq!(f.data.kind(), FrameKind::Bins);
    assert_eq!(f.topic().to_string(), "bins/1/1");
    let parts = encode_frame(&f).unwrap();
    let back = decode_frame(&refs(&parts)).unwrap();
    same(&back, &f);
    let b = bins(&back);
    assert_eq!(b.mag.len(), 16);
    assert!(b.mag[3].is_nan() && b.phase[9].is_nan() && b.coh[3].is_nan());
    assert_eq!(b.meta.first_bin, 88);

    // Blocks, effective averages and the delay (a nudge) move from frame to frame: a delta
    // carries them, and the configuration stamp the nudge moved.
    let base = CodedFrame::from_frame(&f).unwrap();
    let mut next = f.clone();
    next.stamp.seq += 1;
    next.stamp.config_rev.0 += 1;
    if let FrameData::Bins(b) = &mut next.data {
        b.meta.blocks += 1;
        b.meta.eff_avg = 10.25;
        b.meta.delay.0 += 0.001;
        b.meta.nudged.0 += 0.001;
        b.mag[0] += 0.5;
    }
    let x = CodedFrame::from_frame(&next).unwrap();
    let d = encode_delta(&x, &base).unwrap().expect("the base fits");
    let got = decode_wire(&refs(&d))
        .unwrap()
        .apply(Some(&base))
        .unwrap()
        .to_frame()
        .unwrap();
    same(&got, &next);
    // Any static field goes as a keyframe.
    let mut moved = next.clone();
    if let FrameData::Bins(b) = &mut moved.data {
        b.meta.mic_curve = !b.meta.mic_curve;
    }
    let m = CodedFrame::from_frame(&moved).unwrap();
    assert!(encode_delta(&m, &base).unwrap().is_none());
}

/// The topic names the stage the metadata states; a frame whose two differ is refused.
#[test]
fn a_stage_frame_on_another_stages_topic_is_refused() {
    let f = samples::bins_frame();
    let mut parts = encode_frame(&f).unwrap();
    parts[0] = b"bins/1/2".to_vec();
    assert!(decode_frame(&refs(&parts)).is_err());
    parts[0] = b"d/1/tf".to_vec();
    assert!(decode_frame(&refs(&parts)).is_err());
}

#[test]
fn a_tf_states_its_stage_layout() {
    let f = samples::tf_frame();
    let parts = encode_frame(&f).unwrap();
    let back = decode_frame(&refs(&parts)).unwrap();
    let layout = tf(&back).meta.bins.as_deref().expect("a layout");
    assert_eq!(layout, &samples::bin_layout());
    assert_eq!(layout.stages.len(), layout.blends.len() + 1);
    let mut none = f.clone();
    if let FrameData::Tf(t) = &mut none.data {
        t.meta.bins = None;
    }
    let back = decode_frame(&refs(&encode_frame(&none).unwrap())).unwrap();
    assert_eq!(tf(&back).meta.bins, None);
}
