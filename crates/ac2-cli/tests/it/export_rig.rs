//! An export whose pack outlasts the reply deadline many times over: the CLI waits while the
//! daemon is alive and packing, says so on stderr, and another session command meanwhile is
//! told what the rig is doing. Slow storage is a named pipe in place of the session's
//! manifest: the daemon's read of it blocks until the test writes to it.
#![cfg(unix)]
#![allow(clippy::unwrap_used)]

use std::time::Duration;

use crate::common::*;

/// The pack: many times the reply deadline below, so the wait is what carries the export.
const PACK: Duration = Duration::from_secs(2);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_export_waits_while_the_daemon_packs() {
    let (h, ep) = rig();
    let name = format!("slow-{}", std::process::id());
    let dir = test_sessions().join(&name);
    std::fs::create_dir_all(&dir).unwrap();
    let pipe = dir.join("session.json");
    let ok = std::process::Command::new("mkfifo")
        .arg(&pipe)
        .status()
        .unwrap()
        .success();
    assert!(ok, "mkfifo");
    let writer = std::thread::spawn(move || {
        std::thread::sleep(PACK);
        // Opening for writing waits for the daemon's read; what it reads is no manifest,
        // so the export ends in the daemon's answer, not a timeout.
        std::fs::write(pipe, b"not a manifest").unwrap();
    });

    let out = tempfile::tempdir().unwrap();
    let file = out.path().join("slow.ac2session");
    let file_arg = file.to_string_lossy().into_owned();
    // Short against the pack, generous for hello and an ordinary reply.
    let args = [
        "--timeout",
        "400ms",
        "session",
        "export",
        &file_arg,
        "--session",
        &name,
    ];
    let export = ac2(&ep, false, &args);
    let other = format!("other-{name}");
    let meanwhile = async {
        tokio::time::sleep(PACK / 2).await;
        run_plain(&ep, &["session", "save-as", &other]).await
    };
    let (export, (code, text)) = tokio::join!(export, meanwhile);
    writer.join().unwrap();

    assert_ne!(code, 0, "{text}");
    assert!(
        text.contains(
            "the rig is packing a session to export for a client on the rig's own machine ("
        ) && text.contains(" s so far); ask for file.save again once it is done"),
        "{text}"
    );

    assert_ne!(export.code, 0, "{} {}", export.stdout, export.stderr);
    assert!(
        export
            .stderr
            .starts_with("the daemon is packing the session; waiting while it works (Ctrl-C"),
        "{}",
        export.stderr
    );
    assert!(
        !export.stderr.contains("did not answer"),
        "{}",
        export.stderr
    );
    assert!(!file.exists());
    h.shutdown();
    std::fs::remove_dir_all(&dir).unwrap();
}
