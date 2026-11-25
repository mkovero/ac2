//! Roles over CURVE, against the daemon: a viewer key sends every command and only its set
//! runs, it never holds the generator but may stop it, a demotion or revoke takes effect on
//! the live connections at once, the rigs-only topics stay a rig's, the role outlives a
//! restart and the local pair is always an operator.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::{Duration, Instant};

use ac2_proto::frame::FrameData;
use ac2_proto::model::{
    ClientRole, GenAction, Generator, GeneratorDesired, GeneratorSettings, Signal,
};
use ac2_proto::units::{Dbfs, Hz, MeasId};
use ac2_proto::{Command, DataMessage, ErrorCode, ErrorDetail, ProtoError, Topic};
use ac2_zmq::{AuthorizedKeys, CurveClient, KeyPair, Role};
use ac2d::{Daemon, Handle, NetworkSecurity};
use common::*;

const T: Duration = Duration::from_secs(5);

/// A network-mode daemon whose authorized clients are `laptop` (an operator) and `phone` (a
/// viewer), with their credentials.
struct Rig {
    h: Handle,
    security: NetworkSecurity,
    laptop: KeyPair,
    phone: KeyPair,
}

fn security(dir: &std::path::Path) -> NetworkSecurity {
    NetworkSecurity {
        server_key_file: dir.join("server.key"),
        authorized_clients_file: dir.join("authorized_clients"),
    }
}

fn start(dir: &std::path::Path, backend: ac2_audio::FakeBackend) -> Rig {
    let laptop = KeyPair::generate().unwrap();
    let phone = KeyPair::generate().unwrap();
    let mut keys = AuthorizedKeys::new();
    keys.insert("laptop", laptop.public, Role::Operator)
        .unwrap();
    keys.insert("phone", phone.public, Role::Viewer).unwrap();
    let security = security(dir);
    keys.save(&security.authorized_clients_file).unwrap();
    let h = Daemon::start(net_config(backend, security.clone())).unwrap();
    Rig {
        h,
        security,
        laptop,
        phone,
    }
}

impl Rig {
    fn curve(&self, keys: &KeyPair) -> CurveClient {
        CurveClient {
            keys: keys.clone(),
            server_key: self.h.server_public_key().unwrap(),
        }
    }

    fn ctrl(&self, keys: &KeyPair) -> Client {
        let ep = net_endpoints(&self.h).0;
        Client::connect_with(self.h.context(), ep.as_str(), Some(&self.curve(keys)))
    }

    fn data(&self, keys: &KeyPair, prefixes: &[&[u8]]) -> Sub {
        let ep = net_endpoints(&self.h).1;
        Sub::connect_with(
            self.h.context(),
            ep.as_str(),
            prefixes,
            Some(&self.curve(keys)),
            None,
        )
    }

    /// A client of the local pair: the OS user on the rig's own host.
    fn local(&self) -> Client {
        Client::connect(self.h.context(), self.h.ctrl_endpoint())
    }
}

fn role(c: &mut Client) -> ClientRole {
    c.welcome(Command::Hello {
        client: "roles-test".into(),
    })
    .role
}

fn is_view_only(e: &ProtoError) -> bool {
    e.code == ErrorCode::Refused && e.detail == Some(ErrorDetail::ViewOnly)
}

fn generator(c: &mut Client) -> Generator {
    c.state().generator
}

fn tone() -> GeneratorDesired {
    GeneratorDesired {
        settings: GeneratorSettings {
            signal: Signal::Sine { freq: Hz(1000.0) },
            level: Dbfs(-20.0),
            band: None,
            outputs: vec![0],
        },
        armed: true,
        firing: true,
    }
}

/// The loopback input's meter, remembering how far it has read.
struct Meter {
    sub: Sub,
    seen_end: u64,
}

impl Meter {
    fn new(h: &Handle) -> Self {
        Self {
            sub: Sub::connect(h.context(), h.data_endpoint(), &[b"d/1/levels"]),
            seen_end: 0,
        }
    }

    /// The peak of the next levels frame, and where it ends.
    fn next(&mut self) -> f32 {
        let f = self
            .sub
            .frame(T, |f| matches!(f.data, FrameData::Levels(_)))
            .expect("levels frame");
        self.seen_end = f.stamp.audio_sample.0 + 1;
        match &f.data {
            FrameData::Levels(l) => l.peak[0],
            _ => unreachable!(),
        }
    }

    /// The peak level in dBFS over exactly `seconds` of audio run now.
    fn peak(&mut self, d: &mut ac2_audio::FakeDriver, seconds: f64) -> f32 {
        let before = end_sample(d);
        while self.seen_end < before {
            self.next();
        }
        run(d, seconds);
        let end = end_sample(d);
        let mut peak = f32::NEG_INFINITY;
        while self.seen_end < end {
            peak = peak.max(self.next());
        }
        peak
    }
}

/// Every command a viewer sends is refused `view_only` unless it is in the viewer's set, and
/// a refused one changes nothing: the state's revision, the authorized clients, the
/// generator stay as they were.
#[test]
fn a_viewer_sends_every_command_and_only_its_set_runs() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let rig = start(dir.path(), manual_rig());
    let mut op = rig.local();
    op.ok(Command::StreamOpen {
        config: stream(false),
    });
    op.ok(Command::MeasCreate {
        config: spl("meter", 0),
    });
    let mut phone = rig.ctrl(&rig.phone);
    assert_eq!(role(&mut phone), ClientRole::Viewer);
    let clients = |c: &mut Client| c.server(Command::ServerInfo).network.authorized;
    let authorized = clients(&mut op);
    let mut ran = Vec::new();
    // The two downloads a viewer may send in one shape only: the working state (not a named
    // session from the host), and an end that names no file on the host.
    let both_shapes = [
        Command::FileDownloadBegin {
            session: Some(ac2_proto::model::SessionRef::Name {
                name: "friday".into(),
            }),
        },
        Command::FileDownloadEnd {
            transfer: ac2_proto::units::TransferId(4),
            saved_as: None,
        },
    ];
    for cmd in ac2_proto::samples::commands()
        .into_iter()
        .chain(both_shapes)
    {
        let name = cmd.name();
        let allowed = cmd.allowed(&ClientRole::Viewer);
        let shape = format!("{cmd:?}");
        let before = op.state_rev();
        match phone.call(cmd) {
            Err(e) if is_view_only(&e) => {
                assert!(!allowed, "{shape} is a viewer's, yet refused: {e:?}");
            }
            // Anything else is the command's own answer: it went past the role gate.
            _ => {
                assert!(allowed, "{shape} is not a viewer's, yet it was not refused");
                ran.push(name);
            }
        }
        // Neither a refused command nor a read a viewer may send changes the rig.
        assert_eq!(op.state_rev(), before, "{shape} changed the state");
    }
    assert!(
        ran.contains(&"state.snapshot")
            && ran.contains(&"gen.stop")
            && ran.contains(&"file.download_begin")
            && ran.contains(&"file.download_end"),
        "{ran:?}"
    );
    assert_eq!(clients(&mut op), authorized);
    let g = generator(&mut op);
    assert!(g.owner.is_none() && !g.armed && !g.firing, "{g:?}");
    assert_eq!(op.meas(MeasId(1)).config.name, "meter");
    rig.h.shutdown();
}

/// A viewer neither takes the lease nor refreshes the operator's, and its `gen.stop` stops
/// the operator's output: anyone who hears it may stop it.
#[test]
fn a_viewer_takes_no_lease_and_stops_the_output() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let backend = manual_rig();
    let rig = start(dir.path(), backend.clone());
    let mut laptop = rig.ctrl(&rig.laptop);
    let mut phone = rig.ctrl(&rig.phone);
    let mut levels = Meter::new(&rig.h);
    laptop.ok(Command::StreamOpen {
        config: stream(false),
    });
    laptop.ok(Command::MeasCreate {
        config: spl("loopback meter", 0),
    });
    laptop.ok(Command::MeasStart { meas: MeasId(1) });
    let mut d = driver(&backend);
    for force in [false, true] {
        let e = phone.call(Command::GenAcquire { force }).unwrap_err();
        assert!(is_view_only(&e), "{e:?}");
    }
    let token = laptop.acquire();
    laptop.ok(Command::GenSet {
        lease_token: token,
        desired: tone(),
    });
    let e = phone
        .call(Command::GenRefresh { lease_token: token })
        .unwrap_err();
    assert!(is_view_only(&e), "{e:?}");
    let e = phone
        .call(Command::GenSet {
            lease_token: token,
            desired: tone(),
        })
        .unwrap_err();
    assert!(is_view_only(&e), "{e:?}");
    assert!(
        levels.peak(&mut d, 0.4) > -30.0,
        "the operator's tone plays"
    );

    phone.ok(Command::GenStop);
    let g = generator(&mut laptop);
    assert!(!g.firing, "{g:?}");
    assert_eq!(g.last_action.map(|a| a.action), Some(GenAction::Stop));
    levels.peak(&mut d, 0.3);
    assert_eq!(levels.peak(&mut d, 0.2), f32::NEG_INFINITY, "stopped");
    rig.h.shutdown();
}

/// An operator made a viewer while it holds the lease loses it at once: the output fades
/// out as on expiry, and its next command is refused as a viewer's.
#[test]
fn demotion_mid_lease_stops_the_output_and_refuses_the_next_command() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let backend = manual_rig();
    let rig = start(dir.path(), backend.clone());
    let mut laptop = rig.ctrl(&rig.laptop);
    let mut host = rig.local();
    let mut levels = Meter::new(&rig.h);
    laptop.ok(Command::StreamOpen {
        config: stream(false),
    });
    laptop.ok(Command::MeasCreate {
        config: spl("loopback meter", 0),
    });
    laptop.ok(Command::MeasStart { meas: MeasId(1) });
    let mut d = driver(&backend);
    let token = laptop.acquire();
    laptop.ok(Command::GenSet {
        lease_token: token,
        desired: tone(),
    });
    assert!(levels.peak(&mut d, 0.4) > -30.0, "the tone plays");

    // Its own key it may not demote: that would lock the operator out.
    let e = laptop
        .call(Command::AuthAuthorize {
            name: "laptop".into(),
            key: rig.laptop.public.to_z85(),
            role: ClientRole::Viewer,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");

    host.ok(Command::AuthAuthorize {
        name: "laptop".into(),
        key: rig.laptop.public.to_z85(),
        role: ClientRole::Viewer,
    });
    let g = generator(&mut host);
    assert!(g.owner.is_none() && !g.armed && !g.firing, "{g:?}");
    assert_eq!(g.last_action.map(|a| a.action), Some(GenAction::Expiry));
    levels.peak(&mut d, 0.3);
    assert_eq!(levels.peak(&mut d, 0.2), f32::NEG_INFINITY, "faded out");

    let e = laptop
        .call(Command::GenRefresh { lease_token: token })
        .unwrap_err();
    assert!(is_view_only(&e), "{e:?}");
    assert_eq!(role(&mut laptop), ClientRole::Viewer);
    // Made an operator again, it drives the rig on the same connection.
    host.ok(Command::AuthAuthorize {
        name: "laptop".into(),
        key: rig.laptop.public.to_z85(),
        role: ClientRole::Operator,
    });
    assert_eq!(role(&mut laptop), ClientRole::Operator);
    laptop.acquire();
    rig.h.shutdown();
}

/// Everything queued within `quiet`, then nothing.
fn drain(sub: &Sub, quiet: Duration) {
    while sub.next(quiet).is_some() {}
}

/// A revoked viewer's data connection is dropped at once: its subscriptions end and a
/// resubscription is not taken. A demoted operator's is dropped too, and comes back as a
/// viewer's.
#[test]
fn revoke_drops_the_data_connection() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let rig = start(dir.path(), realtime_rig());
    let mut host = rig.local();
    host.ok(Command::StreamOpen {
        config: stream(false),
    });
    let ka: &[u8] = b"ka";
    let phone = rig.data(&rig.phone, &[ka]);
    let laptop = rig.data(&rig.laptop, &[ka]);
    assert!(phone.ka(T).is_some(), "a viewer gets the keepalive");
    assert!(laptop.ka(T).is_some());

    host.ok(Command::AuthAuthorize {
        name: "laptop".into(),
        key: rig.laptop.public.to_z85(),
        role: ClientRole::Viewer,
    });
    let rejoined = Instant::now() + T;
    loop {
        match laptop.next(T) {
            Some(DataMessage::Flow(ac2_proto::FlowNotice::Rejoin)) => break,
            Some(_) => assert!(Instant::now() < rejoined, "no rejoin after the demotion"),
            None => panic!("no rejoin after the demotion"),
        }
    }
    assert!(laptop.ka(T).is_some(), "back as a viewer");

    host.ok(Command::AuthRevoke {
        name: "phone".into(),
    });
    drain(&phone, Duration::from_millis(300));
    assert!(
        phone.ka(Duration::from_secs(2)).is_none(),
        "revoked, yet fed"
    );
    assert!(laptop.ka(T).is_some(), "the others carry on");
    rig.h.shutdown();
}

/// A viewer asking for a remote rig's input audio or source levels gets none: those go to
/// an admitted rig only. Its stream levels come as an operator's would.
#[test]
fn a_viewer_gets_no_rigs_only_topics() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let rig = start(dir.path(), realtime_rig());
    let mut host = rig.local();
    host.ok(Command::StreamOpen {
        config: stream(false),
    });
    let audio = Topic::Audio { channel: 0 }.to_string();
    let source = Topic::SourceLevels.to_string();
    let levels = Topic::StreamLevels.to_string();
    let phone = rig.data(
        &rig.phone,
        &[audio.as_bytes(), source.as_bytes(), levels.as_bytes()],
    );
    assert!(
        phone
            .frame(T, |f| matches!(f.data, FrameData::StreamLevels(_)))
            .is_some()
    );
    let until = Instant::now() + Duration::from_secs(2);
    while let Some(m) = phone.raw(until.saturating_duration_since(Instant::now())) {
        let topic = m.first().map(Vec::as_slice).unwrap_or_default();
        assert!(
            topic != audio.as_bytes() && topic != source.as_bytes(),
            "a viewer was sent {}",
            String::from_utf8_lossy(topic)
        );
    }
    rig.h.shutdown();
}

/// The role is the authorized-clients file's: a viewer authorized over the protocol is a
/// viewer after a restart, and the local pair is an operator either side of it.
#[test]
fn the_role_survives_a_restart_and_the_local_pair_is_an_operator() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let rig = start(dir.path(), manual_rig());
    let tablet = KeyPair::generate().unwrap();
    let mut host = rig.local();
    assert_eq!(role(&mut host), ClientRole::Operator);
    host.ok(Command::AuthAuthorize {
        name: "tablet".into(),
        key: tablet.public.to_z85(),
        role: ClientRole::Viewer,
    });
    assert_eq!(role(&mut rig.ctrl(&tablet)), ClientRole::Viewer);
    let Rig {
        h,
        security,
        laptop,
        phone,
    } = rig;
    h.shutdown();

    let h = Daemon::start(net_config(manual_rig(), security.clone())).unwrap();
    let rig = Rig {
        h,
        security,
        laptop,
        phone,
    };
    assert_eq!(role(&mut rig.ctrl(&tablet)), ClientRole::Viewer);
    assert_eq!(role(&mut rig.ctrl(&rig.phone)), ClientRole::Viewer);
    assert_eq!(role(&mut rig.ctrl(&rig.laptop)), ClientRole::Operator);
    assert_eq!(role(&mut rig.local()), ClientRole::Operator);
    let listed: Vec<(String, ClientRole)> = rig
        .local()
        .server(Command::ServerInfo)
        .network
        .authorized
        .into_iter()
        .map(|a| (a.name, a.role))
        .collect();
    assert_eq!(
        listed,
        vec![
            ("laptop".into(), ClientRole::Operator),
            ("phone".into(), ClientRole::Viewer),
            ("tablet".into(), ClientRole::Viewer),
        ]
    );
    rig.h.shutdown();
}

/// The async client learns a new role without reconnecting: the daemon's `rejoin` makes it
/// say `hello` again. As a viewer it refuses a change itself, unsent, with the daemon's
/// `view_only`; made an operator again, it drives the rig.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_client_learns_its_new_role_and_refuses_what_it_may_not_send() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let rig = start(dir.path(), manual_rig());
    let (ctrl, data) = net_endpoints(&rig.h);
    let mut cc = ac2_client::ClientConfig::new(ac2_client::Endpoints { ctrl, data }, "roles");
    cc.curve = Some(rig.curve(&rig.laptop));
    let c = ac2_client::Client::connect(cc).await.unwrap();
    c.wait_synced(T).await.unwrap();
    assert_eq!(c.role(), ClientRole::Operator);

    let set = |role| Command::AuthAuthorize {
        name: "laptop".into(),
        key: rig.laptop.public.to_z85(),
        role,
    };
    let mut host = rig.local();
    host.ok(set(ClientRole::Viewer));
    let until = Instant::now() + T;
    while c.role() != ClientRole::Viewer {
        assert!(
            Instant::now() < until,
            "the client never learnt it is a viewer"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    match c.call(Command::GenAcquire { force: false }).await {
        Err(ac2_client::ClientError::Daemon(e)) => assert!(is_view_only(&e), "{e:?}"),
        other => panic!("a viewer's acquire: {other:?}"),
    }
    c.call(Command::StateSnapshot).await.unwrap();

    host.ok(set(ClientRole::Operator));
    let until = Instant::now() + T;
    while c.role() != ClientRole::Operator {
        assert!(
            Instant::now() < until,
            "the client never learnt it is an operator"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    c.call(Command::GenAcquire { force: false }).await.unwrap();
    drop(c);
    rig.h.shutdown();
}

/// A viewer exports the working state as a copy: the archive is written, and the working
/// state does not become that file's (the daemon records an origin for an operator only).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_viewer_exports_a_copy_of_the_working_state() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let rig = start(dir.path(), manual_rig());
    let (ctrl, data) = net_endpoints(&rig.h);
    let mut cc = ac2_client::ClientConfig::new(ac2_client::Endpoints { ctrl, data }, "roles");
    cc.curve = Some(rig.curve(&rig.phone));
    let c = ac2_client::Client::connect(cc).await.unwrap();
    c.wait_synced(T).await.unwrap();
    assert_eq!(c.role(), ClientRole::Viewer);
    let to = dir.path().join("copy.ac2session");
    let (p, bytes) = ac2_client::sessions::export(&c, None, &to, |_, _| {})
        .await
        .unwrap();
    assert_eq!(p, to);
    assert_eq!(std::fs::metadata(&to).unwrap().len(), bytes);
    let mut host = rig.local();
    assert_eq!(host.state().working.origin, None);
    drop(c);
    rig.h.shutdown();
}
