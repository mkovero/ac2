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
use ac2_proto::{Command, ErrorCode, ErrorDetail};
use ac2d::{AutosaveConfig, Daemon, Handle};
use common::*;

pub(crate) fn start(name: &str, dir: &Path, autosave: bool) -> Handle {
    init_log();
    let mut cfg = config(manual_rig(), inproc(&unique(name)));
    cfg.session_dir = Some(dir.join("sessions"));
    cfg.autosave = autosave.then(|| AutosaveConfig {
        dir: dir.join("autosave"),
        restore: true,
    });
    Daemon::start(cfg).unwrap()
}

pub(crate) fn working(c: &mut Client) -> WorkingSession {
    c.state().working
}

pub(crate) fn traces(c: &mut Client) -> Vec<TraceMeta> {
    c.traces(Command::TraceList)
}

pub(crate) fn import(c: &mut Client, name: &str) -> TraceMeta {
    import_fixture(c, name, name, TraceOwner::Imported)
}

fn rename(c: &mut Client, t: &TraceMeta, name: &str) {
    let mut edit = t.edit.clone();
    edit.name = name.into();
    c.ok(Command::TraceUpdate { trace: t.id, edit });
}

pub(crate) fn named(name: &str) -> SessionRef {
    SessionRef::Name { name: name.into() }
}

pub(crate) fn save(
    c: &mut Client,
    name: &str,
    overwrite: bool,
) -> Result<SessionFile, ac2_proto::ProtoError> {
    c.call(Command::FileSave {
        session: named(name),
        overwrite,
    })
    .map(|r| r.session_file())
}

/// Uploads `bytes` in chunks of the daemon's size; returns the upload.
fn upload(c: &mut Client, file: &str, bytes: &[u8]) -> TransferId {
    let t = c.transfer(Command::FileUploadBegin {
        file: file.into(),
        bytes: bytes.len() as u64,
        sha256: ac2_archive::sha256_hex(bytes),
    });
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
    let t = c.transfer(Command::FileDownloadBegin { session });
    let mut data = Vec::new();
    while (data.len() as u64) < t.bytes {
        let b = c.chunk(Command::FileDownloadChunk {
            transfer: t.transfer,
            offset: data.len() as u64,
        });
        data.extend_from_slice(&b.0);
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
    // A measurement's own panes travel in the archive.
    let meter = c.measurement(Command::MeasCreate {
        config: MeasConfig {
            scope: ac2_proto::model::MeasScope::Session,
            name: "FOH SPL".into(),
            kind: MeasKind::Spl {
                config: SplConfig::on_input(0, Weighting::A, TimeWeighting::Fast),
            },
        },
    });
    let layout = ac2_proto::samples::pane_layout();
    c.ok(Command::MeasLayout {
        meas: meter.id,
        layout: Some(layout.clone()),
    });
    let before = traces(c);
    let (t, bytes) = download(c, None);
    assert_eq!(t.file_name, "untitled.ac2session");
    // Written by the client: the working state belongs to that file, clean.
    let stick = "/run/media/foh/STICK/friday.ac2session";
    c.ok(Command::FileDownloadEnd {
        transfer: t.transfer,
        saved_as: Some(stick.into()),
    });
    // Every client mirrors the origin: the file's name, never where it lies on that machine.
    let w = working(c);
    assert_eq!(
        w.origin,
        Some(SessionOrigin::Client {
            path: "friday.ac2session".into()
        })
    );
    assert!(!w.dirty);
    c.ok(Command::FileNew { discard: false });

    let up = upload(c, stick, &bytes);
    let loaded = c.session_file(Command::FileLoad {
        session: SessionRef::Upload { transfer: up },
        discard: false,
    });
    assert_eq!(loaded.name, "friday.ac2session");
    assert_eq!(loaded.path, "friday.ac2session");
    assert_eq!(loaded.traces, 2);
    let after = traces(c);
    assert_eq!(
        after.iter().map(|t| &t.edit).collect::<Vec<_>>(),
        before.iter().map(|t| &t.edit).collect::<Vec<_>>()
    );
    let s = c.snapshot(Command::StateSnapshot);
    let m = s.state.measurements.iter().find(|m| m.id == meter.id);
    assert_eq!(m.and_then(|m| m.layout.clone()), Some(layout.clone()));
    let w = working(c);
    assert_eq!(
        w.origin,
        Some(SessionOrigin::Client {
            path: "friday.ac2session".into()
        })
    );
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
    let l = c.sessions(Command::FileList);
    assert!(l.iter().any(|f| f.name == "from stick" && f.traces == 2));
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

    let begin = |c: &mut Client, bytes: &[u8], sha: String| {
        c.transfer(Command::FileUploadBegin {
            file: "x.ac2session".into(),
            bytes: bytes.len() as u64,
            sha256: sha,
        })
        .transfer
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
    let logs = salvage_logs(&sdir);
    c.ok(Command::FileNew { discard: true });
    let e = c
        .call(Command::FileLoad {
            session: named("old"),
            discard: false,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Unsupported);
    let s = c.salvage(Command::FileSalvage {
        session: named("old"),
    });
    assert_eq!(s.version, Some(19));
    assert_eq!(s.imported.len(), 2, "{s:?}");
    let refused: Vec<&str> = s.refused.iter().map(|r| r.file.as_str()).collect();
    assert_eq!(refused, ["traces/99-junk.csv", "spl/5-old.csv"], "{s:?}");
    assert!(s.refused[1].reason.contains("version 2"), "{s:?}");
    assert_eq!(traces(c).len(), 2);
    assert!(working(c).dirty, "salvaged traces are unsaved");

    // The current-version log is a stopped session meter on the logged input, holding it.
    assert_eq!(s.spl_logs.len(), 1, "{s:?}");
    let st = c.state();
    let m = st
        .measurements
        .iter()
        .find(|m| m.id == s.spl_logs[0])
        .unwrap();
    assert_eq!(m.config.name, "FOH (salvaged)");
    assert_eq!(m.config.scope, MeasScope::Session);
    assert!(!m.running);
    let MeasKind::Spl { config } = &m.config.kind else {
        panic!("{m:?}")
    };
    assert_eq!(config.input, 3);
    let hist = match c.ok(Command::SplHistoryGet {
        meas: m.id,
        span: HistorySpan::Newest { seconds: 60 },
    }) {
        ac2_proto::ReplyBody::SplHistory(h) => h,
        r => panic!("{r:?}"),
    };
    assert_eq!(
        (hist.first_second, hist.last_second),
        (Some(0), Some(logs - 1))
    );
    h.shutdown();
}

/// Writes a current-version SPL log with its band log, and one of version 2, into the
/// session at `sdir`; the current log's row count.
fn salvage_logs(sdir: &Path) -> u64 {
    use ac2_proto::spl_log::{SplLogInfo, export_csv};
    let t0 = 1_790_000_000_000_000_000u64;
    let info = SplLogInfo {
        meas: MeasId(5),
        name: "FOH".into(),
        input: 3,
        mic: None,
        zone: "Europe/Helsinki".into(),
        started: WallNs(t0),
    };
    let n = 30u64;
    let rows: Vec<SplLogRow> = (0..n)
        .map(|k| SplLogRow {
            second: k,
            run: 0,
            start: WallNs(t0 + k * 1_000_000_000),
            clock: LogClock::Synced,
            gap: (k == 0).then_some(LogGap::Known),
            measured: Seconds(1.0),
            laeq: Dbfs(-30.0),
            lceq: Dbfs(-28.0),
            lzeq: Dbfs(-27.0),
            lcpeak: Dbfs(-12.0),
            lafmax: Dbfs(-25.0),
            sensitivity: Some(Db(120.0)),
            position: None,
        })
        .collect();
    let spl = sdir.join("spl");
    std::fs::create_dir_all(&spl).unwrap();
    std::fs::write(spl.join("4-5.csv"), export_csv(&info, &rows)).unwrap();
    std::fs::write(
        spl.join("4-5.bands.csv"),
        ac2_traces::band_log::export_csv(&info, &[]),
    )
    .unwrap();
    let old = export_csv(&info, &rows).replacen("# ac2 spl log v3", "# ac2 spl log v2", 1);
    std::fs::write(spl.join("5-old.csv"), old).unwrap();
    n
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
    keys.insert("laptop", laptop.public, ac2_zmq::Role::Operator)
        .unwrap();
    let authorized = dir.path().join("authorized_clients");
    keys.save(&authorized).unwrap();
    let security = ac2d::NetworkSecurity {
        server_key_file: dir.path().join("server.key"),
        authorized_clients_file: authorized,
    };
    let mut cfg = net_config(manual_rig(), security);
    cfg.session_dir = Some(dir.path().join("sessions"));
    let h = Daemon::start(cfg).unwrap();
    let ctx = Context::new().unwrap();
    let curve = CurveClient {
        keys: laptop,
        server_key: h.server_public_key().unwrap(),
    };
    let mut c = Client::connect_with(&ctx, net_endpoints(&h).0.as_str(), Some(&curve));
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
    let t = c.transfer(Command::FileDownloadBegin { session: None });
    assert!(t.bytes > 0);
    h.shutdown();
}

/// Copies session `from` to `to` with `edit` applied to its manifest.
fn edited_copy(dir: &Path, from: &str, to: &str, edit: impl Fn(&mut serde_json::Value)) {
    let (src, dst) = (
        dir.join("sessions").join(from),
        dir.join("sessions").join(to),
    );
    std::fs::create_dir_all(dst.join("traces")).unwrap();
    for e in std::fs::read_dir(src.join("traces")).unwrap() {
        let e = e.unwrap();
        std::fs::copy(e.path(), dst.join("traces").join(e.file_name())).unwrap();
    }
    let text = std::fs::read_to_string(src.join("session.json")).unwrap();
    let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
    edit(&mut v);
    std::fs::write(
        dst.join("session.json"),
        serde_json::to_string_pretty(&v).unwrap(),
    )
    .unwrap();
}

/// A saved delay, nudge or trace edit the live commands would refuse refuses the whole
/// load before anything changes: the working state, its unsaved changes and the recovery
/// slot stay as they were.
#[test]
fn a_session_with_values_out_of_range_refuses_to_load() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("load-limits", dir.path(), true);
    let (mut c, _sub) = connect(&h, &[]);
    let c = &mut c;
    import(c, "rew_export.txt");
    c.ok(Command::MeasCreate {
        config: transfer("tf"),
    });
    save(c, "good", false).unwrap();
    let delay = |applied: f64, nudged: f64| {
        move |v: &mut serde_json::Value| {
            v["measurements"][0]["delay"] =
                serde_json::json!({ "applied": applied, "nudged": nudged, "tracking": false });
        }
    };
    edited_copy(dir.path(), "good", "far", delay(1e12, 0.0));
    edited_copy(dir.path(), "good", "far-back", delay(-10.5, 0.0));
    edited_copy(dir.path(), "good", "nudged", delay(1.0, 1e12));
    edited_copy(dir.path(), "good", "offset", |v| {
        v["traces"][0]["meta"]["edit"]["offset"] = serde_json::json!(1e6);
    });
    edited_copy(dir.path(), "good", "trace-nudge", |v| {
        v["traces"][0]["meta"]["edit"]["delay_nudge"] = serde_json::json!(-1e12);
    });
    // A trace's recorded epoch must leave the loaded session a newer one, and the sessions
    // after it newer ones still.
    let captured_in = |epoch: u32| {
        move |v: &mut serde_json::Value| {
            v["traces"][0]["meta"]["source"] = serde_json::json!({
                "type": "captured",
                "meas": 1,
                "meas_name": "tf",
                "epoch": epoch,
                "at_sample": 0,
            });
        }
    };
    edited_copy(dir.path(), "good", "epoch", captured_in(u32::MAX));
    edited_copy(dir.path(), "good", "epoch-edge", captured_in(u32::MAX / 2));
    edited_copy(
        dir.path(),
        "good",
        "epoch-last",
        captured_in(u32::MAX / 2 - 1),
    );
    // The good session still loads with an in-range delay.
    edited_copy(dir.path(), "good", "near", delay(-10.0, 0.5));

    // Unsaved changes, so a load that went ahead would keep a recovery copy.
    let t = import(c, "smaart_tf.txt");
    let before = c.state();
    assert!(before.working.dirty);
    assert_eq!(before.working.recover, None);
    for name in [
        "far",
        "far-back",
        "nudged",
        "offset",
        "trace-nudge",
        "epoch",
        "epoch-edge",
    ] {
        let e = c
            .call(Command::FileLoad {
                session: named(name),
                discard: true,
            })
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::Invalid, "{name}: {e:?}");
        let after = c.state();
        assert_eq!(after.measurements, before.measurements, "{name}");
        assert_eq!(after.traces, before.traces, "{name}");
        assert_eq!(after.working, before.working, "{name}");
    }
    assert!(traces(c).iter().any(|x| x.id == t.id));

    c.ok(Command::FileLoad {
        session: named("near"),
        discard: true,
    });
    let st = c.state();
    assert_eq!(st.measurements.len(), 1);
    let d = st.delay(st.measurements[0].id).expect("transfer delay");
    assert_eq!((d.applied, d.nudged), (Seconds(-10.0), Seconds(0.5)));

    c.ok(Command::FileLoad {
        session: named("epoch-last"),
        discard: true,
    });
    assert_eq!(c.state().stream.epoch.0, u32::MAX / 2);
    h.shutdown();
}

/// The name rule holds for names an operator gives, not for what a session file keeps: a
/// session whose measurement has a blank name loads, and the name is checked again only
/// when the measurement is renamed.
#[test]
fn a_stored_measurement_name_outside_the_rule_still_loads() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("stored-name", dir.path(), false);
    let (mut c, _sub) = connect(&h, &[]);
    let c = &mut c;
    c.ok(Command::MeasCreate {
        config: transfer("Unnamed-later"),
    });
    save(c, "kept", false).unwrap();
    let sdir = dir.path().join("sessions/kept");
    let m = std::fs::read_to_string(sdir.join("session.json")).unwrap();
    assert!(m.contains("\"Unnamed-later\""));
    std::fs::write(
        sdir.join("session.json"),
        m.replace("\"Unnamed-later\"", "\"  \""),
    )
    .unwrap();
    c.ok(Command::FileNew { discard: true });
    c.ok(Command::FileLoad {
        session: named("kept"),
        discard: false,
    });
    let s = c.state();
    let meas = s.measurements.iter().find(|m| m.config.name == "  ");
    let meas = meas.expect("the measurement loaded with its stored name");
    let mut config = meas.config.clone();
    let id = meas.id;
    assert_eq!(
        c.update(id, config.clone()).unwrap_err().code,
        ErrorCode::Invalid
    );
    config.name = "Main".into();
    c.update_ok(id, config);
    h.shutdown();
}

/// A loaded capture names the curve it was taken of; a measurement made after the load
/// never gets that id, so its captures never take the slot of that stranger's.
#[test]
fn a_new_measurement_never_takes_a_loaded_captures_source_id() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("source-ids", dir.path(), false);
    let (mut c, _sub) = connect(&h, &[]);
    import(&mut c, "rew_export.txt");
    c.ok(Command::MeasCreate {
        config: transfer("tf"),
    });
    save(&mut c, "good", false).unwrap();
    h.shutdown();
    edited_copy(dir.path(), "good", "stranger", |v| {
        v["traces"][0]["meta"]["source"] = serde_json::json!({
            "type": "captured",
            "meas": 9,
            "meas_name": "gone",
            "epoch": 1,
            "at_sample": 0,
        });
    });
    let h = start("source-ids-2", dir.path(), false);
    let (mut c, _sub) = connect(&h, &[]);
    c.ok(Command::FileLoad {
        session: named("stranger"),
        discard: true,
    });
    let m = c.measurement(Command::MeasCreate {
        config: transfer("new"),
    });
    assert!(m.id.0 > 9, "{:?}", m.id);
    h.shutdown();
}
