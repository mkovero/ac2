//! Session file work runs off the control thread: while a save, load, salvage or recover
//! waits on its disk,
//! lease refreshes, the state and other clients are still answered, a second file operation
//! is refused until the first is answered, and a save writes the content as it was when it
//! began. Slow storage is a named pipe in place of a session's manifest: reading it blocks
//! until the test writes to it.
#![cfg(unix)]
#![allow(clippy::unwrap_used)]

use crate::common;

use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::Duration;

use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Command, ErrorCode, Reply, ReplyBody, Request};
use ac2d::{AutosaveConfig, Daemon, Handle};
use common::*;

fn start(name: &str, dir: &Path) -> Handle {
    init_log();
    let mut cfg = config(manual_rig(), inproc(&unique(name)));
    cfg.session_dir = dir.join("sessions");
    Daemon::start(cfg).unwrap()
}

/// [`start`] with an autosave beside the session directory (the recovery copy on disk).
fn start_autosaved(name: &str, dir: &Path) -> Handle {
    init_log();
    let mut cfg = config(manual_rig(), inproc(&unique(name)));
    cfg.session_dir = dir.join("sessions");
    cfg.autosave = Some(AutosaveConfig {
        dir: dir.join("autosave"),
        restore: true,
    });
    Daemon::start(cfg).unwrap()
}

/// A named pipe at `p`: reading it waits for a writer.
fn mkfifo(p: &Path) {
    let ok = std::process::Command::new("mkfifo")
        .arg(p)
        .status()
        .unwrap()
        .success();
    assert!(ok, "mkfifo");
}

/// A session directory `name` whose manifest is a named pipe: reading it waits for
/// [`release`].
fn slow_session(h_dir: &Path, name: &str) -> PathBuf {
    let dir = h_dir.join("sessions").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let pipe = dir.join("session.json");
    mkfifo(&pipe);
    pipe
}

fn acquire(c: &mut Client) -> LeaseToken {
    match c.ok(Command::GenAcquire { force: false }) {
        ReplyBody::Lease(l) => l.lease_token,
        other => panic!("{other:?}"),
    }
}

/// Every other file operation is refused while one is under way.
fn others_refused(c: &mut Client) {
    for cmd in [
        Command::FileSave {
            session: named("other"),
            overwrite: false,
        },
        Command::FileRecover,
        Command::FileSalvage {
            session: named("other"),
        },
    ] {
        let e = c.call(cmd).unwrap_err();
        assert_eq!(e.code, ErrorCode::Refused, "{e:?}");
    }
}

/// Lets one read of `pipe` finish, with content that is no manifest.
fn release(pipe: PathBuf) -> JoinHandle<()> {
    std::thread::spawn(move || std::fs::write(pipe, b"not a manifest").unwrap())
}

fn import(c: &mut Client) -> TraceMeta {
    let content = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../ac2-traces/tests/fixtures/rew_export.txt"),
    )
    .unwrap();
    match c.ok(Command::TraceImport {
        file_name: "rew_export.txt".into(),
        format: ImportFormat::Auto,
        role: ImportRole::Trace,
        ir: None,
        content: Blob(content),
    }) {
        ReplyBody::Trace(t) => t,
        other => panic!("{other:?}"),
    }
}

fn working(c: &mut Client) -> WorkingSession {
    match c.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => s.state.working,
        other => panic!("{other:?}"),
    }
}

fn named(name: &str) -> SessionRef {
    SessionRef::Name { name: name.into() }
}

/// Sends `cmd` without waiting; returns its id.
fn send(c: &mut Client, cmd: Command) -> RequestId {
    let id = c.next_id();
    c.send(&Request::new(id, cmd));
    id
}

fn reply(c: &Client, id: RequestId, within: Duration) -> Option<Reply> {
    let r = c.recv(within)?;
    assert_eq!(r.id, id);
    Some(r)
}

#[test]
fn a_load_waiting_on_its_disk_blocks_no_one() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("slow-load", dir.path());
    let (mut a, _sub) = connect(&h, &[]);
    let (mut b, _sub_b) = connect(&h, &[]);
    import(&mut a);
    let lease = match b.ok(Command::GenAcquire { force: false }) {
        ReplyBody::Lease(l) => l.lease_token,
        other => panic!("{other:?}"),
    };
    let pipe = slow_session(dir.path(), "slow");

    let load = send(
        &mut a,
        Command::FileLoad {
            session: named("slow"),
            discard: true,
        },
    );
    // The control thread may not even have started it; either way the load is not
    // answered while its manifest cannot be read.
    assert!(reply(&a, load, Duration::from_millis(300)).is_none());

    // Another client's lease refresh, a state request and the universal stop.
    b.ok(Command::GenRefresh { lease_token: lease });
    assert_eq!(working(&mut b).origin, None);
    b.ok(Command::GenStop);
    // One file operation at a time.
    let e = b
        .call(Command::FileSave {
            session: named("other"),
            overwrite: false,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");
    let e = b.call(Command::FileNew { discard: true }).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");
    let writer = release(pipe);
    let r = reply(&a, load, Duration::from_secs(10)).expect("the load is answered");
    writer.join().unwrap();
    let e = r.result.unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid, "{e:?}");
    // A refused load changed nothing, and file operations are taken again.
    let w = working(&mut a);
    assert!(w.dirty && w.recover.is_none(), "{w:?}");
    b.ok(Command::FileSave {
        session: named("other"),
        overwrite: false,
    });
    h.shutdown();
}

#[test]
fn a_save_writes_the_content_it_began_with() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("slow-save", dir.path());
    let (mut a, _sub) = connect(&h, &[]);
    let (mut b, _sub_b) = connect(&h, &[]);
    let t = import(&mut a);
    // Saving onto a session that is not the working state's asks: it reads the manifest
    // there first, and that read waits.
    let pipe = slow_session(dir.path(), "slow");
    let save = send(
        &mut a,
        Command::FileSave {
            session: named("slow"),
            overwrite: false,
        },
    );
    assert!(reply(&a, save, Duration::from_millis(300)).is_none());

    // An edit while the save waits.
    let mut edit = t.edit.clone();
    edit.name = "edited meanwhile".into();
    b.ok(Command::TraceUpdate { trace: t.id, edit });

    let writer = release(pipe);
    let r = reply(&a, save, Duration::from_secs(10)).expect("the save is answered");
    writer.join().unwrap();
    match r.result.unwrap() {
        ReplyBody::SessionFile(f) => assert_eq!(f.traces, 1),
        other => panic!("{other:?}"),
    }
    // It belongs to the session now, and the edit made after the save began is unsaved.
    let w = working(&mut b);
    assert_eq!(
        w.origin,
        Some(SessionOrigin::Name {
            name: "slow".into()
        })
    );
    assert!(w.dirty, "the edit came after the snapshot");

    // What was written is the content as the save began.
    b.ok(Command::FileLoad {
        session: named("slow"),
        discard: true,
    });
    let ts = match b.ok(Command::TraceList) {
        ReplyBody::Traces(t) => t,
        other => panic!("{other:?}"),
    };
    assert_eq!(ts.len(), 1);
    assert_eq!(ts[0].edit.name, t.edit.name);
    h.shutdown();
}

#[test]
fn a_retry_of_a_pending_file_operation_runs_once() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("slow-retry", dir.path());
    let (mut a, _sub) = connect(&h, &[]);
    import(&mut a);
    let pipe = slow_session(dir.path(), "slow");
    let id = a.next_id();
    let req = Request::new(
        id,
        Command::FileSave {
            session: named("slow"),
            overwrite: false,
        },
    );
    a.send(&req);
    assert!(reply(&a, id, Duration::from_millis(200)).is_none());
    // The client gave up waiting and sends it again: the same request, not a second save.
    a.send(&req);
    assert!(reply(&a, id, Duration::from_millis(200)).is_none());
    let writer = release(pipe);
    let r = reply(&a, id, Duration::from_secs(10)).expect("answered");
    writer.join().unwrap();
    assert!(r.result.is_ok(), "{:?}", r.result);
    // Each copy gets the one save's reply; no second save follows it.
    let again = reply(&a, id, Duration::from_secs(1)).expect("the retry answered");
    assert_eq!(again.result, r.result);
    assert!(a.recv(Duration::from_millis(200)).is_none());
    h.shutdown();
}

#[test]
fn a_salvage_waiting_on_its_disk_blocks_no_one() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("slow-salvage", dir.path());
    let (mut a, _sub) = connect(&h, &[]);
    let (mut b, _sub_b) = connect(&h, &[]);
    import(&mut a);
    a.ok(Command::FileSave {
        session: named("s"),
        overwrite: false,
    });
    let pipe = dir.path().join("sessions/s/traces/zz.csv");
    mkfifo(&pipe);
    let lease = acquire(&mut b);

    let salvage = send(
        &mut a,
        Command::FileSalvage {
            session: named("s"),
        },
    );
    assert!(reply(&a, salvage, Duration::from_millis(300)).is_none());
    b.ok(Command::GenRefresh { lease_token: lease });
    assert_eq!(
        working(&mut b).origin,
        Some(SessionOrigin::Name { name: "s".into() })
    );
    others_refused(&mut b);

    let writer = release(pipe);
    let r = reply(&a, salvage, Duration::from_secs(10)).expect("the salvage is answered");
    writer.join().unwrap();
    match r.result.unwrap() {
        ReplyBody::Salvage(s) => {
            assert_eq!(s.imported.len(), 1, "{s:?}");
            assert_eq!(s.refused.len(), 1, "{s:?}");
            assert_eq!(s.refused[0].file, "traces/zz.csv");
        }
        other => panic!("{other:?}"),
    }
    h.shutdown();
}

#[test]
fn a_recover_waiting_on_its_disk_blocks_no_one_and_commits_only_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    // A first run keeps an edited working state on disk for file.recover.
    {
        let h = start_autosaved("slow-recover-1", dir.path());
        let (mut a, _sub) = connect(&h, &[]);
        import(&mut a);
        a.ok(Command::FileNew { discard: true });
        assert!(working(&mut a).recover.is_some());
        h.shutdown();
    }
    let rd = dir.path().join("autosave.replaced");
    assert!(rd.join("session.json").is_file());
    let tf = std::fs::read_dir(rd.join("traces"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .next()
        .expect("the kept trace's file");
    let bytes = std::fs::read(&tf).unwrap();
    std::fs::remove_file(&tf).unwrap();
    mkfifo(&tf);

    let h = start_autosaved("slow-recover-2", dir.path());
    let (mut a, _sub) = connect(&h, &[]);
    let (mut b, _sub_b) = connect(&h, &[]);
    assert!(working(&mut a).recover.is_some());
    let lease = acquire(&mut b);

    let recover = send(&mut a, Command::FileRecover);
    assert!(reply(&a, recover, Duration::from_millis(300)).is_none());
    b.ok(Command::GenRefresh { lease_token: lease });
    assert!(working(&mut b).recover.is_some());
    others_refused(&mut b);
    // The working state changes while the copy is read: the recover is refused.
    import(&mut b);
    let writer = {
        let (tf, bytes) = (tf.clone(), bytes.clone());
        std::thread::spawn(move || std::fs::write(tf, bytes).unwrap())
    };
    let r = reply(&a, recover, Duration::from_secs(10)).expect("the recover is answered");
    writer.join().unwrap();
    let e = r.result.unwrap_err();
    assert_eq!(e.code, ErrorCode::Conflict, "{e:?}");
    let w = working(&mut a);
    assert!(w.dirty && w.recover.is_some(), "{w:?}");

    // Asked again on a readable copy: swapped, the replaced state kept on disk in turn.
    std::fs::remove_file(&tf).unwrap();
    std::fs::write(&tf, &bytes).unwrap();
    a.ok(Command::FileRecover);
    let w = working(&mut a);
    assert!(w.dirty && w.recover.is_some(), "{w:?}");
    h.shutdown();
    assert!(rd.join("session.json").is_file());
    assert!(rd.join("working.json").is_file());
}
