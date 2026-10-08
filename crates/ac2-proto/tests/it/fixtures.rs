//! Cross-language fixtures (`fixtures/protocol`, `tools/protocol/fixtures.py`).
//!
//! - `rust_*.bin` must equal what this build encodes from `samples`; regenerate with
//!   `AC2_UPDATE_FIXTURES=1 cargo test -p ac2-proto --test it fixtures::`, then run
//!   `tools/protocol/fixtures.py check` so Python decodes them.
//! - `py_*.bin` are written by Python; they must decode here to exactly the samples.
//!
//! Container: u32 LE part count, then per part u32 LE length + bytes.

use std::path::PathBuf;

use ac2_proto::frame::FrameKind;
use ac2_proto::units::RequestId;
use ac2_proto::*;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/protocol")
}

fn container(parts: &[Vec<u8>]) -> Vec<u8> {
    let mut out = (parts.len() as u32).to_le_bytes().to_vec();
    for p in parts {
        out.extend_from_slice(&(p.len() as u32).to_le_bytes());
        out.extend_from_slice(p);
    }
    out
}

fn read_container(name: &str) -> Vec<Vec<u8>> {
    let path = dir().join(name);
    let b = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let word = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().expect("fixture")) as usize;
    let mut off = 4;
    let mut parts = Vec::new();
    for _ in 0..word(0) {
        let n = word(off);
        parts.push(b[off + 4..off + 4 + n].to_vec());
        off += 4 + n;
    }
    assert_eq!(off, b.len(), "{name}: trailing bytes");
    parts
}

fn kind_name(k: FrameKind) -> String {
    rmp_serde::from_slice::<String>(&rmp_serde::to_vec(&k).expect("fixture")).expect("fixture")
}

fn request(i: usize, cmd: Command) -> Request {
    let mut r = Request::new(RequestId(i as u64), cmd);
    if r.cmd.is_mutation() {
        r.expect_rev = Some(units::Rev(41));
    }
    r
}

#[derive(serde::Serialize)]
struct GridFixture {
    grid: GridDef,
    grid_id: GridId,
}

fn rust_fixtures() -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for f in samples::frames() {
        let name = format!("rust_frame_{}.bin", kind_name(f.data.kind()));
        out.push((name, container(&encode_frame(&f).expect("fixture"))));
    }
    let reqs: Vec<Vec<u8>> = samples::commands()
        .into_iter()
        .enumerate()
        .map(|(i, c)| encode_request(&request(i, c)).expect("fixture"))
        .collect();
    out.push(("rust_requests.bin".into(), container(&reqs)));
    let reps: Vec<Vec<u8>> = samples::replies()
        .into_iter()
        .enumerate()
        .map(|(i, r)| encode_reply(&Reply::new(RequestId(i as u64), r)).expect("fixture"))
        .collect();
    out.push(("rust_replies.bin".into(), container(&reps)));
    let evts: Vec<Vec<u8>> = samples::events()
        .iter()
        .map(|e| encode_event(e).expect("fixture"))
        .collect();
    out.push(("rust_events.bin".into(), container(&evts)));
    let grids: Vec<Vec<u8>> = samples::grids()
        .into_iter()
        .map(|g| {
            rmp_serde::to_vec_named(&GridFixture {
                grid_id: g.id(),
                grid: g,
            })
            .expect("fixture")
        })
        .collect();
    out.push(("rust_grids.bin".into(), container(&grids)));
    out
}

#[test]
fn rust_fixtures_are_current() {
    let update = std::env::var_os("AC2_UPDATE_FIXTURES").is_some();
    for (name, bytes) in rust_fixtures() {
        let path = dir().join(&name);
        if update {
            std::fs::write(&path, &bytes).expect("fixture");
        } else {
            let committed = std::fs::read(&path).unwrap_or_default();
            assert!(
                committed == bytes,
                "{name} is stale: AC2_UPDATE_FIXTURES=1 cargo test -p ac2-proto --test it fixtures::"
            );
        }
    }
}

#[test]
fn python_frames_decode_to_the_samples() {
    for f in samples::frames() {
        let name = format!("py_frame_{}.bin", kind_name(f.data.kind()));
        let parts = read_container(&name);
        let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
        let got = decode_frame(&refs).unwrap_or_else(|e| panic!("{name}: {e}"));
        // NaN payloads make PartialEq useless; this build's encoding is canonical.
        assert_eq!(
            encode_frame(&got).expect("fixture"),
            encode_frame(&f).expect("fixture"),
            "{name}"
        );
    }
}

#[test]
fn python_requests_decode_to_the_samples() {
    let cmds = samples::commands();
    let parts = read_container("py_requests.bin");
    assert!(parts.len() >= 6);
    for b in parts {
        let got = decode_request(&b).expect("fixture");
        let i = got.id.0 as usize;
        assert_eq!(got, request(i, cmds[i].clone()));
    }
}

#[test]
fn python_events_decode_to_the_samples() {
    let evts = samples::events();
    let parts = read_container("py_events.bin");
    assert!(parts.len() >= 5);
    for b in parts {
        let got = decode_event(&b).expect("fixture");
        let want = evts.iter().find(|e| e.rev == got.rev).expect("fixture");
        assert_eq!(&got, want);
    }
}

/// The wire lock ties the encoded fixtures to `PROTO_VERSION`: any change to what goes on the
/// wire must come with a version bump, so mismatched builds refuse each other at `hello`
/// instead of misreading each other's messages.
#[test]
fn wire_changes_bump_the_protocol_version() {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    let mut fixtures = rust_fixtures();
    fixtures.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, bytes) in &fixtures {
        h.update(name.as_bytes());
        h.update((bytes.len() as u64).to_le_bytes());
        h.update(bytes);
    }
    let digest: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    let current = format!("version {}\nsha256 {digest}\n", ac2_proto::PROTO_VERSION);
    let path = dir().join("WIRE_LOCK");
    if std::env::var_os("AC2_UPDATE_FIXTURES").is_some() {
        let committed = std::fs::read_to_string(&path).unwrap_or_default();
        let committed_version = committed
            .lines()
            .find_map(|l| l.strip_prefix("version "))
            .and_then(|v| v.parse::<u16>().ok());
        let committed_digest = committed.lines().find_map(|l| l.strip_prefix("sha256 "));
        assert!(
            committed_digest == Some(digest.as_str())
                || committed_version != Some(ac2_proto::PROTO_VERSION),
            "the wire format changed: bump PROTO_VERSION before updating the fixtures"
        );
        std::fs::write(&path, &current).expect("wire lock");
        return;
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        committed == current,
        "the wire format or PROTO_VERSION changed without updating {}:\n\
         bump PROTO_VERSION if the encoded messages changed (pre-1.0: no compatibility shims),\n\
         then AC2_UPDATE_FIXTURES=1 cargo test -p ac2-proto --test it fixtures::",
        path.display()
    );
}
