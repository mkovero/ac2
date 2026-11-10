//! Reducer tests of the 3D spectrograph's rows gliding back between frames.

use super::*;
use std::time::Duration;

/// History slot length of the default 30 s history, ns (900 slots).
const SLOT_NS: u64 = 33_333_333;
/// Frames 100 ms apart: three slots each.
const HOP_NS: u64 = 3 * SLOT_NS + 1;

/// A spectrum frame of Sub (2) captured `i` hops after the first, drained at `drained`.
fn frame(t: &mut T, i: u64, drained: Instant) {
    use ac2_client::{Latest, TopicFrame};
    use ac2_proto::Frame;
    use ac2_proto::FrameData;
    use ac2_proto::frame::{SpecFrame, SpecMeta};
    let grid = GridDef::Linear {
        fs: Hz(48_000.0),
        n: 8,
    };
    let data = FrameData::Spec(SpecFrame {
        meas: MeasId(2),
        meta: SpecMeta {
            window: Window::Hann,
            scale: LevelScale::Dbfs,
            level: ac2_proto::model::SpecLevel::Tone,
            cal: CalStatus::Uncalibrated,
            mic_curve: false,
            smoothing: None,
            math: None,
            bands: None,
        },
        level: vec![-60.0, -40.0, -20.0, -30.0, -60.0],
    });
    // Mid-slot, so each hop moves the newest slot exactly three on.
    let wall = 53_700_000_000 * SLOT_NS + SLOT_NS / 2 + i * HOP_NS;
    let stamp = ac2_proto::frame::FrameStamp {
        seq: 100 + i,
        capture_wall_ns: ac2_proto::units::WallNs(wall),
        ..ac2_proto::samples::stamp(Some(grid.clone()))
    };
    let f = TopicFrame {
        topic: data.topic(),
        frame: Arc::new(Frame { stamp, data }),
        received: drained,
        since_new: Duration::ZERO,
        age: Some(0.0),
        stale: false,
    };
    let mut latest = Latest::default();
    latest.frames.insert(f.topic.to_string().into(), f);
    let mut grids = std::collections::BTreeMap::new();
    grids.insert(grid.id(), Arc::new(grid));
    t.conn(ConnEvent::Data(Arc::new(crate::conn::DataSnapshot {
        latest,
        grids,
        drained,
    })));
}

fn three_d(t: &mut T) -> PaneId {
    let p = t.pane(PaneKind::Spectrum);
    if let Some(v) = t.st.layout.view_mut(p) {
        v.modes.spectrum = ac2_scene::view::SpectrumMode::Spectrograph3d;
    }
    p
}

/// Each frame moves the newest slot three on; the rows take the time until the next frame
/// to get there, then stand still: the pacer runs only while they move.
#[test]
fn the_3d_rows_glide_to_each_frame_and_stand_still_once_there() {
    let mut t = T::new();
    let p = three_d(&mut t);
    let t0 = Instant::now();
    let hop = Duration::from_millis(100);
    for i in 0..5u32 {
        frame(&mut t, u64::from(i), t0 + hop * i);
    }
    let at = t0 + hop * 4;
    let start = t.st.row_lag(p, at);
    assert!((start - 3.0).abs() < 1e-6, "{start}");
    assert!(t.st.rows_gliding(at));
    let mut last = start;
    for ms in [20, 40, 60, 80] {
        let lag = t.st.row_lag(p, at + Duration::from_millis(ms));
        assert!(lag < last && lag > 0.0, "{ms} ms: {lag} after {last}");
        last = lag;
    }
    // About the time between frames later the rows are there and stay.
    let end = at + hop + Duration::from_millis(1);
    assert_eq!(t.st.row_lag(p, end), 0.0);
    assert!(!t.st.rows_gliding(end));
    // The next frame mid-glide carries on from where the rows are drawn.
    let mid = at + Duration::from_millis(50);
    let drawn = t.st.row_lag(p, mid);
    frame(&mut t, 5, mid);
    assert!((t.st.row_lag(p, mid) - (drawn + 3.0)).abs() < 1e-6);
}

/// Settings › Display "jumps to each new frame": the rows stand where the data puts them.
#[test]
fn without_the_glide_the_3d_rows_jump() {
    let mut t = T::new();
    let p = three_d(&mut t);
    t.st.prefs.spec_ease = false;
    let t0 = Instant::now();
    for i in 0..3u32 {
        frame(&mut t, u64::from(i), t0 + Duration::from_millis(100) * i);
    }
    let at = t0 + Duration::from_millis(200);
    assert_eq!(t.st.row_lag(p, at), 0.0);
    assert!(!t.st.rows_gliding(at));
    assert!(t.st.rig.row_glide.is_empty());
}

/// A 2D view of the same measurement does not run the pacer for the rows.
#[test]
fn rows_glide_only_on_a_3d_view() {
    let mut t = T::new();
    let p = t.pane(PaneKind::Spectrum);
    if let Some(v) = t.st.layout.view_mut(p) {
        v.modes.spectrum = ac2_scene::view::SpectrumMode::Spectrograph;
    }
    let t0 = Instant::now();
    for i in 0..3u32 {
        frame(&mut t, u64::from(i), t0 + Duration::from_millis(100) * i);
    }
    assert!(!t.st.rows_gliding(t0 + Duration::from_millis(200)));
}
