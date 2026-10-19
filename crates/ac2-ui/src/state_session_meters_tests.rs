//! Reducer tests of the session meters, the sweep progress strip, recording and new measurements.

use super::*;

#[test]
fn input_meters_follow_the_dialog() {
    use ac2_client::{Latest, TopicFrame};
    use ac2_proto::frame::{ClipFlags, PreviewLevelsFrame, PreviewLevelsMeta, SessionLevelsFrame};
    use ac2_proto::topic::Topic;
    use ac2_proto::{Frame, FrameData};
    let mut t = T::new();
    let frame = |data: FrameData| TopicFrame {
        topic: data.topic(),
        frame: Arc::new(Frame {
            stamp: ac2_proto::samples::stamp(None),
            data,
        }),
        received: Instant::now(),
        since_new: std::time::Duration::ZERO,
        age: Some(0.0),
        stale: false,
    };
    let mut latest = Latest::default();
    for f in [
        frame(FrameData::SessionLevels(SessionLevelsFrame {
            meta: ac2_proto::frame::LevelsMeta {
                channels: vec![0, 1],
            },
            peak: vec![-10.0, -30.0],
            rms: vec![-20.0, -40.0],
            clip: vec![ClipFlags::NONE, ClipFlags::HELD],
        })),
        frame(FrameData::PreviewLevels(PreviewLevelsFrame {
            meta: PreviewLevelsMeta {
                backend: BackendKind::Fake,
                device: DeviceId("fake:loop".into()),
                channels: vec![0, 1, 2, 3],
            },
            peak: vec![-6.0, f32::NEG_INFINITY, -50.0, -50.0],
            rms: vec![-12.0, f32::NEG_INFINITY, -60.0, -60.0],
            clip: vec![ClipFlags::NONE; 4],
        })),
    ] {
        latest.frames.insert(f.topic.to_string().into(), f);
    }
    assert!(latest.get(&Topic::PreviewLevels).is_some());
    t.st.data = Some(Arc::new(crate::conn::DataSnapshot {
        latest,
        grids: Default::default(),
        drained: Instant::now(),
    }));
    // A measurement dialog shows the session's meters.
    t.st.update(Msg::Command(CommandId::NewTransfer), &t.keys);
    let m = t.st.input_meters();
    assert_eq!(m.len(), 2);
    assert_eq!(m[&0].text, "\u{2212}20.0");
    assert_eq!(m[&1].state, ac2_scene::meter::MeterState::Clip);
    t.key("Escape");
    // The session dialog on a device the session does not capture: its preview.
    open_dialog(&mut t, backends(true));
    let m = t.st.input_meters();
    assert_eq!(m.len(), 4);
    assert_eq!(m[&0].text, "\u{2212}12.0");
    assert_eq!(m[&1].state, ac2_scene::meter::MeterState::Silent);
}

/// The sidebar's always-on meters: every captured input by name and role, its reading,
/// and what the running sweep (else the selected measurement) uses it as.
#[test]
fn sidebar_meters_name_every_session_input_by_role() {
    use ac2_client::{Latest, TopicFrame};
    use ac2_proto::frame::{ClipFlags, SessionLevelsFrame};
    use ac2_proto::{Frame, FrameData};
    use ac2_scene::meter::{InputUse, MeterState};
    let mut t = T::new();
    let mut s = daemon_state();
    if let Some(o) = &mut s.session.open {
        o.backend = BackendKind::Fake;
        o.config.input_channels = vec![0, 1, 2];
        o.config.loopback = Some(LoopbackRoute {
            output: 0,
            input: 0,
        });
    }
    s.inputs = vec![InputSetup {
        channel: 1,
        mic: Some("MM1 34804".into()),
        curve: CurveChoice::Curve {
            label: "90°".into(),
        },
    }];
    s.mics = vec![mm1()];
    t.conn(mirror(s.clone()));
    let labels = |t: &T| -> Vec<(String, Option<InputUse>)> {
        t.st.session_inputs()
            .into_iter()
            .map(|r| (r.label, r.used))
            .collect()
    };
    // Before the device list: the mic's name, else `Input N`, with the role.
    assert_eq!(
        labels(&t),
        vec![
            ("Input 1 · reference".into(), Some(InputUse::Reference)),
            (
                "MM1 34804 · 90° · mic (in 2)".into(),
                Some(InputUse::Measurement)
            ),
            ("Input 3".into(), None),
        ]
    );
    // The device's channel names once listed.
    t.conn(ConnEvent::Devices(Ok(backends(true))));
    assert_eq!(
        labels(&t),
        vec![
            (
                "Loop return · reference (in 1)".into(),
                Some(InputUse::Reference)
            ),
            (
                "MM1 34804 · 90° · mic (in 2)".into(),
                Some(InputUse::Measurement)
            ),
            ("Line 3 (in 3)".into(), None),
        ]
    );
    // Readings from the session's meters; an input without one reads as no data.
    let mut latest = Latest::default();
    let data = FrameData::SessionLevels(SessionLevelsFrame {
        meta: ac2_proto::frame::LevelsMeta {
            channels: vec![0, 1],
        },
        peak: vec![-10.0, -1.0],
        rms: vec![-20.0, -12.0],
        clip: vec![ClipFlags::NONE, ClipFlags::HELD],
    });
    let f = TopicFrame {
        topic: data.topic(),
        frame: Arc::new(Frame {
            stamp: ac2_proto::samples::stamp(None),
            data,
        }),
        received: Instant::now(),
        since_new: std::time::Duration::ZERO,
        age: Some(0.0),
        stale: false,
    };
    latest.frames.insert(f.topic.to_string().into(), f);
    t.st.data = Some(Arc::new(crate::conn::DataSnapshot {
        latest,
        grids: Default::default(),
        drained: Instant::now(),
    }));
    let rows = t.st.session_inputs();
    assert_eq!(rows[0].reading.with_unit(), "\u{2212}20.0 dBFS");
    assert_eq!(rows[0].reading.state, MeterState::Signal);
    assert_eq!(rows[1].reading.state, MeterState::Clip);
    assert_eq!(rows[2].reading.state, MeterState::NoData);

    // A running sweep's inputs are the marked ones while it runs.
    let mut run = sweep_run(SweepStatus::Playing { repeat: 1 });
    run.reference_input = 2;
    s.sweep = Some(run);
    t.conn(mirror(s.clone()));
    let used: Vec<_> = labels(&t).into_iter().map(|(_, u)| u).collect();
    assert_eq!(
        used,
        vec![None, Some(InputUse::Measurement), Some(InputUse::Reference)]
    );
}

/// The meters stay subscribed while a session is open, whatever dialog opens and closes,
/// and the subscription ends with the session.
#[test]
fn session_meters_stay_subscribed_while_a_session_is_open() {
    let mut t = T::disconnected();
    connected_to(&mut t, "local daemon");
    let r = t.conn(mirror(daemon_state()));
    assert!(
        r.iter().any(|x| matches!(x, Request::Meters(true))),
        "{r:?}"
    );
    let r = t.st.update(Msg::Command(CommandId::NewTransfer), &t.keys);
    assert!(!r.iter().any(|x| matches!(x, Request::Meters(_))), "{r:?}");
    let r = t.key("Escape");
    assert!(!r.iter().any(|x| matches!(x, Request::Meters(_))), "{r:?}");
    let r = t.conn(mirror(no_session_state()));
    assert!(
        r.iter().any(|x| matches!(x, Request::Meters(false))),
        "{r:?}"
    );
}

/// A set of sweeps on the progress strip: which one of how many, the bar, the time left
/// counted from when each step was seen, analysing, and gone once it ends.
#[test]
fn sweep_progress_strip_counts_steps_and_time_left() {
    let mut t = T::new();
    let tick = |t: &mut T, now_s: f64| {
        t.st.update(Msg::Tick { now_s, dt_s: 0.02 }, &t.keys);
    };
    tick(&mut t, 10.0);
    assert_eq!(t.st.operation(), None);
    let mut s = daemon_state();
    s.generator.owner = Some(ClientId("c1".into()));
    s.generator.armed = true;
    s.generator.firing = true;
    let mut run = sweep_run(SweepStatus::Playing { repeat: 1 });
    run.repeats = 2;
    // Each step: 3.1 s of sweep and 1 s of silence.
    s.sweep = Some(run.clone());
    t.conn(mirror(s.clone()));
    let p = t.st.operation().expect("progress");
    assert_eq!(p.title, "sweep \"Run 1\"");
    assert_eq!(p.step, "sweep 1 of 2");
    assert_eq!(p.fraction, 0.0);
    assert_eq!(p.remaining.as_deref(), Some("about 9 s left"));
    tick(&mut t, 12.05);
    let p = t.st.operation().expect("progress");
    assert!((p.fraction - 0.25).abs() < 1e-3, "{}", p.fraction);
    assert_eq!(p.remaining.as_deref(), Some("about 7 s left"));

    tick(&mut t, 14.0);
    run.status = SweepStatus::Playing { repeat: 2 };
    s.sweep = Some(run.clone());
    t.conn(mirror(s.clone()));
    let p = t.st.operation().expect("progress");
    assert_eq!(p.step, "sweep 2 of 2");
    assert!((p.fraction - 0.5).abs() < 1e-3, "{}", p.fraction);
    assert_eq!(p.remaining.as_deref(), Some("about 5 s left"));

    // Stop (the strip's button sends the same command as Esc).
    let r = t.st.update(Msg::Command(CommandId::StimulusStop), &t.keys);
    assert!(r.iter().any(|x| matches!(x, Request::StimStop)), "{r:?}");

    run.status = SweepStatus::Analysing;
    s.sweep = Some(run.clone());
    t.conn(mirror(s.clone()));
    let p = t.st.operation().expect("progress");
    assert_eq!(p.step, "analysing…");
    assert_eq!(p.remaining, None);

    run.status = SweepStatus::Failed {
        reason: SweepFailure::Stopped,
        msg: "stopped".into(),
    };
    s.sweep = Some(run);
    t.conn(mirror(s));
    assert_eq!(t.st.operation(), None);
    assert_eq!(t.st.sweep.step_seen, None);
}

#[test]
fn record_toggles_and_the_indicator_names_the_inputs() {
    let mut t = T::new();
    assert_eq!(t.st.recording_label(), None);
    let r = t.st.update(Msg::Command(CommandId::Record), &t.keys);
    let [
        Request::Call {
            cmd: Command::RecStart { request },
            ..
        },
    ] = r.as_slice()
    else {
        panic!("{r:?}");
    };
    assert_eq!(request.inputs, vec![0, 1], "every input of the session");
    assert_eq!(request.max_duration, Seconds(RECORD_MAX_S));

    let mut s = daemon_state();
    s.recording = Some(RecordingRun {
        name: "rec-x".into(),
        path: "/r/rec-x.wav".into(),
        inputs: vec![0, 1],
        sample_rate_hz: 48_000,
        session_epoch: s.session.epoch,
        start_sample: SampleIndex(0),
        started_at: WallNs(0),
        started_by: ClientId("c1".into()),
        frames: 48_000 * 5,
        bytes: 116 + 48_000 * 5 * 8,
        discontinuities: 0,
        max_duration: Seconds(RECORD_MAX_S),
        max_bytes: None,
        status: RecordingStatus::Recording,
    });
    t.conn(mirror(s.clone()));
    let l = t.st.recording_label().expect("indicator");
    assert_eq!(l.text, "REC 0:05 · 1.9 MB");
    assert!(
        l.detail
            .starts_with("Recording Input 1, Input 2 to /r/rec-x.wav"),
        "{}",
        l.detail
    );
    let r = t.st.update(Msg::Command(CommandId::Record), &t.keys);
    assert!(
        matches!(
            r.as_slice(),
            [Request::Call {
                cmd: Command::RecStop,
                ..
            }]
        ),
        "{r:?}"
    );

    // No session, nothing recording: the toggle says how to open one.
    t.conn(mirror(no_session_state()));
    assert!(
        t.st.update(Msg::Command(CommandId::Record), &t.keys)
            .is_empty()
    );
    assert!(t.last_toast().contains("no audio session to record"));

    // Replay asks for the recording and plays it in real time.
    t.st.update(Msg::Command(CommandId::ReplayRecording), &t.keys);
    t.text("rec-x");
    let r = t.key("Enter");
    assert!(
        matches!(r.as_slice(), [Request::Call { cmd: Command::SessionReplay {
            recording: RecordingRef::Name { name },
            pace: ReplayPace::Realtime,
        }, .. }] if name == "rec-x"),
        "{r:?}"
    );
}

#[test]
fn close_session_and_delete_measurement() {
    let mut t = T::new();
    let r = t.st.update(Msg::Command(CommandId::CloseSession), &t.keys);
    assert!(
        matches!(
            r.as_slice(),
            [Request::Call {
                cmd: Command::SessionClose,
                ..
            }]
        ),
        "{r:?}"
    );
    t.st.update(Msg::Command(CommandId::DeleteSelected), &t.keys);
    let r = t.key("Enter");
    assert!(
        matches!(r.as_slice(), [Request::Call { cmd: Command::MeasDelete { meas: MeasId(1), traces: ac2_proto::model::OwnedTraces::Keep }, what }]
            if what == "Main L deleted"),
        "{r:?}"
    );
    t.conn(mirror(no_session_state()));
    assert!(
        t.st.update(Msg::Command(CommandId::CloseSession), &t.keys)
            .is_empty()
    );
    assert!(t.last_toast().contains("no audio session"));
    assert!(
        t.st.update(Msg::Command(CommandId::DeleteSelected), &t.keys)
            .is_empty()
    );
    assert_eq!(t.st.overlay, Overlay::None);
    assert!(t.last_toast().contains("select a measurement"));
}

#[test]
fn new_measurements_start_and_become_selected() {
    let mut t = T::new();
    assert_eq!(t.st.selected, Some(MeasId(1)));
    t.st.update(Msg::Command(CommandId::NewTransfer), &t.keys);
    assert_eq!(form(&t).kind, FormKind::Transfer);
    let r = t.key("Enter");
    let c = created(&r).expect("meas.create");
    // One transfer exists, so this is the second; inputs from the session (1 → 2).
    assert_eq!(c.name, "Reference → Input 2");
    assert!(matches!(
        &c.kind,
        MeasKind::Transfer { config } if config.reference_input == 0
            && config.measurement_input == 1
            && config.depth == DepthPolicy::EqualConfidence
    ));
    assert_eq!(t.st.overlay, Overlay::None);
    // Created: selected at once, kept while the mirror has not caught up, then confirmed.
    let m = meas(7, "Reference → Input 2", transfer());
    t.conn(ConnEvent::MeasCreated(Box::new(m.clone())));
    assert_eq!(t.st.selected, Some(MeasId(7)));
    t.conn(mirror(daemon_state()));
    assert_eq!(t.st.selected, Some(MeasId(7)));
    let mut s = daemon_state();
    let before = t.st.layout.panes();
    // Another client's measurement shows in the list only: the panes stay as they are.
    s.measurements.push(meas(8, "Their TF", transfer()));
    t.conn(mirror(s.clone()));
    assert_eq!(t.st.layout.panes(), before);
    s.measurements.push(m);
    t.conn(mirror(s.clone()));
    assert_eq!(t.st.selected, Some(MeasId(7)));
    // The new transfer measurement opens in the transfer stack, its three panes showing it,
    // the magnitude on top with the focus.
    let l = &t.st.layout;
    let stack: Vec<_> = l
        .panes()
        .into_iter()
        .filter(|id| l.view(*id).is_some_and(|v| v.meas == Some(MeasId(7))))
        .map(|id| {
            let v = l.view(id).expect("view");
            (v.kind, v.modes.transfer, t.st.pane_meas(id).map(|m| m.id))
        })
        .collect();
    assert_eq!(
        stack,
        [
            (PaneKind::Transfer, TransferView::Magnitude, Some(MeasId(7))),
            (PaneKind::Transfer, TransferView::Phase, Some(MeasId(7))),
            (PaneKind::Transfer, TransferView::Coherence, Some(MeasId(7))),
        ]
    );
    assert_eq!(l.panes().len(), before.len() + 2);
    assert_eq!(t.st.pane_meas(l.focus).map(|m| m.id), Some(MeasId(7)));
    assert_eq!(l.focused().modes.transfer, TransferView::Magnitude);
    // Laid out once: a later mirror leaves the panes as the operator arranges them.
    t.conn(mirror(s.clone()));
    assert_eq!(t.st.layout.panes().len(), before.len() + 2);
    // The selection is the operator's again: Tab moves on and a later mirror keeps it.
    t.key("Tab");
    assert!(t.st.live_row_selected() || t.st.selected != Some(MeasId(7)));
    let sel = (t.st.selected, t.st.live_row_selected());
    t.conn(mirror(s));
    assert_eq!((t.st.selected, t.st.live_row_selected()), sel);

    // Spectrum, RTA and SPL meter dialogs make their kinds on the first non-reference input.
    for (cmd, want) in [
        (CommandId::NewSpectrum, "spectrum"),
        (CommandId::NewRta, "rta"),
        (CommandId::NewSpl, "spl"),
    ] {
        t.st.update(Msg::Command(cmd), &t.keys);
        let r = t.key("Enter");
        let c = created(&r).unwrap_or_else(|| panic!("{want}: {r:?}"));
        let kind = match &c.kind {
            MeasKind::Spectrum { config } => ("spectrum", config.input),
            MeasKind::Rta { config } => ("rta", config.input),
            MeasKind::Spl { config } => ("spl", config.input),
            MeasKind::Transfer { .. } | MeasKind::Math { .. } | MeasKind::Sweep { .. } => {
                ("tf", 99)
            }
        };
        assert_eq!(kind, (want, 1));
    }
    // Inputs are picked by name among the captured ones: ←/→ walks them, stopping at the
    // ends.
    t.st.update(Msg::Command(CommandId::NewSpl), &t.keys);
    assert_eq!(form(&t).channel(crate::forms::FieldId::Input), Some(1));
    assert_eq!(form(&t).fields[0].display(), "2 · Input 2");
    t.key("Right");
    assert_eq!(form(&t).channel(crate::forms::FieldId::Input), Some(1));
    t.key("Left");
    assert_eq!(form(&t).channel(crate::forms::FieldId::Input), Some(0));
    t.key("Left");
    assert_eq!(form(&t).channel(crate::forms::FieldId::Input), Some(0));
    t.key("Escape");
    assert_eq!(t.st.overlay, Overlay::None);

    // No session: nothing to measure on; the Sessions dialog, where one is started.
    t.conn(mirror(no_session_state()));
    let r = t.st.update(Msg::Command(CommandId::NewRta), &t.keys);
    assert!(matches!(r.as_slice(), [Request::SessionList]), "{r:?}");
    assert!(t.st.sessions_dialog().is_some(), "{:?}", t.st.overlay);
}
