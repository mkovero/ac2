//! Round trips of every message variant, strictness, and version refusal.

use std::collections::BTreeSet;

use ac2_proto::frame::{MAX_HEADER_BYTES, ValidityMask};
use ac2_proto::samples;
use ac2_proto::units::RequestId;
use ac2_proto::*;

/// Names serde lists in its "unknown variant" error: the complete variant set of an enum,
/// derived from the type itself rather than from a hand-kept list.
pub fn serde_variant_names(err: &str) -> BTreeSet<String> {
    let tail = err.split("expected one of").nth(1).unwrap_or("");
    tail.split('`')
        .enumerate()
        .filter(|(i, _)| i % 2 == 1)
        .map(|(_, s)| s.to_string())
        .collect()
}

#[derive(serde::Serialize)]
struct BadOp<'a> {
    v: u16,
    id: u64,
    cmd: BadCmd<'a>,
    expect_rev: Option<u64>,
}

#[derive(serde::Serialize)]
struct BadCmd<'a> {
    op: &'a str,
}

fn all_command_names() -> BTreeSet<String> {
    let b = rmp_serde::to_vec_named(&BadOp {
        v: PROTO_VERSION,
        id: 1,
        cmd: BadCmd { op: "no.such" },
        expect_rev: None,
    })
    .expect("sample message");
    let err = decode_request(&b).expect_err("must be refused").to_string();
    serde_variant_names(&err)
}

#[test]
fn every_command_roundtrips_and_samples_cover_all_variants() {
    let mut seen = BTreeSet::new();
    for (i, cmd) in samples::commands().into_iter().enumerate() {
        let mut req = Request::new(RequestId(i as u64), cmd.clone());
        if cmd.is_mutation() {
            req.expect_rev = Some(units::Rev(41));
        }
        let b = encode_request(&req).expect("sample message");
        let back = decode_request(&b).expect("sample message");
        assert_eq!(back, req, "{}", cmd.name());
        assert!(
            seen.insert(cmd.name().to_string()),
            "duplicate {}",
            cmd.name()
        );
    }
    let all = all_command_names();
    assert!(all.len() > 30, "{all:?}");
    assert_eq!(seen, all);
}

#[test]
fn op_on_the_wire_is_the_command_name() {
    #[derive(serde::Deserialize)]
    struct Peek {
        cmd: PeekCmd,
    }
    #[derive(serde::Deserialize)]
    struct PeekCmd {
        op: String,
    }
    for cmd in samples::commands() {
        let b = encode_request(&Request::new(RequestId(1), cmd.clone())).expect("sample message");
        let p: Peek = rmp_serde::from_slice(&b).expect("sample message");
        assert_eq!(p.cmd.op, cmd.name());
    }
}

#[test]
fn every_reply_roundtrips() {
    for (i, r) in samples::replies().into_iter().enumerate() {
        let rep = Reply::new(RequestId(i as u64), r);
        let b = encode_reply(&rep).expect("sample message");
        let back = decode_reply(&b).expect("sample message");
        // TraceData holds NaN: compare re-encoded bytes.
        assert_eq!(encode_reply(&back).expect("sample message"), b);
    }
}

#[test]
fn every_event_roundtrips() {
    let mut kinds = BTreeSet::new();
    for e in samples::events() {
        let b = encode_event(&e).expect("sample message");
        assert_eq!(decode_event(&b).expect("sample message"), e);
        kinds.insert(e.change.kind());
        let parts = encode_event_message(&e).expect("sample message");
        let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
        assert_eq!(
            decode_data_message(&refs).expect("sample message"),
            DataMessage::Event(e)
        );
    }
    assert_eq!(kinds.len(), 8);
}

#[test]
fn event_wire_shape_is_rev_kind_payload() {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct Shape {
        rev: u64,
        kind: String,
        payload: serde::de::IgnoredAny,
    }
    for e in samples::events() {
        let b = encode_event(&e).expect("sample message");
        let s: Shape = rmp_serde::from_slice(&b).expect("sample message");
        assert_eq!(s.rev, e.rev.0);
        assert_eq!(s.kind, e.change.kind());
    }
}

fn bytes_of(f: &Frame) -> Vec<Vec<u8>> {
    encode_frame(f).expect("sample message")
}

fn decode_parts(parts: &[Vec<u8>]) -> Result<Frame, DecodeError> {
    let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
    decode_frame(&refs)
}

#[test]
fn every_frame_kind_roundtrips() {
    let mut kinds = BTreeSet::new();
    for f in samples::frames() {
        let parts = bytes_of(&f);
        let back = decode_parts(&parts).expect("sample message");
        assert_eq!(bytes_of(&back), parts);
        assert_eq!(back.topic(), f.topic());
        kinds.insert(format!("{:?}", f.data.kind()));
    }
    assert_eq!(kinds.len(), 8);
}

#[test]
fn tf_frame_size() {
    for eff in [false, true] {
        let parts = bytes_of(&samples::tf_frame(eff));
        let total: usize = parts.iter().map(Vec::len).sum();
        let arrays = if eff { 5 } else { 4 };
        assert_eq!(parts.len(), 2 + arrays);
        assert!(parts[1].len() < MAX_HEADER_BYTES);
        println!(
            "tf 480 cols eff_avg={eff}: topic {} B, header {} B, arrays {} x {} B, total {} B",
            parts[0].len(),
            parts[1].len(),
            arrays,
            parts[2].len(),
            total
        );
    }
}

#[test]
fn nan_and_bit_patterns_survive() {
    let f = samples::tf_frame(false);
    let back = decode_parts(&bytes_of(&f)).expect("sample message");
    let (FrameData::Tf(a), FrameData::Tf(b)) = (&f.data, &back.data) else {
        panic!()
    };
    let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&a.mag), bits(&b.mag));
    assert!(b.mag[0].is_nan());
    assert_eq!(b.validity[0], ValidityMask::THINNED);
    assert_eq!(b.eff_avg, None);
}

#[test]
fn misaligned_payload_decodes() {
    let parts = bytes_of(&samples::tf_frame(true));
    // Shift every array part by one byte inside a larger buffer so it is not 4-aligned.
    let shifted: Vec<Vec<u8>> = parts
        .iter()
        .map(|p| {
            let mut v = vec![0u8];
            v.extend_from_slice(p);
            v
        })
        .collect();
    let refs: Vec<&[u8]> = shifted.iter().map(|p| &p[1..]).collect();
    let back = decode_frame(&refs).expect("sample message");
    assert_eq!(bytes_of(&back), parts);
}

#[test]
fn malformed_frames_are_refused() {
    let good = bytes_of(&samples::tf_frame(false));
    let mut p = good.clone();
    p[2].pop();
    assert!(matches!(
        decode_parts(&p),
        Err(DecodeError::ArrayLength { index: 0, .. })
    ));
    let mut p = good.clone();
    p.pop();
    assert!(matches!(
        decode_parts(&p),
        Err(DecodeError::ArrayCount { .. })
    ));
    let mut p = good.clone();
    p[0] = b"d/1/rta".to_vec();
    assert_eq!(decode_parts(&p), Err(DecodeError::KindMismatch));
    let mut p = good.clone();
    p[1] = vec![0x80; MAX_HEADER_BYTES + 1];
    assert!(matches!(
        decode_parts(&p),
        Err(DecodeError::HeaderTooLarge(_))
    ));
    let mut p = good.clone();
    p[5] = vec![0xff; p[5].len()];
    assert_eq!(
        decode_parts(&p),
        Err(DecodeError::UnknownBits(frame::ArrayName::Validity))
    );
    assert!(matches!(
        decode_parts(&good[..1]),
        Err(DecodeError::Parts(1))
    ));
}

#[test]
fn encode_refuses_ragged_arrays() {
    let mut f = samples::tf_frame(false);
    if let FrameData::Tf(t) = &mut f.data {
        t.coh.pop();
    }
    assert!(matches!(encode_frame(&f), Err(EncodeError::Length { .. })));
}

#[test]
fn unknown_fields_are_refused() {
    #[derive(serde::Serialize)]
    struct Extra {
        v: u16,
        id: u64,
        cmd: Cmd,
        expect_rev: Option<u64>,
        surprise: bool,
    }
    #[derive(serde::Serialize)]
    struct Cmd {
        op: &'static str,
    }
    let b = rmp_serde::to_vec_named(&Extra {
        v: PROTO_VERSION,
        id: 1,
        cmd: Cmd { op: "gen.stop" },
        expect_rev: None,
        surprise: true,
    })
    .expect("sample message");
    assert!(matches!(decode_request(&b), Err(CtrlError::Malformed(_))));

    #[derive(serde::Serialize)]
    struct Args {
        force: bool,
        extra: u8,
    }
    #[derive(serde::Serialize)]
    struct Req2 {
        v: u16,
        id: u64,
        cmd: Cmd2,
        expect_rev: Option<u64>,
    }
    #[derive(serde::Serialize)]
    struct Cmd2 {
        op: &'static str,
        args: Args,
    }
    let b = rmp_serde::to_vec_named(&Req2 {
        v: PROTO_VERSION,
        id: 1,
        cmd: Cmd2 {
            op: "gen.acquire",
            args: Args {
                force: true,
                extra: 1,
            },
        },
        expect_rev: None,
    })
    .expect("sample message");
    assert!(matches!(decode_request(&b), Err(CtrlError::Malformed(_))));
}

#[test]
fn version_mismatch_is_a_typed_refusal() {
    let mut req = Request::new(RequestId(9), Command::GenStop);
    req.v = PROTO_VERSION + 1;
    let b = encode_request(&req).expect("sample message");
    assert_eq!(
        decode_request(&b),
        Err(CtrlError::VersionMismatch {
            ours: PROTO_VERSION,
            theirs: PROTO_VERSION + 1
        })
    );
    let env = peek_envelope(&b).expect("sample message");
    let refusal = Reply::version_refusal(env.id.expect("sample message"), env.v);
    let back =
        decode_reply(&encode_reply(&refusal).expect("sample message")).expect("sample message");
    let err = back.result.expect_err("must be refused");
    assert_eq!(err.code, ErrorCode::VersionMismatch);
    assert_eq!(
        err.detail,
        Some(ErrorDetail::Version {
            daemon: PROTO_VERSION,
            client: PROTO_VERSION + 1
        })
    );

    // A message without `v` is refused, never defaulted.
    #[derive(serde::Serialize)]
    struct NoV {
        id: u64,
        cmd: &'static str,
    }
    let b = rmp_serde::to_vec_named(&NoV {
        id: 1,
        cmd: "gen.stop",
    })
    .expect("sample message");
    assert_eq!(decode_request(&b), Err(CtrlError::MissingVersion));
    assert_eq!(
        peek_envelope(&b).expect("sample message").id,
        Some(RequestId(1))
    );

    // A frame header at another version.
    let mut parts = bytes_of(&samples::frames().pop().expect("sample message"));
    // `v` is the first key of the header map: fixmap marker, fixstr "v", positive fixint.
    assert_eq!(&parts[1][1..3], b"\xa1v");
    parts[1][3] = (PROTO_VERSION + 1) as u8;
    assert!(matches!(
        decode_parts(&parts),
        Err(DecodeError::VersionMismatch { .. })
    ));
}

#[test]
fn grids_roundtrip_and_ids() {
    for g in samples::grids() {
        let b = rmp_serde::to_vec_named(&g).expect("sample message");
        let back: GridDef = rmp_serde::from_slice(&b).expect("sample message");
        assert_eq!(back.id(), g.id());
        assert_eq!(back, g);
    }
}

#[test]
fn oversized_ctrl_is_refused_before_parsing() {
    let b = vec![0u8; ctrl::MAX_CTRL_BYTES + 1];
    assert_eq!(decode_request(&b), Err(CtrlError::TooLarge(b.len())));
}
