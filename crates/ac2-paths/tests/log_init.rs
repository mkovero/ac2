//! The global install as the programs do it: its own test binary, because a process gets
//! one global subscriber.
#![cfg(feature = "log")]

use std::fs;

#[test]
fn init_writes_tracing_and_log_records_to_the_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = ac2_paths::log::init_in(Some(dir.path()), "ac2-ui").expect("log file");
    assert_eq!(path, dir.path().join("ac2-ui.log"));
    tracing::info!(target: "ac2_ui", "app line");
    // The audio backends log through the `log` crate.
    log::warn!(target: "ac2_audio::jack_host", "backend line");
    let s = fs::read_to_string(&path).expect("log");
    assert!(s.contains("app line"), "{s}");
    assert!(s.contains("backend line"), "{s}");
    // A second install fails without touching the first.
    assert_eq!(ac2_paths::log::init_in(None, "again"), None);
}
