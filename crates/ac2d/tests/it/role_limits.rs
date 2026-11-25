//! What a key on the network can take of the daemon, whatever its role: transfers, the file
//! slot, subscriptions, its own entry, and what it reads of the host. Each test drives a
//! network-mode daemon with CURVE clients, as `roles` does.
//!
//! A slow file job is made with the file system, never with sleeps: a session whose manifest
//! is a named pipe (reading it waits for the test), or a staging directory left full of files
//! where the next download stages (making room for it takes a while).
#![cfg(unix)]
#![allow(clippy::unwrap_used)]

use crate::common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ac2_proto::model::{
    ClientRole, ExportFormat, Recordings, SessionOrigin, SessionRef, StateFile, TraceOwner,
};
use ac2_proto::units::{MeasId, TransferId};
use ac2_proto::{Change, Command, DataMessage, DataUp, ErrorCode, ProtoError, Request, Topic};
use ac2_zmq::{AuthorizedKeys, CurveClient, KeyPair, Role};
use ac2d::{DaemonConfig, Handle, NetworkSecurity};
use common::*;

const T: Duration = Duration::from_secs(10);

/// A network-mode daemon with an operator `laptop` and viewers `phone`, `tablet`, `watch`,
/// `kiosk`, `wall`.
struct Rig {
    h: Handle,
    dir: PathBuf,
    laptop: KeyPair,
    phone: KeyPair,
    tablet: KeyPair,
    watch: KeyPair,
    kiosk: KeyPair,
    wall: KeyPair,
}

fn start_with(
    dir: &Path,
    backend: ac2_audio::FakeBackend,
    tweak: impl FnOnce(&mut DaemonConfig),
) -> Rig {
    init_log();
    let [laptop, phone, tablet, watch, kiosk, wall] =
        std::array::from_fn(|_| KeyPair::generate().unwrap());
    let mut keys = AuthorizedKeys::new();
    keys.insert("laptop", laptop.public, Role::Operator)
        .unwrap();
    for (n, k) in [
        ("phone", &phone),
        ("tablet", &tablet),
        ("watch", &watch),
        ("kiosk", &kiosk),
        ("wall", &wall),
    ] {
        keys.insert(n, k.public, Role::Viewer).unwrap();
    }
    let security = NetworkSecurity {
        server_key_file: dir.join("server.key"),
        authorized_clients_file: dir.join("authorized_clients"),
    };
    keys.save(&security.authorized_clients_file).unwrap();
    let mut cfg = net_config(backend, security);
    cfg.session_dir = dir.join("sessions");
    tweak(&mut cfg);
    let h = ac2d::Daemon::start(cfg).unwrap();
    Rig {
        h,
        dir: dir.to_owned(),
        laptop,
        phone,
        tablet,
        watch,
        kiosk,
        wall,
    }
}

fn start(dir: &Path) -> Rig {
    start_with(dir, manual_rig(), |_| {})
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
        let mut c = Client::connect_with(self.h.context(), ep.as_str(), Some(&self.curve(keys)));
        c.welcome(Command::Hello {
            client: "role-limits-test".into(),
        });
        c
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

    fn local(&self) -> Client {
        Client::connect(self.h.context(), self.h.ctrl_endpoint())
    }

    fn sessions(&self) -> PathBuf {
        self.dir.join("sessions")
    }

    /// Fills the staging directory the daemon's `n`th staged transfer will use, so that
    /// making room for it takes a while: the download it belongs to stays pending.
    fn slow_stage(&self, n: u64) {
        // The daemon empties the staging directory as its control thread starts, before it
        // answers any request: one answer first, or that cleanup removes what is made here.
        self.local().call(Command::ServerInfo).unwrap();
        let d = self
            .sessions()
            .join(".incoming")
            .join(format!("download-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        for i in 0..40_000 {
            std::fs::File::create(d.join(i.to_string())).unwrap();
        }
    }

    /// A session `name` whose manifest is a named pipe: reading it waits until a writer
    /// opens it. Returns the pipe.
    fn slow_session(&self, name: &str) -> PathBuf {
        let dir = self.sessions().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let pipe = dir.join("session.json");
        let ok = std::process::Command::new("mkfifo")
            .arg(&pipe)
            .status()
            .unwrap()
            .success();
        assert!(ok, "mkfifo");
        pipe
    }
}

fn send(c: &mut Client, cmd: Command) -> ac2_proto::units::RequestId {
    let id = c.next_id();
    c.send(&Request::new(id, cmd));
    id
}

/// The reply to `id`, which must be the next one `c` receives.
fn reply(c: &Client, id: ac2_proto::units::RequestId) -> Result<ac2_proto::ReplyBody, ProtoError> {
    let r = c.recv(T).expect("a reply");
    assert_eq!(r.id, id, "{r:?}");
    r.result
}

/// Sends `cmd` and waits for the answer to a `server.info` sent after it on the same
/// connection: `cmd` has then reached the daemon and, if it runs as a job, begun.
fn begun(c: &mut Client, cmd: Command) -> ac2_proto::units::RequestId {
    let id = send(c, cmd);
    let probe = send(c, Command::ServerInfo);
    let r = c.recv(T).expect("the probe's reply");
    assert_eq!(r.id, probe, "the job ended before the probe: {r:?}");
    id
}

fn refused(r: Result<ac2_proto::ReplyBody, ProtoError>) -> ProtoError {
    let e = r.expect_err("refused");
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");
    e
}

fn export(trace: ac2_proto::units::TraceId) -> Command {
    Command::TraceExport {
        trace,
        format: ExportFormat::Ac2Csv,
    }
}

fn working_download() -> Command {
    Command::FileDownloadBegin { session: None }
}

/// A viewer holds one transfer at a time, counting a download still being packed; viewers
/// together hold at most half of all transfers, so an operator always finds room.
#[test]
fn a_viewer_holds_one_transfer_and_viewers_half() {
    let dir = tempfile::tempdir().unwrap();
    let rig = start(dir.path());
    let mut host = rig.local();
    let trace = import_fixture(
        &mut host,
        "rew_export.txt",
        "rew_export.txt",
        TraceOwner::Imported,
    )
    .id;

    rig.slow_stage(1);
    let mut phone = rig.ctrl(&rig.phone);
    let pending = begun(&mut phone, working_download());
    let e = refused(phone.call(export(trace)));
    assert!(e.msg.contains("one transfer"), "{e:?}");
    assert!(matches!(
        reply(&phone, pending),
        Ok(ac2_proto::ReplyBody::Transfer(_))
    ));

    // The phone holds one, three more viewers one each; the wall finds the viewers' share
    // taken.
    let mut held = Vec::new();
    for k in [&rig.tablet, &rig.watch, &rig.kiosk] {
        let mut c = rig.ctrl(k);
        c.transfer(export(trace));
        held.push(c);
    }
    let mut wall = rig.ctrl(&rig.wall);
    let e = refused(wall.call(export(trace)));
    assert!(e.msg.contains("viewers"), "{e:?}");
    // An operator still has room.
    let mut laptop = rig.ctrl(&rig.laptop);
    laptop.transfer(export(trace));
    rig.h.shutdown();
}

/// A viewer syncs a state over one ctrl message with every transfer slot taken: a
/// snapshot transfer is outside the transfer quota, the viewers' share of it and a viewer's
/// one at a time, so resyncing takes nothing from an operator and is never refused for room.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_viewer_syncs_a_big_state_with_every_transfer_slot_taken() {
    use crate::big_state::{PLANS_OVER_CTRL, UNDER_EVENT_BOUND, big_plan};
    let dir = tempfile::tempdir().unwrap();
    let rig = start(dir.path());
    let mut host = rig.local();
    for i in 0..PLANS_OVER_CTRL {
        host.ok(Command::PlanSave {
            plan: big_plan(&format!("big {i}"), UNDER_EVENT_BOUND),
        });
    }
    // The viewers' share held by four viewers, the rest by the operator and the host's
    // clients (two each, a peer's share).
    let mut phone = rig.ctrl(&rig.phone);
    phone.transfer(working_download());
    let mut held = Vec::new();
    for k in [&rig.tablet, &rig.watch, &rig.kiosk] {
        let mut c = rig.ctrl(k);
        c.transfer(working_download());
        held.push(c);
    }
    let mut laptop = rig.ctrl(&rig.laptop);
    laptop.transfer(working_download());
    laptop.transfer(working_download());
    host.transfer(working_download());
    host.transfer(working_download());
    let e = refused(rig.local().call(working_download()));
    assert!(e.msg.contains("transfers are under way"), "{e:?}");

    // The phone, holding its one transfer, is sent the state as a transfer all the same.
    let t = match phone.call(Command::StateSnapshot).unwrap() {
        ac2_proto::ReplyBody::SnapshotTransfer(t) => t,
        other => panic!("{other:?}"),
    };
    assert!(t.transfer.bytes as usize > ac2_proto::MAX_CTRL_BYTES);

    // A viewer's mirror connecting now syncs from such a transfer.
    let (ctrl, data) = net_endpoints(&rig.h);
    let mut cc = ac2_client::ClientConfig::new(ac2_client::Endpoints { ctrl, data }, "watch");
    cc.curve = Some(rig.curve(&rig.watch));
    let watch = ac2_client::Client::connect(cc).await.unwrap();
    watch.wait_synced(T).await.unwrap();
    assert_eq!(watch.role(), ClientRole::Viewer);
    let v = watch.view();
    assert_eq!(v.state.as_ref().unwrap().plans.len(), PLANS_OVER_CTRL);
    assert_eq!(v.snapshots, 1);
    drop(watch);
    tokio::task::spawn_blocking(move || rig.h.shutdown())
        .await
        .unwrap();
}

/// A transfer kept busy still ends at its lifetime.
#[test]
fn a_transfer_ends_at_its_lifetime_however_busy() {
    let dir = tempfile::tempdir().unwrap();
    let lifetime = Duration::from_millis(800);
    let rig = start_with(dir.path(), manual_rig(), |c| {
        c.transfers.lifetime = lifetime
    });
    let mut host = rig.local();
    let trace = import_fixture(
        &mut host,
        "rew_export.txt",
        "rew_export.txt",
        TraceOwner::Imported,
    )
    .id;
    let mut laptop = rig.ctrl(&rig.laptop);
    let began = Instant::now();
    let t = laptop.transfer(export(trace));
    let gone = loop {
        match laptop.call(Command::FileDownloadChunk {
            transfer: t.transfer,
            offset: 0,
        }) {
            Ok(_) => assert!(
                began.elapsed() < T,
                "still served after {:?}",
                began.elapsed()
            ),
            Err(e) => break e,
        }
    };
    assert!(began.elapsed() >= lifetime, "{gone:?}");
    assert_eq!(gone.code, ErrorCode::NotFound, "{gone:?}");
    rig.h.shutdown();
}

/// An operator's file operation never waits on a viewer's download: it cuts it, and the
/// viewer is told to ask again. A viewer then waits a few seconds before its next one.
#[test]
fn an_operators_save_cuts_a_viewers_download() {
    let dir = tempfile::tempdir().unwrap();
    let rig = start(dir.path());
    let mut host = rig.local();
    rig.slow_stage(1);
    let mut phone = rig.ctrl(&rig.phone);
    let pending = begun(&mut phone, working_download());
    host.session_file(Command::FileSave {
        session: SessionRef::Name {
            name: "show".into(),
        },
        overwrite: false,
    });
    let e = refused(reply(&phone, pending));
    assert!(e.msg.contains("file.save came first"), "{e:?}");
    let e = refused(phone.call(working_download()));
    assert!(e.msg.contains("try again in"), "{e:?}");
    rig.h.shutdown();
}

/// `file.list` requests while one reads the disk attach to it: every one is answered from
/// that one reading, none refused as busy.
#[test]
fn file_lists_attach_to_the_one_running() {
    let dir = tempfile::tempdir().unwrap();
    let rig = start(dir.path());
    let pipe = rig.slow_session("slow");
    let mut host = rig.local();
    let mut laptop = rig.ctrl(&rig.laptop);
    let mut phone = rig.ctrl(&rig.phone);
    let first = begun(&mut host, Command::FileList);
    let second = begun(&mut laptop, Command::FileList);
    let third = begun(&mut phone, Command::FileList);
    std::thread::spawn(move || std::fs::write(pipe, b"not a manifest").unwrap());
    for (c, id) in [(&host, first), (&laptop, second), (&phone, third)] {
        assert!(reply(c, id).is_ok());
    }
    rig.h.shutdown();
}

/// A demoted operator's download, begun before, is refused when packed: nothing is handed
/// out to a role that may no longer take it, and no transfer is left.
#[test]
fn a_demotion_cuts_what_the_operator_began() {
    let dir = tempfile::tempdir().unwrap();
    let rig = start(dir.path());
    let mut host = rig.local();
    host.ok(Command::FileSave {
        session: SessionRef::Name {
            name: "show".into(),
        },
        overwrite: false,
    });
    rig.slow_stage(1);
    let mut laptop = rig.ctrl(&rig.laptop);
    let pending = begun(&mut laptop, working_download());
    host.ok(Command::AuthAuthorize {
        name: "laptop".into(),
        key: rig.laptop.public.to_z85(),
        role: ClientRole::Viewer,
    });
    let e = refused(reply(&laptop, pending));
    assert!(e.msg.contains("role"), "{e:?}");
    let e = laptop
        .call(Command::FileDownloadChunk {
            transfer: TransferId(1),
            offset: 0,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound, "{e:?}");
    rig.h.shutdown();
}

/// A loopback detection sounding when its client is demoted is cut, and its finding is
/// not handed out.
#[test]
fn a_demotion_cuts_a_sounding_detection() {
    let dir = tempfile::tempdir().unwrap();
    let rig = start_with(dir.path(), realtime_rig(), |_| {});
    let mut laptop = rig.ctrl(&rig.laptop);
    let token = laptop.acquire();
    let pending = begun(
        &mut laptop,
        Command::StreamDetectLoopback {
            lease_token: token,
            backend: ac2_proto::model::BackendKind::Fake,
            input_device: ac2_proto::model::DeviceId(ac2_audio::fake::FAKE_DEVICE_ID.into()),
            output_device: ac2_proto::model::DeviceId(ac2_audio::fake::FAKE_DEVICE_ID.into()),
            output: 0,
            level: ac2_proto::units::Dbfs(-30.0),
        },
    );
    let mut host = rig.local();
    host.ok(Command::AuthAuthorize {
        name: "laptop".into(),
        key: rig.laptop.public.to_z85(),
        role: ClientRole::Viewer,
    });
    // Refused, whether the cut burst or the role check answers first.
    let e = refused(reply(&laptop, pending));
    assert!(e.msg.contains("role") || e.msg.contains("cut"), "{e:?}");
    // The device is free again: the host's own detection runs.
    let token = host.acquire();
    host.loopback_detection(Command::StreamDetectLoopback {
        lease_token: token,
        backend: ac2_proto::model::BackendKind::Fake,
        input_device: ac2_proto::model::DeviceId(ac2_audio::fake::FAKE_DEVICE_ID.into()),
        output_device: ac2_proto::model::DeviceId(ac2_audio::fake::FAKE_DEVICE_ID.into()),
        output: 0,
        level: ac2_proto::units::Dbfs(-30.0),
    });
    rig.h.shutdown();
}

/// A data peer subscribing without end is dropped past the limit: it must rejoin.
#[test]
fn a_subscription_flood_drops_the_peer() {
    let dir = tempfile::tempdir().unwrap();
    let rig = start(dir.path());
    let ka: &[u8] = b"ka";
    let phone = rig.data(&rig.phone, &[ka]);
    assert!(phone.ka(T).is_some());
    // With `ka`, one past the daemon's limit of 4096 subscriptions per data peer.
    for i in 0..4096 {
        phone.subscribe(ac2_proto::Subscription::Meas(MeasId(i)));
    }
    phone.up(DataUp::Keyframe { topic: Topic::Ka });
    let until = Instant::now() + T;
    loop {
        match phone.next(T) {
            Some(DataMessage::Flow(ac2_proto::FlowNotice::Rejoin)) => break,
            Some(_) => assert!(Instant::now() < until, "not dropped"),
            None => panic!("not dropped"),
        }
    }
    rig.h.shutdown();
}

/// No `auth.*` command lets an operator lock itself out, and a rig's key never takes an
/// app's name.
#[test]
fn an_operator_cannot_cut_itself_off() {
    let dir = tempfile::tempdir().unwrap();
    let rig = start(dir.path());
    let mut laptop = rig.ctrl(&rig.laptop);
    let other = KeyPair::generate().unwrap();
    for (key, role) in [
        (rig.laptop.public.to_z85(), ClientRole::Viewer),
        (other.public.to_z85(), ClientRole::Operator),
    ] {
        refused(laptop.call(Command::AuthAuthorize {
            name: "laptop".into(),
            key,
            role,
        }));
    }
    // The same entry again changes nothing and is allowed.
    laptop.server(Command::AuthAuthorize {
        name: "laptop".into(),
        key: rig.laptop.public.to_z85(),
        role: ClientRole::Operator,
    });
    for name in ["laptop", "phone"] {
        let e = laptop
            .call(Command::AuthAdmitRig {
                key: other.public.to_z85(),
                rig_id: ac2_proto::units::RigId(7),
                name: name.into(),
            })
            .unwrap_err();
        assert!(e.msg.contains("app's name"), "{e:?}");
    }
    laptop.acquire();
    rig.h.shutdown();
}

/// `gen.stop` with nothing to stop changes nothing, whoever sends it.
#[test]
fn an_idle_gen_stop_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let rig = start(dir.path());
    let mut host = rig.local();
    let rev = host.state_rev();
    let generator = host.state().generator;
    let mut phone = rig.ctrl(&rig.phone);
    let mut laptop = rig.ctrl(&rig.laptop);
    for c in [&mut phone, &mut laptop, &mut host] {
        assert_eq!(c.ack(Command::GenStop), rev);
    }
    assert_eq!(host.state_rev(), rev);
    assert_eq!(host.state().generator, generator);
    rig.h.shutdown();
}

/// What a network client reads of the host: base names, not paths; a viewer sees its own
/// key only and not where recordings lie.
#[test]
fn a_network_client_reads_no_host_paths() {
    let dir = tempfile::tempdir().unwrap();
    let rec = dir.path().join("recordings");
    let rig = start_with(dir.path(), manual_rig(), |c| {
        c.recording_dir = Some(rec.clone())
    });
    let mut host = rig.local();
    let at = dir.path().join("elsewhere").join("friday.ac2session");
    host.ok(Command::FileSave {
        session: SessionRef::Name {
            name: "show".into(),
        },
        overwrite: false,
    });
    // Subscribed once the keepalive subscribed with it arrives.
    let (evt, ka): (&[u8], &[u8]) = (b"evt", b"ka");
    let events = rig.data(&rig.laptop, &[evt, ka]);
    let local_events = Sub::connect(rig.h.context(), rig.h.data_endpoint(), &[evt, ka]);
    assert!(events.ka(T).is_some() && local_events.ka(T).is_some());
    host.ok(Command::FileSave {
        session: SessionRef::Path {
            path: at.display().to_string(),
        },
        overwrite: true,
    });
    let origin = |sub: &Sub| {
        let e = sub
            .event(
                T,
                |e| matches!(&e.change, Change::Working(w) if w.origin.is_some()),
            )
            .expect("the working state's event");
        match e.change {
            Change::Working(w) => w.origin,
            _ => unreachable!(),
        }
    };
    assert_eq!(
        origin(&events),
        Some(SessionOrigin::Path {
            path: "friday.ac2session".into()
        })
    );
    assert!(matches!(
        origin(&local_events),
        Some(SessionOrigin::Path { path }) if path.contains('/')
    ));
    let mut phone = rig.ctrl(&rig.phone);
    let mut laptop = rig.ctrl(&rig.laptop);

    let seen = phone.server(Command::ServerInfo);
    let names: Vec<_> = seen
        .network
        .authorized
        .iter()
        .map(|a| a.name.as_str())
        .collect();
    assert_eq!(names, ["phone"]);
    assert_eq!(seen.recordings, Recordings::Kept);
    assert_eq!(
        laptop.server(Command::ServerInfo).network.authorized.len(),
        6
    );
    assert!(matches!(
        host.server(Command::ServerInfo).recordings,
        Recordings::Dir { .. }
    ));

    for c in [&mut phone, &mut laptop] {
        for f in c.sessions(Command::FileList) {
            assert!(!f.path.contains('/'), "{f:?}");
        }
        let origin = c.state().working.origin;
        assert_eq!(
            origin,
            Some(SessionOrigin::Path {
                path: "friday.ac2session".into()
            })
        );
    }
    assert!(
        host.sessions(Command::FileList)
            .iter()
            .all(|f| f.path.contains('/'))
    );
    assert!(matches!(
        host.state().working.origin,
        Some(SessionOrigin::Path { path }) if path.contains('/')
    ));
    rig.h.shutdown();
}

/// An authorized-clients file another user or the group could write admits no one; the
/// damage says why.
#[test]
fn a_writable_client_list_admits_no_one() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let rig = start_with(dir.path(), manual_rig(), |c| {
        let f = &c.keys.as_ref().unwrap().authorized_clients_file;
        std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o622)).unwrap();
    });
    let mut host = rig.local();
    assert!(
        host.server(Command::ServerInfo)
            .network
            .authorized
            .is_empty()
    );
    let damaged = host.state().rig.damaged_files;
    assert!(
        damaged
            .iter()
            .any(|d| d.kind == StateFile::AuthorizedClients && d.reason.contains("writable")),
        "{damaged:?}"
    );
    rig.h.shutdown();
}

/// Deleting and renaming are an operator's: a viewer is refused as view-only, and a
/// network operator's renamed session reads by its base name only.
#[test]
fn a_viewer_deletes_nothing_and_an_operator_reads_base_names() {
    let dir = tempfile::tempdir().unwrap();
    let rig = start(dir.path());
    let mut host = rig.local();
    host.ok(Command::FileSave {
        session: SessionRef::Name {
            name: "friday".into(),
        },
        overwrite: false,
    });
    host.ok(Command::FileNew { discard: true });
    let mut phone = rig.ctrl(&rig.phone);
    for cmd in [
        Command::FileDelete {
            name: "friday".into(),
        },
        Command::FileRename {
            name: "friday".into(),
            to: "x".into(),
        },
        Command::FileUploadAbort {
            transfer: TransferId(1),
        },
        Command::RecDelete {
            recording: ac2_proto::model::RecordingRef::Name { name: "x".into() },
        },
    ] {
        let e = refused(phone.call(cmd));
        assert_eq!(e.detail, Some(ac2_proto::ErrorDetail::ViewOnly), "{e:?}");
    }
    let mut laptop = rig.ctrl(&rig.laptop);
    let f = match laptop
        .call(Command::FileRename {
            name: "friday".into(),
            to: "friday show".into(),
        })
        .unwrap()
    {
        ac2_proto::ReplyBody::SessionFile(f) => f,
        other => panic!("{other:?}"),
    };
    assert_eq!(f.path, "friday show");
    assert!(rig.sessions().join("friday show/session.json").is_file());
    laptop
        .call(Command::FileDelete {
            name: "friday show".into(),
        })
        .unwrap();
    assert!(!rig.sessions().join("friday show").exists());
    rig.h.shutdown();
}
