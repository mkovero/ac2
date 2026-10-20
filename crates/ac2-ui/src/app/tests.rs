use std::collections::BTreeMap;

use ac2_client::{Latest, TopicFrame};
use ac2_proto::frame::{ClipFlags, LevelsMeta, SessionLevelsFrame};
use ac2_proto::units::MeasId;
use ac2_proto::{Frame, FrameData};

use super::*;

fn frame(topic: Topic, seq: u64, peak: f32, stale: bool) -> TopicFrame {
    let mut stamp = ac2_proto::samples::stamp(None);
    stamp.seq = seq;
    TopicFrame {
        topic,
        frame: Arc::new(Frame {
            stamp,
            data: FrameData::SessionLevels(SessionLevelsFrame {
                meta: LevelsMeta { channels: vec![0] },
                peak: vec![peak],
                rms: vec![-30.0],
                clip: vec![ClipFlags::NONE],
            }),
        }),
        received: Instant::now(),
        since_new: Duration::ZERO,
        age: Some(0.0),
        stale,
    }
}

fn snap(frames: &[TopicFrame]) -> DataSnapshot {
    DataSnapshot {
        latest: Latest {
            frames: frames
                .iter()
                .map(|f| (f.topic.to_string().into(), f.clone()))
                .collect(),
            ..Latest::default()
        },
        grids: BTreeMap::new(),
        drained: Instant::now(),
    }
}

fn data(meas: u32, stream: Stream) -> Topic {
    Topic::Data {
        meas: MeasId(meas),
        stream,
    }
}

/// Only the panes drawing a changed topic are rebuilt: new content, a STALE flip, a
/// topic gone; a new `seq` with the same content, and the input meters, rebuild none.
#[test]
fn new_frames_touch_the_panes_that_draw_them() {
    let tf = data(1, Stream::Tf);
    let spl = data(4, Stream::Spl);
    let old = snap(&[
        frame(tf, 1, -10.0, false),
        frame(spl, 1, -10.0, false),
        frame(Topic::SessionLevels, 1, -10.0, false),
    ]);
    let same = snap(&[
        frame(tf, 2, -10.0, false),
        frame(spl, 2, -10.0, false),
        frame(Topic::SessionLevels, 2, -11.0, false),
    ]);
    assert!(data_touches(Some(&old), Some(&same)).is_empty());
    let tf_new = snap(&[
        frame(tf, 2, -12.0, false),
        frame(spl, 1, -10.0, false),
        frame(Topic::SessionLevels, 1, -10.0, false),
    ]);
    assert_eq!(
        data_touches(Some(&old), Some(&tf_new)),
        vec![PaneKind::Transfer]
    );
    let spl_stale = snap(&[frame(tf, 1, -10.0, false), frame(spl, 1, -10.0, true)]);
    assert_eq!(
        data_touches(Some(&old), Some(&spl_stale)),
        vec![PaneKind::Spl]
    );
    assert_eq!(data_touches(None, Some(&old)), PaneKind::ALL.to_vec());
}

/// Every motion in progress moves its panes on a tick, whatever else moves with it: the
/// Leq bars easing after their once-a-second reading never hold the spectrum glide.
#[test]
fn a_tick_moves_every_pane_in_motion() {
    assert!(tick_touches(false, false, false).is_empty());
    assert_eq!(
        tick_touches(false, true, true),
        vec![PaneKind::Spl, PaneKind::Spectrum]
    );
    assert_eq!(
        tick_touches(true, true, true),
        vec![
            PaneKind::Transfer,
            PaneKind::Spectrum,
            PaneKind::Distortion,
            PaneKind::Spl
        ]
    );
    assert_eq!(tick_touches(false, false, true), vec![PaneKind::Spectrum]);
}
