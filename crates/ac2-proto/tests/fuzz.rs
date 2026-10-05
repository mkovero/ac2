//! Property tests: random frames round-trip; malformed input never panics.

use ac2_proto::frame::{ClipFlags, LevelsFrame, LevelsMeta, ProtectionFlags, TfFrame, TfMeta};
use ac2_proto::model::{DelayPick, SplLogWhich};
use ac2_proto::units::*;
use ac2_proto::*;
use proptest::prelude::*;

fn parts_ref(p: &[Vec<u8>]) -> Vec<&[u8]> {
    p.iter().map(Vec::as_slice).collect()
}

fn any_f32_bits() -> impl Strategy<Value = f32> {
    any::<u32>().prop_map(f32::from_bits)
}

fn tf_strategy() -> impl Strategy<Value = Frame> {
    (
        0usize..600,
        any::<u64>(),
        any::<u32>(),
        any::<bool>(),
        0u32..128,
    )
        .prop_flat_map(|(n, seq, meas, eff, prot)| {
            (
                prop::collection::vec(any_f32_bits(), n),
                prop::collection::vec(any_f32_bits(), n),
                prop::collection::vec(any_f32_bits(), n),
                prop::collection::vec(0u32..(1 << 9), n),
                Just((seq, meas, eff, prot)),
            )
        })
        .prop_map(|(mag, phase, coh, val, (seq, meas, eff, prot))| {
            let mut stamp = samples::stamp(Some(samples::log_grid()));
            stamp.seq = seq;
            stamp.protection = ProtectionFlags(prot);
            Frame {
                stamp,
                data: FrameData::Tf(TfFrame {
                    meas: MeasId(meas),
                    meta: TfMeta {
                        delay: Seconds(0.001),
                        frozen: eff,
                        smoothing: None,
                        mic_curve: false,
                        average: None,
                    },
                    mag,
                    phase,
                    coh,
                    validity: val.into_iter().map(frame::ValidityMask).collect(),
                }),
            }
        })
}

fn levels_strategy() -> impl Strategy<Value = Frame> {
    (0usize..64)
        .prop_flat_map(|n| {
            (
                prop::collection::vec(any::<u16>(), n),
                prop::collection::vec(any_f32_bits(), n),
                prop::collection::vec(any_f32_bits(), n),
                prop::collection::vec(0u32..4, n),
            )
        })
        .prop_map(|(channels, peak, rms, clip)| Frame {
            stamp: samples::stamp(None),
            data: FrameData::Levels(LevelsFrame {
                meas: MeasId(2),
                meta: LevelsMeta { channels },
                peak,
                rms,
                clip: clip.into_iter().map(ClipFlags).collect(),
            }),
        })
}

/// Mutation applied to a valid frame's parts.
#[derive(Debug, Clone)]
enum Mutation {
    Flip { part: usize, byte: usize, bit: u8 },
    Truncate { part: usize, len: usize },
    Extend { part: usize, extra: Vec<u8> },
    DropPart(usize),
    DupPart(usize),
    Replace { part: usize, bytes: Vec<u8> },
}

fn mutation() -> impl Strategy<Value = Mutation> {
    prop_oneof![
        (0usize..8, any::<usize>(), 0u8..8).prop_map(|(part, byte, bit)| Mutation::Flip {
            part,
            byte,
            bit
        }),
        (0usize..8, any::<usize>()).prop_map(|(part, len)| Mutation::Truncate { part, len }),
        (0usize..8, prop::collection::vec(any::<u8>(), 1..9))
            .prop_map(|(part, extra)| Mutation::Extend { part, extra }),
        (0usize..8).prop_map(Mutation::DropPart),
        (0usize..8).prop_map(Mutation::DupPart),
        (0usize..8, prop::collection::vec(any::<u8>(), 0..64))
            .prop_map(|(part, bytes)| Mutation::Replace { part, bytes }),
    ]
}

fn apply(parts: &mut Vec<Vec<u8>>, m: &Mutation) {
    let n = parts.len();
    match m {
        Mutation::Flip { part, byte, bit } => {
            let p = &mut parts[part % n];
            if !p.is_empty() {
                let i = byte % p.len();
                p[i] ^= 1 << bit;
            }
        }
        Mutation::Truncate { part, len } => {
            let p = &mut parts[part % n];
            let l = if p.is_empty() { 0 } else { len % p.len() };
            p.truncate(l);
        }
        Mutation::Extend { part, extra } => parts[part % n].extend_from_slice(extra),
        Mutation::DropPart(i) => {
            parts.remove(i % n);
        }
        Mutation::DupPart(i) => {
            let p = parts[i % n].clone();
            parts.insert(i % n, p);
        }
        Mutation::Replace { part, bytes } => parts[part % n] = bytes.clone(),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn random_tf_frames_roundtrip(f in tf_strategy()) {
        let parts = encode_frame(&f).expect("sample message");
        let back = decode_frame(&parts_ref(&parts));
        if f.stamp.protection.is_known() {
            let back = back.expect("sample message");
            prop_assert_eq!(encode_frame(&back).expect("sample message"), parts);
        } else {
            prop_assert!(back.is_err());
        }
    }

    #[test]
    fn random_levels_frames_roundtrip(f in levels_strategy()) {
        let parts = encode_frame(&f).expect("sample message");
        let back = decode_frame(&parts_ref(&parts)).expect("sample message");
        prop_assert_eq!(encode_frame(&back).expect("sample message"), parts);
    }

    #[test]
    fn mutated_frames_never_panic(
        which in 0usize..8,
        ms in prop::collection::vec(mutation(), 1..4),
    ) {
        let f = samples::frames().swap_remove(which);
        let mut parts = encode_frame(&f).expect("sample message");
        for m in &ms {
            if parts.is_empty() { break; }
            apply(&mut parts, m);
        }
        // Must return, Ok or Err; a successful decode must re-encode losslessly.
        if let Ok(back) = decode_data_message(&parts_ref(&parts))
            && let DataMessage::Frame(fr) = back
        {
            let again = encode_frame(&fr).expect("sample message");
            let fr2 = decode_frame(&parts_ref(&again)).expect("sample message");
            prop_assert_eq!(encode_frame(&fr2).expect("sample message"), again);
        }
    }

    #[test]
    fn random_parts_never_panic(parts in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..300), 0..12)) {
        let _ = decode_data_message(&parts_ref(&parts));
    }

    #[test]
    fn random_ctrl_bytes_never_panic(b in prop::collection::vec(any::<u8>(), 0..512)) {
        let _ = decode_request(&b);
        let _ = decode_reply(&b);
        let _ = peek_envelope(&b);
        let _ = decode_event(&b);
    }

    #[test]
    fn mutated_requests_never_panic(
        which in 0usize..41,
        flips in prop::collection::vec((any::<usize>(), 0u8..8), 1..6),
        cut in any::<usize>(),
    ) {
        let cmds = samples::commands();
        let cmd = cmds[which % cmds.len()].clone();
        let mut b = encode_request(&Request::new(RequestId(1), cmd)).expect("sample message");
        for (i, bit) in flips {
            let l = b.len();
            b[i % l] ^= 1 << bit;
        }
        let l = b.len();
        b.truncate(cut % (l + 1));
        let _ = decode_request(&b);
    }

    #[test]
    fn parametric_commands_roundtrip(
        meas in any::<u32>(),
        delay in any::<f64>().prop_filter("finite", |x| x.is_finite()),
        flag in any::<bool>(),
        idx in any::<u8>(),
        name in "[ -~]{0,40}",
        token in any::<u128>(),
        rev in prop::option::of(any::<u64>()),
    ) {
        let m = MeasId(meas);
        let cmds = vec![
            Command::DelaySet { meas: m, delay: Seconds(delay) },
            Command::DelayInsert { meas: m, pick: DelayPick::Ranked { index: idx } },
            Command::MeasFreeze { meas: m, frozen: flag },
            Command::TraceCapture { meas: m, name: name.clone(), slot: None },
            Command::GenRefresh { lease_token: LeaseToken(token) },
            Command::GenAcquire { force: flag },
            Command::Hello { client: name.clone() },
            Command::SplLogGet {
                meas: m,
                log: if flag { SplLogWhich::Previous } else { SplLogWhich::Current },
                from: u64::from(idx),
                max: u32::from(idx),
            },
            Command::SplLogNew { meas: m },
            Command::SplHistoryGet { meas: m, seconds: u32::from(idx) },
        ];
        for cmd in cmds {
            let mut req = Request::new(RequestId(u64::from(meas)), cmd);
            req.expect_rev = rev.map(Rev);
            let b = encode_request(&req).expect("sample message");
            prop_assert_eq!(decode_request(&b).expect("sample message"), req);
        }
    }
}
