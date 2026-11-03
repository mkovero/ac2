//! Keyframes and deltas: a chain applied in order rebuilds every frame's codes exactly; a
//! delta on the wrong base or with codes that miss its CRC is refused, never drawn.

use ac2_proto::frame::{Elem, ProtectionFlags, TfFrame, TfMeta, ValidityMask};
use ac2_proto::units::*;
use ac2_proto::*;
use proptest::prelude::*;

fn refs(p: &[Vec<u8>]) -> Vec<&[u8]> {
    p.iter().map(Vec::as_slice).collect()
}

fn coded(f: &Frame) -> CodedFrame {
    CodedFrame::from_frame(f).expect("quantises")
}

fn delta(x: &CodedFrame, base: &CodedFrame) -> Vec<Vec<u8>> {
    encode_delta(x, base)
        .expect("encodes")
        .expect("the base fits")
}

fn wire(parts: &[Vec<u8>]) -> WireFrame {
    decode_wire(&refs(parts)).expect("decodes")
}

/// Metadata compared by their bytes: NaN-safe.
fn same_coded(a: &CodedFrame, b: &CodedFrame) {
    assert_eq!(a.topic, b.topic);
    assert_eq!(a.stamp, b.stamp);
    assert_eq!(a.n, b.n);
    assert_eq!(a.arrays, b.arrays);
    assert_eq!(a.codes, b.codes);
    assert_eq!(
        rmp_serde::to_vec(&a.meta).expect("meta"),
        rmp_serde::to_vec(&b.meta).expect("meta")
    );
}

#[test]
fn chains_apply_to_the_same_codes_and_reencode_to_the_same_bytes() {
    for (name, frames) in samples::chains() {
        let c: Vec<CodedFrame> = frames.iter().map(coded).collect();
        let key = encode_key(&c[0]).expect("encodes");
        let WireFrame::Key(k) = wire(&key) else {
            panic!("{name}: a keyframe")
        };
        same_coded(&k, &c[0]);
        assert_eq!(encode_key(&k).expect("encodes"), key, "{name}");
        let mut held = k;
        for (i, x) in c.iter().enumerate().skip(1) {
            let d = delta(x, &c[i - 1]);
            let w = wire(&d);
            assert!(matches!(w, WireFrame::Delta(_)), "{name} {i}");
            assert_eq!(w.seq(), x.stamp.seq);
            let next = w.apply(Some(&held)).expect("applies on its base");
            same_coded(&next, x);
            assert_eq!(delta(&next, &held), d, "{name} {i}: re-encodes");
            // The values: what a keyframe of the same frame decodes to.
            assert_eq!(
                encode_frame(&next.to_frame().expect("values")).expect("encodes"),
                encode_frame(&frames[i]).expect("encodes"),
                "{name} {i}"
            );
            held = next;
        }
    }
}

#[test]
fn a_tf_delta_is_smaller_than_its_keyframe() {
    let (_, frames) = samples::chains().remove(0);
    let (a, b) = (coded(&frames[0]), coded(&frames[1]));
    let key: usize = encode_key(&b).expect("encodes").iter().map(Vec::len).sum();
    let d: usize = delta(&b, &a).iter().map(Vec::len).sum();
    assert!(d * 2 < key, "delta {d} B, keyframe {key} B");
}

/// Every kind deltas on a frame of its own topic: its 16-bit and `f32` codes moved, its
/// metadata unchanged.
#[test]
fn every_kind_deltas() {
    for f in samples::frames() {
        let base = coded(&f);
        let mut x = base.clone();
        x.stamp.seq += 1;
        x.stamp.audio_sample = SampleIndex(x.stamp.audio_sample.0 + 1);
        let mut at = 0;
        for d in &base.arrays {
            let k = match d.elem {
                Elem::F32 | Elem::U32 => 4,
                Elem::I16 | Elem::I16MagOffset | Elem::U16 | Elem::Bfp16 => 2,
                Elem::I8 | Elem::U8 => 1,
            } * base.n as usize
                + usize::from(d.elem == Elem::Bfp16);
            let codes = &mut x.codes[at..at + k];
            match d.elem {
                // Any `i16` code decodes; an angle array is kept to angles by its delta.
                Elem::I16MagOffset => {
                    for c in codes.chunks_exact_mut(2) {
                        let q = i16::from_le_bytes([c[0], c[1]]).wrapping_sub(5);
                        c.copy_from_slice(&q.to_le_bytes());
                    }
                }
                // Hundredths stay hundredths; NaN (255) stays.
                Elem::U8 => {
                    for c in codes.iter_mut() {
                        *c = match *c {
                            100 => 99,
                            255 => 255,
                            q => q + 1,
                        };
                    }
                }
                Elem::I16 if d.unit != frame::Unit::Deg => {
                    for c in codes.chunks_exact_mut(2) {
                        let q = i16::from_le_bytes([c[0], c[1]]).wrapping_add(3);
                        c.copy_from_slice(&q.to_le_bytes());
                    }
                }
                Elem::F32 => {
                    for c in codes.chunks_exact_mut(4) {
                        c[0] ^= 1;
                    }
                }
                _ => {}
            }
            at += k;
        }
        let d = delta(&x, &base);
        let got = wire(&d).apply(Some(&base)).expect("applies");
        same_coded(&got, &x);
        // Kinds without arrays carry no payload: the short header alone.
        if base.arrays.is_empty() {
            let key = encode_key(&x).expect("encodes");
            assert!(
                d[1].len() < key[1].len(),
                "{:?}: {} B",
                f.data.kind(),
                d[1].len()
            );
        }
    }
}

#[test]
fn a_wrong_base_or_a_bad_crc_is_refused() {
    let (_, frames) = samples::chains().remove(0);
    let c: Vec<CodedFrame> = frames.iter().map(coded).collect();
    let d = delta(&c[2], &c[1]);
    // On another frame than its base, held or not.
    assert_eq!(
        wire(&d).apply(Some(&c[0])),
        Err(DecodeError::WrongBase {
            held: c[0].stamp.seq,
            base: c[1].stamp.seq
        })
    );
    assert_eq!(wire(&d).apply(None), Err(DecodeError::NoBase));
    assert_eq!(decode_frame(&refs(&d)), Err(DecodeError::NoBase));
    // The base's seq on another topic.
    let mut other = c[1].clone();
    other.topic = Topic::Data {
        meas: MeasId(9),
        stream: Stream::Tf,
    };
    assert!(matches!(
        wire(&d).apply(Some(&other)),
        Err(DecodeError::WrongBase { .. })
    ));
    // The right seq, but codes that are not the base's (a frame the receiver patched): the
    // CRC catches what no sequence check can.
    let mut patched = c[1].clone();
    patched.codes[0] ^= 1;
    assert!(matches!(
        wire(&d).apply(Some(&patched)),
        Err(DecodeError::Crc { .. })
    ));
    // Damaged payload bytes: a payload error or a CRC miss, never a panic.
    for i in 0..d[1].len() {
        let mut bad = d.clone();
        bad[1][i] ^= 0x5a;
        if let Ok(w) = decode_wire(&refs(&bad)) {
            let _ = w.apply(Some(&c[1]));
        }
    }
}

#[test]
fn a_base_that_does_not_fit_forces_a_keyframe() {
    let f = samples::tf_frame();
    let base = coded(&f);
    let mut misfits: Vec<CodedFrame> = Vec::new();
    let mut g = f.clone();
    g.stamp.session_epoch = SessionEpoch(g.stamp.session_epoch.0 + 1);
    misfits.push(coded(&g));
    let mut g = f.clone();
    g.stamp.grid_id = None;
    misfits.push(coded(&g));
    let mut g = f.clone();
    g.stamp.config_rev = Rev(g.stamp.config_rev.0 + 1);
    misfits.push(coded(&g));
    let mut g = f.clone();
    if let FrameData::Tf(t) = &mut g.data {
        t.meta.delay = Seconds(0.5);
    }
    misfits.push(coded(&g));
    // Another array set: the average's range band goes.
    let mut g = f.clone();
    if let FrameData::Tf(t) = &mut g.data {
        assert!(t.range.is_some());
        t.range = None;
    }
    misfits.push(coded(&g));
    // Another n.
    let mut g = f.clone();
    if let FrameData::Tf(t) = &mut g.data {
        t.mag.pop();
        t.phase.pop();
        t.coh.pop();
        t.validity.pop();
        if let Some(a) = &mut t.align {
            for v in [
                &mut a.phase_before,
                &mut a.phase_after,
                &mut a.sum_before,
                &mut a.sum_after,
            ] {
                v.pop();
            }
        }
        if let Some(r) = &mut t.range {
            for v in [&mut r.lo, &mut r.hi, &mut r.coh] {
                v.pop();
            }
        }
    }
    misfits.push(coded(&g));
    for (i, m) in misfits.iter().enumerate() {
        assert_eq!(encode_delta(m, &base), Ok(None), "misfit {i}");
    }
    // Only `filled` changed: a delta.
    let mut g = f.clone();
    if let FrameData::Tf(t) = &mut g.data {
        t.meta.filled = 0.25;
    }
    assert!(encode_delta(&coded(&g), &base).expect("encodes").is_some());
}

fn any_f32_bits() -> impl Strategy<Value = f32> {
    any::<u32>().prop_map(f32::from_bits)
}

/// Columns that mostly repeat between two frames, as a deeper analysis stage's do.
fn column_pair() -> impl Strategy<Value = (f32, f32)> {
    prop_oneof![
        any_f32_bits().prop_map(|x| (x, x)),
        (any_f32_bits(), any_f32_bits()),
        (-180.0f32..180.0, -1.0f32..1.0).prop_map(|(x, d)| (x, x + d)),
    ]
}

fn tf(n_mag: Vec<(f32, f32)>, phase: Vec<(f32, f32)>, coh: Vec<(f32, f32)>, first: bool) -> Frame {
    let pick = |v: &[(f32, f32)]| -> Vec<f32> {
        v.iter().map(|(a, b)| if first { *a } else { *b }).collect()
    };
    let mut stamp = samples::stamp(Some(samples::log_grid()));
    stamp.seq += u64::from(!first);
    stamp.protection = ProtectionFlags(0);
    Frame {
        stamp,
        data: FrameData::Tf(TfFrame {
            meas: MeasId(1),
            meta: TfMeta {
                delay: Seconds(0.001),
                nudged: Seconds(0.0),
                smoothing: None,
                mic_curve: false,
                math: None,
                unresolved: None,
                filled: if first { 0.5 } else { 0.625 },
            },
            mag: pick(&n_mag),
            phase: pick(&phase),
            coh: pick(&coh),
            validity: vec![ValidityMask(u32::from(!first)); n_mag.len()],
            align: None,
            // Any edges on any curve: the offsets from `mag` wrap, NaN and ±∞ included.
            range: Some(model::RangeBand {
                lo: pick(&phase),
                hi: pick(&n_mag).iter().map(|x| x - 3.0).collect(),
                coh: pick(&coh),
            }),
        }),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Any pair of transfer functions of one grid: the delta rebuilds the second's codes.
    #[test]
    fn random_tf_deltas_apply_exactly(
        cols in (0usize..600).prop_flat_map(|n| (
            prop::collection::vec(column_pair(), n),
            prop::collection::vec(column_pair(), n),
            prop::collection::vec(column_pair(), n),
        ))
    ) {
        let (mag, phase, coh) = cols;
        let a = coded(&tf(mag.clone(), phase.clone(), coh.clone(), true));
        let b = coded(&tf(mag, phase, coh, false));
        let d = delta(&b, &a);
        let got = wire(&d).apply(Some(&a)).expect("applies");
        prop_assert_eq!(&got.codes, &b.codes);
        prop_assert_eq!(got.stamp, b.stamp);
        // Its values quantise to the same codes again: every encoding, the range band's
        // offsets and floored hundredths too, is idempotent.
        let again = coded(&got.to_frame().expect("values"));
        prop_assert_eq!(&again.codes, &b.codes);
    }

    /// Short headers: `v` and the delta form, then anything.
    #[test]
    fn fuzzed_short_headers_never_panic(
        fields in prop::collection::vec(any::<u8>(), 0..64),
        len in 0u8..16,
        payload in prop::collection::vec(any::<u8>(), 0..64),
    ) {
        let base = coded(&samples::tf_frame());
        let mut body = vec![0x90 | len, PROTO_VERSION as u8, 0x01];
        body.extend_from_slice(&fields);
        body.extend_from_slice(&payload);
        let parts = vec![b"d/1/tf".to_vec(), body];
        if let Ok(w) = decode_wire(&refs(&parts)) {
            let _ = w.apply(Some(&base));
        }
    }
}
