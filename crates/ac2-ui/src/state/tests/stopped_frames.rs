//! A stopped measurement's frames, kept or dropped by their configuration rev against the
//! mirror's.

use super::*;

/// Main L's tf and stage-1 bins frames, at configuration revs `tf` and `bins`.
fn main_l_frames(tf: u64, bins: u64) -> ConnEvent {
    let mut latest = ac2_client::Latest::default();
    for (mut f, rev) in [
        (ac2_proto::samples::tf_frame(), tf),
        (ac2_proto::samples::bins_frame(), bins),
    ] {
        f.stamp.config_rev = Rev(rev);
        let f = ac2_client::TopicFrame {
            topic: f.data.topic(),
            frame: Arc::new(f),
            received: Instant::now(),
            since_new: std::time::Duration::ZERO,
            age: Some(0.0),
            stale: false,
            broken: false,
        };
        latest.frames.insert(f.topic.to_string().into(), f);
    }
    let mut grids = std::collections::BTreeMap::new();
    let g = ac2_proto::samples::log_grid();
    grids.insert(g.id(), Arc::new(g));
    ConnEvent::Data(Arc::new(crate::conn::DataSnapshot {
        latest,
        grids,
        drained: Instant::now(),
    }))
}

/// The configuration revs of Main L's frames the app holds: (tf, bins).
fn held_revs(t: &T) -> (Option<u64>, Option<u64>) {
    let frames = t.st.rig.data.as_ref().map(|d| d.latest.frames.values());
    let rev = |kind: fn(&ac2_proto::topic::Topic) -> bool| {
        frames
            .clone()
            .into_iter()
            .flatten()
            .find(|f| kind(&f.topic))
            .map(|f| f.frame.stamp.config_rev.0)
    };
    (
        rev(|t| matches!(t, ac2_proto::topic::Topic::Data { .. })),
        rev(|t| matches!(t, ac2_proto::topic::Topic::Bins { .. })),
    )
}

/// Main L stopped at configuration rev `rev`.
fn stopped_at(rev: u64) -> ConnEvent {
    let mut s = daemon_state();
    let m = s
        .measurements
        .iter_mut()
        .find(|m| m.id == MeasId(1))
        .expect("Main L");
    m.running = false;
    m.config_rev = Rev(rev);
    mirror(s)
}

/// A stopped measurement's frames are judged one by one against its rev in the mirror:
/// whichever arrives first, the result re-sent at the new rev stays and only a frame of an
/// older one goes.
#[test]
fn a_stopped_measurements_frames_go_by_their_own_rev() {
    // The mirror first: frames of the old rev go, those re-sent at the new one stay.
    let mut t = T::new();
    t.conn(stopped_at(7));
    t.conn(main_l_frames(5, 7));
    assert_eq!(held_revs(&t), (None, Some(7)));
    t.conn(main_l_frames(7, 7));
    assert_eq!(held_revs(&t), (Some(7), Some(7)));

    // The frames first: the mirror does not yet call them outdated, and once it moves to
    // their rev they still are not.
    let mut t = T::new();
    t.conn(stopped_at(5));
    t.conn(main_l_frames(7, 7));
    assert_eq!(held_revs(&t), (Some(7), Some(7)));
    t.conn(stopped_at(7));
    assert_eq!(held_revs(&t), (Some(7), Some(7)));
    // Reset while stopped: a newer rev and no result re-sent.
    t.conn(stopped_at(9));
    assert_eq!(held_revs(&t), (None, None));
}

/// A stopped transfer whose delay moved: its frames carry the newer of the configuration's
/// and the delay's rev, so the kept result's frames go once the delay's rev passes them.
#[test]
fn a_stopped_transfers_frames_go_when_its_delay_moves() {
    let moved = |delay_rev: u64| {
        let mut s = with_delays(daemon_state());
        let m = s
            .measurements
            .iter_mut()
            .find(|m| m.id == MeasId(1))
            .expect("Main L");
        m.running = false;
        m.config_rev = Rev(7);
        let d = s
            .delays
            .iter_mut()
            .find(|d| d.meas == MeasId(1))
            .expect("delay");
        d.rev = Rev(delay_rev);
        mirror(s)
    };
    let mut t = T::new();
    t.conn(moved(7));
    t.conn(main_l_frames(7, 7));
    assert_eq!(held_revs(&t), (Some(7), Some(7)));
    t.conn(moved(9));
    assert_eq!(held_revs(&t), (None, None));
}
