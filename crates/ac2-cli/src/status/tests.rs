use super::*;
use ac2_proto::model::{
    Anchor, AudioStopped, Autosave, BackendKind, CalEntry, CalKey, CalMethod, ClockBasis, Clocks,
    CrossRigTransfer, CurveChoice, DamagedFile, DeviceClock, DeviceId, DeviceSelector, Drift,
    ElectricalConnection, InputSetup, LoopbackRoute, Mic, MicCurveRef, MinuteCount,
    NotLockedReason, OpenSession, Pair, PairSide, PeerClock, Recovery, RemoteAudio, RemoteRig,
    RemoteSide, SampleClock, SensitivitySource, SessionConfig, SplCal, StandingRig,
    StandingSession, StateFile, StopCause, SystemClock,
};
use ac2_proto::units::{
    ClientId, DaemonIncarnation, Db, DbSpl, Dbfs, Hz, MvPerPa, Rev, RigId, Seconds, SessionEpoch,
    Volts,
};

const S: u64 = 1_000_000_000;
const NOW: WallNs = WallNs(1_790_000_000 * S);
const DAY: u64 = 86_400 * S;
const PI: RigId = RigId(0x000a_b1c6);
const KETUNKOLO: RigId = RigId(0x6b65_7475);
const REMOTE_SLOT_0: u16 = ac2_proto::model::REMOTE_BASE;

fn welcome(name: &str) -> Welcome {
    Welcome {
        server: "ac2d 0.1.0 (build 0.1.0+202486c60e44)".into(),
        client_id: ClientId("local-006b8b4567".into()),
        daemon_incarnation: DaemonIncarnation(0xaf47_6c4a_fd9f_f09b),
        session_epoch: SessionEpoch(12),
        rev: Rev(3274),
        rig_id: RigId(0x26b1_c286_d80b_073e),
        name: name.into(),
    }
}

fn utc(_: WallNs) -> i32 {
    0
}

fn page(name: &str, st: &State, verbose: bool) -> String {
    text(&Input {
        welcome: &welcome(name),
        session: &st.session,
        state: st,
        build: Build::Same,
        now: NOW,
        offset_s: &utc,
        verbose,
    })
}

fn open_session(input_channels: Vec<u16>, outputs: u16) -> OpenSession {
    OpenSession {
        config: SessionConfig {
            backend: Some(BackendKind::Jack),
            input_device: DeviceSelector::Default,
            output_device: DeviceSelector::Default,
            input_channels,
            spare_inputs: 0,
            output_channels: outputs,
            sample_rate_hz: Some(96_000),
            buffer_frames: Some(256),
            loopback: Some(LoopbackRoute {
                output: 0,
                input: 0,
            }),
            remote_inputs: Vec::new(),
            link_offset: None,
        },
        backend: BackendKind::Jack,
        input_device: DeviceId("jack".into()),
        output_device: DeviceId("jack".into()),
        sample_rate_hz: 96_000,
        buffer_frames: 256,
        clock: ClockRelation::SingleCallback,
        opened_at: WallNs(NOW.0 - 3_600 * S),
        replay: None,
        clock_source: None,
        driver_change: None,
        driver_reset: None,
        stated_latency: None,
        spare_ports: Vec::new(),
    }
}

fn mic_input(channel: u16, mic: Option<&str>, pair: Option<PairSide>) -> InputSetup {
    InputSetup {
        channel,
        kind: InputKind::Mic {
            mic: mic.map(Into::into),
            curve: match mic {
                Some(_) => CurveChoice::Curve {
                    label: "90°".into(),
                },
                None => CurveChoice::NotChosen,
            },
        },
        pair,
    }
}

fn line_input(channel: u16, pair: Option<PairSide>) -> InputSetup {
    InputSetup {
        channel,
        kind: InputKind::Line,
        pair,
    }
}

fn mm1() -> Mic {
    Mic {
        name: "MM1 34804".into(),
        curves: vec![MicCurveRef {
            label: "90°".into(),
            file_name: "449350_34804_90Grad.txt".into(),
            content_hash: "0123456789abcdef".into(),
            points: 100,
            f_lo: Hz(50.0),
            f_hi: Hz(20_000.0),
            imported_at: WallNs(NOW.0 - 60 * DAY),
            stated_sensitivity: Some(15.0),
        }],
    }
}

fn acoustic_cal(channel: u16, days_ago: u64) -> CalEntry {
    CalEntry {
        key: CalKey {
            device: DeviceId("jack".into()),
            channel,
            mic: "MM1 34804".into(),
        },
        spl: SplCal {
            sensitivity: Db(120.5),
            method: CalMethod::Acoustic {
                calibrator_level: DbSpl(114.0),
            },
            freq: Hz(1000.0),
            measured: Dbfs(-6.5),
            calibrated_at: WallNs(NOW.0 - days_ago * DAY),
        },
    }
}

fn electrical_cal(channel: u16, days_ago: u64) -> CalEntry {
    CalEntry {
        spl: SplCal {
            sensitivity: Db(135.31),
            method: CalMethod::Electrical {
                connection: ElectricalConnection::InLine,
                volts: Volts(0.015),
                full_scale: Volts(1.5),
                mic_sensitivity: MvPerPa(15.0),
                mic_sensitivity_from: SensitivitySource::DataSheet {
                    label: "90°".into(),
                    file_name: "449350_34804_90Grad.txt".into(),
                },
                loading: Some(ac2_proto::model::MicLoading {
                    mic_impedance: ac2_proto::units::Ohms(330.0),
                    input_impedance: ac2_proto::units::Ohms(2000.0),
                }),
                uncertainty: Db(1.0),
            },
            freq: Hz(1000.0),
            measured: Dbfs(-40.0),
            calibrated_at: WallNs(NOW.0 - days_ago * DAY),
        },
        ..acoustic_cal(channel, days_ago)
    }
}

fn clock(relation: PeerRelation, ppm: Option<f64>) -> PeerClock {
    PeerClock {
        relation,
        ppm,
        span: ppm.map(|_| Seconds(60.0)),
        last_change: None,
    }
}

/// A rig with nothing remote: the sample state's odds and ends cleared.
fn lone(input_channels: Vec<u16>) -> State {
    let mut st = ac2_proto::samples::state();
    st.session = Session {
        epoch: SessionEpoch(12),
        open: Some(open_session(input_channels, 6)),
        stopped: None,
    };
    st.timing.drift = Some(Drift {
        ppm: 1.3,
        span: Seconds(2.0),
        warning: false,
        at: NOW,
    });
    st.clocks = Clocks {
        system: SystemClock::Synced { maxerror_us: 2500 },
        capture: Some(DeviceClock {
            device: DeviceId("jack".into()),
            rate: SampleClock::Measured {
                ppm: 1.0,
                span: Seconds(60.0),
                against: ClockBasis::SystemClock,
            },
        }),
        stamp_map: None,
    };
    st.remote = RemoteState {
        peers: Vec::new(),
        rigs: Vec::new(),
        inputs: Vec::new(),
        link_offset: None,
        transfers: Vec::new(),
        standing: Vec::new(),
    };
    st.inputs = Vec::new();
    st.pairs = Vec::new();
    st.mics = vec![mm1()];
    st.calibrations = Vec::new();
    st.line_calibrations = Vec::new();
    st.measurements.truncate(1);
    st.autosave = Autosave {
        state: AutosaveState::Saved,
        saved_at: Some(WallNs(NOW.0 - 120 * S)),
        log_failures: Vec::new(),
    };
    st.rig.damaged_files = Vec::new();
    st
}

/// pupu as a rig reported it: eight inputs and one of ac2pi-0ab1c6's, a transfer function
/// stopped by that rig's clock, and ketunkolo's levels still being measured.
fn pupu() -> State {
    let mut st = lone((0..8).chain([REMOTE_SLOT_0]).collect());
    let pi = "ac2 on ac2pi-0ab1c6";
    let remote = RemoteInput {
        slot: 0,
        rig: PI,
        rig_name: pi.into(),
        channel: 0,
    };
    if let Some(o) = &mut st.session.open {
        o.config.remote_inputs = vec![remote];
    }
    let mut input = ac2_proto::samples::remote_state().inputs[0].clone();
    input.slot = 0;
    input.audio = RemoteAudio::Streaming;
    input.anchor = Some(Anchor::SystemTime);
    input.late_frames_hour = 1;
    input.slips = 0;
    let meas = st.measurements[0].id;
    st.measurements[0].config.name = "Reference -> usb-behku".into();
    if let Some(d) = &mut st.measurements[0].delay {
        d.applied = Seconds(-0.145_062);
    }
    st.remote = RemoteState {
        peers: Vec::new(),
        rigs: vec![RemoteRig {
            rig: PI,
            name: pi.into(),
            link: RemoteLink::Up,
            clock: clock(
                PeerRelation::NotLocked {
                    reason: NotLockedReason::OwnClock { rig: PI, ppm: 7.8 },
                },
                Some(7.8),
            ),
            bytes_per_s: 400_000.0,
            window: 262_144,
        }],
        inputs: vec![input],
        link_offset: Some(Seconds(0.015)),
        transfers: vec![CrossRigTransfer {
            meas,
            rig: PI,
            side: RemoteSide::Measurement,
            state: CrossTfState::Stopped {
                why: StopWhy::NotLocked,
                held: true,
            },
            relock: None,
            slip: None,
            delay_drift: None,
            late: false,
        }],
        standing: vec![
            StandingRig {
                rig: PI,
                name: pi.into(),
                link: RemoteLink::Up,
                session: Some(StandingSession {
                    sample_rate_hz: 96_000,
                    channels: vec![0],
                }),
                clock: clock(
                    PeerRelation::NotLocked {
                        reason: NotLockedReason::CrossRate {
                            ppm: 7.7,
                            over: Seconds(60.0),
                        },
                    },
                    Some(7.7),
                ),
                marks_since: Some(WallNs(NOW.0 - 917 * S)),
                counting: None,
            },
            StandingRig {
                rig: KETUNKOLO,
                name: "ac2 on ketunkolo".into(),
                link: RemoteLink::Up,
                session: Some(StandingSession {
                    sample_rate_hz: 96_000,
                    channels: (0..8).collect(),
                }),
                clock: clock(PeerRelation::Measuring, Some(-0.69)),
                marks_since: Some(WallNs(NOW.0 - 958 * S)),
                counting: Some(MinuteCount {
                    beyond: 2,
                    counted: 2,
                    needed: 10,
                }),
            },
        ],
    };
    st.inputs = vec![
        mic_input(0, Some("MM1 34804"), Some(PairSide::Left)),
        mic_input(1, Some("MM1 34804"), Some(PairSide::Right)),
        line_input(2, None),
        line_input(4, Some(PairSide::Left)),
        line_input(5, Some(PairSide::Right)),
        mic_input(REMOTE_SLOT_0, None, None),
    ];
    st.pairs = vec![
        Pair {
            left: 0,
            right: 1,
            name: None,
        },
        Pair {
            left: 4,
            right: 5,
            name: None,
        },
    ];
    st.calibrations = vec![electrical_cal(0, 43), acoustic_cal(1, 27)];
    st.autosave.state = AutosaveState::Pending;
    st
}

fn widest(t: &str) -> usize {
    t.lines().map(|l| l.chars().count()).max().unwrap_or(0)
}

#[test]
fn pupu_reads_at_a_glance() {
    let t = page("pupu", &pupu(), false);
    assert_eq!(t, PUPU);
    assert!(widest(&t) <= PAGE, "{}", widest(&t));
}

#[test]
fn pupu_verbose_adds_identifiers_and_the_long_words() {
    let t = page("pupu", &pupu(), true);
    assert_eq!(t, PUPU_VERBOSE);
}

const PUPU: &str = include_str!("golden/pupu.txt");
const PUPU_VERBOSE: &str = include_str!("golden/pupu_verbose.txt");

#[test]
fn each_rig_is_named_once_and_no_remote_input_by_number() {
    let t = page("pupu", &pupu(), false);
    let rigs = t
        .split("\n\n")
        .find(|s| s.starts_with("RIGS"))
        .expect("a RIGS section");
    assert_eq!(rigs.matches("ac2 on ac2pi-0ab1c6").count(), 1, "{rigs}");
    assert_eq!(rigs.matches("ac2 on ketunkolo").count(), 1, "{rigs}");
    assert!(!t.contains("32769"), "{t}");
}

#[test]
fn trouble_lines_and_only_they_start_with_a_bang() {
    let t = page("pupu", &pupu(), false);
    let bangs: Vec<&str> = t.lines().filter(|l| l.starts_with('!')).collect();
    assert!(
        bangs[0].starts_with("! ac2 on ac2pi-0ab1c6 ") && bangs[0].contains("CLOCKS NOT LOCKED"),
        "{bangs:?}"
    );
    assert!(
        bangs
            .iter()
            .any(|l| l.starts_with("! Reference -> usb-behku"))
    );
    // A late frame in the hour is trouble for the input it came on; uncalibrated is not.
    assert!(bangs.iter().any(|l| l.contains("in 1 streaming")));
    assert!(
        !bangs.iter().any(|l| l.contains("uncalibrated")),
        "{bangs:?}"
    );
    for l in t.lines().filter(|l| !l.starts_with('!')) {
        assert!(
            l.is_empty() || l.starts_with("  ") || l.starts_with(char::is_alphanumeric),
            "{l}"
        );
    }
}

#[test]
fn levels_disagreeing_with_the_session_are_said_beside_it() {
    let mut st = pupu();
    st.remote.standing[0].clock = clock(PeerRelation::Agree, Some(0.12));
    let t = page("pupu", &st, false);
    assert!(
        t.contains("levels: clocks agree · +0.12 ppm over 60 s"),
        "{t}"
    );
    // Agreeing in state, the session's relation alone is said.
    let t = page("pupu", &pupu(), false);
    assert!(!t.contains("levels:"), "{t}");
}

#[test]
fn a_lone_rig_with_one_mic_has_no_rigs_or_measurements() {
    let mut st = lone(vec![0, 1]);
    st.inputs = vec![mic_input(0, Some("MM1 34804"), None)];
    st.calibrations = vec![acoustic_cal(0, 3)];
    let t = page("ac2 on studio", &st, false);
    assert_eq!(t, LONE);
    assert!(widest(&t) <= PAGE, "{}", widest(&t));
    let v = page("ac2 on studio", &st, true);
    assert_eq!(v, LONE_VERBOSE);
}

const LONE: &str = include_str!("golden/lone.txt");
const LONE_VERBOSE: &str = include_str!("golden/lone_verbose.txt");

#[test]
fn a_daemon_with_no_session_open_says_closed() {
    let mut st = lone(vec![]);
    st.session.open = None;
    st.session.stopped = None;
    let t = page("ac2 on studio", &st, false);
    assert_eq!(
        t,
        "ac2 on studio   ac2d 0.1.0+202486c6\n\
         \n\
         AUDIO   closed\n\
         \n\
         AUTOSAVE   autosaved 2 min ago"
    );
}

#[test]
fn stopped_audio_failed_autosave_and_damaged_files_are_trouble() {
    let mut st = lone(vec![0]);
    st.session.stopped = Some(AudioStopped {
        since: WallNs(NOW.0 - 12 * S),
        cause: StopCause::HostEnded,
        recovery: Recovery::Waiting {
            attempt: 2,
            error: "the simulated device is gone".into(),
            next_at: WallNs(NOW.0 + 3 * S),
        },
    });
    st.autosave.state = AutosaveState::Failed {
        reason: "disk full".into(),
    };
    st.rig.damaged_files.push(DamagedFile {
        kind: StateFile::Calibrations,
        path: "/c/calibrations.json".into(),
        set_aside_as: Some("/c/calibrations.json.damaged-5".into()),
        reason: "expected value; started without it".into(),
        at: WallNs(0),
    });
    let t = page("pupu", &st, false);
    assert!(
        t.contains("\n! audio   STOPPED · audio host ended the stream at "),
        "{t}"
    );
    assert!(t.contains(" · reopening (attempt 2, next in 3 s)\n"), "{t}");
    assert!(
        t.contains(
            "AUTOSAVE\n!           FAILED: disk full\n! damaged   calibrations calibrations.json \
             damaged: expected value; started without it; set aside as calibrations.json.damaged-5"
        ),
        "{t}"
    );
    let v = page("pupu", &st, true);
    assert!(
        v.contains("attempt 2 failed: the simulated device is gone · next in 3 s"),
        "{v}"
    );
}

#[test]
fn a_stale_local_daemon_says_how_to_restart_it() {
    let st = lone(vec![0]);
    let t = text(&Input {
        welcome: &welcome("pupu"),
        session: &st.session,
        state: &st,
        build: Build::Stale {
            daemon: "0.1.0+aaaa".into(),
            ours: "0.1.0+bbbb".into(),
        },
        now: NOW,
        offset_s: &utc,
        verbose: false,
    });
    assert!(
        t.starts_with(
            "pupu      ac2d 0.1.0+202486c6\n\
             ! STALE   daemon build 0.1.0+aaaa, this ac2 is 0.1.0+bbbb\n\
             !         restart it: `ac2 daemon stop && ac2 daemon start`\n"
        ),
        "{t}"
    );
}

#[test]
fn consecutive_channels_collapse_to_ranges() {
    assert_eq!(channel_ranges(&[0, 1, 2, 3, 4, 5, 6, 7]), "1–8");
    assert_eq!(channel_ranges(&[0, 1, 4, 6, 7, 8]), "1, 2, 5, 7–9");
    assert_eq!(channel_ranges(&[3]), "4");
    assert_eq!(channel_ranges(&[]), "");
}

#[test]
fn the_header_cuts_the_build_to_its_commit() {
    assert_eq!(
        server_short("ac2d 0.1.0 (build 0.1.0+202486c60e44)"),
        "ac2d 0.1.0+202486c6"
    );
    assert_eq!(server_short("ac2d 0.1.0 (build abc)"), "ac2d abc");
    assert_eq!(server_short("some daemon"), "some daemon");
}
