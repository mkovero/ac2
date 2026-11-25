//! Frame quantisation: a frame's arrays to their wire codes.

use crate::event::{Event, encode_event};
use crate::packing::{self, array_size};
use crate::topic::Topic;

use super::flags::MaskBits;
use super::schema::{desc, level_unit, spec_unit};
use super::{
    ArrayDesc, ArrayName, CodedFrame, EncodeError, Frame, FrameData, FrameMeta, MAX_N, Unit,
};

enum Col<'a> {
    F(&'a [f32]),
    /// Values coded relative to the same column's `mag`, the second slice.
    OnMag(&'a [f32], &'a [f32]),
    Owned(Vec<f32>),
    U(Vec<u32>),
    I(Vec<i8>),
}

impl Col<'_> {
    fn len(&self) -> usize {
        match self {
            Col::F(v) | Col::OnMag(v, _) => v.len(),
            Col::Owned(v) => v.len(),
            Col::U(v) => v.len(),
            Col::I(v) => v.len(),
        }
    }
}

/// A levels frame's arrays ([`super::levels::cols`]) as columns.
fn levels_cols<'a>(cols: &mut Vec<(ArrayDesc, Col<'a>)>, f: &super::levels::LevelsRef<'a>) {
    use super::levels::Values;
    cols.extend(super::levels::cols(f).into_iter().map(|(d, v)| {
        let c = match v {
            Values::F(v) => Col::F(v),
            Values::Owned(v) => Col::Owned(v),
            Values::U(v) => Col::U(v),
        };
        (d, c)
    }));
}

/// Every pair's points in turn, as one x and one y column.
fn gonio_cols<'a>(cols: &mut Vec<(ArrayDesc, Col<'a>)>, f: &super::GonioFrame) {
    let n = f.pairs.len() * super::GONIO_POINTS;
    let (mut x, mut y) = (Vec::with_capacity(n), Vec::with_capacity(n));
    for p in &f.pairs {
        x.extend(p.points.iter().map(|q| q[0]));
        y.extend(p.points.iter().map(|q| q[1]));
    }
    cols.push((desc(ArrayName::GonioX, Unit::GonioUnit), Col::I(x)));
    cols.push((desc(ArrayName::GonioY, Unit::GonioUnit), Col::I(y)));
}

fn mask_slice<T: MaskBits>(v: &[T]) -> Vec<u32> {
    v.iter().map(T::to_u32).collect()
}

/// Encode a frame as a keyframe: its parts `[topic, body]`.
pub fn encode_frame(frame: &Frame) -> Result<Vec<Vec<u8>>, EncodeError> {
    super::wire::encode_key(&quantise(frame)?)
}

/// A frame's arrays quantised to their codes, in the kind's array order.
pub(super) fn quantise(frame: &Frame) -> Result<CodedFrame, EncodeError> {
    let mut cols: Vec<(ArrayDesc, Col<'_>)> = Vec::new();
    let meta = match &frame.data {
        FrameData::Tf(f) => {
            cols.push((desc(ArrayName::Mag, Unit::Db), Col::F(&f.mag)));
            cols.push((desc(ArrayName::Phase, Unit::Deg), Col::F(&f.phase)));
            cols.push((desc(ArrayName::Coh, Unit::Coherence), Col::F(&f.coh)));
            cols.push((
                desc(ArrayName::Validity, Unit::Bitmask),
                Col::U(mask_slice(&f.validity)),
            ));
            if let Some(al) = &f.align {
                for (name, unit, v) in [
                    (ArrayName::AlignPhaseBefore, Unit::Deg, &al.phase_before),
                    (ArrayName::AlignPhaseAfter, Unit::Deg, &al.phase_after),
                    (ArrayName::AlignSumBefore, Unit::Db, &al.sum_before),
                    (ArrayName::AlignSumAfter, Unit::Db, &al.sum_after),
                ] {
                    cols.push((desc(name, unit), Col::F(v)));
                }
            }
            if let Some(r) = &f.range {
                for (name, v) in [(ArrayName::RangeLo, &r.lo), (ArrayName::RangeHi, &r.hi)] {
                    cols.push((desc(name, Unit::Db), Col::OnMag(v, &f.mag)));
                }
                cols.push((desc(ArrayName::RangeCoh, Unit::Coherence), Col::F(&r.coh)));
            }
            FrameMeta::Tf(f.meta.clone())
        }
        FrameData::Bins(f) => {
            cols.push((desc(ArrayName::Mag, Unit::Db), Col::F(&f.mag)));
            cols.push((desc(ArrayName::Phase, Unit::Deg), Col::F(&f.phase)));
            cols.push((desc(ArrayName::Coh, Unit::Coherence), Col::F(&f.coh)));
            FrameMeta::Bins(f.meta.clone())
        }
        FrameData::Ir(f) => {
            cols.push((
                desc(ArrayName::IrLinear, Unit::FullScale),
                Col::F(&f.linear),
            ));
            if let Some(e) = &f.etc {
                cols.push((desc(ArrayName::IrEtc, Unit::Db), Col::F(e)));
            }
            FrameMeta::Ir(f.meta)
        }
        FrameData::Rta(f) => {
            cols.push((
                desc(ArrayName::Level, level_unit(f.meta.scale)),
                Col::F(&f.level),
            ));
            cols.push((
                desc(ArrayName::Validity, Unit::Bitmask),
                Col::U(mask_slice(&f.validity)),
            ));
            FrameMeta::Rta(f.meta.clone())
        }
        FrameData::Spec(f) => {
            cols.push((
                desc(ArrayName::Level, spec_unit(f.meta.scale, f.meta.level)),
                Col::F(&f.level),
            ));
            FrameMeta::Spec(f.meta.clone())
        }
        FrameData::Spl(f) => FrameMeta::Spl(f.meta),
        FrameData::BandLeq(f) => {
            let unit = level_unit(f.meta.scale);
            cols.push((desc(ArrayName::Leq, unit), Col::F(&f.leq)));
            cols.push((desc(ArrayName::Limit, unit), Col::F(&f.limit)));
            cols.push((desc(ArrayName::Allowed, unit), Col::F(&f.allowed)));
            cols.push((desc(ArrayName::Recover, Unit::Seconds), Col::F(&f.recover)));
            cols.push((
                desc(ArrayName::LeqFlags, Unit::Bitmask),
                Col::U(mask_slice(&f.flags)),
            ));
            FrameMeta::BandLeq(f.meta.clone())
        }
        FrameData::Leq(f) => {
            let unit = level_unit(f.meta.scale);
            cols.push((desc(ArrayName::Leq, unit), Col::F(&f.leq)));
            cols.push((desc(ArrayName::Elapsed, Unit::Seconds), Col::F(&f.elapsed)));
            cols.push((
                desc(ArrayName::Measured, Unit::Seconds),
                Col::F(&f.measured),
            ));
            cols.push((desc(ArrayName::Allowed, unit), Col::F(&f.allowed)));
            cols.push((desc(ArrayName::Recover, Unit::Seconds), Col::F(&f.recover)));
            cols.push((desc(ArrayName::Least, unit), Col::F(&f.least)));
            cols.push((desc(ArrayName::OverIn, Unit::Seconds), Col::F(&f.over_in)));
            cols.push((
                desc(ArrayName::LeqFlags, Unit::Bitmask),
                Col::U(mask_slice(&f.flags)),
            ));
            FrameMeta::Leq(f.meta)
        }
        FrameData::Levels(f) => {
            cols.push((desc(ArrayName::Peak, Unit::Dbfs), Col::F(&f.peak)));
            cols.push((desc(ArrayName::Rms, Unit::Dbfs), Col::F(&f.rms)));
            cols.push((
                desc(ArrayName::Clip, Unit::Bitmask),
                Col::U(mask_slice(&f.clip)),
            ));
            FrameMeta::Levels(f.meta.clone())
        }
        FrameData::StreamLevels(f) => {
            levels_cols(
                &mut cols,
                &super::levels::LevelsRef {
                    ballistic: f.meta.ballistic,
                    peak: &f.peak,
                    rms: &f.rms,
                    clip: &f.clip,
                    ppm: &f.ppm,
                    true_peak: &f.true_peak,
                    max: &f.max,
                    overs: &f.overs,
                    pair_levels: &f.pair_levels,
                    loudness: &f.loudness,
                },
            );
            FrameMeta::StreamLevels(f.meta.clone())
        }
        FrameData::PreviewLevels(f) => {
            levels_cols(
                &mut cols,
                &super::levels::LevelsRef {
                    ballistic: f.meta.ballistic,
                    peak: &f.peak,
                    rms: &f.rms,
                    clip: &f.clip,
                    ppm: &f.ppm,
                    true_peak: &f.true_peak,
                    max: &f.max,
                    overs: &f.overs,
                    pair_levels: &f.pair_levels,
                    loudness: &f.loudness,
                },
            );
            FrameMeta::PreviewLevels(f.meta.clone())
        }
        FrameData::SourceLevels(f) => {
            levels_cols(
                &mut cols,
                &super::levels::LevelsRef {
                    ballistic: f.meta.ballistic,
                    peak: &f.peak,
                    rms: &f.rms,
                    clip: &f.clip,
                    ppm: &f.ppm,
                    true_peak: &f.true_peak,
                    max: &f.max,
                    overs: &f.overs,
                    pair_levels: &f.pair_levels,
                    loudness: &f.loudness,
                },
            );
            FrameMeta::SourceLevels(f.meta.clone())
        }
        FrameData::StreamGonio(f) => {
            gonio_cols(&mut cols, f);
            FrameMeta::StreamGonio(f.meta())
        }
        FrameData::PreviewGonio(f) => {
            gonio_cols(&mut cols, f);
            FrameMeta::PreviewGonio(f.meta())
        }
        FrameData::Timing(m) => FrameMeta::Timing(*m),
        FrameData::Ka(m) => FrameMeta::Ka(m.clone()),
    };

    // Every array has one element per column, pair or source: `n` is the column arrays'
    // length, and each array must have the count its name belongs to.
    let n = cols
        .iter()
        .find(|(d, _)| d.name.per() == super::ArrayPer::Column)
        .map_or(0, |(_, c)| c.len());
    if n > MAX_N as usize {
        return Err(EncodeError::TooManyElements(n));
    }
    for (d, c) in &mut cols {
        let got = c.len();
        let expected = meta.count(d.name.per(), n as u32).unwrap_or(usize::MAX);
        if got != expected {
            return Err(EncodeError::Length {
                name: d.name,
                expected,
                got,
            });
        }
        d.len = got as u32;
    }

    let mut codes = Vec::with_capacity(
        cols.iter()
            .map(|(d, _)| array_size(d.elem, d.len as usize))
            .sum(),
    );
    for (d, c) in &cols {
        match c {
            Col::F(v) => packing::put_f32(&mut codes, d.elem, d.unit == Unit::Deg, v),
            Col::Owned(v) => packing::put_f32(&mut codes, d.elem, d.unit == Unit::Deg, v),
            Col::OnMag(v, mag) => {
                let at = codes.len();
                packing::put_f32(&mut codes, d.elem, false, v);
                let mut m = Vec::with_capacity(2 * mag.len());
                packing::put_f32(&mut m, super::Elem::I16, false, mag);
                packing::to_mag_offsets(&mut codes[at..], &m);
            }
            Col::U(v) => packing::put_u32(&mut codes, v),
            Col::I(v) => packing::put_i8(&mut codes, v),
        }
    }
    Ok(CodedFrame {
        topic: frame.topic(),
        stamp: frame.stamp,
        n: n as u32,
        arrays: cols.iter().map(|(d, _)| *d).collect(),
        meta,
        codes,
    })
}

/// Encode an event as data-socket parts `[evt, body]`.
pub fn encode_event_message(e: &Event) -> Result<Vec<Vec<u8>>, EncodeError> {
    Ok(vec![Topic::Evt.to_bytes(), encode_event(e)?])
}
