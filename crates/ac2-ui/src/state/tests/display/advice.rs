//! Reducer tests of the transfer pane's headroom advice: fed from the session's input
//! meters, the transfer frame's coherence and the mirrored generator, shown as the pane's
//! last banner, capped by the system max level, never changing a level, and switched off
//! from the Display page.

use super::*;

use ac2_client::{Latest, TopicFrame};
use ac2_proto::FrameData;
use ac2_proto::frame::{ClipFlags, SessionLevelsFrame, ValidityMask};

/// One snapshot at `now_s`: transfer measurement 1 with coherence `coh` everywhere, and
/// the session meters of inputs 0 (reference, `ref_peak` dBFS) and 1 (measurement,
/// `meas_rms` dBFS RMS).
fn feed(t: &mut T, now_s: f64, coh: f32, ref_peak: f32, meas_rms: f32) {
    t.st.apply(Msg::Tick { now_s, dt_s: 0.1 }, &t.keys);
    let mut latest = Latest::default();
    let mut f = ac2_proto::samples::tf_frame();
    if let FrameData::Tf(tf) = &mut f.data {
        tf.meas = MeasId(1);
        tf.meta.math = None;
        tf.coh.iter_mut().for_each(|c| *c = coh);
        tf.validity.iter_mut().for_each(|v| *v = ValidityMask::NONE);
    }
    f.stamp.protection = ac2_proto::frame::ProtectionFlags::NONE;
    let meters = ac2_proto::Frame {
        stamp: f.stamp,
        data: FrameData::SessionLevels(SessionLevelsFrame {
            meta: ac2_proto::frame::SessionLevelsMeta {
                channels: vec![0, 1],
                ballistic: Default::default(),
                pair_levels: vec![],
                loudness: vec![],
            },
            peak: vec![ref_peak, meas_rms + 12.0],
            rms: vec![ref_peak - 12.0, meas_rms],
            clip: vec![ClipFlags::NONE; 2],
            ppm: vec![f32::NEG_INFINITY; 2],
            true_peak: vec![f32::NEG_INFINITY; 2],
            max: vec![f32::NEG_INFINITY; 2],
            overs: vec![0; 2],
        }),
    };
    for f in [f, meters] {
        let f = TopicFrame {
            topic: f.data.topic(),
            frame: Arc::new(f),
            received: Instant::now(),
            since_new: std::time::Duration::ZERO,
            age: Some(0.0),
            stale: false,
        };
        latest.frames.insert(f.topic.to_string().into(), f);
    }
    let mut grids = std::collections::BTreeMap::new();
    let g = ac2_proto::samples::log_grid();
    grids.insert(g.id(), Arc::new(g));
    let reqs = t.conn(ConnEvent::Data(Arc::new(crate::conn::DataSnapshot {
        latest,
        grids,
        drained: Instant::now(),
    })));
    // Advice only: no stimulus request ever leaves the app for it.
    assert!(
        reqs.iter().all(|r| !format!("{r:?}").starts_with("Stim")),
        "{reqs:?}"
    );
}

/// The transfer pane's banner texts.
fn banners(t: &T) -> Vec<String> {
    crate::scenes::transfer(
        &t.st,
        t.pane(PaneKind::Transfer),
        &Theme::dark(),
        SIZE,
        now(),
    )
    .banners
    .iter()
    .map(|b| b.text.clone())
    .collect()
}

fn advice(t: &T) -> Option<String> {
    banners(t).into_iter().find(|b| b.contains("STIMULUS"))
}

#[test]
fn low_coherence_on_a_quiet_input_advises_raising_capped_by_the_max_level() {
    let mut t = T::new();
    t.go(PaneKind::Transfer);
    feed(&mut t, 10.0, 0.4, -20.0, -84.3);
    assert_eq!(advice(&t), None, "not before it held 2 s");
    feed(&mut t, 11.0, 0.4, -20.0, -84.3);
    assert_eq!(advice(&t), None);
    feed(&mut t, 12.1, 0.4, -20.0, -84.3);
    assert_eq!(
        advice(&t).as_deref(),
        Some("RAISE STIMULUS · about +18 dB"),
        "{:?}",
        banners(&t)
    );
    // The rig's generator plays 5.5 dB under the max level: the step stops there.
    t.remirror(|s| {
        s.generator.firing = true;
        s.generator.armed = true;
        s.generator.ceiling = Dbfs(-50.0);
        s.generator.settings = Some(ac2_proto::model::GeneratorSettings {
            signal: ac2_proto::model::Signal::Pink,
            level: Dbfs(-55.5),
            band: None,
            outputs: vec![0],
        });
    });
    feed(&mut t, 12.3, 0.4, -20.0, -84.3);
    assert_eq!(
        advice(&t).as_deref(),
        Some("RAISE STIMULUS · +5 dB to max level")
    );
    // Off on the Display page: gone at once, and remembered.
    t.key("Ctrl+P");
    t.st.apply(Msg::Settings(SettingsMsg::Page(Page::Display)), &t.keys);
    t.st.apply(
        Msg::Settings(SettingsMsg::Display(crate::settings::DisplayRow::Advice, 1)),
        &t.keys,
    );
    assert!(!t.st.prefs.advice.on && t.st.prefs_dirty);
    t.key("Escape");
    feed(&mut t, 12.4, 0.4, -20.0, -84.3);
    assert_eq!(advice(&t), None);
}

#[test]
fn a_hot_reference_advises_lowering_and_clip_takes_over() {
    let mut t = T::new();
    t.go(PaneKind::Transfer);
    feed(&mut t, 1.0, 0.95, -1.5, -30.0);
    assert_eq!(
        advice(&t).as_deref(),
        Some("LOWER STIMULUS · ref peak −1.5 dBFS")
    );
    // Back under for less than the hold time: still shown, no flicker.
    feed(&mut t, 2.0, 0.95, -15.0, -30.0);
    feed(&mut t, 5.0, 0.95, -15.0, -30.0);
    assert!(advice(&t).is_some());
    feed(&mut t, 7.1, 0.95, -15.0, -30.0);
    assert_eq!(advice(&t), None);
}

/// A threshold changed on the Display page applies to the next frame.
#[test]
fn the_low_level_threshold_is_a_display_setting() {
    let mut t = T::new();
    t.go(PaneKind::Transfer);
    t.st.apply(
        Msg::Settings(SettingsMsg::Display(
            crate::settings::DisplayRow::AdviceLowLevel,
            -1,
        )),
        &t.keys,
    );
    assert_eq!(t.st.prefs.advice.low_level_dbfs, -80.0);
    assert_eq!(
        t.last_toast(),
        "advise raising with the measurement input under −80.0 dBFS RMS"
    );
    // −75 dBFS RMS is now not low: no raise however low the coherence.
    for i in 0..40 {
        feed(&mut t, f64::from(i) * 0.1, 0.4, -20.0, -75.0);
    }
    assert_eq!(advice(&t), None);
}
