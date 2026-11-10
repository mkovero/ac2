//! Remote inputs between two in-process daemons on fake devices: pairing, the filtered
//! state an admitted rig sees, the joined feed under a spectrum, an SPL meter and the
//! session meters, the refusals, and removal ending the stream.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::{Duration, Instant};

use ac2_proto::frame::FrameData;
use ac2_proto::model::{
    MeasConfig, MeasKind, MeasScope, REMOTE_BASE, RemoteAudio, RemoteInput, RemoteLink,
    RemoteState, RigPeer, SessionConfig, SpectrumConfig, SystemClock,
};
use ac2_proto::units::{MeasId, RigId};
use ac2_proto::{Command, ErrorCode, ReplyBody};
use ac2_zmq::{AuthorizedKeys, Context, CurveClient, KeyPair, PublicKey};
use ac2d::config::SystemClockSource;
use ac2d::{Daemon, Handle, Listen, NetworkSecurity};
use common::*;

const T: Duration = Duration::from_secs(20);

/// A network-mode daemon on a real-time fake device whose system clock is stated synced,
/// with `app` authorized and, when given, `keys` as its server key pair.
fn rig(dir: &std::path::Path, name: &str, app: &KeyPair, keys: Option<&KeyPair>) -> Handle {
    let d = dir.join(name);
    std::fs::create_dir_all(&d).unwrap();
    let mut authorized = AuthorizedKeys::new();
    authorized.insert("app", app.public).unwrap();
    authorized.save(&d.join("authorized_clients")).unwrap();
    if let Some(k) = keys {
        let text = format!(
            "public {}\nsecret {}\n",
            k.public.to_z85(),
            k.secret.to_z85()
        );
        let p = d.join("server.key");
        std::fs::write(&p, text).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
    let listen = Listen::Network {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
        security: NetworkSecurity {
            server_key_file: d.join("server.key"),
            authorized_clients_file: d.join("authorized_clients"),
        },
    };
    let mut cfg = config(realtime_rig(), listen);
    cfg.name = name.into();
    cfg.system_clock = SystemClockSource::Stated(SystemClock::Synced { maxerror_us: 100 });
    Daemon::start(cfg).unwrap()
}

fn client(ctx: &Context, h: &Handle, keys: &KeyPair) -> (Client, RigId) {
    let curve = CurveClient {
        keys: keys.clone(),
        server_key: h.server_public_key().unwrap(),
    };
    let mut c = Client::connect_with(ctx, h.ctrl_endpoint(), Some(&curve));
    let ReplyBody::Welcome(w) = c.ok(Command::Hello {
        client: "test".into(),
    }) else {
        panic!("welcome");
    };
    (c, w.rig_id)
}

fn remote(c: &mut Client) -> RemoteState {
    let ReplyBody::Snapshot(s) = c.ok(Command::StateSnapshot) else {
        panic!("snapshot");
    };
    s.state.remote
}

fn wait_remote(c: &mut Client, what: &str, mut pred: impl FnMut(&RemoteState) -> bool) {
    let t0 = Instant::now();
    loop {
        let r = remote(c);
        if pred(&r) {
            return;
        }
        assert!(t0.elapsed() < T, "{what}: {r:#?}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// This rig's inputs 0 and the other rig's input 1 as remote slot 0.
fn with_remote(rig: RigId, sample_rate_hz: u32) -> SessionConfig {
    SessionConfig {
        input_channels: vec![0, REMOTE_BASE],
        sample_rate_hz: Some(sample_rate_hz),
        remote_inputs: vec![RemoteInput {
            slot: 0,
            rig,
            rig_name: "rig-b".into(),
            channel: 1,
        }],
        ..session(false)
    }
}

fn spectrum(input: u16) -> MeasConfig {
    MeasConfig {
        scope: MeasScope::Session,
        name: "remote spectrum".into(),
        kind: MeasKind::Spectrum {
            config: SpectrumConfig::on_input(input),
        },
    }
}

fn create(c: &mut Client, config: MeasConfig) -> MeasId {
    let ReplyBody::Measurement(m) = c.ok(Command::MeasCreate { config }) else {
        panic!("measurement");
    };
    m.id
}

struct Paired {
    _dir: tempfile::TempDir,
    app: KeyPair,
    a: Handle,
    b: Handle,
    ca: Client,
    cb: Client,
}

/// Rig A takes rig B's input 1 as remote slot 0: B admits A, A pairs with B and opens its
/// session, checking on the way what an admitted rig may and may not do on B.
fn paired() -> Paired {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let app = KeyPair::generate().unwrap();
    let a_keys = KeyPair::generate().unwrap();
    let b = rig(dir.path(), "rig-b", &app, None);
    let a = rig(dir.path(), "rig-a", &app, Some(&a_keys));
    assert_eq!(a.server_public_key(), Some(a_keys.public));
    let ctx = Context::new().unwrap();
    let (mut cb, b_id) = client(&ctx, &b, &app);
    let (mut ca, a_id) = client(&ctx, &a, &app);
    cb.ok(Command::SessionOpen {
        config: session(false),
    });

    // B admits A's key as a rig: A may read B's source state and nothing else.
    cb.ok(Command::AuthAdmitRig {
        key: a_keys.public.to_z85(),
        rig_id: a_id,
        name: "rig-a".into(),
    });
    let (mut as_rig, _) = client(&ctx, &b, &a_keys);
    let ReplyBody::SourceState(s) = as_rig.ok(Command::SourceState) else {
        panic!("source state");
    };
    assert_eq!(s.rig, b_id);
    let inputs: Vec<u16> = s
        .session
        .unwrap()
        .inputs
        .iter()
        .map(|i| i.channel)
        .collect();
    assert_eq!(inputs, vec![0, 1]);
    let e = as_rig.call(Command::SessionStatus).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");
    let e = as_rig.call(Command::StateSnapshot).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");

    // An input of this rig is no remote input.
    let e = ca
        .call(Command::SessionOpen {
            config: with_remote(a_id, FS),
        })
        .unwrap_err();
    assert!(e.msg.contains("that input is on this rig"), "{e:?}");

    ca.ok(Command::RigPeers {
        peers: vec![RigPeer {
            rig: b_id,
            name: "rig-b".into(),
            ctrl: b.ctrl_endpoint().to_owned(),
            data: b.data_endpoint().to_owned(),
            key: b.server_public_key().unwrap().to_z85(),
        }],
    });
    ca.ok(Command::SessionOpen {
        config: with_remote(b_id, FS),
    });
    Paired {
        _dir: dir,
        app,
        a,
        b,
        ca,
        cb,
    }
}

#[test]
fn pairing_admits_a_rig_to_its_inputs_only() {
    let Paired {
        _dir,
        a,
        b,
        mut ca,
        mut cb,
        ..
    } = paired();
    let b_id = remote(&mut ca).peers[0].rig;
    wait_remote(&mut ca, "rig-b's state", |r| {
        r.inputs
            .first()
            .is_some_and(|i| i.audio != RemoteAudio::Waiting)
    });
    // B's rate is known now: a session at another one is refused before anything closes.
    let e = ca
        .call(Command::SessionOpen {
            config: with_remote(b_id, 2 * FS),
        })
        .unwrap_err();
    assert!(e.msg.contains("rig-b runs 48 kHz"), "{e:?}");

    // A transfer function across the rigs waits for the clocks between them.
    let tf = create(
        &mut ca,
        MeasConfig {
            kind: MeasKind::Transfer {
                config: ac2_proto::model::TransferConfig {
                    reference_input: 0,
                    measurement_input: REMOTE_BASE,
                    ..match transfer("x").kind {
                        MeasKind::Transfer { config } => config,
                        _ => unreachable!(),
                    }
                },
            },
            ..transfer("across")
        },
    );
    let e = ca.call(Command::MeasStart { meas: tf }).unwrap_err();
    assert!(e.msg.contains("needs the clocks between rigs"), "{e:?}");

    // The remote input's calibration is its rig's.
    let e = ca
        .call(Command::SessionInputs {
            inputs: vec![ac2_proto::model::InputSetup {
                channel: REMOTE_BASE,
                kind: ac2_proto::model::InputKind::Line,
                pair: None,
            }],
            pairs: Vec::new(),
        })
        .unwrap_err();
    assert!(e.msg.contains("set on rig-b"), "{e:?}");

    // Removing A on B ends A's stream at once and A's link says why.
    cb.ok(Command::ServerRevoke {
        name: "rig-a".into(),
    });
    wait_remote(&mut ca, "not admitted", |r| {
        r.rigs.first().map(|r| &r.link) == Some(&RemoteLink::NotAdmitted)
    });
    a.shutdown();
    b.shutdown();
}

#[test]
fn slow_spectrum_spl_and_meters_read_a_remote_input() {
    let Paired {
        _dir,
        app,
        a,
        b,
        mut ca,
        ..
    } = paired();
    wait_remote(&mut ca, "streaming", |r| {
        r.inputs.first().map(|i| i.audio) == Some(RemoteAudio::Streaming)
            && r.rigs.first().map(|r| &r.link) == Some(&RemoteLink::Up)
    });
    // A spectrum and an SPL meter on the remote input, and the session meters, all read it.
    let sub = Sub::connect_with(
        a.context(),
        a.data_endpoint(),
        &[b"d/", b"session/levels"],
        Some(&CurveClient {
            keys: app.clone(),
            server_key: a.server_public_key().unwrap(),
        }),
        None,
    );
    let spec = create(&mut ca, spectrum(REMOTE_BASE));
    ca.ok(Command::MeasStart { meas: spec });
    let meter = create(&mut ca, spl("remote meter", REMOTE_BASE));
    ca.ok(Command::MeasStart { meas: meter });
    let mut seen = (false, false, false);
    let t0 = Instant::now();
    while !(seen.0 && seen.1 && seen.2) {
        assert!(t0.elapsed() < T, "frames seen {seen:?}");
        let Some(f) = sub.frame(Duration::from_secs(1), |_| true) else {
            continue;
        };
        sub.credit();
        match &f.data {
            FrameData::Spec(_) => seen.0 = true,
            FrameData::Spl(_) => seen.1 = true,
            FrameData::SessionLevels(l) => {
                seen.2 = seen.2 || l.meta.channels == vec![0, REMOTE_BASE];
            }
            _ => {}
        }
    }
    let r = remote(&mut ca);
    assert_eq!(r.inputs[0].slips, 0, "{r:#?}");
    assert!(r.link_offset.is_some());
    a.shutdown();
    b.shutdown();
}

#[test]
fn a_rig_whose_key_changed_is_named() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let app = KeyPair::generate().unwrap();
    let b = rig(dir.path(), "rig-b", &app, None);
    let a = rig(dir.path(), "rig-a", &app, None);
    let ctx = Context::new().unwrap();
    let (_cb, b_id) = client(&ctx, &b, &app);
    let (mut ca, _) = client(&ctx, &a, &app);
    // The key the app pinned is not the one rig B holds.
    let other: PublicKey = KeyPair::generate().unwrap().public;
    ca.ok(Command::RigPeers {
        peers: vec![RigPeer {
            rig: b_id,
            name: "rig-b".into(),
            ctrl: b.ctrl_endpoint().to_owned(),
            data: b.data_endpoint().to_owned(),
            key: other.to_z85(),
        }],
    });
    ca.ok(Command::SessionOpen {
        config: with_remote(b_id, FS),
    });
    wait_remote(&mut ca, "key changed", |r| {
        r.rigs.first().map(|r| &r.link) == Some(&RemoteLink::KeyChanged)
    });
    a.shutdown();
    b.shutdown();
}
