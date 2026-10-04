//! Autosave on the fake rig: a captured trace and its measurement are written shortly after
//! they change (one write per burst of edits), the status reaches clients as the `autosave`
//! entity, a restarted daemon comes back with them disarmed and without an audio session,
//! the previous autosave is kept as a backup, an unwritable directory shows as a failure,
//! another format version and `--no-restore` move the autosave aside.
#![allow(clippy::unwrap_used)]

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Change, Command, ReplyBody};
use ac2d::{AutosaveConfig, Daemon, Handle};
use common::*;

const T: Duration = Duration::from_secs(10);

fn start(name: &str, dir: &Path, restore: bool) -> (Handle, ac2_audio::FakeBackend) {
    init_log();
    let backend = manual_rig();
    let mut cfg = config(backend.clone(), inproc(&unique(name)));
    cfg.lease_expiry = Duration::from_secs(60);
    cfg.session_dir = dir.join("sessions");
    cfg.autosave = Some(AutosaveConfig {
        dir: dir.join("autosave"),
        restore,
    });
    (Daemon::start(cfg).unwrap(), backend)
}

fn state(c: &mut Client) -> State {
    match c.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => s.state,
        other => panic!("{other:?}"),
    }
}

fn trace_data(c: &mut Client, id: TraceId) -> TraceData {
    match c.ok(Command::TraceGet { trace: id }) {
        ReplyBody::TraceData(d) => *d,
        other => panic!("{other:?}"),
    }
}

/// The next `autosave` event matching `pred`.
fn autosave_event(sub: &Sub, pred: impl Fn(&Autosave) -> bool) -> Autosave {
    let e = sub
        .event(T, |e| matches!(&e.change, Change::Autosave(a) if pred(a)))
        .expect("autosave event");
    match e.change {
        Change::Autosave(a) => a,
        _ => unreachable!(),
    }
}

/// Session open, a running transfer measurement with a result, a capture into slot 1.
fn capture(h: &Handle, backend: &ac2_audio::FakeBackend, c: &mut Client) -> TraceMeta {
    c.ok(Command::SessionOpen {
        config: session(true),
    });
    c.ok(Command::MeasCreate {
        config: transfer("main"),
    });
    c.ok(Command::MeasStart { meas: MeasId(1) });
    let tok = match c.ok(Command::GenAcquire { force: false }) {
        ReplyBody::Lease(l) => l.lease_token,
        other => panic!("{other:?}"),
    };
    c.ok(Command::GenSet {
        lease_token: tok,
        desired: GeneratorDesired {
            settings: GeneratorSettings {
                signal: Signal::Pink,
                level: Dbfs(-20.0),
                band: None,
                outputs: vec![0],
            },
            armed: true,
            firing: true,
        },
    });
    let tf = Sub::connect(h.context(), h.data_endpoint(), &[b"d/1/tf"]);
    let mut d = driver(backend);
    for _ in 0..4 {
        run(&mut d, 0.5);
        c.ok(Command::GenRefresh { lease_token: tok });
    }
    tf.frame(T, |f| matches!(f.data, ac2_proto::FrameData::Tf(_)))
        .expect("tf frame");
    match c.ok(Command::TraceCapture {
        meas: MeasId(1),
        name: "kept".into(),
        slot: Some(1),
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

/// From an empty daemon: a trace is captured, the status goes pending → saved, the daemon
/// restarts, and the trace (data and slot) and the measurement are back, disarmed, without
/// an audio session. The next change keeps the first autosave as the backup.
#[test]
fn traces_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (h, backend) = start("autosave-restart", dir.path(), true);
    let (mut c, sub) = connect(&h, &[b"evt"]);
    let first = state(&mut c).autosave;
    assert_eq!(
        first,
        Autosave {
            state: AutosaveState::Saved,
            saved_at: None
        }
    );

    let kept = capture(&h, &backend, &mut c);
    let kept_data = trace_data(&mut c, kept.id);
    autosave_event(&sub, |a| a.state == AutosaveState::Pending);
    let saved = autosave_event(&sub, |a| a.state == AutosaveState::Saved);
    let saved_at = saved.saved_at.expect("saved_at");
    assert_eq!(state(&mut c).autosave, saved);
    assert!(dir.path().join("autosave/session.json").exists());
    assert!(!dir.path().join("autosave/session.prev.json").exists());
    drop((c, sub));
    h.shutdown();

    let (h, _backend) = start("autosave-restart-2", dir.path(), true);
    let (mut c, sub) = connect(&h, &[b"evt"]);
    let st = state(&mut c);
    assert_eq!(st.traces, vec![kept.clone()]);
    assert_eq!(st.traces[0].edit.slot, Some(1));
    let back = trace_data(&mut c, kept.id);
    let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&back.mag_db), bits(&kept_data.mag_db));
    assert_eq!(st.measurements.len(), 1);
    assert_eq!(st.measurements[0].config, transfer("main"));
    assert!(st.session.open.is_none(), "no audio session is opened");
    assert!(!st.generator.armed && !st.generator.firing);
    assert_eq!(st.generator.owner, None);
    assert_eq!(
        st.autosave,
        Autosave {
            state: AutosaveState::Saved,
            saved_at: Some(saved_at)
        }
    );

    // A change after the restore: written again, the restored autosave becomes the backup.
    rename(&mut c, &kept, "renamed");
    // (The restore's own status may still be queued on a subscriber that connected first.)
    let again = autosave_event(&sub, |a| {
        a.state == AutosaveState::Saved && a.saved_at != Some(saved_at)
    });
    assert!(again.saved_at.expect("saved_at") > saved_at);
    let prev = ac2_traces::session::read_manifest_named(
        &dir.path().join("autosave"),
        ac2_traces::session::PREV_MANIFEST,
    )
    .unwrap();
    assert_eq!(prev.saved_at, saved_at);
    assert_eq!(prev.traces[0].meta.edit.name, "kept");
    let cur = ac2_traces::session::read_manifest(&dir.path().join("autosave")).unwrap();
    assert_eq!(cur.traces[0].meta.edit.name, "renamed");
    drop((c, sub));
    h.shutdown();
}

/// A burst of edits is one write, after the edits stop.
#[test]
fn a_burst_of_edits_is_one_write() {
    let dir = tempfile::tempdir().unwrap();
    let (h, _backend) = start("autosave-burst", dir.path(), true);
    let (mut c, sub) = connect(&h, &[b"evt"]);
    let t = match c.ok(Command::TraceImport {
        file_name: "target.txt".into(),
        format: ImportFormat::Auto,
        role: ImportRole::Target,
        content: Blob(b"20 -3.0\n1000 0.0\n20000 -6.0\n".to_vec()),
    }) {
        ReplyBody::Trace(t) => t,
        other => panic!("{other:?}"),
    };
    autosave_event(&sub, |a| a.state == AutosaveState::Saved);

    let mut last = Instant::now();
    for i in 0..5 {
        rename(&mut c, &t, &format!("edit {i}"));
        last = Instant::now();
        std::thread::sleep(Duration::from_millis(100));
    }
    let mut events = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(4);
    while let Some(e) = sub.event(deadline.saturating_duration_since(Instant::now()), |e| {
        matches!(e.change, Change::Autosave(_))
    }) {
        if let Change::Autosave(a) = e.change {
            if a.state == AutosaveState::Saved {
                assert!(
                    last.elapsed() >= Duration::from_millis(1400),
                    "written {:?} after the last edit",
                    last.elapsed()
                );
            }
            events.push(a.state);
        }
    }
    assert_eq!(events, [AutosaveState::Pending, AutosaveState::Saved]);
    let cur = ac2_traces::session::read_manifest(&dir.path().join("autosave")).unwrap();
    assert_eq!(cur.traces[0].meta.edit.name, "edit 4");
    drop((c, sub));
    h.shutdown();
}

/// A change made just before shutdown is written on the way out.
#[test]
fn shutdown_writes_what_is_pending() {
    let dir = tempfile::tempdir().unwrap();
    let (h, _backend) = start("autosave-flush", dir.path(), true);
    let (mut c, _sub) = connect(&h, &[b"evt"]);
    c.ok(Command::MeasCreate {
        config: spl("level", 1),
    });
    drop(c);
    h.shutdown();
    let m = ac2_traces::session::read_manifest(&dir.path().join("autosave")).unwrap();
    assert_eq!(m.measurements.len(), 1);
    assert_eq!(m.measurements[0].config, spl("level", 1));
}

/// A directory that cannot be written shows as a failure with the file system's reason.
#[test]
fn an_unwritable_directory_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    // A file where the autosave's parent directory should be: no user can create it.
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"").unwrap();
    let (h, _backend) = start("autosave-fail", &blocker, true);
    let (mut c, sub) = connect(&h, &[b"evt"]);
    c.ok(Command::MeasCreate {
        config: spl("level", 1),
    });
    let a = autosave_event(&sub, |a| matches!(a.state, AutosaveState::Failed { .. }));
    let AutosaveState::Failed { reason } = &a.state else {
        unreachable!()
    };
    assert!(reason.contains("blocker"), "{reason}");
    assert_eq!(a.saved_at, None);
    assert_eq!(state(&mut c).autosave, a);
    drop((c, sub));
    h.shutdown();
}

fn write_autosave(dir: &Path, version: Option<u32>) -> PathBuf {
    let (h, _backend) = start("autosave-seed", dir, true);
    let (mut c, _sub) = connect(&h, &[b"evt"]);
    c.ok(Command::MeasCreate {
        config: spl("level", 1),
    });
    drop(c);
    h.shutdown();
    let p = dir.join("autosave");
    if let Some(v) = version {
        let m = p.join("session.json");
        let text = std::fs::read_to_string(&m).unwrap();
        let other = text.replace(
            &format!("\"version\": {}", ac2_traces::session::VERSION),
            &format!("\"version\": {v}"),
        );
        assert_ne!(text, other);
        std::fs::write(&m, other).unwrap();
    }
    p
}

/// An autosave of another session format version is set aside, not deleted; the daemon
/// starts empty.
#[test]
fn another_version_is_set_aside() {
    let dir = tempfile::tempdir().unwrap();
    write_autosave(dir.path(), Some(999));
    let (h, _backend) = start("autosave-version", dir.path(), true);
    let (mut c, _sub) = connect(&h, &[b"evt"]);
    let st = state(&mut c);
    assert!(st.measurements.is_empty());
    assert!(!dir.path().join("autosave").exists());
    assert!(dir.path().join("autosave.v999/session.json").exists());
    drop(c);
    h.shutdown();
}

/// `--no-restore`: an empty start, the autosave kept aside.
#[test]
fn no_restore_starts_empty() {
    let dir = tempfile::tempdir().unwrap();
    write_autosave(dir.path(), None);
    let (h, _backend) = start("autosave-norestore", dir.path(), false);
    let (mut c, _sub) = connect(&h, &[b"evt"]);
    assert!(state(&mut c).measurements.is_empty());
    let aside = ac2_traces::session::read_manifest(&dir.path().join("autosave.unrestored"));
    assert_eq!(aside.unwrap().measurements.len(), 1);
    drop(c);
    h.shutdown();
}

fn log_page(c: &mut Client, meas: MeasId) -> SplLogPage {
    match c.ok(Command::SplLogGet {
        meas,
        log: SplLogWhich::Current,
        from: 0,
        max: 1000,
    }) {
        ReplyBody::SplLogPage(p) => p,
        other => panic!("{other:?}"),
    }
}

/// The SPL log file the autosave's manifest names, and its rows.
fn spl_file(dir: &Path) -> (String, usize) {
    let (s, on_disk) =
        ac2_traces::session::load_named(&dir.join("autosave"), ac2_traces::session::MANIFEST)
            .unwrap();
    (on_disk[0].file.clone(), s.spl_logs[0].rows.len())
}

/// An SPL meter's per-second log is in the autosave: after a restart the meter, its Leq
/// windows and every logged second are back. The log growing is not an autosave write: the
/// rows are appended to one file, which carries on after the restart.
#[test]
fn spl_log_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (h, backend) = start("autosave-spl", dir.path(), true);
    let (mut c, sub) = connect(&h, &[b"evt"]);
    c.ok(Command::SessionOpen {
        config: session(false),
    });
    c.ok(Command::MeasCreate {
        config: spl("meter", 1),
    });
    c.ok(Command::MeasStart { meas: MeasId(1) });
    autosave_event(&sub, |a| a.state == AutosaveState::Saved);
    let mut d = driver(&backend);
    run(&mut d, 3.5);
    let deadline = Instant::now() + T;
    let logged = loop {
        let p = log_page(&mut c, MeasId(1));
        if p.total >= 3 {
            break p;
        }
        assert!(Instant::now() < deadline, "seconds never logged");
        run(&mut d, 0.1);
    };
    assert!(logged.rows.iter().all(|r| r.measured == Seconds(1.0)));
    // Logging seconds is no change to write.
    assert!(
        sub.event(Duration::from_secs(2), |e| matches!(
            e.change,
            Change::Autosave(_)
        ))
        .is_none(),
        "an autosave while only the log grew"
    );
    drop((c, sub));
    h.shutdown();
    let (file, rows) = spl_file(dir.path());
    assert!(
        rows >= logged.rows.len(),
        "the last rows are written at shutdown"
    );

    let (h, _backend) = start("autosave-spl-2", dir.path(), true);
    let (mut c, _sub) = connect(&h, &[b"evt"]);
    let st = state(&mut c);
    assert_eq!(st.measurements.len(), 1);
    assert_eq!(st.spl_logs.len(), 1);
    assert_eq!(st.spl_logs[0].windows.len(), 5);
    let back = log_page(&mut c, MeasId(1));
    assert!(back.total >= logged.total);
    for (a, b) in back.rows.iter().zip(&logged.rows) {
        assert_eq!(
            (a.start, a.measured, a.sensitivity),
            (b.start, b.measured, b.sensitivity)
        );
        assert!((a.laeq.0 - b.laeq.0).abs() < 1e-3 || a.laeq.0 == b.laeq.0);
    }
    drop(c);
    h.shutdown();

    // Running again after the restart: the run clock starts where the log does, not at
    // the restart, and the time the daemon was down is a gap.
    let (h, backend) = start("autosave-spl-3", dir.path(), true);
    let (mut c, sub) = connect(&h, &[b"evt", b"d/1/leq"]);
    c.ok(Command::SessionOpen {
        config: session(false),
    });
    c.ok(Command::MeasStart { meas: MeasId(1) });
    let mut d = driver(&backend);
    let deadline = Instant::now() + T;
    let run_after = loop {
        run(&mut d, 1.2);
        let f = sub.frame(
            Duration::from_millis(200),
            |f| matches!(&f.data, ac2_proto::FrameData::Leq(l) if l.meta.logged > back.total),
        );
        if let Some(f) = f
            && let ac2_proto::FrameData::Leq(l) = f.data
        {
            break l.meta.run.expect("a run");
        }
        assert!(Instant::now() < deadline, "no leq frame after the restart");
    };
    assert_eq!(run_after.started_at, logged.rows[0].start);
    assert!(run_after.until.0 > logged.rows[logged.rows.len() - 1].start.0);
    assert!(run_after.gaps.0 >= 0.0);
    assert!(!run_after.trimmed);
    let total = log_page(&mut c, MeasId(1)).total;
    drop((c, sub));
    h.shutdown();
    // The same file, appended to across the restarts, holds every row.
    let (again, rows) = spl_file(dir.path());
    assert_eq!(again, file);
    assert_eq!(rows as u64, total);
}
