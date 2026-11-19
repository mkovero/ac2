//! Remote inputs between two in-process daemons on fake devices: pairing, the filtered
//! state an admitted rig sees, the joined feed under a spectrum, an SPL meter and the
//! session meters, the refusals, and removal ending the stream.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::{Duration, Instant};

use ac2_proto::frame::FrameData;
use ac2_proto::model::SessionRef;
use ac2_proto::model::{
    MeasConfig, MeasKind, MeasScope, REMOTE_BASE, RemoteAudio, RemoteInput, RemoteLink,
    RemoteState, RigPeer, SessionConfig, SpectrumConfig, SystemClock,
};
use ac2_proto::units::{MeasId, RigId};
use ac2_proto::{Command, ErrorCode};
use ac2_zmq::{AuthorizedKeys, Context, CurveClient, KeyPair, PublicKey};
use ac2d::config::SystemClockSource;
use ac2d::{Daemon, Handle, Listen, NetworkSecurity};
use common::*;

const T: Duration = Duration::from_secs(20);

/// A network-mode daemon on a real-time fake device whose system clock is stated synced,
/// with `app` authorized and, when given, `keys` as its server key pair.
fn rig(dir: &std::path::Path, name: &str, app: &KeyPair, keys: Option<&KeyPair>) -> Handle {
    rig_on(dir, name, app, keys, None)
}

/// [`rig`], advertising and browsing on `mdns` when given.
fn rig_on(
    dir: &std::path::Path,
    name: &str,
    app: &KeyPair,
    keys: Option<&KeyPair>,
    mdns: Option<ac2_discovery::Options>,
) -> Handle {
    rig_at(dir, name, app, keys, mdns, 0)
}

/// [`rig_on`] with ctrl on `port` and data on `port + 1`, as `ac2d` binds them; any free
/// ports for 0.
fn rig_at(
    dir: &std::path::Path,
    name: &str,
    app: &KeyPair,
    keys: Option<&KeyPair>,
    mdns: Option<ac2_discovery::Options>,
    port: u16,
) -> Handle {
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
    let data = if port == 0 { 0 } else { port + 1 };
    let listen = Listen::Network {
        ctrl: format!("tcp://127.0.0.1:{port}"),
        data: format!("tcp://127.0.0.1:{data}"),
        security: NetworkSecurity {
            server_key_file: d.join("server.key"),
            authorized_clients_file: d.join("authorized_clients"),
        },
    };
    let mut cfg = config(realtime_rig(), listen);
    cfg.name = name.into();
    cfg.system_clock = SystemClockSource::Stated(SystemClock::Synced { maxerror_us: 100 });
    cfg.advertise = mdns.map(|mdns| ac2d::Advertise { mdns });
    cfg.recording_dir = Some(d.join("recordings"));
    Daemon::start(cfg).unwrap()
}

fn client(ctx: &Context, h: &Handle, keys: &KeyPair) -> (Client, RigId) {
    let curve = CurveClient {
        keys: keys.clone(),
        server_key: h.server_public_key().unwrap(),
    };
    let mut c = Client::connect_with(ctx, h.ctrl_endpoint(), Some(&curve));
    let w = c.welcome(Command::Hello {
        client: "test".into(),
    });
    (c, w.rig_id)
}

fn remote(c: &mut Client) -> RemoteState {
    let s = c.snapshot(Command::StateSnapshot);
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
    let m = c.measurement(Command::MeasCreate { config });
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
    let s = as_rig.source_state(Command::SourceState);
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

    // A transfer function across the rigs starts, gated on the clocks between them: it
    // averages only once they agree, and says so meanwhile.
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
    ca.ok(Command::MeasStart { meas: tf });
    wait_remote(&mut ca, "the cross-rig entry", |r| !r.transfers.is_empty());
    let r = remote(&mut ca);
    let x = &r.transfers[0];
    assert_eq!((x.meas, x.rig), (tf, b_id), "{r:#?}");
    assert_eq!(x.side, ac2_proto::model::RemoteSide::Measurement);
    // Under a minute of stream the clocks cannot agree yet: it waits, and says why.
    assert_ne!(
        r.rigs[0].clock.relation,
        ac2_proto::model::PeerRelation::Agree,
        "{r:#?}"
    );
    assert!(
        matches!(x.state, ac2_proto::model::CrossTfState::Stopped { .. }),
        "{r:#?}"
    );

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

    // A new link offset reopens the audio I/O: the remote input anchors again, at it, and
    // the transfer function across the rigs waits for the clocks again.
    let before = remote(&mut ca).link_offset;
    ca.ok(Command::SessionOpen {
        config: SessionConfig {
            link_offset: Some(ac2_proto::units::Seconds(0.08)),
            ..with_remote(b_id, FS)
        },
    });
    wait_remote(&mut ca, "the new link offset", |r| {
        r.link_offset == Some(ac2_proto::units::Seconds(0.08)) && !r.transfers.is_empty()
    });
    assert_ne!(before, Some(ac2_proto::units::Seconds(0.08)));
    // Anchored again, the clocks are measured again: it does not average meanwhile.
    let r = remote(&mut ca);
    assert!(
        matches!(
            r.transfers[0].state,
            ac2_proto::model::CrossTfState::Stopped {
                why: ac2_proto::model::StopWhy::Measuring | ac2_proto::model::StopWhy::CantTell,
                ..
            }
        ),
        "{r:#?}"
    );

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

/// A rig whose key the other rig authorized as an app, not as a rig, is told so (its audio
/// would be refused without a word); admitted as a rig on the same connection, it streams.
#[test]
fn slow_a_rig_admitted_as_an_app_is_told_so_until_admitted_as_a_rig() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let app = KeyPair::generate().unwrap();
    let a_keys = KeyPair::generate().unwrap();
    let b = rig(dir.path(), "rig-b", &app, None);
    let a = rig(dir.path(), "rig-a", &app, Some(&a_keys));
    let ctx = Context::new().unwrap();
    let (mut cb, b_id) = client(&ctx, &b, &app);
    let (mut ca, a_id) = client(&ctx, &a, &app);
    cb.ok(Command::SessionOpen {
        config: session(false),
    });
    cb.ok(Command::ServerAuthorize {
        name: "rig-a".into(),
        key: a_keys.public.to_z85(),
    });
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
    wait_remote(&mut ca, "not admitted", |r| {
        r.rigs.first().map(|r| &r.link) == Some(&RemoteLink::NotAdmitted)
    });
    cb.ok(Command::AuthAdmitRig {
        key: a_keys.public.to_z85(),
        rig_id: a_id,
        name: "rig-a".into(),
    });
    wait_remote(&mut ca, "streaming", |r| {
        r.inputs.first().map(|i| i.audio) == Some(RemoteAudio::Streaming)
            && r.rigs.first().map(|r| &r.link) == Some(&RemoteLink::Up)
    });
    a.shutdown();
    b.shutdown();
}

/// The open session's input channels and remote inputs.
fn session_inputs(c: &mut Client) -> (Vec<u16>, Vec<RemoteInput>) {
    let s = c.snapshot(Command::StateSnapshot).state.session;
    let cfg = s.open.expect("an open session").config;
    (cfg.input_channels, cfg.remote_inputs)
}

/// A session file names its remote inputs and its measurements name their channels: loading
/// it gives the open session those remote inputs back, and a file with none takes the open
/// session's away. The local inputs stay the device's.
#[test]
fn a_session_load_gives_back_its_remote_inputs() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(manual_rig(), inproc(&unique("remote-load")));
    cfg.session_dir = dir.path().join("sessions");
    let h = Daemon::start(cfg).unwrap();
    let (mut c, _) = connect(&h, &[]);
    let save = |c: &mut Client, name: &str| {
        c.ok(Command::FileSave {
            session: SessionRef::Name { name: name.into() },
            overwrite: true,
        });
    };
    let load = |c: &mut Client, name: &str| {
        c.ok(Command::FileLoad {
            session: SessionRef::Name { name: name.into() },
            discard: true,
        });
    };
    // Rig B is not paired: its input waits for it, and the file still records it.
    let b = RigId(0xb);
    c.ok(Command::SessionOpen {
        config: session(false),
    });
    save(&mut c, "local");
    let with = with_remote(b, FS);
    c.ok(Command::SessionOpen {
        config: with.clone(),
    });
    create(&mut c, spectrum(REMOTE_BASE));
    save(&mut c, "remote");

    load(&mut c, "local");
    assert_eq!(session_inputs(&mut c), (vec![0], Vec::new()));
    c.ok(Command::SessionOpen {
        config: session(false),
    });
    load(&mut c, "remote");
    assert_eq!(
        session_inputs(&mut c),
        (vec![0, 1, REMOTE_BASE], with.remote_inputs.clone())
    );
    wait_remote(&mut c, "the loaded input is run", |r| {
        r.inputs.iter().map(|i| i.slot).eq([0])
    });
    // Loaded again: the same remote inputs, nothing reopened for them.
    load(&mut c, "remote");
    assert_eq!(session_inputs(&mut c).1, with.remote_inputs);
    h.shutdown();
}

/// Rig B's address moved (a new DHCP lease): A, paired with B where B no longer answers,
/// finds B's mDNS advert by the key the app pinned, pairs it at the advertised address and
/// takes its input again. Runs on loopback with a private mDNS port; where the host has no
/// multicast on loopback the test reports a skip unless `AC2_REQUIRE_MDNS=1`.
#[test]
fn slow_a_paired_rig_is_followed_to_its_new_address() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let port = std::net::UdpSocket::bind(("127.0.0.1", 0))
        .and_then(|s| s.local_addr())
        .map_or(53_532, |a| a.port());
    let mdns = ac2_discovery::Options {
        mdns_port: port,
        loopback_only: true,
    };
    let app = KeyPair::generate().unwrap();
    let a_keys = KeyPair::generate().unwrap();
    // B as `ac2d` serves: data on the port after ctrl, which an advert does not carry.
    let b = rig_at(
        dir.path(),
        "rig-b",
        &app,
        None,
        Some(mdns.clone()),
        free_pair(),
    );
    let a = rig_on(dir.path(), "rig-a", &app, Some(&a_keys), Some(mdns));
    let ctx = Context::new().unwrap();
    let (mut cb, b_id) = client(&ctx, &b, &app);
    let (mut ca, a_id) = client(&ctx, &a, &app);
    cb.ok(Command::SessionOpen {
        config: session(false),
    });
    cb.ok(Command::AuthAdmitRig {
        key: a_keys.public.to_z85(),
        rig_id: a_id,
        name: "rig-a".into(),
    });
    // Paired where B was: nothing answers there now.
    let gone = "tcp://127.0.0.1:9";
    ca.ok(Command::RigPeers {
        peers: vec![RigPeer {
            rig: b_id,
            name: "rig-b".into(),
            ctrl: gone.into(),
            data: "tcp://127.0.0.1:10".into(),
            key: b.server_public_key().unwrap().to_z85(),
        }],
    });
    ca.ok(Command::SessionOpen {
        config: with_remote(b_id, FS),
    });

    let t0 = Instant::now();
    let r = loop {
        let r = remote(&mut ca);
        let followed = r.peers.first().is_some_and(|p| p.ctrl == b.ctrl_endpoint());
        let up = r.rigs.first().map(|r| &r.link) == Some(&RemoteLink::Up);
        if followed && up {
            break r;
        }
        if t0.elapsed() > T {
            assert!(
                followed || std::env::var_os("AC2_REQUIRE_MDNS").is_none_or(|v| v != "1"),
                "no mDNS answer on loopback within {T:?}: {r:#?}"
            );
            if !followed {
                eprintln!(
                    "skipped: no multicast on loopback here (AC2_REQUIRE_MDNS=1 makes this fail)"
                );
                a.shutdown();
                b.shutdown();
                return;
            }
            panic!("followed but the link is not up: {r:#?}");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(r.peers[0].data, b.data_endpoint());
    wait_remote(&mut ca, "streaming from the new address", |r| {
        r.inputs.first().map(|i| i.audio) == Some(RemoteAudio::Streaming)
    });
    a.shutdown();
    b.shutdown();
}

/// A free TCP port on loopback whose next port is free too.
fn free_pair() -> u16 {
    loop {
        let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let p = l.local_addr().unwrap().port();
        if p < u16::MAX && std::net::TcpListener::bind(("127.0.0.1", p + 1)).is_ok() {
            return p;
        }
    }
}

/// A recording of a remote input: while its rig's session is closed the file holds zeros on
/// that channel, and the sidecar marks them; a replay offers the channel as a device input
/// named as recorded.
#[test]
fn slow_a_remote_input_is_recorded_with_its_gaps_marked() {
    use ac2_proto::model::{RecordRequest, RecordingRef, ReplayPace};
    use ac2_proto::units::Seconds;
    use ac2_traces::raw;
    let Paired {
        _dir,
        a,
        b,
        mut ca,
        mut cb,
        ..
    } = paired();
    let streaming = |r: &RemoteState| {
        r.inputs.first().map(|i| i.audio) == Some(RemoteAudio::Streaming)
            && r.rigs.first().map(|r| &r.link) == Some(&RemoteLink::Up)
    };
    wait_remote(&mut ca, "streaming", streaming);
    let rec = ca.recording(Command::RecStart {
        request: RecordRequest {
            inputs: vec![0, REMOTE_BASE],
            name: Some("joined".into()),
            max_duration: Seconds(60.0),
            max_bytes: None,
        },
    });
    std::thread::sleep(Duration::from_millis(700));
    cb.ok(Command::SessionClose);
    wait_remote(&mut ca, "the source gone", |r| !streaming(r));
    std::thread::sleep(Duration::from_millis(700));
    cb.ok(Command::SessionOpen {
        config: session(false),
    });
    wait_remote(&mut ca, "streaming again", streaming);
    std::thread::sleep(Duration::from_millis(700));
    let run = ca.recording(Command::RecStop);

    let dir = _dir.path().join("rig-a").join("recordings");
    let s = raw::read_sidecar(&dir, &rec.name).unwrap();
    let remote = s.audio.channels[1]
        .remote
        .clone()
        .expect("the remote source");
    assert_eq!((remote.rig_name.as_str(), remote.channel), ("rig-b", 1));
    assert_eq!(s.audio.channels[1].name.as_deref(), Some("rig-b In 2"));
    assert!(
        s.remote_gaps.iter().all(|g| g.channel == 1),
        "{:?}",
        s.remote_gaps
    );
    let lost: u64 = s.remote_gaps.iter().map(|g| g.frames).sum();
    assert!(
        lost >= u64::from(FS) / 2,
        "the closed source's time is marked: {:?}",
        s.remote_gaps
    );
    let frames = s.end.as_ref().unwrap().frames;
    assert!(lost < frames, "{lost} of {frames}");
    assert_eq!(run.remote_gaps, s.remote_gap_count());
    // The marked frames hold zeros.
    let mut r = raw::WavReader::open(&raw::audio_path(&dir, &rec.name)).unwrap();
    let mut all = vec![0.0f32; frames as usize * 2];
    let mut got = 0;
    while got < all.len() {
        let n = r.read(&mut all[got..]).unwrap();
        if n == 0 {
            break;
        }
        got += n * 2;
    }
    for g in &s.remote_gaps {
        for f in g.frame..g.frame + g.frames {
            assert_eq!(all[f as usize * 2 + 1], 0.0, "frame {f}");
        }
    }

    // The replay offers the remote channel as input 1, named as recorded.
    let session = ca.session(Command::SessionReplay {
        recording: RecordingRef::Name { name: rec.name },
        pace: ReplayPace::Fast,
    });
    let open = session.open.expect("open");
    assert_eq!(open.config.input_channels, vec![0, 1]);
    assert!(open.config.remote_inputs.is_empty());
    a.shutdown();
    b.shutdown();
}
