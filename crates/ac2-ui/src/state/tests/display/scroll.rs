//! Reducer tests of the spectrographs scrolling on the display's clock between frames.

use super::*;
use std::time::Duration;

/// Frames 100 ms of audio apart at the fixtures' 48 kHz: three slots each.
const HOP: u64 = 4_800;

/// A spectrum frame of Sub (2) ending `i` hops after the first, drained at `drained`.
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
    let audio = 48_000 * 3600 + 800 + i * HOP;
    let stamp = ac2_proto::frame::FrameStamp {
        seq: 100 + i,
        audio_sample: ac2_proto::units::SampleIndex(audio),
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

/// Frames every 100 ms of audio arriving 100 ms apart: between frames the time axis runs
/// on at the display's pace, the same distance every display frame, up to the next frame's
/// due time, where a stalled stream waits and the pacer stops.
#[test]
fn the_spectrograph_scrolls_evenly_and_waits_when_frames_stop() {
    let mut t = T::new();
    let p = three_d(&mut t);
    let t0 = Instant::now();
    let hop = Duration::from_millis(100);
    for i in 0..5u32 {
        frame(&mut t, u64::from(i), t0 + hop * i);
    }
    let at = t0 + hop * 4;
    // As the newest frame arrives the axis stands at its audio: half a slot into its slot
    // (to within the slot length's rounding to whole ns, an hour into the session).
    let start = t.st.scroll_lead(p, at);
    assert!((start + 0.5).abs() < 5e-3, "{start}");
    assert!(t.st.spectrographs_scrolling(at));
    // Half a slot per display frame of 1/60 s.
    let mut last = start;
    for f in 1..=5u32 {
        let lead = t.st.scroll_lead(p, at + Duration::from_micros(16_667) * f);
        assert!(
            (lead - last - 0.5).abs() < 1e-3,
            "frame {f}: {lead} after {last}"
        );
        last = lead;
    }
    // Past the next frame's due time the axis waits, and so does the pacer.
    let end = at + hop + Duration::from_millis(1);
    assert!((t.st.scroll_lead(p, end) - 2.5).abs() < 5e-3);
    assert!(!t.st.spectrographs_scrolling(end));
    // The next frame, on time, finds the axis where it would have been: no jump.
    let next = at + hop;
    let before = t.st.scroll_lead(p, next);
    frame(&mut t, 5, next);
    assert!((t.st.scroll_lead(p, next) - (before - 3.0)).abs() < 1e-6);
}

/// Settings › Display "jumps to each new frame": the rows stand where the data puts them.
#[test]
fn without_the_scroll_the_rows_jump() {
    let mut t = T::new();
    let p = three_d(&mut t);
    t.st.prefs.spec_ease = false;
    let t0 = Instant::now();
    for i in 0..3u32 {
        frame(&mut t, u64::from(i), t0 + Duration::from_millis(100) * i);
    }
    let at = t0 + Duration::from_millis(250);
    assert_eq!(t.st.scroll_lead(p, at), 0.0);
    assert!(!t.st.spectrographs_scrolling(at));
    assert!(t.st.rig.scroll.is_empty());
}

/// The flat spectrograph scrolls on the same clock as the 3D view.
#[test]
fn the_flat_spectrograph_scrolls_too() {
    let mut t = T::new();
    let p = t.pane(PaneKind::Spectrum);
    if let Some(v) = t.st.layout.view_mut(p) {
        v.modes.spectrum = ac2_scene::view::SpectrumMode::Spectrograph;
    }
    let t0 = Instant::now();
    for i in 0..3u32 {
        frame(&mut t, u64::from(i), t0 + Duration::from_millis(100) * i);
    }
    let at = t0 + Duration::from_millis(220);
    assert!(t.st.spectrographs_scrolling(at));
    assert!((t.st.scroll_lead(p, at) - 0.1).abs() < 5e-3);
    // A plain spectrum view has no time axis to scroll.
    if let Some(v) = t.st.layout.view_mut(p) {
        v.modes.spectrum = ac2_scene::view::SpectrumMode::Spectrum;
    }
    assert!(!t.st.spectrographs_scrolling(at));
}
