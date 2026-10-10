//! The working state's session (`docs/design/sessions.md`): unsaved changes follow the
//! edits and saves, `file.new` / `file.load` refuse to drop unsaved changes unless told and
//! keep what they replace for `file.recover`, a save onto another session asks, archives
//! travel in checked chunks within their limits, an old-format session's traces can be
//! salvaged, remote clients still never name daemon paths, and a restart keeps all of it.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::path::Path;
use std::time::Duration;

use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Command, ErrorCode, ErrorDetail, ReplyBody};
use ac2d::{AutosaveConfig, Daemon, Handle};
use common::*;

fn start(name: &str, dir: &Path, autosave: bool) -> Handle {
    init_log();
    let mut cfg = config(manual_rig(), inproc(&unique(name)));
    cfg.session_dir = dir.join("sessions");
    cfg.autosave = autosave.then(|| AutosaveConfig {
        dir: dir.join("autosave"),
        restore: true,
    });
    Daemon::start(cfg).unwrap()
}

fn working(c: &mut Client) -> WorkingSession {
    match c.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => s.state.working,
        other => panic!("{other:?}"),
    }
}

fn traces(c: &mut Client) -> Vec<TraceMeta> {
    match c.ok(Command::TraceList) {
        ReplyBody::Traces(t) => t,
        other => panic!("{other:?}"),
    }
}

fn import(c: &mut Client, name: &str) -> TraceMeta {
    let content = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../ac2-traces/tests/fixtures")
            .join(name),
    )
    .unwrap();
    match c.ok(Command::TraceImport {
        file_name: name.into(),
        format: ImportFormat::Auto,
        role: ImportRole::Trace,
        ir: None,
        content: Blob(content),
    }) {
        ReplyBody::Trace(t) => t,
        other => panic!("{other:?}"),
    }
}

fn rename(c: &mut Client, t: &TraceMeta, name: &str) {
    let mut edit = t.edit.clone();
    edit.name = name.into();
    c.ok(Command::TraceUpdate { trace: t.id, edit });
}

fn named(name: &str) -> SessionRef {
    SessionRef::Name { name: name.into() }
}

fn save(c: &mut Client, name: &str, overwrite: bool) -> Result<SessionFile, ac2_proto::ProtoError> {
    c.call(Command::FileSave {
        session: named(name),
        overwrite,
    })
    .map(|r| match r {
        ReplyBody::SessionFile(f) => f,
        other => panic!("{other:?}"),
    })
}

/// Uploads `bytes` in chunks of the daemon's size; returns the upload.
fn upload(c: &mut Client, file: &str, bytes: &[u8]) -> TransferId {
    let t = match c.ok(Command::FileUploadBegin {
        file: file.into(),
        bytes: bytes.len() as u64,
        sha256: ac2_archive::sha256_hex(bytes),
    }) {
        ReplyBody::Transfer(t) => t,
        other => panic!("{other:?}"),
    };
    for (i, chunk) in bytes.chunks(t.chunk_bytes as usize).enumerate() {
        c.ok(Command::FileUploadChunk {
            transfer: t.transfer,
            offset: (i * t.chunk_bytes as usize) as u64,
            data: Blob(chunk.to_vec()),
        });
    }
    t.transfer
}

/// Downloads the working state (`None`) or a session; returns the transfer and the bytes.
fn download(c: &mut Client, session: Option<SessionRef>) -> (Transfer, Vec<u8>) {
    let t = match c.ok(Command::FileDownloadBegin { session }) {
        ReplyBody::Transfer(t) => t,
        other => panic!("{other:?}"),
    };
    let mut data = Vec::new();
    while (data.len() as u64) < t.bytes {
        match c.ok(Command::FileDownloadChunk {
            transfer: t.transfer,
            offset: data.len() as u64,
        }) {
            ReplyBody::Chunk(b) => data.extend_from_slice(&b.0),
            other => panic!("{other:?}"),
        }
    }
    ac2_archive::verify(&data, &t.sha256).unwrap();
    (t, data)
}

#[test]
fn unsaved_changes_follow_edits_and_saves() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("dirty", dir.path(), false);
    let (mut c, _sub) = connect(&h, &[]);
    let c = &mut c;
    let w = working(c);
    assert_eq!(
        (w.origin.clone(), w.dirty, w.recover.clone()),
        (None, false, None)
    );
    let t = import(c, "rew_export.txt");
    assert!(working(c).dirty, "untitled with a trace");
    let f = save(c, "friday", false).unwrap();
    let w = working(c);
    assert_eq!(
        w.origin,
        Some(SessionOrigin::Name {
            name: "friday".into()
        })
    );
    assert!(!w.dirty);
    assert_eq!(w.saved_at, Some(f.saved_at));
    rename(c, &t, "renamed");
    assert!(working(c).dirty);
    // Undone by hand: the same content again.
    rename(c, &t, &t.edit.name);
    assert!(!working(c).dirty);
    // A measurement starting or stopping is not an edit.
    c.ok(Command::MeasCreate {
        config: transfer("main"),
    });
    assert!(working(c).dirty);
    save(c, "friday", false).unwrap();
    h.shutdown();
}

#[test]
fn new_and_load_keep_unsaved_changes_for_recover() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("guard", dir.path(), false);
    let (mut c, _sub) = connect(&h, &[]);
    let c = &mut c;
    import(c, "rew_export.txt");
    save(c, "a", false).unwrap();
    let t = import(c, "smaart_tf.txt");
    let e = c.call(Command::FileNew { discard: false }).unwrap_err();
    assert_eq!(e.code, ErrorCode::Conflict);
    assert_eq!(
        e.detail,
        Some(ErrorDetail::UnsavedChanges {
            origin: Some(SessionOrigin::Name { name: "a".into() })
        })
    );
    let e = c
        .call(Command::FileLoad {
            session: named("a"),
            discard: false,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Conflict);
    assert_eq!(traces(c).len(), 2, "a refusal changes nothing");

    c.ok(Command::FileNew { discard: true });
    assert!(traces(c).is_empty());
    let w = working(c);
    assert_eq!((w.origin.clone(), w.dirty), (None, false));
    let r = w.recover.expect("recovery copy");
    assert_eq!((r.measurements, r.traces), (0, 2));
    assert_eq!(r.origin, Some(SessionOrigin::Name { name: "a".into() }));

    // Recover: the replaced state with its session and unsaved changes; the empty state
    // it replaces was clean, so nothing more is kept.
    c.ok(Command::FileRecover);
    let ts = traces(c);
    assert_eq!(ts.len(), 2);
    assert!(ts.iter().any(|x| x.edit.name == t.edit.name));
    let w = working(c);
    assert_eq!(w.origin, Some(SessionOrigin::Name { name: "a".into() }));
    assert!(w.dirty);
    assert_eq!(w.recover, None);
    let e = c.call(Command::FileRecover).unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);

    // A clean state opens another session without asking and keeps no copy.
    save(c, "a", false).unwrap();
    c.ok(Command::FileLoad {
        session: named("a"),
        discard: false,
    });
    assert_eq!(working(c).recover, None);
    h.shutdown();
}

#[test]
fn saving_onto_another_session_asks() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("overwrite", dir.path(), false);
    let (mut c, _sub) = connect(&h, &[]);
    let c = &mut c;
    import(c, "rew_export.txt");
    save(c, "a", false).unwrap();
    import(c, "smaart_tf.txt");
    save(c, "b", false).unwrap();
    let e = save(c, "a", false).unwrap_err();
    assert_eq!(e.code, ErrorCode::Conflict);
    assert!(matches!(
        e.detail,
        Some(ErrorDetail::SessionExists { ref name, .. }) if name == "a"
    ));
    // Its own session is saved again without asking.
    save(c, "b", false).unwrap();
    let f = save(c, "a", true).unwrap();
    assert_eq!(f.traces, 2);
    assert_eq!(
        working(c).origin,
        Some(SessionOrigin::Name { name: "a".into() })
    );
    h.shutdown();
}

#[test]
fn archives_round_trip_in_checked_chunks() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("archive", dir.path(), false);
    let (mut c, _sub) = connect(&h, &[]);
    let c = &mut c;
    import(c, "rew_export.txt");
    import(c, "smaart_tf.txt");
    let before = traces(c);
    let (t, bytes) = download(c, None);
    assert_eq!(t.file_name, "untitled.ac2session");
    // Written by the client: the working state belongs to that file, clean.
    let stick = "/run/media/foh/STICK/friday.ac2session";
    c.ok(Command::FileDownloadEnd {
        transfer: t.transfer,
        saved_as: Some(stick.into()),
    });
    let w = working(c);
    assert_eq!(w.origin, Some(SessionOrigin::Client { path: stick.into() }));
    assert!(!w.dirty);
    c.ok(Command::FileNew { discard: false });

    let up = upload(c, stick, &bytes);
    match c.ok(Command::FileLoad {
        session: SessionRef::Upload { transfer: up },
        discard: false,
    }) {
        ReplyBody::SessionFile(f) => {
            assert_eq!(f.name, "friday.ac2session");
            assert_eq!(f.path, stick);
            assert_eq!(f.traces, 2);
        }
        other => panic!("{other:?}"),
    }
    let after = traces(c);
    assert_eq!(
        after.iter().map(|t| &t.edit).collect::<Vec<_>>(),
        before.iter().map(|t| &t.edit).collect::<Vec<_>>()
    );
    let w = working(c);
    assert_eq!(w.origin, Some(SessionOrigin::Client { path: stick.into() }));
    assert!(!w.dirty);
    // Used up.
    let e = c
        .call(Command::FileLoad {
            session: SessionRef::Upload { transfer: up },
            discard: true,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);

    // Stored as a named session without opening it, then listed.
    let up = upload(c, stick, &bytes);
    c.ok(Command::FileImport {
        transfer: up,
        name: "from stick".into(),
        overwrite: false,
    });
    match c.ok(Command::FileList) {
        ReplyBody::Sessions(l) => {
            assert!(l.iter().any(|f| f.name == "from stick" && f.traces == 2))
        }
        other => panic!("{other:?}"),
    }
    // A named session downloads too.
    let (t, _) = download(c, Some(named("from stick")));
    assert_eq!(t.file_name, "from stick.ac2session");
    c.ok(Command::FileDownloadEnd {
        transfer: t.transfer,
        saved_as: None,
    });
    h.shutdown();
}

#[test]
fn transfers_refuse_damage_disorder_and_size() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("limits", dir.path(), false);
    let (mut c, _sub) = connect(&h, &[]);
    let c = &mut c;
    let e = c
        .call(Command::FileUploadBegin {
            file: "huge.ac2session".into(),
            bytes: ac2_archive::MAX_ARCHIVE_BYTES + 1,
            sha256: "0".repeat(64),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);
    assert!(e.msg.contains("at most"), "{}", e.msg);

    let begin = |c: &mut Client, bytes: &[u8], sha: String| match c.ok(Command::FileUploadBegin {
        file: "x.ac2session".into(),
        bytes: bytes.len() as u64,
        sha256: sha,
    }) {
        ReplyBody::Transfer(t) => t.transfer,
        other => panic!("{other:?}"),
    };
    let data = b"not really an archive".to_vec();
    // Out of order.
    let t = begin(c, &data, ac2_archive::sha256_hex(&data));
    let e = c
        .call(Command::FileUploadChunk {
            transfer: t,
            offset: 4,
            data: Blob(data[4..].to_vec()),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);
    // Damaged: the checksum does not match.
    let t = begin(c, &data, "0".repeat(64));
    let e = c
        .call(Command::FileUploadChunk {
            transfer: t,
            offset: 0,
            data: Blob(data.clone()),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);
    assert!(e.msg.contains("checksum"), "{}", e.msg);
    // Intact but not an archive: refused when opened, the state untouched.
    let t = begin(c, &data, ac2_archive::sha256_hex(&data));
    c.ok(Command::FileUploadChunk {
        transfer: t,
        offset: 0,
        data: Blob(data.clone()),
    });
    let e = c
        .call(Command::FileLoad {
            session: SessionRef::Upload { transfer: t },
            discard: false,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid, "{e:?}");
    // Another client cannot use it.
    let (mut other, _s2) = connect(&h, &[]);
    let e = other
        .call(Command::FileLoad {
            session: SessionRef::Upload { transfer: t },
            discard: false,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    h.shutdown();
}

#[test]
fn an_old_session_refuses_to_load_and_salvages_its_traces() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("salvage", dir.path(), false);
    let (mut c, _sub) = connect(&h, &[]);
    let c = &mut c;
    import(c, "rew_export.txt");
    import(c, "smaart_tf.txt");
    save(c, "old", false).unwrap();
    let sdir = dir.path().join("sessions/old");
    let m = std::fs::read_to_string(sdir.join("session.json")).unwrap();
    std::fs::write(
        sdir.join("session.json"),
        m.replace(
            &format!("\"version\": {}", ac2_traces::session::VERSION),
            "\"version\": 19",
        ),
    )
    .unwrap();
    std::fs::write(sdir.join("traces/99-junk.csv"), "# ac2 trace export v0\n").unwrap();
    c.ok(Command::FileNew { discard: true });
    let e = c
        .call(Command::FileLoad {
            session: named("old"),
            discard: false,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Unsupported);
    let s = match c.ok(Command::FileSalvage {
        session: named("old"),
    }) {
        ReplyBody::Salvage(s) => s,
        other => panic!("{other:?}"),
    };
    assert_eq!(s.version, Some(19));
    assert_eq!(s.imported.len(), 2, "{s:?}");
    assert_eq!(s.refused.len(), 1, "{s:?}");
    assert_eq!(s.refused[0].file, "traces/99-junk.csv");
    assert_eq!(traces(c).len(), 2);
    assert!(working(c).dirty, "salvaged traces are unsaved");
    h.shutdown();
}

#[test]
fn a_restart_keeps_the_session_unsaved_changes_and_recovery_copy() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("restart", dir.path(), true);
    let (mut c, _sub) = connect(&h, &[]);
    let t = import(&mut c, "rew_export.txt");
    save(&mut c, "show", false).unwrap();
    rename(&mut c, &t, "edited");
    let before = working(&mut c);
    assert!(before.dirty);
    drop(c);
    h.shutdown();

    let h = start("restart2", dir.path(), true);
    let (mut c, _sub) = connect(&h, &[]);
    let w = working(&mut c);
    assert_eq!(
        w.origin,
        Some(SessionOrigin::Name {
            name: "show".into()
        })
    );
    assert!(w.dirty, "still unsaved relative to the save of show");
    assert_eq!(w.saved_at, before.saved_at);
    c.ok(Command::FileNew { discard: true });
    assert!(working(&mut c).recover.is_some());
    drop(c);
    h.shutdown();

    let h = start("restart3", dir.path(), true);
    let (mut c, _sub) = connect(&h, &[]);
    let w = working(&mut c);
    assert_eq!((w.origin, w.dirty), (None, false));
    let r = w.recover.expect("recovery copy after restart");
    assert_eq!(r.traces, 1);
    c.ok(Command::FileRecover);
    let ts = traces(&mut c);
    assert_eq!(ts.len(), 1);
    assert_eq!(ts[0].edit.name, "edited");
    let w = working(&mut c);
    assert_eq!(
        w.origin,
        Some(SessionOrigin::Name {
            name: "show".into()
        })
    );
    assert!(w.dirty);
    drop(c);
    h.shutdown();
    // An autosave of an older daemon (no working.json) restores untitled, with its
    // content counted as unsaved.
    std::fs::remove_file(dir.path().join("autosave/working.json")).unwrap();
    let h = start("restart4", dir.path(), true);
    let (mut c, _sub) = connect(&h, &[]);
    std::thread::sleep(Duration::from_millis(10));
    let w = working(&mut c);
    assert_eq!((w.origin, w.dirty), (None, true));
    h.shutdown();
}

#[test]
fn remote_clients_never_name_daemon_paths() {
    use ac2_zmq::{AuthorizedKeys, Context, CurveClient, KeyPair};
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let laptop = KeyPair::generate().unwrap();
    let mut keys = AuthorizedKeys::new();
    keys.insert("laptop", laptop.public).unwrap();
    let authorized = dir.path().join("authorized_clients");
    keys.save(&authorized).unwrap();
    let listen = ac2d::Listen::Network {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
        security: ac2d::NetworkSecurity {
            server_key_file: dir.path().join("server.key"),
            authorized_clients_file: authorized,
        },
    };
    let mut cfg = config(manual_rig(), listen);
    cfg.session_dir = dir.path().join("sessions");
    let h = Daemon::start(cfg).unwrap();
    let ctx = Context::new().unwrap();
    let curve = CurveClient {
        keys: laptop,
        server_key: h.server_public_key().unwrap(),
    };
    let mut c = Client::connect_with(&ctx, h.ctrl_endpoint(), Some(&curve));
    let path = SessionRef::Path {
        path: dir.path().join("x").to_string_lossy().into_owned(),
    };
    let e = c
        .call(Command::FileSave {
            session: path.clone(),
            overwrite: true,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused);
    for cmd in [
        Command::FileLoad {
            session: path.clone(),
            discard: true,
        },
        Command::FileSalvage {
            session: path.clone(),
        },
        Command::FileDownloadBegin {
            session: Some(path),
        },
    ] {
        let e = c.call(cmd).unwrap_err();
        assert_eq!(e.code, ErrorCode::Refused, "{e:?}");
    }
    // Its own files travel as archives: the working state downloads.
    let t = match c.ok(Command::FileDownloadBegin { session: None }) {
        ReplyBody::Transfer(t) => t,
        other => panic!("{other:?}"),
    };
    assert!(t.bytes > 0);
    h.shutdown();
}
