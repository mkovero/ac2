//! Stimulus lease (Q6) end to end on the fake rig: acquire / held / force, expiry silences
//! the output (observed through the loopback capture), stop is universal.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::{Duration, Instant};

use ac2_audio::{FakeBackend, FakeDriver};
use ac2_proto::frame::FrameData;
use ac2_proto::model::{
    BandLimit, FilterOrder, GenAction, GeneratorDesired, GeneratorSettings, Signal,
};
use ac2_proto::units::{ClientId, Dbfs, Hz, LeaseToken, MeasId, RequestId};
use ac2_proto::{Command, ErrorCode, ErrorDetail, ReplyBody, Request};
use ac2_zmq::{AuthorizedKeys, CurveClient, KeyPair};
use ac2d::{Daemon, Listen, NetworkSecurity};
use common::*;

const T: Duration = Duration::from_secs(5);
/// Q6 default.
const EXPIRY: Duration = Duration::from_millis(1500);

fn client_id(c: &mut Client) -> ClientId {
    match c.ok(Command::Hello {
        client: "test".into(),
    }) {
        ReplyBody::Welcome(w) => w.client_id,
        other => panic!("{other:?}"),
    }
}

fn acquire(c: &mut Client, force: bool) -> Result<LeaseToken, ac2_proto::ProtoError> {
    c.call(Command::GenAcquire { force }).map(|r| match r {
        ReplyBody::Lease(l) => l.lease_token,
        other => panic!("{other:?}"),
    })
}

/// Pink noise high-passed at 50 Hz. Full-band pink reaches down to 5 Hz, where a 200–300 ms
/// meter span holds one or two cycles: its RMS over such a span scatters by up to +2.7 /
/// −1.6 dB from seed to seed (about 1 % of seeds beyond ±1.5 dB), and the daemon picks a fresh
/// seed every time. Above 50 Hz the same spans stay within ±0.75 dB.
fn fire(level: f64) -> GeneratorDesired {
    GeneratorDesired {
        settings: GeneratorSettings {
            signal: Signal::Pink,
            level: Dbfs(level),
            band: Some(BandLimit {
                highpass: Some(Hz(50.0)),
                lowpass: None,
                order: FilterOrder::Second,
            }),
            outputs: vec![0],
        },
        armed: true,
        firing: true,
    }
}

/// Loopback input meter: a levels subscription that remembers how far it has read.
struct Meter {
    sub: Sub,
    seen_end: u64,
}

impl Meter {
    fn next(&mut self) -> (u64, f32, f32) {
        let f = self
            .sub
            .frame(T, |f| matches!(f.data, FrameData::Levels(_)))
            .expect("levels frame");
        let FrameData::Levels(l) = &f.data else {
            unreachable!()
        };
        self.seen_end = f.stamp.audio_sample.0 + 1;
        (self.seen_end, l.peak[0], l.rms[0])
    }

    /// Runs `seconds` of audio; (peak, rms) dBFS of the loopback input over exactly that span.
    fn level(&mut self, d: &mut FakeDriver, seconds: f64) -> (f32, f32) {
        // The meter interval must be closed at the current end before new audio arrives.
        let before = end_sample(d);
        while self.seen_end < before {
            self.next();
        }
        run(d, seconds);
        let end = end_sample(d);
        let mut peak = f32::NEG_INFINITY;
        let mut sq = 0.0f64;
        let mut n = 0.0;
        while self.seen_end < end {
            let (_, p, r) = self.next();
            peak = peak.max(p);
            sq += 10f64.powf(f64::from(r) / 10.0);
            n += 1.0;
        }
        (peak, (10.0 * (sq / n).log10()) as f32)
    }
}

#[test]
fn lease_acquire_force_expiry_and_universal_stop() {
    init_log();
    let backend = manual_rig();
    let mut cfg = config(backend.clone(), inproc("lease"));
    cfg.lease_expiry = EXPIRY;
    let h = Daemon::start(cfg).unwrap();
    let (mut a, sub) = connect(&h, &[b"evt"]);
    let mut m = Meter {
        sub: Sub::connect(h.context(), h.data_endpoint(), &[b"d/1/levels"]),
        seen_end: 0,
    };
    let mut b = Client::connect(h.context(), h.ctrl_endpoint());
    let id_a = client_id(&mut a);
    let id_b = client_id(&mut b);

    a.ok(Command::SessionOpen {
        config: session(false),
    });
    a.ok(Command::MeasCreate {
        config: spl("loopback meter", 0),
    });
    a.ok(Command::MeasStart { meas: MeasId(1) });

    // Acquire, and a second client is refused with the holder named.
    let tok_a = acquire(&mut a, false).unwrap();
    let e = acquire(&mut b, false).unwrap_err();
    assert_eq!(e.code, ErrorCode::LeaseHeld);
    assert_eq!(
        e.detail,
        Some(ErrorDetail::LeaseHeld {
            owner: id_a.clone()
        })
    );
    // Stimulus commands need the lease.
    let e = b
        .call(Command::GenSet {
            lease_token: tok_a,
            desired: fire(-20.0),
        })
        .unwrap_err();
    assert_eq!(
        e.code,
        ErrorCode::LeaseRequired,
        "a token is bound to its client"
    );
    // Firing requires armed; levels above the ceiling are refused.
    let mut unarmed = fire(-20.0);
    unarmed.armed = false;
    let e = a
        .call(Command::GenSet {
            lease_token: tok_a,
            desired: unarmed,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused);
    let e = a
        .call(Command::GenSet {
            lease_token: tok_a,
            desired: fire(-3.0),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused);

    // Fire: routes the generator within the running stream.
    a.ok(Command::GenSet {
        lease_token: tok_a,
        desired: fire(-20.0),
    });
    let mut d = driver(&backend);
    // Fade-in plus a full 300 ms RMS window of signal before the meter is read.
    let _fade_in = m.level(&mut d, 0.35);
    let (peak, rms) = m.level(&mut d, 0.3);
    assert!(
        (rms + 20.0).abs() < 1.5,
        "pink at -20 dBFS RMS on the loopback: {rms}"
    );
    assert!(
        peak < -20.0 + 20.0 * 6f32.log10() + 0.5,
        "peak within the crest bound: {peak}"
    );
    a.ok(Command::GenRefresh { lease_token: tok_a });
    let (_, rms) = m.level(&mut d, 0.2);
    assert!((rms + 20.0).abs() < 1.5);

    // Stop refreshing. After expiry + fade the output is silent.
    a.ok(Command::GenRefresh { lease_token: tok_a });
    let refreshed = Instant::now();
    let ev = sub
        .event(EXPIRY + T, |e| {
            matches!(&e.change, ac2_proto::Change::Generator(g)
                if g.last_action.as_ref().is_some_and(|x| x.action == GenAction::Expiry))
        })
        .expect("expiry event");
    let waited = refreshed.elapsed();
    assert!(
        waited >= EXPIRY - Duration::from_millis(20),
        "not before expiry: {waited:?}"
    );
    let ac2_proto::Change::Generator(g) = &ev.change else {
        unreachable!()
    };
    assert_eq!(g.owner, None);
    assert!(!g.armed && !g.firing);
    // The audit names whose lease expired.
    assert_eq!(
        g.last_action.as_ref().and_then(|x| x.client.as_ref()),
        Some(&id_a)
    );
    // Fade (20 ms) plus cable delay fit in the first 50 ms; then nothing at all.
    let _ = m.level(&mut d, 0.05);
    // Sample peak proves silence; the meter's RMS integrates over 300 ms and only decays.
    let (peak, rms) = m.level(&mut d, 0.2);
    assert!(peak == f32::NEG_INFINITY, "silent: peak {peak}, rms {rms}");
    let e = a
        .call(Command::GenRefresh { lease_token: tok_a })
        .unwrap_err();
    assert_eq!(
        e.code,
        ErrorCode::LeaseRequired,
        "an expired lease cannot be refreshed"
    );

    // B takes the free lease and fires; A stops it without a lease (stop is universal).
    let tok_b = acquire(&mut b, false).unwrap();
    b.ok(Command::GenSet {
        lease_token: tok_b,
        desired: fire(-26.0),
    });
    // Fade-in plus a full 300 ms RMS window of signal before the meter is read.
    let _fade_in = m.level(&mut d, 0.35);
    let (_, rms) = m.level(&mut d, 0.2);
    assert!((rms + 26.0).abs() < 1.5, "{rms}");
    match a.ok(Command::GenStop) {
        ReplyBody::Ack { .. } => {}
        other => panic!("{other:?}"),
    }
    let _ = m.level(&mut d, 0.05);
    let (peak, _) = m.level(&mut d, 0.2);
    assert_eq!(
        peak,
        f32::NEG_INFINITY,
        "stopped by a client without the lease"
    );
    let (s, _, _) = match a.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => (s.state, s.rev, ()),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        s.generator.owner,
        Some(id_b.clone()),
        "stop keeps the owner"
    );
    assert!(!s.generator.armed && !s.generator.firing);
    assert_eq!(
        s.generator
            .last_action
            .as_ref()
            .map(|x| (x.action, x.client.clone())),
        Some((GenAction::Stop, Some(id_a.clone())))
    );

    // Force takeover: B fires again, A forces; output stops and A owns a disarmed lease.
    b.ok(Command::GenRefresh { lease_token: tok_b });
    b.ok(Command::GenSet {
        lease_token: tok_b,
        desired: fire(-26.0),
    });
    // Fade-in plus a full 300 ms RMS window of signal before the meter is read.
    let _fade_in = m.level(&mut d, 0.35);
    let (_, rms) = m.level(&mut d, 0.2);
    assert!((rms + 26.0).abs() < 1.5, "{rms}");
    let e = acquire(&mut a, false).unwrap_err();
    assert_eq!(e.code, ErrorCode::LeaseHeld);
    let tok_a2 = acquire(&mut a, true).unwrap();
    let _ = m.level(&mut d, 0.05);
    let (peak, _) = m.level(&mut d, 0.2);
    assert_eq!(peak, f32::NEG_INFINITY, "force stops output first");
    let e = b
        .call(Command::GenRefresh { lease_token: tok_b })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::LeaseRequired);
    match a.ok(Command::GenRelease {
        lease_token: tok_a2,
    }) {
        ReplyBody::Ack { .. } => {}
        other => panic!("{other:?}"),
    }
    // Keep the device running while the stream fades out on shutdown.
    let stop = std::thread::spawn(move || {
        h.shutdown();
    });
    while !stop.is_finished() {
        d.run_blocks(4);
        std::thread::sleep(Duration::from_millis(1));
    }
    stop.join().unwrap();
}

/// A network-mode daemon on the manual fake rig whose only authorized client is `laptop`,
/// and that client's CURVE credentials: every connection made with them is the same
/// principal, whatever process or socket carries it.
fn curve_rig(dir: &std::path::Path) -> (FakeBackend, ac2d::Handle, CurveClient) {
    let key = KeyPair::generate().unwrap();
    let mut keys = AuthorizedKeys::new();
    keys.insert("laptop", key.public).unwrap();
    let authorized = dir.join("authorized_clients");
    keys.save(&authorized).unwrap();
    let listen = Listen::Network {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
        security: NetworkSecurity {
            server_key_file: dir.join("server.key"),
            authorized_clients_file: authorized,
        },
    };
    let backend = manual_rig();
    let mut cfg = config(backend.clone(), listen);
    cfg.lease_expiry = EXPIRY;
    let h = Daemon::start(cfg).unwrap();
    let curve = CurveClient {
        keys: key,
        server_key: h.server_public_key().unwrap(),
    };
    (backend, h, curve)
}

/// A request under an explicit id: a second connection of the same principal must not reuse
/// the first one's ids, or the daemon answers it from the first one's stored replies.
fn call_as(c: &mut Client, id: u64, cmd: Command) -> Result<ReplyBody, ac2_proto::ProtoError> {
    c.call_req(Request::new(RequestId(id), cmd)).result
}

fn generator(c: &mut Client, id: u64) -> ac2_proto::model::Generator {
    match call_as(c, id, Command::StateSnapshot).unwrap() {
        ReplyBody::Snapshot(s) => s.state.generator,
        other => panic!("{other:?}"),
    }
}

/// A lease belongs to the client instance holding its token, not to the key: a new
/// connection with the same key (a reconnect, or a second process) that acquires gets a
/// lease that comes up disarmed, with the old output faded out. It never inherits firing
/// output it did not set, and the old token is dead.
#[test]
fn same_key_new_connection_acquires_disarmed() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let (backend, h, curve) = curve_rig(dir.path());
    let mut a = Client::connect_with(h.context(), h.ctrl_endpoint(), Some(&curve));
    let mut m = Meter {
        sub: Sub::connect_with(
            h.context(),
            h.data_endpoint(),
            &[b"d/1/levels"],
            Some(&curve),
            None,
        ),
        seen_end: 0,
    };
    a.ok(Command::SessionOpen {
        config: session(false),
    });
    a.ok(Command::MeasCreate {
        config: spl("loopback meter", 0),
    });
    a.ok(Command::MeasStart { meas: MeasId(1) });
    let tok_a = acquire(&mut a, false).unwrap();
    a.ok(Command::GenSet {
        lease_token: tok_a,
        desired: fire(-20.0),
    });
    let mut d = driver(&backend);
    let _fade_in = m.level(&mut d, 0.35);
    let (_, rms) = m.level(&mut d, 0.3);
    assert!((rms + 20.0).abs() < 1.5, "A fires: {rms}");

    // A's socket drops within the lease; B, same key, acquires without force.
    drop(a);
    let mut b = Client::connect_with(h.context(), h.ctrl_endpoint(), Some(&curve));
    let tok_b = match call_as(&mut b, 1000, Command::GenAcquire { force: false }).unwrap() {
        ReplyBody::Lease(l) => l.lease_token,
        other => panic!("{other:?}"),
    };
    assert_ne!(tok_b, tok_a);
    let g = generator(&mut b, 1001);
    assert!(
        !g.armed && !g.firing,
        "a new holder never inherits armed={} firing={}",
        g.armed,
        g.firing
    );
    assert_eq!(g.owner.as_ref().map(|o| o.0.as_str()), Some("laptop"));
    assert_eq!(
        g.last_action.as_ref().map(|x| x.action),
        Some(GenAction::Acquire)
    );
    // The output faded out and stays silent while B holds a fresh lease.
    let _ = m.level(&mut d, 0.05);
    let (peak, rms) = m.level(&mut d, 0.2);
    assert!(peak == f32::NEG_INFINITY, "silent: peak {peak}, rms {rms}");

    // A, back on a new socket with its old token, holds nothing any more.
    let mut a2 = Client::connect_with(h.context(), h.ctrl_endpoint(), Some(&curve));
    let e = call_as(&mut a2, 2000, Command::GenRefresh { lease_token: tok_a }).unwrap_err();
    assert_eq!(e.code, ErrorCode::LeaseRequired);
    let e = call_as(
        &mut a2,
        2001,
        Command::GenSet {
            lease_token: tok_a,
            desired: fire(-20.0),
        },
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::LeaseRequired);
    // B arms and fires explicitly under its own token.
    call_as(
        &mut b,
        1002,
        Command::GenSet {
            lease_token: tok_b,
            desired: fire(-26.0),
        },
    )
    .unwrap();
    let _fade_in = m.level(&mut d, 0.35);
    let (_, rms) = m.level(&mut d, 0.2);
    assert!((rms + 26.0).abs() < 1.5, "B fires: {rms}");
    h.shutdown();
}

/// The holder's own token keeps the lease across a transport hiccup: a refresh from a new
/// socket of the same key carrying the existing token changes nothing, still firing.
#[test]
fn lease_refresh_with_token_survives_a_new_socket() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let (_backend, h, curve) = curve_rig(dir.path());
    let mut a = Client::connect_with(h.context(), h.ctrl_endpoint(), Some(&curve));
    a.ok(Command::SessionOpen {
        config: session(false),
    });
    let tok = acquire(&mut a, false).unwrap();
    a.ok(Command::GenSet {
        lease_token: tok,
        desired: fire(-20.0),
    });
    a.ok(Command::GenRefresh { lease_token: tok });
    let g = generator(&mut a, 500);
    assert!(g.armed && g.firing);

    drop(a);
    let mut a2 = Client::connect_with(h.context(), h.ctrl_endpoint(), Some(&curve));
    call_as(&mut a2, 1000, Command::GenRefresh { lease_token: tok }).unwrap();
    let g = generator(&mut a2, 1001);
    assert!(
        g.armed && g.firing,
        "refreshing with the held token keeps the output"
    );
    assert_eq!(
        g.last_action.as_ref().map(|x| x.action),
        Some(GenAction::Fire)
    );
    call_as(&mut a2, 1002, Command::GenRelease { lease_token: tok }).unwrap();
    h.shutdown();
}
