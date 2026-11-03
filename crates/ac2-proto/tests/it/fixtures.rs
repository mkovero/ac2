//! Cross-language fixtures (`fixtures/protocol`, `tools/protocol/fixtures.py`).
//!
//! - `rust_*.bin` must equal what this build encodes from `samples`; regenerate with
//!   `AC2_UPDATE_FIXTURES=1 cargo test -p ac2-proto --test it fixtures::`, then run
//!   `tools/protocol/fixtures.py check` so Python decodes them.
//! - `py_*.bin` are written by Python; they must decode here to exactly the samples.
//! - `rust_chain_<name>.bin` is a delta chain (`samples::chains`): a keyframe, then deltas,
//!   each message's two parts in turn; `rust_chain_<name>_keys.bin` the same frames as
//!   keyframes. Python applies the chain and must get the keyframes' values;
//!   `py_chain_<name>.bin` is Python's chain of those frames, which must apply here to the
//!   samples' codes.
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
    for (name, frames) in samples::chains() {
        let c: Vec<CodedFrame> = frames
            .iter()
            .map(|f| CodedFrame::from_frame(f).expect("fixture"))
            .collect();
        let mut chain = encode_key(&c[0]).expect("fixture");
        let mut keys = chain.clone();
        for w in c.windows(2) {
            chain.extend(encode_delta(&w[1], &w[0]).expect("fixture").expect("fits"));
            keys.extend(encode_key(&w[1]).expect("fixture"));
        }
        out.push((format!("rust_chain_{name}.bin"), container(&chain)));
        out.push((format!("rust_chain_{name}_keys.bin"), container(&keys)));
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
        .map(|(i, r)| encode_reply(&samples::reply(i, r)).expect("fixture"))
        .collect();
    out.push(("rust_replies.bin".into(), container(&reps)));
    let evts: Vec<Vec<u8>> = samples::events()
        .iter()
        .map(|e| encode_event(e).expect("fixture"))
        .collect();
    out.push(("rust_events.bin".into(), container(&evts)));
    let data: Vec<Vec<u8>> = samples::data_requests()
        .iter()
        .map(|r| encode_data_request(r).expect("fixture"))
        .collect();
    out.push(("rust_data_requests.bin".into(), container(&data)));
    let notices: Vec<Vec<u8>> = samples::notices()
        .into_iter()
        .flat_map(|n| encode_notice(n).expect("fixture"))
        .collect();
    out.push(("rust_notices.bin".into(), container(&notices)));
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
fn python_chains_apply_to_the_samples() {
    for (name, frames) in samples::chains() {
        let file = format!("py_chain_{name}.bin");
        let parts = read_container(&file);
        assert_eq!(parts.len(), 2 * frames.len(), "{file}");
        let mut held: Option<CodedFrame> = None;
        for (msg, f) in parts.chunks(2).zip(&frames) {
            let refs: Vec<&[u8]> = msg.iter().map(Vec::as_slice).collect();
            let w = decode_wire(&refs).unwrap_or_else(|e| panic!("{file}: {e}"));
            assert_eq!(
                matches!(w, WireFrame::Key(_)),
                held.is_none(),
                "{file}: a keyframe, then deltas"
            );
            let got = w
                .apply(held.as_ref())
                .unwrap_or_else(|e| panic!("{file}: {e}"));
            let want = CodedFrame::from_frame(f).expect("fixture");
            assert_eq!(got.codes, want.codes, "{file}");
            assert_eq!(got.stamp, want.stamp, "{file}");
            assert_eq!(
                encode_key(&got).expect("fixture"),
                encode_key(&want).expect("fixture"),
                "{file}"
            );
            held = Some(got);
        }
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

#[test]
fn python_data_requests_decode_to_the_samples() {
    let got: Vec<DataRequest> = read_container("py_data_requests.bin")
        .iter()
        .map(|b| decode_data_request(b).expect("fixture"))
        .collect();
    assert_eq!(got, samples::data_requests());
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
