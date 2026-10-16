//! Reducer tests of the banners a pane shows: STALE against a stopped measurement,
//! clock drift, stopped audio, a log not saved, and why the IR pane has no IR.

use super::*;

/// The newest frame of spectrum measurement `id`, aged `age_s` with the client's STALE flag.
fn aged_spec(t: &T, id: u32, age_s: f64) -> ac2_client::TopicFrame {
    let mut f =
        t.st.spectrum_frame(MeasId(id))
            .expect("spectrum frame")
            .clone();
    f.age = Some(age_s);
    f.since_new = std::time::Duration::from_secs_f64(age_s);
    f.stale = true;
    f
}

/// A stopped measurement's last frame is its final result: no STALE banner (nor its last
/// protection flags), the curve not dimmed, its caption saying `stopped`. A running one
/// whose frames stop is STALE.
#[test]
fn a_stopped_measurement_is_not_stale() {
    let mut t = T::new();
    spec_frame(&mut t, 2, vec![-60.0; 5]);
    let now = crate::scenes::Now {
        instant: Instant::now(),
        wall: WallNs(0),
    };
    let f = aged_spec(&t, 2, 23.0);
    let running = crate::scenes::status(&t.st, &[&f], None, now);
    assert_eq!(running.frame_age_s, Some(23.0));
    assert!(
        ac2_scene::banner::banners(&running)
            .iter()
            .any(|b| b.text.starts_with("STALE")),
        "{running:?}"
    );

    t.conn(with_running(2, false));
    let stopped = crate::scenes::status(&t.st, &[&f], None, now);
    assert_eq!(stopped.frame_age_s, None);
    assert!(
        ac2_scene::banner::banners(&stopped).is_empty(),
        "{stopped:?}"
    );
    let fresh = crate::scenes::freshness(&t.st, &f);
    assert!(fresh.is_stopped() && !fresh.is_stale(), "{fresh:?}");
}

/// The daemon's committed drift reaches every pane's banners once it is a warning; a value
/// below the threshold shows nothing.
#[test]
fn committed_clock_drift_shows_a_banner() {
    let mut t = T::new();
    let with_drift = |ppm: f64, warning: bool| {
        let mut s = daemon_state();
        s.timing.drift = Some(ac2_proto::model::Drift {
            ppm,
            span: ac2_proto::units::Seconds(30.0),
            warning,
            at: WallNs(0),
        });
        mirror(s)
    };
    t.conn(with_drift(0.3, false));
    let calm = crate::scenes::status(&t.st, &[], None, now());
    assert_eq!(calm.clock_drift_ppm, None);
    assert!(ac2_scene::banner::banners(&calm).is_empty(), "{calm:?}");
    t.conn(with_drift(49.6, true));
    let s = crate::scenes::status(&t.st, &[], None, now());
    let texts: Vec<String> = ac2_scene::banner::banners(&s)
        .into_iter()
        .map(|b| b.text)
        .collect();
    assert_eq!(texts, ["CLOCK DRIFT · 50 ppm"]);
}

/// An SPL log the daemon cannot write is a LOG NOT SAVED banner on every pane, naming the
/// meter, the reason and for how long; it clears when the daemon writes the log again.
#[test]
fn a_log_the_daemon_cannot_write_shows_until_it_is_written() {
    use ac2_proto::model::{LogWriteFailure, MeterLog};
    let mut t = T::new();
    let mut s = daemon_state();
    t.conn(mirror(s.clone()));
    assert_eq!(
        crate::scenes::status(&t.st, &[], None, now()).log_not_saved,
        None
    );
    s.autosave.log_failures = vec![LogWriteFailure {
        meas: MeasId(2),
        log: MeterLog::Spl,
        reason: "disk full".into(),
        since: WallNs(0),
    }];
    t.conn(mirror(s.clone()));
    let b = ac2_scene::banner::banners(&crate::scenes::status(&t.st, &[], None, now()));
    assert_eq!(b.len(), 1, "{b:?}");
    assert_eq!(b[0].text, "LOG NOT SAVED · disk full");
    assert_eq!(
        b[0].detail.as_deref(),
        Some("SPL log of Sub not saved for 0.0 s: disk full; retried every 30 s")
    );
    s.autosave.log_failures.clear();
    t.conn(mirror(s));
    assert!(ac2_scene::banner::banners(&crate::scenes::status(&t.st, &[], None, now())).is_empty());
}

/// A stopped session's audio reaches every pane's banners in place of STALE, and the top
/// bar's texts; it clears when the daemon reopens the session.
#[test]
fn stopped_audio_shows_a_banner_until_the_session_is_back() {
    use ac2_proto::model::{AudioStopped, Recovery, StopCause};
    let mut t = T::new();
    let mut s = daemon_state();
    s.session.stopped = Some(AudioStopped {
        since: WallNs(0),
        cause: StopCause::NotDelivering { after_ms: 1000 },
        recovery: Recovery::Waiting {
            attempt: 2,
            error: "No JACK server: start JACK".into(),
            next_at: WallNs(5_000_000_000),
        },
    });
    t.st.local_zone = crate::scenes::LocalZone::Fixed { offset_s: 0 };
    t.conn(mirror(s.clone()));
    let st = crate::scenes::status(&t.st, &[], None, now());
    let b = ac2_scene::banner::banners(&st);
    assert_eq!(
        b[0].text,
        "AUDIO STOPPED · device not delivering since 0:00"
    );
    assert_eq!(
        b[0].detail.as_deref(),
        Some("attempt 2 failed: No JACK server: start JACK · next in 5 s · measurements paused")
    );
    let bar = crate::scenes::audio_stopped(&t.st, WallNs(0)).map(|a| a.bar);
    assert_eq!(
        bar.as_ref().map(|b| b[0].as_str()),
        Some("audio stopped · reopening (attempt 2, next in 5 s)")
    );
    s.session.stopped = None;
    t.conn(mirror(s));
    let st = crate::scenes::status(&t.st, &[], None, now());
    assert!(ac2_scene::banner::banners(&st).is_empty());
    assert!(crate::scenes::audio_stopped(&t.st, WallNs(0)).is_none());
}

/// The IR pane follows its transfer measurement's state: the transfer stream's NO
/// REFERENCE as a banner and as the reason there is no IR; a stopped measurement says so and
/// which key starts it; a kept IR of a stopped measurement is tagged `stopped`, no STALE.
#[test]
fn the_ir_pane_says_why_there_is_no_ir() {
    use ac2_client::{Latest, TopicFrame};
    use ac2_proto::frame::{IrFrame, IrMeta, ProtectionFlags};
    use ac2_proto::{Frame, FrameData};
    let theme = Theme::dark();
    let snapshot = |frames: Vec<Frame>| {
        let mut latest = Latest::default();
        for f in frames {
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
        ConnEvent::Data(Arc::new(crate::conn::DataSnapshot {
            latest,
            grids: std::collections::BTreeMap::new(),
            drained: Instant::now(),
        }))
    };
    let mut tf = ac2_proto::samples::tf_frame();
    tf.stamp.protection = ProtectionFlags::NO_REFERENCE;
    let ir = Frame {
        stamp: ac2_proto::samples::stamp(None),
        data: FrameData::Ir(IrFrame {
            meas: MeasId(1),
            meta: IrMeta {
                sample_rate: Hz(48_000.0),
                t0: Seconds(-0.0005),
                dt: Seconds(0.0001),
                inserted_delay: Seconds(0.0125),
            },
            linear: vec![0.0, 0.5, 0.1, -0.05, 0.0],
            etc: None,
        }),
    };
    let mut t = T::new();
    t.key("Alt+3");
    let texts = |s: &ac2_scene::ir::IrScene| -> Vec<String> {
        s.banners.iter().map(|b| b.text.clone()).collect()
    };
    // Nothing yet from a running measurement.
    let s = crate::scenes::ir(&t.st, t.ir_pane(), &t.keys, &theme, SIZE, now()).expect("scene");
    assert_eq!(s.note.as_deref(), Some("Main L: no IR frame yet"));
    // Its transfer stream says nothing drives the reference: banner and reason, which
    // follow this app's stimulus.
    t.conn(snapshot(vec![tf.clone()]));
    let ir_and_banner = |t: &T| {
        let s = crate::scenes::ir(&t.st, t.ir_pane(), &t.keys, &theme, SIZE, now()).expect("scene");
        assert!(
            texts(&s).contains(&"NO REFERENCE".to_owned()),
            "{:?}",
            texts(&s)
        );
        let detail = s
            .banners
            .iter()
            .find(|r| r.text == "NO REFERENCE")
            .and_then(|r| r.detail.clone());
        (s.note.unwrap_or_default(), detail.unwrap_or_default())
    };
    assert_eq!(
        ir_and_banner(&t),
        (
            "no reference: nothing is playing — Space arms, Enter starts the stimulus".into(),
            "stimulus off: Space arms, Enter starts it".into()
        )
    );
    t.st.stimulus.level = Some(Dbfs(-20.0));
    t.key("Space");
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    assert_eq!(
        ir_and_banner(&t),
        (
            "no reference: armed — Enter starts the stimulus".into(),
            "stimulus armed: Enter starts it".into()
        )
    );
    t.key("Enter");
    t.conn(ConnEvent::Stimulus(StimEvent::Set { firing: true }));
    assert_eq!(
        ir_and_banner(&t),
        (
            "no reference: nothing is driving the loopback".into(),
            "reference silent: check the loopback cable".into()
        )
    );
    t.key("Escape");
    t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    // On the sweep view Space arms a sweep: the noise is armed from a transfer pane.
    t.put(PaneKind::Distortion);
    assert_eq!(
        ir_and_banner(&t),
        (
            "no reference: nothing is playing — arm the stimulus from a transfer pane".into(),
            "stimulus off: arm it from a transfer pane".into()
        )
    );
    t.put_ir();
    // Another client's stimulus, armed and silent: not this app's keys to press.
    let mut other = daemon_state();
    other.generator.owner = Some(ClientId("other".into()));
    other.generator.armed = true;
    t.conn(mirror(other));
    assert_eq!(
        ir_and_banner(&t).0,
        "no reference: nothing is driving the loopback"
    );
    // Stopped: no fault from a measurement that no longer runs, and the key that starts it.
    let mut st = daemon_state();
    st.measurements[1].running = false;
    t.conn(mirror(st.clone()));
    let s = crate::scenes::ir(&t.st, t.ir_pane(), &t.keys, &theme, SIZE, now()).expect("scene");
    assert_eq!(s.note.as_deref(), Some("Main L stopped — S starts it"));
    assert!(!texts(&s).contains(&"NO REFERENCE".to_owned()));
    let r = t.key("S");
    assert!(
        r.iter().any(|x| matches!(
            x,
            Request::Call { cmd: Command::MeasStart { meas }, .. } if *meas == MeasId(1)
        )),
        "{r:?}"
    );
    // Its kept IR is no longer the system's: not drawn as live, the note stays.
    t.conn(snapshot(vec![tf, ir]));
    let s = crate::scenes::ir(&t.st, t.ir_pane(), &t.keys, &theme, SIZE, now()).expect("scene");
    assert_eq!(s.note.as_deref(), Some("Main L stopped — S starts it"));
    assert_eq!(s.tag, None);
    assert!(texts(&s).iter().all(|b| !b.starts_with("STALE")));
}
