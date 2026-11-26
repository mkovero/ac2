//! Deleting and renaming what the daemon keeps: `file.delete` and `file.rename` take a
//! saved session's name and nothing else, refuse names that are not one plain component
//! (on any OS), links, and the session the working state belongs to; a rename never
//! replaces and the working state follows it. `rec.delete` refuses the recording being
//! written or replayed, and a delete a stopped daemon left half done is finished at start.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::path::Path;
use std::time::Duration;

use ac2_proto::model::*;
use ac2_proto::units::Seconds;
use ac2_proto::{Change, Command, ErrorCode, ProtoError, ReplyBody};
use ac2_traces::raw;
use ac2d::{Daemon, Handle};
use common::*;

const T: Duration = Duration::from_secs(20);

fn start(name: &str, dir: &Path) -> Handle {
    init_log();
    let mut cfg = config(manual_rig(), inproc(&unique(name)));
    cfg.session_dir = Some(dir.join("sessions"));
    Daemon::start(cfg).unwrap()
}

fn save(c: &mut Client, name: &str) {
    c.ok(Command::FileSave {
        session: SessionRef::Name { name: name.into() },
        overwrite: false,
    });
}

fn names(c: &mut Client) -> Vec<String> {
    let ReplyBody::Sessions(l) = c.ok(Command::FileList) else {
        panic!("a session list")
    };
    let mut n: Vec<_> = l.into_iter().map(|s| s.name).collect();
    n.sort();
    n
}

fn delete(c: &mut Client, name: &str) -> Result<ReplyBody, ProtoError> {
    c.call(Command::FileDelete { name: name.into() })
}

fn rename(c: &mut Client, name: &str, to: &str) -> Result<ReplyBody, ProtoError> {
    c.call(Command::FileRename {
        name: name.into(),
        to: to.into(),
    })
}

#[track_caller]
fn code(r: Result<ReplyBody, ProtoError>, want: ErrorCode) -> ProtoError {
    let e = r.expect_err("refused");
    assert_eq!(e.code, want, "{e:?}");
    e
}

/// What the staging directory holds: nothing once a delete is done.
fn staged(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir.join("sessions/.incoming"))
        .map(|rd| {
            rd.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn sessions_are_deleted_and_renamed_by_name_and_never_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("file-ops", dir.path());
    let (mut c, _sub) = connect(&h, &[]);
    let c = &mut c;
    import_fixture(c, "rew_export.txt", "rew_export.txt", TraceOwner::Imported);
    save(c, "thursday");
    save(c, "friday");
    save(c, "open one");
    assert_eq!(names(c), ["friday", "open one", "thursday"]);

    // Renamed: listed under its new name, readable there.
    let r = rename(c, "thursday", "thursday soundcheck").unwrap();
    assert!(matches!(r, ReplyBody::SessionFile(f) if f.name == "thursday soundcheck"));
    assert_eq!(names(c), ["friday", "open one", "thursday soundcheck"]);
    // Never onto another session, nor onto anything else under that name.
    let e = code(
        rename(c, "thursday soundcheck", "friday"),
        ErrorCode::Conflict,
    );
    assert!(e.msg.contains("friday"), "{e:?}");
    std::fs::write(dir.path().join("sessions/notes"), "x").unwrap();
    code(rename(c, "friday", "notes"), ErrorCode::Conflict);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("sessions/notes")).unwrap(),
        "x"
    );
    code(rename(c, "nothing", "something"), ErrorCode::NotFound);
    code(rename(c, "friday", "friday"), ErrorCode::Invalid);
    // A name is one plain component, the same on every OS.
    for bad in [
        "..",
        "../friday",
        "a/b",
        "a\\b",
        ".hidden",
        "show.",
        " x",
        "CON",
        "nul.txt",
        "com1",
        "/tmp/x",
        "C:x",
    ] {
        code(rename(c, "friday", bad), ErrorCode::Invalid);
        code(delete(c, bad), ErrorCode::Invalid);
    }
    // A folder that is not a session is not touched.
    std::fs::create_dir(dir.path().join("sessions/photos")).unwrap();
    code(delete(c, "photos"), ErrorCode::Invalid);
    code(delete(c, "notes"), ErrorCode::Invalid);
    assert!(dir.path().join("sessions/photos").is_dir());

    // The open session is not deleted; renamed, the working state follows it.
    assert_eq!(
        c.state().working.origin,
        Some(SessionOrigin::Name {
            name: "open one".into()
        })
    );
    let e = code(delete(c, "open one"), ErrorCode::Refused);
    assert!(e.msg.contains("open session"), "{e:?}");
    rename(c, "open one", "saturday").unwrap();
    let w = c.state().working;
    assert_eq!(
        w.origin,
        Some(SessionOrigin::Name {
            name: "saturday".into()
        })
    );
    assert!(!w.dirty, "a rename changes no content");
    save(c, "saturday");

    // Deleted: gone from the list and the disk, nothing left behind in staging.
    assert!(matches!(delete(c, "friday"), Ok(ReplyBody::Ack { .. })));
    assert_eq!(names(c), ["saturday", "thursday soundcheck"]);
    assert!(!dir.path().join("sessions/friday").exists());
    assert_eq!(staged(dir.path()), Vec::<String>::new());
    code(delete(c, "friday"), ErrorCode::NotFound);
    h.shutdown();
}

/// The open session is found by its folder, not by how its path was spelt: opened by a
/// path through a link to the session directory (as a name differing only in case reaches
/// it on a file system that ignores case), it is still refused for `file.delete`.
#[cfg(unix)]
#[test]
fn the_open_session_reached_another_way_is_not_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("file-ops-alias", dir.path());
    let (mut c, _s) = connect(&h, &[]);
    save(&mut c, "show");
    let alias = dir.path().join("alias");
    std::os::unix::fs::symlink(dir.path().join("sessions"), &alias).unwrap();
    c.ok(Command::FileLoad {
        session: SessionRef::Path {
            path: alias.join("show").to_string_lossy().into_owned(),
        },
        discard: true,
    });
    let e = code(delete(&mut c, "show"), ErrorCode::Refused);
    assert!(e.msg.contains("open session"), "{e:?}");
    assert_eq!(names(&mut c), ["show"]);
    h.shutdown();
}

/// A link in the session directory is never followed: what it points to stays. Unix only:
/// a directory symlink on Windows needs a privilege a test run does not have.
#[cfg(unix)]
#[test]
fn a_link_in_the_session_directory_is_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("file-ops-link", dir.path());
    let (mut c, _sub) = connect(&h, &[]);
    let c = &mut c;
    save(c, "real");
    c.ok(Command::FileNew { discard: true });
    let outside = tempfile::tempdir().unwrap();
    let elsewhere = outside.path().join("elsewhere");
    std::fs::rename(dir.path().join("sessions/real"), &elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, dir.path().join("sessions/linked")).unwrap();
    code(delete(c, "linked"), ErrorCode::Invalid);
    code(rename(c, "linked", "moved"), ErrorCode::Invalid);
    assert!(elsewhere.join("session.json").is_file());
    h.shutdown();
}

/// One file operation at a time: a delete or rename waits for no load, it is refused.
#[cfg(unix)]
#[test]
fn a_delete_or_rename_is_refused_while_another_file_operation_runs() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("file-ops-busy", dir.path());
    let (mut c, _sub) = connect(&h, &[]);
    save(&mut c, "a");
    c.ok(Command::FileNew { discard: true });
    // A session whose manifest is a pipe: its load waits until the pipe is written.
    let pipe = dir.path().join("sessions/slow/session.json");
    std::fs::create_dir_all(pipe.parent().unwrap()).unwrap();
    let ok = std::process::Command::new("mkfifo")
        .arg(&pipe)
        .status()
        .unwrap()
        .success();
    assert!(ok, "mkfifo");
    let load = c.next_id();
    c.send(&ac2_proto::Request::new(
        load,
        Command::FileLoad {
            session: SessionRef::Name {
                name: "slow".into(),
            },
            discard: true,
        },
    ));
    // Answered first, so the load runs as a job.
    let probe = c.next_id();
    c.send(&ac2_proto::Request::new(probe, Command::ServerInfo));
    assert_eq!(c.recv(T).expect("the probe's reply").id, probe);
    let e = code(delete(&mut c, "a"), ErrorCode::Refused);
    assert!(e.msg.contains("under way"), "{e:?}");
    code(rename(&mut c, "a", "b"), ErrorCode::Refused);
    std::fs::write(&pipe, b"{}").unwrap();
    assert_eq!(c.recv(T).expect("the load answers").id, load);
    assert!(dir.path().join("sessions/a").is_dir());
    delete(&mut c, "a").unwrap();
    h.shutdown();
}

fn record(name: &str) -> Command {
    Command::RecStart {
        request: RecordRequest {
            inputs: vec![0],
            name: Some(name.into()),
            max_duration: Seconds(0.5),
            max_bytes: None,
        },
    }
}

fn by_name(name: &str) -> RecordingRef {
    RecordingRef::Name { name: name.into() }
}

#[test]
fn a_recording_is_deleted_unless_written_or_replayed() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let rec_dir = dir.path().join("recordings");
    let backend = manual_rig();
    let mut cfg = config(backend.clone(), inproc(&unique("rec-delete")));
    cfg.session_dir = Some(dir.path().join("sessions"));
    cfg.recording_dir = Some(rec_dir.clone());
    let h = Daemon::start(cfg).unwrap();
    let (mut c, sub) = connect(&h, &[b"evt"]);
    c.ok(Command::StreamOpen {
        config: stream(false),
    });
    let mut d = driver(&backend);

    // Being written: refused, the file kept.
    c.ok(record("take 1"));
    let e = code(
        c.call(Command::RecDelete {
            recording: by_name("take 1"),
        }),
        ErrorCode::Refused,
    );
    assert!(e.msg.contains("being recorded"), "{e:?}");
    run(&mut d, 1.0);
    sub.event(
        T,
        |e| matches!(&e.change, Change::Recording(r) if !r.active()),
    )
    .expect("the recording ends");
    c.ok(record("take 2"));
    run(&mut d, 1.0);
    sub.event(
        T,
        |e| matches!(&e.change, Change::Recording(r) if !r.active() && r.name == "take 2"),
    )
    .expect("the second recording ends");

    // Being replayed: refused.
    c.ok(Command::StreamClose);
    c.ok(Command::StreamReplay {
        recording: by_name("take 1"),
        pace: ReplayPace::Fast,
    });
    let e = code(
        c.call(Command::RecDelete {
            recording: by_name("take 1"),
        }),
        ErrorCode::Refused,
    );
    assert!(e.msg.contains("replayed"), "{e:?}");
    // Another one is not in use: deleted, both files and the list entry.
    c.ok(Command::RecDelete {
        recording: by_name("take 2"),
    });
    assert!(!raw::audio_path(&rec_dir, "take 2").exists());
    assert!(!raw::sidecar_path(&rec_dir, "take 2").exists());
    let ReplyBody::Recordings(l) = c.ok(Command::RecList) else {
        panic!("a recording list")
    };
    assert_eq!(
        l.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        ["take 1"]
    );
    code(
        c.call(Command::RecDelete {
            recording: by_name("take 2"),
        }),
        ErrorCode::NotFound,
    );
    code(
        c.call(Command::RecDelete {
            recording: by_name("../take 1"),
        }),
        ErrorCode::Invalid,
    );
    c.ok(Command::StreamClose);
    c.ok(Command::RecDelete {
        recording: by_name("take 1"),
    });
    assert_eq!(std::fs::read_dir(&rec_dir).unwrap().count(), 0);
    h.shutdown();
}

/// A recording's delete cut short after its first step (the sidecar set aside) is
/// finished by the next start: never listed half deleted.
#[test]
fn a_half_done_recording_delete_is_finished_at_start() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let rec_dir = dir.path().join("recordings");
    std::fs::create_dir_all(&rec_dir).unwrap();
    let wav = raw::audio_path(&rec_dir, "cut");
    std::fs::write(&wav, raw::wav::to_bytes(1, 48_000, &[0.0; 16])).unwrap();
    let marker = rec_dir.join(".cut.ac2rec.json.deleting");
    std::fs::write(&marker, b"{}").unwrap();
    let mut cfg = config(manual_rig(), inproc(&unique("rec-cut")));
    cfg.session_dir = Some(dir.path().join("sessions"));
    cfg.recording_dir = Some(rec_dir.clone());
    let h = Daemon::start(cfg).unwrap();
    let (mut c, _sub) = connect(&h, &[]);
    let ReplyBody::Recordings(l) = c.ok(Command::RecList) else {
        panic!("a recording list")
    };
    assert!(l.is_empty(), "{l:?}");
    assert!(!wav.exists() && !marker.exists());
    h.shutdown();
}
