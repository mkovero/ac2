//! A client waits for a session download as long as the daemon packs it, however short its
//! reply deadline, and a client that asks again (it stopped waiting) cuts its own download
//! instead of being refused until the pack nobody fetches is done. Slow storage is a named
//! pipe in place of a session's manifest: reading it blocks until the test writes to it.
#![cfg(unix)]
#![allow(clippy::unwrap_used)]

use crate::common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ac2_client::sessions::{self, Progress};
use ac2_client::{ClientConfig, ClientError, Endpoints, Retry};
use ac2_proto::model::*;
use ac2_proto::{Command, ErrorCode, ReplyBody, Request};
use ac2d::{Daemon, Handle};
use common::*;

fn start(name: &str, dir: &Path) -> Handle {
    init_log();
    let mut cfg = config(manual_rig(), inproc(&unique(name)));
    cfg.session_dir = Some(dir.join("sessions"));
    Daemon::start(cfg).unwrap()
}

/// A session directory `name` whose manifest is a named pipe: reading it waits for a
/// writer.
fn slow_session(dir: &Path, name: &str) -> PathBuf {
    let d = dir.join("sessions").join(name);
    std::fs::create_dir_all(&d).unwrap();
    let pipe = d.join("session.json");
    let ok = std::process::Command::new("mkfifo")
        .arg(&pipe)
        .status()
        .unwrap()
        .success();
    assert!(ok, "mkfifo");
    pipe
}

/// Lets one read of `pipe` finish, with content that is no manifest.
fn release(pipe: PathBuf) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || std::fs::write(pipe, b"not a manifest").unwrap())
}

fn named(name: &str) -> SessionRef {
    SessionRef::Name { name: name.into() }
}

/// A download whose pack takes far longer than the client's three tries is waited for,
/// and the operator hears the daemon is still packing; its answer, when it comes, is the
/// daemon's (here: the pipe held no manifest), never a timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_waits_for_a_download_the_daemon_is_still_packing() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("wait-pack", dir.path());
    let pipe = slow_session(dir.path(), "slow");
    let mut cfg = ClientConfig::new(
        Endpoints {
            ctrl: h.ctrl_endpoint().to_owned(),
            data: h.data_endpoint().to_owned(),
        },
        "wait-pack",
    );
    cfg.context = Some(h.context().clone());
    cfg.retry = Retry {
        timeout: Duration::from_millis(100),
        retries: 2,
    };
    let c = ac2_client::Client::connect(cfg).await.unwrap();
    let packing = Arc::new(Mutex::new(Vec::new()));
    let seen = packing.clone();
    let job = tokio::spawn(async move {
        sessions::download(&c, Some(named("slow")), move |p| {
            if let Progress::Packing(w) = p {
                seen.lock().unwrap().push(w);
            }
        })
        .await
    });
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(!job.is_finished(), "gave up while the daemon packed");
    let writer = release(pipe);
    let r = tokio::time::timeout(Duration::from_secs(10), job)
        .await
        .expect("answered once the pack ends")
        .unwrap();
    writer.join().unwrap();
    match r {
        Err(ClientError::Daemon(e)) => assert_eq!(e.code, ErrorCode::Invalid, "{e:?}"),
        other => panic!("{other:?}"),
    }
    let packing = packing.lock().unwrap();
    assert!(packing.len() >= 5, "{packing:?}");
    h.shutdown();
}

/// A new request from the client whose download is being packed means it stopped waiting
/// for it (an interrupted export asked again): the old download is cut and the new one
/// begins. Another client is still refused while a download runs.
#[test]
fn a_client_asking_again_cuts_its_own_download() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("wait-cut", dir.path());
    let (mut a, _sub) = connect(&h, &[]);
    let (mut b, _sub_b) = connect(&h, &[]);
    import_fixture(
        &mut a,
        "rew_export.txt",
        "rew_export.txt",
        TraceOwner::Imported,
    );
    a.ok(Command::FileSave {
        session: named("ready"),
        overwrite: false,
    });
    let pipe = slow_session(dir.path(), "slow");
    let first = a.next_id();
    a.send(&Request::new(
        first,
        Command::FileDownloadBegin {
            session: Some(named("slow")),
        },
    ));
    assert!(a.recv(Duration::from_millis(300)).is_none());
    let e = b
        .call(Command::FileDownloadBegin {
            session: Some(named("ready")),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");

    let second = a.next_id();
    a.send(&Request::new(
        second,
        Command::FileDownloadBegin {
            session: Some(named("ready")),
        },
    ));
    let cut = a
        .recv(Duration::from_secs(10))
        .expect("the first is answered");
    assert_eq!(cut.id, first);
    let e = cut.result.unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");
    let begun = a
        .recv(Duration::from_secs(10))
        .expect("the second is answered");
    assert_eq!(begun.id, second);
    match begun.result {
        Ok(ReplyBody::Transfer(t)) => {
            assert!(t.bytes > 0);
            a.ok(Command::FileDownloadEnd {
                transfer: t.transfer,
                saved_as: None,
            });
        }
        other => panic!("{other:?}"),
    }
    // The cut worker, still at the pipe, stops there without packing anything.
    release(pipe).join().unwrap();
    h.shutdown();
}
