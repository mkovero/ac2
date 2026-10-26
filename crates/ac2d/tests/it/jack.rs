//! `session.devices` on JACK: names from the server's ports, or the reason it cannot be
//! used. Runs against whatever server is reachable (`jackd -d dummy` locally) and checks
//! the unavailable reason when none is.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used)]

use crate::common;

use ac2_proto::model::{Availability, BackendKind};
use ac2_proto::{Command, ReplyBody};
use ac2d::{BackendChoice, Daemon, DaemonConfig};
use std::time::Duration;

use common::*;

#[test]
fn jack_lists_port_names_or_says_why_not() {
    init_log();
    let backends = ac2d::backends(BackendChoice::Jack).expect("backends");
    let mut cfg = DaemonConfig::new(backends[0].clone(), inproc("jack"), -10.0);
    cfg.backends = backends;
    let h = Daemon::start(cfg).expect("daemon");
    let (mut c, _s) = connect(&h, &[]);
    let ReplyBody::Backends(b) = c.ok(Command::SessionDevices) else {
        panic!("backends");
    };
    assert_eq!(b[0].kind, BackendKind::Jack);
    assert!(
        b.iter().all(|x| x.kind != BackendKind::Fake),
        "real audio never offers the simulated rig"
    );
    match &b[0].availability {
        Availability::Available => {
            let d = &b[0].devices[0];
            let input = d.input.as_ref().expect("capture ports");
            let names = input.channel_names.as_ref().expect("port names");
            assert_eq!(names.len(), usize::from(input.max_channels));
            eprintln!("JACK capture ports: {names:?}");
        }
        Availability::Unavailable { reason } => {
            use ac2_audio::Unavailability as U;
            assert!(
                [U::PipeWireWithoutJack, U::NoJackServer, U::NoJackLibrary]
                    .iter()
                    .any(|u| u.to_string() == *reason),
                "{reason}"
            );
            assert!(b[0].devices.is_empty());
        }
    }
}

/// Peak of each recorder port since the last reset, f32 bits.
struct Recorder {
    peaks: std::sync::Arc<[std::sync::atomic::AtomicU32; 2]>,
    _client: jack::AsyncClient<(), jack::contrib::ClosureProcessHandler<(), RecFn>>,
}

type RecFn = Box<dyn FnMut(&jack::Client, &jack::ProcessScope) -> jack::Control + Send>;

impl Recorder {
    /// Connects `sources` to its two inputs once, like a recording tool would.
    fn start(sources: [&str; 2]) -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        let (client, _) =
            jack::Client::new("ac2d-test-rec", jack::ClientOptions::NO_START_SERVER).unwrap();
        let ins: Vec<_> = (1..=2)
            .map(|i| {
                client
                    .register_port(&format!("in_{i}"), jack::AudioIn::default())
                    .unwrap()
            })
            .collect();
        let names: Vec<String> = ins.iter().map(|p| p.name().unwrap()).collect();
        let peaks = std::sync::Arc::new([AtomicU32::new(0), AtomicU32::new(0)]);
        let p = std::sync::Arc::clone(&peaks);
        let f: RecFn = Box::new(move |_, ps| {
            for (port, peak) in ins.iter().zip(p.iter()) {
                let m = port
                    .as_slice(ps)
                    .iter()
                    .fold(f32::from_bits(peak.load(Ordering::Relaxed)), |m, v| {
                        m.max(v.abs())
                    });
                peak.store(m.to_bits(), Ordering::Relaxed);
            }
            jack::Control::Continue
        });
        let active = client
            .activate_async((), jack::contrib::ClosureProcessHandler::new(f))
            .unwrap();
        for (src, dst) in sources.iter().zip(&names) {
            active.as_client().connect_ports_by_name(src, dst).unwrap();
        }
        Self {
            peaks,
            _client: active,
        }
    }

    /// Peaks over the next `d`.
    fn peaks(&self, d: Duration) -> [f32; 2] {
        use std::sync::atomic::Ordering;
        for p in self.peaks.iter() {
            p.store(0, Ordering::Relaxed);
        }
        std::thread::sleep(d);
        [0, 1].map(|i| f32::from_bits(self.peaks[i].load(Ordering::Relaxed)))
    }

    fn linked(&self, a: &str, b: &str) -> bool {
        self._client
            .as_client()
            .port_by_name(a)
            .is_some_and(|p| p.is_connected_to(b).unwrap_or(false))
    }
}

/// On JACK the daemon connects the outputs the operator chose for the stimulus (and only
/// those) to the physical playback ports, and arming or re-routing never reopens the stream:
/// a recorder patched to our output ports once keeps hearing them.
#[test]
fn chosen_outputs_reach_playback_and_connections_survive_routing() {
    use ac2_proto::model::{
        DeviceSelector, GeneratorDesired, GeneratorSettings, SessionConfig, Signal,
    };
    use ac2_proto::units::{Dbfs, Hz};
    if std::env::var_os("AC2_JACK_DUMMY").is_none() {
        eprintln!("skipping: patching playback ports needs AC2_JACK_DUMMY=1 (dummy driver only)");
        return;
    }
    init_log();
    let backends = ac2d::backends(BackendChoice::Jack).expect("backends");
    let mut cfg = DaemonConfig::new(backends[0].clone(), inproc("jack-out"), -10.0);
    cfg.backends = backends;
    let h = Daemon::start(cfg).expect("daemon");
    let (mut c, _s) = connect(&h, &[]);
    let ReplyBody::Backends(b) = c.ok(Command::SessionDevices) else {
        panic!("backends");
    };
    let Some(dev) = b[0].devices.first() else {
        eprintln!("skipping: no JACK server");
        return;
    };
    let playback = dev
        .output
        .as_ref()
        .and_then(|o| o.channel_names.clone())
        .unwrap_or_default();
    if playback.len() < 2 {
        eprintln!("skipping: fewer than 2 playback ports");
        return;
    }
    // The listing names the dummy ports by their short names.
    let phys = |k: usize| format!("system:{}", playback[k]);
    let ReplyBody::Session(s) = c.ok(Command::SessionOpen {
        config: SessionConfig {
            backend: Some(BackendKind::Jack),
            input_device: DeviceSelector::Default,
            output_device: DeviceSelector::Default,
            input_channels: vec![0],
            spare_inputs: 0,
            output_channels: 2,
            sample_rate_hz: None,
            buffer_frames: None,
            loopback: None,
        },
    }) else {
        panic!("session");
    };
    let rec = Recorder::start(["ac2:out_1", "ac2:out_2"]);
    assert!(
        !rec.linked("ac2:out_1", &phys(0)) && !rec.linked("ac2:out_2", &phys(1)),
        "nothing reaches playback before the operator chooses"
    );
    let ReplyBody::Lease(l) = c.ok(Command::GenAcquire { force: false }) else {
        panic!("lease");
    };
    let set = |c: &mut Client, outputs: Vec<u16>, firing: bool| {
        c.ok(Command::GenSet {
            lease_token: l.lease_token,
            desired: GeneratorDesired {
                settings: GeneratorSettings {
                    signal: Signal::Sine { freq: Hz(1000.0) },
                    level: Dbfs(-30.0),
                    band: None,
                    outputs,
                },
                armed: true,
                firing,
            },
        });
    };
    set(&mut c, vec![0], false);
    assert!(
        rec.linked("ac2:out_1", &phys(0)),
        "the chosen output is connected"
    );
    assert!(
        !rec.linked("ac2:out_2", &phys(1)),
        "never an output not chosen"
    );
    set(&mut c, vec![0], true);
    let p = rec.peaks(Duration::from_millis(400));
    assert!(
        p[0] > 0.03 && p[1] == 0.0,
        "the recorder hears output 1: {p:?}"
    );

    // Re-route: the daemon moves its own connection; the recorder's stay.
    set(&mut c, vec![1], true);
    std::thread::sleep(Duration::from_millis(100));
    assert!(!rec.linked("ac2:out_1", &phys(0)));
    assert!(rec.linked("ac2:out_2", &phys(1)));
    let p = rec.peaks(Duration::from_millis(400));
    assert!(
        p[0] == 0.0 && p[1] > 0.03,
        "the recorder hears output 2: {p:?}"
    );
    let ReplyBody::Snapshot(snap) = c.ok(Command::StateSnapshot) else {
        panic!("snapshot");
    };
    assert_eq!(snap.state.session.epoch, s.epoch, "no reopen");
    c.ok(Command::GenStop);
    c.ok(Command::SessionClose);
    h.shutdown();
}

/// What the operator routed into a spare input survives the daemon's own reopen (a spare
/// added on the Audio I/O page reopens the stream): the source is connected again to the
/// new stream's port of the same input. Capture only.
#[test]
fn spare_input_sources_survive_a_reopen() {
    use ac2_proto::model::{DeviceSelector, SessionConfig};
    if std::env::var_os("AC2_JACK_DUMMY").is_none() {
        eprintln!("skipping: needs AC2_JACK_DUMMY=1 (dummy driver only)");
        return;
    }
    init_log();
    let backends = ac2d::backends(BackendChoice::Jack).expect("backends");
    let mut cfg = DaemonConfig::new(backends[0].clone(), inproc("jack-spare"), -10.0);
    cfg.backends = backends;
    let h = Daemon::start(cfg).expect("daemon");
    let (mut c, _s) = connect(&h, &[]);
    let ReplyBody::Backends(b) = c.ok(Command::SessionDevices) else {
        panic!("backends");
    };
    let Some(input) = b[0].devices.first().and_then(|d| d.input.clone()) else {
        eprintln!("skipping: no JACK capture ports");
        return;
    };
    let first = input.max_channels;
    let open = |c: &mut Client, spares: u16| {
        let ReplyBody::Session(s) = c.ok(Command::SessionOpen {
            config: SessionConfig {
                backend: Some(BackendKind::Jack),
                input_device: DeviceSelector::Default,
                output_device: DeviceSelector::Default,
                input_channels: std::iter::once(0).chain(first..first + spares).collect(),
                spare_inputs: spares,
                output_channels: 0,
                sample_rate_hz: None,
                buffer_frames: None,
                loopback: None,
            },
        }) else {
            panic!("session");
        };
        s.open.expect("open").spare_ports
    };
    let ports = open(&mut c, 1);
    assert_eq!(ports.len(), 1);
    assert_eq!(ports[0].input, first);
    assert_eq!(ports[0].port, format!("ac2:in_{}", first + 1));

    let (src, _) =
        jack::Client::new("ac2d-test-src", jack::ClientOptions::NO_START_SERVER).unwrap();
    let out = src.register_port("out", jack::AudioOut::default()).unwrap();
    let out = out.name().unwrap();
    let src = src.activate_async((), ()).unwrap();
    let src = src.as_client();
    src.connect_ports_by_name(&out, &ports[0].port).unwrap();

    let ports = open(&mut c, 2);
    assert_eq!(ports.len(), 2);
    let linked = |a: &str, b: &str| {
        src.port_by_name(a)
            .is_some_and(|p| p.is_connected_to(b).unwrap_or(false))
    };
    assert!(
        linked(&out, &ports[0].port),
        "connected again after the reopen"
    );
    assert!(
        !linked(&out, &ports[1].port),
        "the new spare is the operator's"
    );
    c.ok(Command::SessionClose);
    h.shutdown();
}
