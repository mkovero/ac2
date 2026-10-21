//! The ASIO backend through [`AsioHost`]: scripted drivers as installed drivers, held,
//! missing and unplugged interfaces, panel changes, clock sources and clock events.

use super::*;

/// Scripted drivers as installed drivers: what [`AsioHost`] lists, loads and opens. Every
/// load of a name gets the same driver, which keeps its settings across loads as hardware
/// does.
struct Installed {
    drivers: Vec<(String, Arc<Scripted>)>,
    slot: &'static Slot,
    creates: AtomicUsize,
}

impl std::fmt::Debug for Installed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Installed")
    }
}

impl Installed {
    fn new(scripts: Vec<(&str, Script)>) -> Arc<Self> {
        let slot: &'static Slot = Box::leak(Box::new(Slot::new()));
        Arc::new(Self {
            drivers: scripts
                .into_iter()
                .map(|(n, s)| (n.to_owned(), Scripted::with_slot(s, slot)))
                .collect(),
            slot,
            creates: AtomicUsize::new(0),
        })
    }

    fn driver(&self, name: &str) -> Arc<Scripted> {
        let d = self
            .drivers
            .iter()
            .find(|(n, _)| n == name)
            .expect("driver");
        Arc::clone(&d.1)
    }
}

impl DriverSource for Arc<Installed> {
    fn installed(&self) -> Result<Vec<String>, AudioError> {
        Ok(self.drivers.iter().map(|(n, _)| n.clone()).collect())
    }
    fn create(&self, name: &str) -> Result<Arc<dyn AsioDriver>, AudioError> {
        self.creates.fetch_add(1, Ordering::SeqCst);
        Ok(self.driver(name) as Arc<dyn AsioDriver>)
    }
    fn slot(&self) -> &'static Slot {
        self.slot
    }
}

fn on(name: &str, req: DuplexRequest) -> DuplexRequest {
    let id = DeviceSelector::Id(DeviceId(name.into()));
    DuplexRequest {
        input_device: id.clone(),
        output_device: id,
        ..req
    }
}

fn unavailable(r: Result<DuplexStream, AudioError>) -> (Unavailability, String) {
    match r {
        Err(e @ AudioError::Unavailable { .. }) => {
            let text = e.to_string();
            let AudioError::Unavailable { reason, .. } = e else {
                unreachable!()
            };
            (reason, text)
        }
        Err(e) => panic!("not unavailable: {e:?}"),
        Ok(_) => panic!("opened"),
    }
}

#[test]
fn a_driver_held_by_another_program_is_named_in_use() {
    let rig = Installed::new(vec![("UMC ASIO Driver", Script::default())]);
    let host = AsioHost::new(Arc::clone(&rig));
    let drv = rig.driver("UMC ASIO Driver");

    // Refused at `init` without a word.
    *drv.init_refusal.lock().expect("scripted state") = Some(String::new());
    let (reason, text) = unavailable(host.open(DuplexRequest::new(vec![0], 0, level())));
    assert_eq!(
        reason,
        Unavailability::AsioDriverInUse {
            driver: "UMC ASIO Driver".into()
        }
    );
    assert!(
        text.ends_with(
            "UMC ASIO Driver is in use by another application (close the DAW or other audio \
             program using it)"
        ),
        "{text}"
    );

    // Loaded, then refused buffers.
    *drv.init_refusal.lock().expect("scripted state") = None;
    *drv.create_refusal.lock().expect("scripted state") = Some(AsioCode::HwMalfunction);
    let (reason, _) = unavailable(host.open(DuplexRequest::new(vec![0], 0, level())));
    assert!(
        matches!(reason, Unavailability::AsioDriverInUse { .. }),
        "{reason:?}"
    );

    // Released by the other program: the next open runs.
    *drv.create_refusal.lock().expect("scripted state") = None;
    let s = host
        .open(DuplexRequest::new(vec![0], 0, level()))
        .expect("open");
    assert_eq!(s.negotiated().input_device.0, "UMC ASIO Driver");
}

#[test]
fn an_interface_that_is_not_there_is_named_with_the_drivers_words() {
    let rig = Installed::new(vec![("Fireface USB", Script::default())]);
    let host = AsioHost::new(Arc::clone(&rig));
    let drv = rig.driver("Fireface USB");
    drv.present.store(false, Ordering::SeqCst);
    let (reason, text) = unavailable(host.open(DuplexRequest::new(vec![0], 0, level())));
    assert_eq!(
        reason,
        Unavailability::AsioHardwareNotPresent {
            driver: "Fireface USB".into(),
            detail: "No ASIO device found".into(),
        }
    );
    assert!(
        text.ends_with(
            "Fireface USB is installed but its interface is not present (connect it and switch \
             it on); the driver says: No ASIO device found"
        ),
        "{text}"
    );
    // A driver that loads but answers ASE_NotPresent to its first call.
    drv.present.store(true, Ordering::SeqCst);
    *drv.init_refusal.lock().expect("scripted state") = None;
    let s = host.open(DuplexRequest::new(vec![0], 0, level()));
    assert!(s.is_ok());
    drop(s);
    drv.present.store(false, Ordering::SeqCst);
    let e = super::super::open::refusal(
        "Fireface USB",
        super::super::open::Step::Other(crate::Operation::Open),
        drv.channel_counts().expect_err("unplugged"),
    );
    assert!(
        matches!(
            e,
            AudioError::Unavailable {
                reason: Unavailability::AsioHardwareNotPresent { .. },
                ..
            }
        ),
        "{e:?}"
    );
}

#[test]
fn an_unplugged_interface_ends_the_stream_and_reopens_once_back() {
    let rig = Installed::new(vec![("Fireface USB", Script::default())]);
    let host = AsioHost::new(Arc::clone(&rig));
    let drv = rig.driver("Fireface USB");
    let mut s = host
        .open(DuplexRequest::new(vec![0, 1], 2, level()))
        .expect("open");
    drv.tick();
    drv.tick();
    assert_eq!(blocks(&mut s).len(), 2);

    // Unplugged: the driver asks for a reset and stops switching.
    drv.present.store(false, Ordering::SeqCst);
    drv.message(selector::RESET_REQUEST, 0);
    assert!(s.events().ended);
    assert_eq!(s.events().reset, Some(ResetReason::ResetRequest));
    drop(s);
    assert!(!drv.slot.is_set());

    // Every attempt while it is away fails as not present; each is a fresh load.
    for _ in 0..3 {
        let (reason, _) = unavailable(host.open(DuplexRequest::new(vec![0, 1], 2, level())));
        assert!(
            matches!(reason, Unavailability::AsioHardwareNotPresent { .. }),
            "{reason:?}"
        );
    }
    // The registry still lists it: the look between attempts says present.
    assert!(matches!(
        host.probe(&DeviceSelector::Default),
        crate::Presence::Present { .. }
    ));

    // Plugged back in: the next attempt opens, indexed from 0 again.
    drv.present.store(true, Ordering::SeqCst);
    let mut s = host
        .open(DuplexRequest::new(vec![0, 1], 2, level()))
        .expect("reopen");
    drv.position.store(0, Ordering::SeqCst);
    drv.tick();
    let b = blocks(&mut s);
    assert_eq!(b[0].0.flags, BlockFlags::FIRST);
    assert_eq!(b[0].0.start_sample, 0);
    assert_eq!(rig.creates.load(Ordering::SeqCst), 5);
}

#[test]
fn a_panel_change_resets_the_stream_and_the_reopen_takes_the_drivers_new_rate() {
    let rig = Installed::new(vec![("Fireface USB", Script::default())]);
    let host = AsioHost::new(Arc::clone(&rig));
    let drv = rig.driver("Fireface USB");
    let s = host
        .open(DuplexRequest::new(vec![0], 2, level()))
        .expect("open");
    assert_eq!(
        (s.negotiated().sample_rate, s.negotiated().buffer_frames),
        (48_000, Some(256))
    );
    drv.tick();

    // The operator sets 44.1 kHz and 512 frames in the panel of the running driver: the
    // call goes to the stream's own driver, which asks for a reset.
    *drv.panel.lock().expect("scripted state") = Some((44_100.0, 512));
    host.control_panel(&DeviceSelector::Default).expect("panel");
    assert_eq!(drv.panel_calls.load(Ordering::SeqCst), 1);
    assert_eq!(rig.creates.load(Ordering::SeqCst), 1, "no second load");
    let ev = s.events();
    assert!(
        ev.ended && ev.reset == Some(ResetReason::ResetRequest),
        "{ev:?}"
    );
    drop(s);

    // Reopened with no rate or buffer asked for: the driver's new ones.
    let s = host
        .open(DuplexRequest::new(vec![0], 2, level()))
        .expect("reopen");
    assert_eq!(
        (s.negotiated().sample_rate, s.negotiated().buffer_frames),
        (44_100, Some(512))
    );
    assert!(
        !drv.calls
            .lock()
            .expect("scripted state")
            .contains(&"set_sample_rate"),
        "the rate is the driver's, never set back"
    );
}

#[test]
fn the_panel_of_a_driver_with_no_stream_stays_loaded_and_the_open_reuses_it() {
    let rig = Installed::new(vec![
        ("Fireface USB", Script::default()),
        ("FlexASIO", Script::default()),
    ]);
    let host = AsioHost::new(Arc::clone(&rig));
    host.control_panel(&DeviceSelector::Id(DeviceId("FlexASIO".into())))
        .expect("panel");
    assert_eq!(rig.driver("FlexASIO").panel_calls.load(Ordering::SeqCst), 1);
    // Listing while the panel's driver is loaded loads nothing else.
    let list = host.enumerate().expect("list");
    assert_eq!(rig.creates.load(Ordering::SeqCst), 1);
    assert!(list[0].input.is_none(), "{:?}", list[0].notes);
    assert!(list[1].input.is_some());
    // Opening it keeps the instance the panel is showing.
    let s = host
        .open(on("FlexASIO", DuplexRequest::new(vec![0], 0, level())))
        .expect("open");
    assert_eq!(rig.creates.load(Ordering::SeqCst), 1);
    // Another driver's panel is refused while a stream runs on this one.
    assert!(matches!(
        host.control_panel(&DeviceSelector::Id(DeviceId("Fireface USB".into()))),
        Err(AudioError::Unavailable { .. })
    ));
    drop(s);
    host.control_panel(&DeviceSelector::Id(DeviceId("Fireface USB".into())))
        .expect("panel");
    assert_eq!(rig.creates.load(Ordering::SeqCst), 2);
}

#[test]
fn clock_sources_are_listed_with_the_current_one_and_named_at_open() {
    let ck = |n: &str, current| ClockDesc {
        name: n.into(),
        current,
    };
    let drv = Scripted::new(Script {
        clocks: Some(vec![
            ck("Internal", false),
            ck(" Word Clock ", true),
            ck("", false),
        ]),
        ..Script::default()
    });
    let c = device_caps(&*drv, "Scripted ASIO", true).expect("caps");
    assert_eq!(
        c.clock_sources,
        vec![
            ClockSource {
                name: "Internal".into(),
                current: false
            },
            ClockSource {
                name: "Word Clock".into(),
                current: true
            },
            ClockSource {
                name: "Clock 3".into(),
                current: false
            },
        ]
    );
    let s = open(&drv, DuplexRequest::new(vec![0], 0, level())).expect("open");
    assert_eq!(s.negotiated().clock_source.as_deref(), Some("Word Clock"));
    // A driver that lists none.
    let drv = Scripted::new(Script::default());
    assert!(
        device_caps(&*drv, "Scripted ASIO", true)
            .expect("caps")
            .clock_sources
            .is_empty()
    );
}

#[test]
fn a_sparse_input_map_makes_buffers_for_exactly_its_channels() {
    let ch = |n: &str| ChannelDesc {
        name: n.into(),
        sample_type: F32,
    };
    let drv = Scripted::new(Script {
        inputs: (1..=8).map(|i| ch(&format!("Analog {i}"))).collect(),
        ..Script::default()
    });
    // Inputs 3 and 7 only (device channels 2 and 6), in the operator's order 7, 3.
    let mut s = open(&drv, DuplexRequest::new(vec![6, 2], 2, level())).expect("open");
    let made: Vec<(bool, u16)> = drv
        .buffers
        .lock()
        .expect("scripted state")
        .channels
        .iter()
        .map(|&(i, c, _)| (i, c))
        .collect();
    assert_eq!(made, [(true, 2), (true, 6), (false, 0), (false, 1)]);
    assert_eq!(s.negotiated().input_channels, 2);
    assert_eq!(s.negotiated().device_input_channels, 8);
    drv.tick();
    drv.tick();
    for (h, v) in blocks(&mut s) {
        assert_eq!(h.channels, 2);
        let frames = v.len() / 2;
        for f in [0, frames - 1] {
            let pos = h.start_sample as i64 + f as i64;
            // The driver's position is the block index here (first switch at 256).
            let pos = pos + i64::from(FRAMES);
            assert_eq!(
                v[f * 2],
                Scripted::signal(6, pos),
                "block {}",
                h.start_sample
            );
            assert_eq!(v[f * 2 + 1], Scripted::signal(2, pos));
        }
    }
}

#[test]
fn driver_buffering_is_stated_when_the_driver_reports_it_and_not_otherwise() {
    let drv = Scripted::new(Script::default());
    let s = open(&drv, DuplexRequest::new(vec![0], 2, level())).expect("open");
    assert_eq!(
        s.negotiated().latency,
        StaticLatency::Frames {
            input: 32,
            output: 48,
            driver_buffering: None,
        },
        "a driver before ASIO 2.3 answers not present: not reported, never 0"
    );
    drop(s);
    *drv.internal.lock().expect("scripted state") = Some((16, 24));
    let s = open(&drv, DuplexRequest::new(vec![0], 2, level())).expect("reopen");
    assert_eq!(
        s.negotiated().latency,
        StaticLatency::Frames {
            input: 32,
            output: 48,
            driver_buffering: Some(DriverBuffering {
                input: 16,
                output: 24
            }),
        }
    );
}

/// A stream playing a sine on output 0, run until it is at full level.
fn emitting(drv: &Arc<Scripted>) -> (DuplexStream, GeneratorHandle) {
    let (mut handle, port) = generator([0]).expect("routes");
    handle
        .set_source(Box::new(Sine::new(1000.0, 48_000, 0.05)))
        .expect("queue");
    let mut req = DuplexRequest::new(vec![0], 2, level());
    req.output = OutputSource::Generator(port);
    let s = open(drv, req).expect("open");
    handle.start();
    // The 20 ms fade-in is under four periods of 256 at 48 kHz.
    for _ in 0..6 {
        drv.tick();
    }
    assert_eq!(handle.state(), OutputState::Active);
    (s, handle)
}

/// Peak of output 0 over frames `range` of the half the last switch wrote.
fn last_peak(drv: &Scripted, range: std::ops::Range<usize>) -> f32 {
    let half = drv.half.load(Ordering::SeqCst);
    range.fold(0.0, |m, f| m.max(drv.output(0, half, f).abs()))
}

#[test]
fn every_clock_event_cuts_the_output_within_one_block_and_names_itself() {
    type Event = fn(&Scripted);
    let cases: [(&str, Event, ResetReason); 6] = [
        (
            "reset request",
            |d| {
                d.message(selector::RESET_REQUEST, 0);
            },
            ResetReason::ResetRequest,
        ),
        (
            "resync request",
            |d| {
                d.message(selector::RESYNC_REQUEST, 0);
            },
            ResetReason::ResyncRequest,
        ),
        (
            "buffer size change",
            |d| {
                d.message(selector::BUFFER_SIZE_CHANGE, 512);
            },
            ResetReason::BufferSizeChange,
        ),
        (
            "another rate",
            |d| d.slot.with(|e| e.expect("engine").rate_changed(44_100.0)),
            ResetReason::RateChange,
        ),
        (
            "the same rate (a relock or another source)",
            |d| d.slot.with(|e| e.expect("engine").rate_changed(48_000.0)),
            ResetReason::RateChange,
        ),
        (
            "no clock",
            |d| d.slot.with(|e| e.expect("engine").rate_changed(0.0)),
            ResetReason::ClockLost,
        ),
    ];
    // 2 ms at 48 kHz.
    const CUT: usize = 96;
    for (what, event, reason) in cases {
        let drv = Scripted::new(Script::default());
        let (s, handle) = emitting(&drv);
        assert!(last_peak(&drv, 0..256) > 0.04, "{what}: playing");
        event(&drv);
        let ev = s.events();
        assert!(ev.ended, "{what}: the stream ends");
        assert_eq!(ev.reset, Some(reason), "{what}");
        // The next block ramps down over the cut and is silent after it.
        drv.tick();
        assert!(last_peak(&drv, 0..CUT / 2) > 0.0, "{what}: a ramp, no step");
        assert_eq!(
            last_peak(&drv, CUT..256),
            0.0,
            "{what}: silent after the cut"
        );
        assert_eq!(handle.state(), OutputState::Silent, "{what}");
        // Silent for good: starting the generator again on this stream does nothing.
        handle.start();
        drv.tick();
        drv.tick();
        assert_eq!(last_peak(&drv, 0..256), 0.0, "{what}: stays silent");
    }
}
