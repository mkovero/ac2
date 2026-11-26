//! A large session exports with a reply deadline far shorter than its pack: the CLI waits
//! while the daemon is alive and packing, instead of giving up after three tries.
#![allow(clippy::unwrap_used)]

use std::io::Write;

use crate::common::*;

/// Incompressible bytes, so the pack takes as long as their size says.
fn noise(len: usize) -> Vec<u8> {
    let mut x = 0x9e37_79b9_7f4a_7c15_u64;
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.truncate(len);
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_large_session_exports_with_a_short_reply_deadline() {
    let (h, ep) = rig();
    let name = format!("big-{}", std::process::id());
    let (code, text) = run_plain(&ep, &["session", "save-as", &name]).await;
    assert_eq!(code, 0, "{text}");
    // A session's traces folder holding 48 MB the pack cannot compress: on any machine
    // that is far longer than the three 50 ms tries below.
    let traces = test_sessions().join(&name).join("traces");
    std::fs::create_dir_all(&traces).unwrap();
    let mut f = std::fs::File::create(traces.join("ballast.csv")).unwrap();
    f.write_all(&noise(48 << 20)).unwrap();
    drop(f);

    let out = tempfile::tempdir().unwrap();
    let file = out.path().join("big.ac2session");
    let file_arg = file.to_string_lossy().into_owned();
    let (code, text) = run_plain(
        &ep,
        &[
            "--timeout",
            "50ms",
            "session",
            "export",
            &file_arg,
            "--session",
            &name,
        ],
    )
    .await;
    assert_eq!(code, 0, "{text}");
    let bytes = std::fs::metadata(&file).unwrap().len();
    assert!(bytes > 48 << 20, "{bytes}");
    h.shutdown();
}
