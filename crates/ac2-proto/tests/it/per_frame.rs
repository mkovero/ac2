//! What a delta carries of its frame's metadata, per kind, and what still breaks the chain:
//! the levels' readings as arrays, a goniometer's gains, a spectrum's band ages, an
//! alignment's fit; and an array's own length, checked against what it belongs to.

use ac2_proto::frame::FrameMeta;
use ac2_proto::units::*;
use ac2_proto::*;

fn coded(f: &Frame) -> CodedFrame {
    CodedFrame::from_frame(f).expect("quantises")
}

fn refs(p: &[Vec<u8>]) -> Vec<&[u8]> {
    p.iter().map(Vec::as_slice).collect()
}

/// `next` as a delta on `base`, applied and turned back into values.
fn through_delta(next: &Frame, base: &Frame) -> Frame {
    let (x, b) = (coded(next), coded(base));
    let d = encode_delta(&x, &b).expect("encodes").expect("fits");
    let got = decode_wire(&refs(&d))
        .expect("decodes")
        .apply(Some(&b))
        .expect("applies");
    assert_eq!(got, x);
    got.to_frame().expect("values")
}

fn sample(pick: impl Fn(&FrameData) -> bool) -> Frame {
    samples::frames()
        .into_iter()
        .find(|f| pick(&f.data))
        .expect("a sample")
}

fn next(f: &Frame) -> Frame {
    let mut g = f.clone();
    g.stamp.seq += 1;
    g.stamp.audio_sample = SampleIndex(g.stamp.audio_sample.0 + 1600);
    g
}

/// Every reading of a levels frame moves from frame to frame and goes in a delta: the
/// meters, the pairs' correlation and quiet side, the sources' loudness, the timing mark.
#[test]
fn a_levels_frame_goes_as_a_delta_with_every_reading_moved() {
    let f = sample(|d| matches!(d, FrameData::SourceLevels(_)));
    let mut g = next(&f);
    let FrameData::SourceLevels(l) = &mut g.data else {
        unreachable!()
    };
    assert!(!l.pair_levels.is_empty() && !l.loudness.is_empty());
    for v in l.peak.iter_mut().chain(&mut l.rms).chain(&mut l.ppm) {
        *v -= 1.5;
    }
    l.overs[0] += 1;
    l.pair_levels[0].correlation = Some(0.5);
    l.pair_levels[0].quiet = None;
    for s in &mut l.loudness {
        s.momentary = Lufs(s.momentary.0 + 0.25);
        s.duration = Seconds(s.duration.0 + 1.0 / 15.0);
        s.integrated = None;
    }
    if let Some(m) = &mut l.meta.mark {
        m.sample = SampleIndex(m.sample.0 + 3200);
    }
    let mark = l.meta.mark;
    let got = through_delta(&g, &f);
    assert_eq!(
        encode_frame(&got).expect("encodes"),
        encode_frame(&coded(&g).to_frame().expect("values")).expect("encodes")
    );
    let FrameData::SourceLevels(r) = &got.data else {
        unreachable!()
    };
    assert_eq!(r.pair_levels[0].correlation, Some(0.5));
    assert_eq!(r.loudness[0].integrated, None);
    assert_eq!(r.meta.mark, mark);
}

/// An identity is static: another pair, another source or a reset (`since`) goes as a
/// keyframe, so a reading is never read against the wrong source.
#[test]
fn a_levels_identity_change_goes_as_a_keyframe() {
    let f = sample(|d| matches!(d, FrameData::StreamLevels(_)));
    let base = coded(&f);
    let mut reset = next(&f);
    if let FrameData::StreamLevels(l) = &mut reset.data {
        l.meta.sources[0].since = WallNs(l.meta.sources[0].since.0 + 1);
    }
    assert_eq!(encode_delta(&coded(&reset), &base), Ok(None));
    let mut other = next(&f);
    if let FrameData::StreamLevels(l) = &mut other.data {
        l.meta.pairs[0].right += 2;
    }
    assert_eq!(encode_delta(&coded(&other), &base), Ok(None));
    // One more source: its arrays grow, a keyframe as well.
    let mut more = next(&f);
    if let FrameData::StreamLevels(l) = &mut more.data {
        let mut s = l.meta.sources[0];
        s.source = frame::LoudnessSource::Input { channel: 7 };
        l.meta.sources.push(s);
        l.loudness.push(l.loudness[0]);
    }
    assert_eq!(encode_delta(&coded(&more), &base), Ok(None));
    assert!(encode_key(&coded(&more)).is_ok());
}

/// A levels frame whose per-source arrays do not match its sources does not encode.
#[test]
fn a_levels_frame_with_a_reading_short_does_not_encode() {
    let mut f = sample(|d| matches!(d, FrameData::StreamLevels(_)));
    if let FrameData::StreamLevels(l) = &mut f.data {
        l.loudness.clear();
    }
    assert!(CodedFrame::from_frame(&f).is_err());
}

/// Bytes of an array of `n` elements (no `bfp16` among the levels arrays).
fn bytes(e: frame::Elem, n: usize) -> usize {
    use frame::Elem::*;
    n * match e {
        F32 | U32 => 4,
        I16 | I16MagOffset | U16 | Bfp16 => 2,
        I8 | U8 => 1,
    }
}

/// An array's `len` must equal the count it belongs to: a keyframe whose per-source array
/// claims fewer or more elements than there are sources (codes sized to match, so the
/// encoder sends it) is refused; a coded frame whose codes do not match its descriptors is
/// not sent at all.
#[test]
fn an_array_len_other_than_its_count_is_refused() {
    let f = sample(|d| matches!(d, FrameData::StreamLevels(_)));
    let c = coded(&f);
    let at = c
        .arrays
        .iter()
        .position(|d| d.name == frame::ArrayName::LufsM)
        .expect("a per-source array");
    let d = c.arrays[at];
    let from: usize = c.arrays[..at]
        .iter()
        .map(|d| bytes(d.elem, d.len as usize))
        .sum();
    let to = from + bytes(d.elem, d.len as usize);
    for len in [0, d.len + 1, 4 * d.len + 3] {
        let mut bad = c.clone();
        bad.arrays[at].len = len;
        bad.codes
            .splice(from..to, vec![0; bytes(d.elem, len as usize)]);
        let parts = encode_key(&bad).expect("sized");
        assert!(decode_wire(&refs(&parts)).is_err(), "len {len}");
        let mut short = bad.clone();
        short.codes.pop();
        assert!(encode_key(&short).is_err(), "len {len}");
        assert!(encode_delta(&short, &c).is_err(), "len {len}");
    }
}

/// A goniometer's auto gains move every frame and go in the delta; another pair count
/// does not fit.
#[test]
fn a_gonio_delta_carries_the_gains() {
    let f = sample(|d| matches!(d, FrameData::StreamGonio(_)));
    let mut g = next(&f);
    if let FrameData::StreamGonio(x) = &mut g.data {
        for p in &mut x.pairs {
            p.gain_dbfs -= 3.0;
        }
    }
    let got = through_delta(&g, &f);
    let (FrameData::StreamGonio(a), FrameData::StreamGonio(b)) = (&got.data, &g.data) else {
        unreachable!()
    };
    assert_eq!(a.pairs, b.pairs);
}

/// A multi-resolution spectrum's band ages and its harmonics go in the delta.
#[test]
fn a_spec_delta_carries_the_band_ages() {
    let f = sample(|d| matches!(d, FrameData::Spec(s) if s.meta.bands.is_some()));
    let mut g = next(&f);
    if let FrameData::Spec(s) = &mut g.data {
        for b in &mut s.meta.bands.as_mut().expect("bands").bands {
            b.age = Some(Seconds(b.age.map_or(0.0, |a| a.0) + 0.05));
        }
        s.meta.harmonics = None;
    }
    let got = through_delta(&g, &f);
    let (FrameData::Spec(a), FrameData::Spec(b)) = (&got.data, &g.data) else {
        unreachable!()
    };
    assert_eq!(a.meta.bands, b.meta.bands);
    assert_eq!(a.meta.harmonics, None);
}

/// An alignment's fit follows the curves and goes in the delta; on a frame of no math it
/// has no place, and the frame goes as a keyframe.
#[test]
fn a_tf_delta_carries_the_alignment() {
    let f = samples::tf_frame();
    let mut g = next(&f);
    if let FrameData::Tf(t) = &mut g.data {
        let math = t.meta.math.as_mut().expect("math");
        assert!(math.align.is_some());
        math.align = None;
    }
    let got = through_delta(&g, &f);
    let FrameData::Tf(t) = &got.data else {
        unreachable!()
    };
    assert_eq!(t.meta.math.as_ref().expect("math").align, None);
    // The way back: the fit returns in a delta.
    let back = through_delta(&next(&f), &g);
    let FrameData::Tf(t) = &back.data else {
        unreachable!()
    };
    assert!(t.meta.math.as_ref().expect("math").align.is_some());
    // No math on the base: the per-frame part cannot hold a fit, so a frame with one
    // goes as a keyframe.
    let mut plain = f.clone();
    if let FrameData::Tf(t) = &mut plain.data {
        t.meta.math = None;
    }
    assert_eq!(encode_delta(&coded(&f), &coded(&plain)), Ok(None));
}

/// The per-frame metadata a delta carries is the kind's alone: a delta of one kind's
/// frame names that kind's fields (spot check through the metadata it rebuilds).
#[test]
fn a_delta_rebuilds_the_static_metadata_from_its_base() {
    let f = samples::bins_frame();
    let mut g = next(&f);
    if let FrameData::Bins(b) = &mut g.data {
        b.meta.blocks += 7;
    }
    let got = through_delta(&g, &f);
    let FrameMeta::Bins(want) = coded(&g).meta else {
        unreachable!()
    };
    let FrameData::Bins(b) = &got.data else {
        unreachable!()
    };
    assert_eq!(b.meta, want);
}
