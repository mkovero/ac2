//! The built `ac2` binary run as a process: what only a real process has (the main thread's
//! stack, the exit code from `main`) is checked on every OS the tests run on.
#![allow(clippy::unwrap_used)]

use std::process::{Command, Output};

use crate::common::rig;

fn ac2(args: &[&str], keys: &std::path::Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ac2"))
        .arg("--key-dir")
        .arg(keys)
        .args(args)
        .output()
        .unwrap()
}

fn says(out: &Output) -> String {
    format!(
        "{:?}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn help_exits_zero() {
    let keys = tempfile::tempdir().unwrap();
    let out = ac2(&["--help"], keys.path());
    assert!(out.status.success(), "{}", says(&out));
    assert!(String::from_utf8_lossy(&out.stdout).contains("Usage"));
}

#[test]
fn a_command_against_a_daemon_exits_zero() {
    let (h, ep) = rig();
    let keys = tempfile::tempdir().unwrap();
    let out = ac2(
        &[
            "--json",
            "--ctrl-endpoint",
            &ep.ctrl,
            "--data-endpoint",
            &ep.data,
            "status",
        ],
        keys.path(),
    );
    h.shutdown();
    assert!(out.status.success(), "{}", says(&out));
    let status: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(status.is_object(), "{}", says(&out));
}
