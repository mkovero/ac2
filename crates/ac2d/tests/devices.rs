//! Device listing, session input meters, the capture-only preview and loopback detection,
//! against the fake rig.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ac2_audio::fake::{FakeDrive, FakePath, Pace};
use ac2_audio::{
    AudioError, Backend, BackendKind as AudioKind, DeviceCaps, DuplexRequest, DuplexStream,
    FakeBackend, FakeConfig, OutputSource,
};
use ac2_proto::frame::{ClipFlags, FrameData};
use ac2_proto::model::{
    Availability, BackendKind, DeviceId, GeneratorDesired, GeneratorSettings, Signal,
};
use ac2_proto::units::{Dbfs, LeaseToken};
use ac2_proto::{Command, ErrorCode, ReplyBody, Subscription, Topic};
use ac2d::{Daemon, DaemonConfig};

use common::*;

const T: Duration = Duration::from_secs(5);

/// Every stream request a backend was asked for: output channel count and whether the
/// output was a generator.
type Opened = Arc<Mutex<Vec<(u16, bool)>>>;

/// The fake rig behind a recorder of what was opened.
#[derive(Debug)]
struct Recording {
    inner: FakeBackend,
    opened: Opened,
}

impl Backend for Recording {
    fn kind(&self) -> AudioKind {
        self.inner.kind()
    }

    fn enumerate(&self) -> Result<Vec<DeviceCaps>, AudioError> {
        self.inner.enumerate()
    }

    fn probe(&self, device: &ac2_audio::DeviceSelector) -> ac2_audio::Presence {
        self.inner.probe(device)
    }

    fn open(&self, request: DuplexRequest) -> Result<DuplexStream, AudioError> {
        let generator = matches!(request.output, OutputSource::Generator(_));
        self.opened
            .lock()
            .unwrap()
            .push((request.output_channels, generator));
        self.inner.open(request)
    }
}

fn named_rig(drive: FakeDrive, noise: f32) -> FakeBackend {
    let names = |n: &[&str]| Some(n.iter().map(|s| (*s).to_owned()).collect());
    FakeBackend::new(FakeConfig {
        sample_rate: FS,
        block_frames: BLOCK,
        inputs: 4,
        outputs: 2,
        drive,
        seed: 3,
        input_noise_rms: noise,
        paths: vec![
            FakePath::loopback(0, 0, LOOP_DELAY),
            FakePath::acoustic(0, 1, LOOP_DELAY + ACOUSTIC_DELAY, vec![ACOUSTIC_GAIN], 1e-5),
        ],
        input_names: names(&["Loop", "Mic A", "Mic B", "Spare"]),
        output_names: names(&["Main", "Aux"]),
        ..FakeConfig::default()
    })
    .unwrap()
}

fn start(backend: Arc<dyn Backend>) -> ac2d::Handle {
    init_log();
    Daemon::start(DaemonConfig::new(backend, inproc("devices"), -10.0)).unwrap()
}

fn fake_dev() -> DeviceId {
    DeviceId(ac2_audio::fake::FAKE_DEVICE_ID.into())
}

fn acquire(c: &mut Client) -> LeaseToken {
    match c.ok(Command::GenAcquire { force: false }) {
        ReplyBody::Lease(l) => l.lease_token,
        other => panic!("{other:?}"),
    }
}

#[test]
fn devices_list_backends_with_channel_names() {
    let h = start(Arc::new(named_rig(FakeDrive::Manual, 0.0)));
    let (mut c, _s) = connect(&h, &[]);
    let ReplyBody::Backends(b) = c.ok(Command::SessionDevices) else {
        panic!("backends");
    };
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].kind, BackendKind::Fake);
    assert_eq!(b[0].availability, Availability::Available);
    assert!(
        b[0].description.contains("Simulated"),
        "{}",
        b[0].description
    );
    let d = &b[0].devices[0];
    let input = d.input.as_ref().unwrap();
    assert_eq!(input.max_channels, 4);
    assert_eq!(input.default_rate_hz, Some(FS));
    assert_eq!(input.default_buffer_frames, Some(BLOCK));
    assert_eq!(
        input.channel_names.as_deref(),
        Some(&["Loop", "Mic A", "Mic B", "Spare"].map(String::from)[..])
    );
    assert_eq!(
        d.output.as_ref().unwrap().channel_names.as_deref(),
        Some(&["Main".to_owned(), "Aux".to_owned()][..])
    );
    // A session on a backend the daemon does not offer is refused, never redirected.
    let mut cfg = session(true);
    cfg.backend = Some(BackendKind::Jack);
    let e = c.call(Command::SessionOpen { config: cfg }).unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    // The default backend is the one the daemon names in the open session.
    let ReplyBody::Session(s) = c.ok(Command::SessionOpen {
        config: session(true),
    }) else {
        panic!("session");
    };
    assert_eq!(s.open.unwrap().backend, BackendKind::Fake);
}

/// `session/levels` meters every session input with no measurement running, at most
/// ~30 frames per second, only while subscribed.
#[test]
fn session_levels_meter_every_input_without_measurements() {
    let rig = named_rig(FakeDrive::Manual, 0.01);
    let fake = rig.clone();
    let h = start(Arc::new(rig));
    let (mut c, s) = connect(&h, &[&Subscription::InputMeters.prefix()]);
    let mut cfg = session(true);
    cfg.input_channels = vec![0, 1, 3];
    c.ok(Command::SessionOpen { config: cfg });
    let mut d = driver(&fake);
    run(&mut d, 1.5);
    let mut frames = Vec::new();
    while let Some(f) = s.frame(Duration::from_millis(300), |f| {
        matches!(f.data, FrameData::SessionLevels(_))
    }) {
        frames.push(f);
    }
    assert!(!frames.is_empty(), "no session/levels frames");
    let FrameData::SessionLevels(last) = &frames.last().unwrap().data else {
        unreachable!()
    };
    assert_eq!(last.meta.channels, vec![0, 1, 3]);
    // Gaussian noise of RMS 0.01 is −37.0 dBFS (0 dBFS = RMS of a full-scale sine).
    let want = 20.0 * (0.01f64 * std::f64::consts::SQRT_2).log10();
    for (i, r) in last.rms.iter().enumerate() {
        assert!((f64::from(*r) - want).abs() < 0.5, "input {i}: {r} dBFS");
    }
    assert!(last.clip.iter().all(|c| *c == ClipFlags::NONE));
    assert_eq!(frames[0].topic(), Topic::SessionLevels);
    // 1.5 s of audio stepped in ~30 chunks: the rate limit keeps the count near 30/s of
    // wall time, never one frame per block (280 blocks).
    assert!(frames.len() < 120, "{} frames", frames.len());
}

/// The preview meters every input of a device and never opens an output; while it runs the
/// loopback input stays silent.
#[test]
fn preview_is_capture_only_and_silent() {
    let opened: Opened = Arc::default();
    let rec = Recording {
        inner: named_rig(FakeDrive::Thread(Pace::Realtime), 0.0),
        opened: Arc::clone(&opened),
    };
    let h = start(Arc::new(rec));
    let (mut c, s) = connect(&h, &[&Subscription::InputMeters.prefix()]);
    let ReplyBody::Preview(p) = c.ok(Command::SessionPreview {
        backend: BackendKind::Fake,
        device: fake_dev(),
    }) else {
        panic!("preview");
    };
    assert_eq!(p.channels, 4);
    assert_eq!(p.sample_rate_hz, FS);
    let f = s
        .frame(T, |f| matches!(f.data, FrameData::PreviewLevels(_)))
        .expect("preview frame");
    let FrameData::PreviewLevels(l) = &f.data else {
        unreachable!()
    };
    assert_eq!(l.meta.backend, BackendKind::Fake);
    assert_eq!(l.meta.device, fake_dev());
    assert_eq!(l.meta.channels, vec![0, 1, 2, 3]);
    // Out 1 loops back into in 1 on this rig: anything emitted would show there (in 2
    // carries the simulated room's noise floor).
    for _ in 0..10 {
        let f = s
            .frame(T, |f| matches!(f.data, FrameData::PreviewLevels(_)))
            .expect("preview frame");
        let FrameData::PreviewLevels(l) = &f.data else {
            unreachable!()
        };
        assert_eq!(l.peak[0], f32::NEG_INFINITY, "{:?}", l.peak);
        assert!(l.peak[1] < -80.0, "{:?}", l.peak);
    }
    // Renewing keeps the one stream; stopping closes it.
    c.ok(Command::SessionPreview {
        backend: BackendKind::Fake,
        device: fake_dev(),
    });
    c.ok(Command::SessionPreviewStop);
    assert_eq!(*opened.lock().unwrap(), vec![(0, false)]);
    // A device the backend does not list is refused.
    let e = c
        .call(Command::SessionPreview {
            backend: BackendKind::Fake,
            device: DeviceId("nope".into()),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
}

#[test]
fn preview_expires_and_closes_when_a_session_opens() {
    let opened: Opened = Arc::default();
    let rec = Recording {
        inner: named_rig(FakeDrive::Thread(Pace::Realtime), 0.0),
        opened: Arc::clone(&opened),
    };
    let h = start(Arc::new(rec));
    let (mut c, s) = connect(&h, &[&Subscription::InputMeters.prefix()]);
    c.ok(Command::SessionPreview {
        backend: BackendKind::Fake,
        device: fake_dev(),
    });
    s.frame(T, |f| matches!(f.data, FrameData::PreviewLevels(_)))
        .expect("preview frame");
    c.ok(Command::SessionOpen {
        config: session(true),
    });
    // No preview frame arrives once the session has replaced it.
    let quiet_from = Instant::now() + Duration::from_millis(200);
    let end = quiet_from + Duration::from_millis(600);
    let (mut late, mut session) = (0, 0);
    while Instant::now() < end {
        match s
            .frame(Duration::from_millis(100), |_| true)
            .map(|f| f.data)
        {
            Some(FrameData::PreviewLevels(_)) if Instant::now() > quiet_from => late += 1,
            Some(FrameData::SessionLevels(_)) => session += 1,
            _ => {}
        }
    }
    assert_eq!(late, 0);
    assert!(session > 0, "the session meters took over");
    assert_eq!(opened.lock().unwrap().len(), 2);

    // A preview nobody renews closes by itself.
    c.ok(Command::SessionClose);
    c.ok(Command::SessionPreview {
        backend: BackendKind::Fake,
        device: fake_dev(),
    });
    s.frame(T, |f| matches!(f.data, FrameData::PreviewLevels(_)))
        .expect("preview frame");
    std::thread::sleep(Duration::from_millis(5_600));
    while s.frame(Duration::from_millis(50), |_| true).is_some() {}
    assert!(
        s.frame(Duration::from_millis(500), |f| matches!(
            f.data,
            FrameData::PreviewLevels(_)
        ))
        .is_none(),
        "the preview outlived its expiry"
    );
}

#[test]
fn detect_loopback_finds_in_1_on_the_fake_rig() {
    let opened: Opened = Arc::default();
    let rec = Recording {
        inner: named_rig(FakeDrive::Thread(Pace::Realtime), 0.0),
        opened: Arc::clone(&opened),
    };
    let h = start(Arc::new(rec));
    let (mut c, _s) = connect(&h, &[]);
    let detect = |token, level| Command::SessionDetectLoopback {
        lease_token: token,
        backend: BackendKind::Fake,
        input_device: fake_dev(),
        output_device: fake_dev(),
        output: 0,
        level,
    };
    // Refused without the lease and without a level; nothing opened.
    let e = c
        .call(detect(LeaseToken(7), Some(Dbfs(-30.0))))
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::LeaseRequired);
    let token = acquire(&mut c);
    let e = c.call(detect(token, None)).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused);
    let e = c.call(detect(token, Some(Dbfs(-3.0)))).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "above the −10 dBFS ceiling");
    let e = c
        .call(Command::SessionDetectLoopback {
            lease_token: token,
            backend: BackendKind::Fake,
            input_device: fake_dev(),
            output_device: fake_dev(),
            output: 5,
            level: Some(Dbfs(-30.0)),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid, "{e:?}");
    assert!(
        opened.lock().unwrap().iter().all(|(_, g)| !g),
        "no emitting stream yet"
    );
    let ReplyBody::LoopbackDetection(d) = c.ok(detect(token, Some(Dbfs(-30.0)))) else {
        panic!("detection");
    };
    assert_eq!(d.loopback, Some(0), "{d:?}");
    assert_eq!(d.ranked[0].input, 0);
    assert_eq!(d.ranked[0].delay_samples.0, i64::from(LOOP_DELAY));
    assert!(d.ranked[0].correlation > 0.99, "{d:?}");
    assert!(d.ranked[0].gain.is_some_and(|g| g.0.abs() < 0.5), "{d:?}");
    // The acoustic path ranks second, at its own delay and gain.
    assert_eq!(d.ranked[1].input, 1);
    assert_eq!(
        d.ranked[1].delay_samples.0,
        i64::from(LOOP_DELAY + ACOUSTIC_DELAY)
    );
    assert!(d.ranked[1].gain.is_some_and(|g| (g.0 + 6.0).abs() < 0.5));
    assert_eq!(d.ranked.len(), 4);
    assert!(d.ranked[2..].iter().all(|r| r.gain.is_none()));
    // One stream, the generator routed to the one output asked for.
    assert_eq!(*opened.lock().unwrap(), vec![(1, true)]);
    // Refused while the stimulus is armed (the burst outlasted the lease's refresh period:
    // take it again).
    c.ok(Command::SessionOpen {
        config: session(true),
    });
    let token = acquire(&mut c);
    c.ok(Command::GenSet {
        lease_token: token,
        desired: GeneratorDesired {
            settings: GeneratorSettings {
                signal: Signal::Pink,
                level: Dbfs(-30.0),
                band: None,
                outputs: vec![0],
            },
            armed: true,
            firing: false,
        },
    });
    let e = c.call(detect(token, Some(Dbfs(-30.0)))).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused);
}

/// Input and output listed as two devices (WASAPI endpoints): each says which direction it
/// has, the loopback is found across them, and a session opens with the output on the other
/// device, reported as of unknown clock relation.
#[test]
fn a_session_plays_on_another_device_than_it_captures_from() {
    use ac2_audio::FakeEndpoints;
    use ac2_audio::fake::{FAKE_INPUT_ID, FAKE_OUTPUT_ID};
    use ac2_proto::model::{ClockRelation, DeviceSelector};
    let mut cfg = named_rig(FakeDrive::Thread(Pace::Realtime), 0.0)
        .config()
        .clone();
    cfg.endpoints = FakeEndpoints::Split;
    let h = start(Arc::new(FakeBackend::new(cfg).unwrap()));
    let (mut c, _s) = connect(&h, &[]);
    let ReplyBody::Backends(b) = c.ok(Command::SessionDevices) else {
        panic!("backends");
    };
    let devs = &b[0].devices;
    assert_eq!(devs.len(), 2, "{devs:?}");
    let (input, output) = (&devs[0], &devs[1]);
    assert_eq!(input.id.0, FAKE_INPUT_ID);
    assert!(input.input.as_ref().is_some_and(|d| d.system_default));
    assert!(input.output.is_none());
    assert_eq!(output.id.0, FAKE_OUTPUT_ID);
    assert!(output.input.is_none());
    assert_eq!(output.output.as_ref().map(|d| d.max_channels), Some(2));
    assert_eq!(input.duplex_clock, ClockRelation::Unknown);

    let (in_id, out_id) = (input.id.clone(), output.id.clone());
    let token = acquire(&mut c);
    // The input endpoint has no output to play the burst on.
    let e = c
        .call(Command::SessionDetectLoopback {
            lease_token: token,
            backend: BackendKind::Fake,
            input_device: in_id.clone(),
            output_device: in_id.clone(),
            output: 0,
            level: Some(Dbfs(-30.0)),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid, "{e:?}");
    let ReplyBody::LoopbackDetection(d) = c.ok(Command::SessionDetectLoopback {
        lease_token: token,
        backend: BackendKind::Fake,
        input_device: in_id.clone(),
        output_device: out_id.clone(),
        output: 0,
        level: Some(Dbfs(-30.0)),
    }) else {
        panic!("detection");
    };
    assert_eq!(d.loopback, Some(0), "{d:?}");
    assert_eq!((&d.input_device, &d.output_device), (&in_id, &out_id));

    let mut config = session(true);
    config.input_device = DeviceSelector::Id { id: in_id.clone() };
    config.output_device = DeviceSelector::Id { id: in_id.clone() };
    let e = c
        .call(Command::SessionOpen {
            config: config.clone(),
        })
        .unwrap_err();
    assert_eq!(
        e.code,
        ErrorCode::NotFound,
        "no outputs on the input endpoint: {e:?}"
    );
    config.output_device = DeviceSelector::Id { id: out_id.clone() };
    let ReplyBody::Session(s) = c.ok(Command::SessionOpen { config }) else {
        panic!("session");
    };
    let o = s.open.unwrap();
    assert_eq!((&o.input_device, &o.output_device), (&in_id, &out_id));
    assert_eq!(o.clock, ClockRelation::Unknown);
}

/// The fake rig presented as a second backend (JACK), so a test can move the preview
/// between backends the way the session dialog's ←/→ does, without a JACK server.
#[derive(Debug)]
struct AsJack(FakeBackend);

impl Backend for AsJack {
    fn kind(&self) -> AudioKind {
        AudioKind::Jack
    }

    fn enumerate(&self) -> Result<Vec<DeviceCaps>, AudioError> {
        let mut caps = self.0.enumerate()?;
        for c in &mut caps {
            c.backend = AudioKind::Jack;
        }
        Ok(caps)
    }

    fn open(&self, request: DuplexRequest) -> Result<DuplexStream, AudioError> {
        self.0.open(request)
    }

    fn probe(&self, device: &ac2_audio::DeviceSelector) -> ac2_audio::Presence {
        self.0.probe(device)
    }
}

/// The next preview frame, skipping any of another device still in flight.
fn preview_of(s: &Sub, kind: BackendKind) -> Option<ac2_proto::frame::Frame> {
    s.frame(
        T,
        |f| matches!(&f.data, FrameData::PreviewLevels(l) if l.meta.backend == kind),
    )
}

/// Cycling the preview between backends and devices, closing and reopening the dialog
/// between rounds: every round meters the device asked for, at once and for as long as it
/// is renewed, and nothing of the previous device keeps arriving.
#[test]
fn preview_follows_the_device_through_every_round() {
    init_log();
    let fake: Arc<dyn Backend> = Arc::new(named_rig(FakeDrive::Thread(Pace::Realtime), 1e-3));
    let jack: Arc<dyn Backend> =
        Arc::new(AsJack(named_rig(FakeDrive::Thread(Pace::Realtime), 1e-3)));
    let mut cfg = DaemonConfig::new(Arc::clone(&fake), inproc("cycle"), -10.0);
    cfg.backends = vec![fake, jack];
    let h = Daemon::start(cfg).unwrap();
    let (mut c, s) = connect(&h, &[&Subscription::InputMeters.prefix()]);
    let preview = |c: &mut Client, kind| {
        c.ok(Command::SessionPreview {
            backend: kind,
            device: fake_dev(),
        })
    };
    for round in 0..3 {
        for kind in [BackendKind::Fake, BackendKind::Jack, BackendKind::Fake] {
            preview(&mut c, kind);
            let f = preview_of(&s, kind).unwrap_or_else(|| panic!("round {round}: {kind:?}"));
            let FrameData::PreviewLevels(l) = &f.data else {
                unreachable!()
            };
            // Noise of RMS 1e-3 on every input: live audio, not a held frame.
            assert!(l.rms.iter().all(|r| *r > -70.0), "{:?}", l.rms);
        }
        // Renewals keep it metering past the expiry.
        for _ in 0..3 {
            std::thread::sleep(Duration::from_millis(2_000));
            preview(&mut c, BackendKind::Fake);
            while s.frame(Duration::from_millis(20), |_| true).is_some() {}
            preview_of(&s, BackendKind::Fake).expect("renewed preview");
        }
        // The dialog closes: the device is released and nothing more arrives.
        c.ok(Command::SessionPreviewStop);
        while s.frame(Duration::from_millis(100), |_| true).is_some() {}
        assert!(
            s.frame(Duration::from_millis(300), |f| matches!(
                f.data,
                FrameData::PreviewLevels(_)
            ))
            .is_none(),
            "round {round}: a stopped preview kept metering"
        );
    }
}

/// A preview whose stream stopped delivering (the host stalled or ended it) is reopened by
/// the next renewal instead of being kept alive with blank meters.
#[test]
fn a_renewal_reopens_a_preview_that_stopped_delivering() {
    let opened: Opened = Arc::default();
    let mut rig = named_rig(FakeDrive::Thread(Pace::Realtime), 1e-3)
        .config()
        .clone();
    // About 0.2 s of audio per stream, then nothing.
    rig.stop_after_blocks = Some(40);
    let rec = Recording {
        inner: FakeBackend::new(rig).unwrap(),
        opened: Arc::clone(&opened),
    };
    let h = start(Arc::new(rec));
    let (mut c, s) = connect(&h, &[&Subscription::InputMeters.prefix()]);
    let preview = |c: &mut Client| {
        c.ok(Command::SessionPreview {
            backend: BackendKind::Fake,
            device: fake_dev(),
        })
    };
    preview(&mut c);
    preview_of(&s, BackendKind::Fake).expect("first frames");
    // The stream goes quiet; past the stall bound a renewal reopens it.
    std::thread::sleep(Duration::from_millis(2_500));
    while s.frame(Duration::from_millis(20), |_| true).is_some() {}
    preview(&mut c);
    preview_of(&s, BackendKind::Fake).expect("frames after the renewal reopened the stream");
    assert_eq!(opened.lock().unwrap().len(), 2);
}
