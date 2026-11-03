//! A frame's codes back to its values, against the kind's schema.

use crate::event::decode_event;
use crate::packing::{self, array_size};
use crate::topic::{Stream, Topic};
use crate::units::MeasId;

use super::flags::MaskBits;
use super::schema::{desc, level_unit, spec_unit};
use super::{
    ArrayDesc, ArrayName, BandLeqFrame, CodedFrame, DataMessage, DecodeError, Frame, FrameData,
    FrameKind, FrameMeta, GONIO_POINTS, GonioFrame, GonioPair, IrFrame, LeqFrame, LevelsFrame,
    PreviewLevelsFrame, RtaFrame, SessionLevelsFrame, SpecFrame, SplFrame, TfFrame, Unit,
};

/// An alignment's four view curves: all of them or none.
fn align_curves(a: &mut Arrays<'_>) -> Result<Option<crate::model::AlignCurves>, DecodeError> {
    let got = [
        a.opt_f32(ArrayName::AlignPhaseBefore, Unit::Deg)?,
        a.opt_f32(ArrayName::AlignPhaseAfter, Unit::Deg)?,
        a.opt_f32(ArrayName::AlignSumBefore, Unit::Db)?,
        a.opt_f32(ArrayName::AlignSumAfter, Unit::Db)?,
    ];
    match got {
        [None, None, None, None] => Ok(None),
        [
            Some(phase_before),
            Some(phase_after),
            Some(sum_before),
            Some(sum_after),
        ] => Ok(Some(crate::model::AlignCurves {
            phase_before,
            phase_after,
            sum_before,
            sum_after,
        })),
        _ => Err(DecodeError::Schema(
            "tf: an alignment's curves come all four or none".into(),
        )),
    }
}

/// An average's range band: all three arrays or none; its edges on `mag`'s codes.
fn range_band(
    a: &mut Arrays<'_>,
    mag: &[u8],
) -> Result<Option<crate::model::RangeBand>, DecodeError> {
    match [
        a.opt_f32_on_mag(ArrayName::RangeLo, mag)?,
        a.opt_f32_on_mag(ArrayName::RangeHi, mag)?,
        a.opt_f32(ArrayName::RangeCoh, Unit::Coherence)?,
    ] {
        [None, None, None] => Ok(None),
        [Some(lo), Some(hi), Some(coh)] => Ok(Some(crate::model::RangeBand { lo, hi, coh })),
        _ => Err(DecodeError::Schema(
            "tf: a range band's arrays come all three or none".into(),
        )),
    }
}

/// A goniometer frame: `n` is the pairs' points in turn, split back per pair.
fn gonio(a: &mut Arrays<'_>, meta: super::GonioMeta, n: u32) -> Result<GonioFrame, DecodeError> {
    if meta.pairs.len() * GONIO_POINTS != n as usize {
        return Err(DecodeError::Schema(
            "gonio: n != pairs × GONIO_POINTS".into(),
        ));
    }
    let x = a.gonio(ArrayName::GonioX)?;
    let y = a.gonio(ArrayName::GonioY)?;
    let pairs = meta
        .pairs
        .into_iter()
        .enumerate()
        .map(|(i, p)| {
            let at = i * GONIO_POINTS..(i + 1) * GONIO_POINTS;
            GonioPair {
                left: p.left,
                right: p.right,
                gain_dbfs: p.gain_dbfs,
                points: x[at.clone()]
                    .iter()
                    .zip(&y[at])
                    .map(|(&x, &y)| [x, y])
                    .collect(),
            }
        })
        .collect();
    Ok(GonioFrame { pairs })
}

struct Arrays<'a> {
    descs: &'a [ArrayDesc],
    /// Each array's bytes in the inflated payload.
    parts: Vec<&'a [u8]>,
    used: Vec<bool>,
}

impl<'a> Arrays<'a> {
    fn find(&mut self, name: ArrayName, unit: Unit) -> Result<Option<&'a [u8]>, DecodeError> {
        let mut hit = None;
        for (i, d) in self.descs.iter().enumerate() {
            if d.name == name {
                if hit.is_some() {
                    return Err(DecodeError::Schema(format!("duplicate {name:?}")));
                }
                let elem = desc(name, unit).elem;
                if d.unit != unit || d.elem != elem {
                    return Err(DecodeError::Schema(format!(
                        "{name:?} must be {unit:?}/{elem:?}, got {:?}/{:?}",
                        d.unit, d.elem
                    )));
                }
                hit = Some(i);
            }
        }
        Ok(hit.map(|i| {
            self.used[i] = true;
            self.parts[i]
        }))
    }

    fn f32(&mut self, name: ArrayName, unit: Unit) -> Result<Vec<f32>, DecodeError> {
        self.opt_f32(name, unit)?
            .ok_or_else(|| DecodeError::Schema(format!("missing {name:?}")))
    }

    fn opt_f32(&mut self, name: ArrayName, unit: Unit) -> Result<Option<Vec<f32>>, DecodeError> {
        let elem = desc(name, unit).elem;
        self.find(name, unit)?
            .map(|b| packing::get_f32(b, elem).ok_or(DecodeError::UndefinedCode(name)))
            .transpose()
    }

    /// An `i16_mag_offset` array in dB, on `mag`'s codes (the same length: every array is
    /// `n` elements).
    fn opt_f32_on_mag(
        &mut self,
        name: ArrayName,
        mag: &[u8],
    ) -> Result<Option<Vec<f32>>, DecodeError> {
        let elem = desc(name, Unit::Db).elem;
        self.find(name, Unit::Db)?
            .map(|b| {
                let mut codes = b.to_vec();
                packing::from_mag_offsets(&mut codes, mag);
                packing::get_f32(&codes, elem).ok_or(DecodeError::UndefinedCode(name))
            })
            .transpose()
    }

    /// The codes of `name`'s array, not marked used.
    fn codes(&self, name: ArrayName) -> Option<&'a [u8]> {
        let i = self.descs.iter().position(|d| d.name == name)?;
        Some(self.parts[i])
    }

    fn mask<T: MaskBits>(&mut self, name: ArrayName) -> Result<Vec<T>, DecodeError> {
        let b = self
            .find(name, Unit::Bitmask)?
            .ok_or_else(|| DecodeError::Schema(format!("missing {name:?}")))?;
        let v: Vec<T> = packing::get_u32(b).into_iter().map(T::from_u32).collect();
        if v.iter().all(T::known) {
            Ok(v)
        } else {
            Err(DecodeError::UnknownBits(name))
        }
    }

    /// A count array (`u32` elements, no bit meaning).
    fn counts(&mut self, name: ArrayName) -> Result<Vec<u32>, DecodeError> {
        let b = self
            .find(name, Unit::Count)?
            .ok_or_else(|| DecodeError::Schema(format!("missing {name:?}")))?;
        Ok(packing::get_u32(b))
    }

    /// A goniometer coordinate array.
    fn gonio(&mut self, name: ArrayName) -> Result<Vec<i8>, DecodeError> {
        let b = self
            .find(name, Unit::GonioUnit)?
            .ok_or_else(|| DecodeError::Schema(format!("missing {name:?}")))?;
        Ok(packing::get_i8(b))
    }

    fn finish(self) -> Result<(), DecodeError> {
        match self.used.iter().position(|u| !u) {
            None => Ok(()),
            Some(i) => Err(DecodeError::Schema(format!(
                "unexpected array {:?}",
                self.descs[i].name
            ))),
        }
    }
}

pub(super) fn stream_kind(s: Stream) -> FrameKind {
    match s {
        Stream::Tf => FrameKind::Tf,
        Stream::Ir => FrameKind::Ir,
        Stream::Rta => FrameKind::Rta,
        Stream::Spec => FrameKind::Spec,
        Stream::Spl => FrameKind::Spl,
        Stream::Leq => FrameKind::Leq,
        Stream::BandLeq => FrameKind::BandLeq,
        Stream::Levels => FrameKind::Levels,
    }
}

/// The kind a topic carries; `None` for `evt`.
pub(super) fn topic_kind(topic: Topic) -> Option<FrameKind> {
    Some(match topic {
        Topic::Data { stream, .. } => stream_kind(stream),
        Topic::SessionLevels => FrameKind::SessionLevels,
        Topic::PreviewLevels => FrameKind::PreviewLevels,
        Topic::SessionGonio => FrameKind::SessionGonio,
        Topic::PreviewGonio => FrameKind::PreviewGonio,
        Topic::Timing => FrameKind::Timing,
        Topic::Ka => FrameKind::Ka,
        Topic::Evt => return None,
    })
}

/// The values of a frame from its codes. The codes' size was checked against `n` and the
/// array descriptors when they were inflated or applied.
pub(super) fn to_frame(c: &CodedFrame) -> Result<Frame, DecodeError> {
    let topic = c.topic;
    if topic_kind(topic) != Some(c.meta.kind()) {
        return Err(DecodeError::KindMismatch);
    }
    if !c.stamp.protection.is_known() {
        return Err(DecodeError::Schema("undefined protection bits".into()));
    }
    let sizes: Vec<usize> = c
        .arrays
        .iter()
        .map(|d| array_size(d.elem, c.n as usize))
        .collect();
    if sizes.iter().sum::<usize>() != c.codes.len() {
        return Err(DecodeError::Payload {
            expected: sizes.iter().sum(),
        });
    }
    let h = c;
    let meas = match topic {
        Topic::Data { meas, .. } => meas,
        _ => MeasId(0),
    };
    let mut rest = c.codes.as_slice();
    let mut a = Arrays {
        descs: &h.arrays,
        parts: sizes
            .iter()
            .map(|k| {
                let (this, more) = rest.split_at(*k);
                rest = more;
                this
            })
            .collect(),
        used: vec![false; h.arrays.len()],
    };
    let mag_codes = a.codes(ArrayName::Mag).unwrap_or_default();
    let data = match h.meta.clone() {
        FrameMeta::Tf(meta) => FrameData::Tf(TfFrame {
            meas,
            meta,
            // `mag` is checked first, so its codes under the range band are `i16`.
            mag: a.f32(ArrayName::Mag, Unit::Db)?,
            phase: a.f32(ArrayName::Phase, Unit::Deg)?,
            coh: a.f32(ArrayName::Coh, Unit::Coherence)?,
            validity: a.mask(ArrayName::Validity)?,
            align: align_curves(&mut a)?,
            range: range_band(&mut a, mag_codes)?,
        }),
        FrameMeta::Ir(meta) => FrameData::Ir(IrFrame {
            meas,
            meta,
            linear: a.f32(ArrayName::IrLinear, Unit::FullScale)?,
            etc: a.opt_f32(ArrayName::IrEtc, Unit::Db)?,
        }),
        FrameMeta::Rta(meta) => FrameData::Rta(RtaFrame {
            meas,
            level: a.f32(ArrayName::Level, level_unit(meta.scale))?,
            validity: a.mask(ArrayName::Validity)?,
            meta,
        }),
        FrameMeta::Spec(meta) => FrameData::Spec(SpecFrame {
            meas,
            level: a.f32(ArrayName::Level, spec_unit(meta.scale, meta.level))?,
            meta,
        }),
        FrameMeta::Spl(meta) => FrameData::Spl(SplFrame { meas, meta }),
        FrameMeta::BandLeq(meta) => {
            if meta
                .windows
                .iter()
                .any(|w| crate::model::band_index(w.band.0).is_none())
                || meta.windows.len() != h.n as usize
            {
                return Err(DecodeError::Schema(
                    "band_leq: not one column per window of one band".into(),
                ));
            }
            let unit = level_unit(meta.scale);
            FrameData::BandLeq(Box::new(BandLeqFrame {
                meas,
                leq: a.f32(ArrayName::Leq, unit)?,
                limit: a.f32(ArrayName::Limit, unit)?,
                allowed: a.f32(ArrayName::Allowed, unit)?,
                recover: a.f32(ArrayName::Recover, Unit::Seconds)?,
                flags: a.mask(ArrayName::LeqFlags)?,
                meta,
            }))
        }
        FrameMeta::Leq(meta) => {
            let unit = level_unit(meta.scale);
            FrameData::Leq(Box::new(LeqFrame {
                meas,
                meta,
                leq: a.f32(ArrayName::Leq, unit)?,
                elapsed: a.f32(ArrayName::Elapsed, Unit::Seconds)?,
                measured: a.f32(ArrayName::Measured, Unit::Seconds)?,
                allowed: a.f32(ArrayName::Allowed, unit)?,
                recover: a.f32(ArrayName::Recover, Unit::Seconds)?,
                least: a.f32(ArrayName::Least, unit)?,
                over_in: a.f32(ArrayName::OverIn, Unit::Seconds)?,
                flags: a.mask(ArrayName::LeqFlags)?,
            }))
        }
        FrameMeta::Levels(meta) => {
            if meta.channels.len() != h.n as usize {
                return Err(DecodeError::Schema("levels: channels.len() != n".into()));
            }
            FrameData::Levels(LevelsFrame {
                meas,
                meta,
                peak: a.f32(ArrayName::Peak, Unit::Dbfs)?,
                rms: a.f32(ArrayName::Rms, Unit::Dbfs)?,
                clip: a.mask(ArrayName::Clip)?,
            })
        }
        FrameMeta::SessionLevels(meta) => {
            if meta.channels.len() != h.n as usize {
                return Err(DecodeError::Schema(
                    "session levels: channels.len() != n".into(),
                ));
            }
            FrameData::SessionLevels(SessionLevelsFrame {
                meta,
                peak: a.f32(ArrayName::Peak, Unit::Dbfs)?,
                rms: a.f32(ArrayName::Rms, Unit::Dbfs)?,
                clip: a.mask(ArrayName::Clip)?,
                ppm: a.f32(ArrayName::Ppm, Unit::Dbfs)?,
                true_peak: a.f32(ArrayName::TruePeak, Unit::Dbtp)?,
                max: a.f32(ArrayName::Max, Unit::Dbfs)?,
                overs: a.counts(ArrayName::Overs)?,
            })
        }
        FrameMeta::PreviewLevels(meta) => {
            if meta.channels.len() != h.n as usize {
                return Err(DecodeError::Schema(
                    "preview levels: channels.len() != n".into(),
                ));
            }
            FrameData::PreviewLevels(PreviewLevelsFrame {
                meta,
                peak: a.f32(ArrayName::Peak, Unit::Dbfs)?,
                rms: a.f32(ArrayName::Rms, Unit::Dbfs)?,
                clip: a.mask(ArrayName::Clip)?,
                ppm: a.f32(ArrayName::Ppm, Unit::Dbfs)?,
                true_peak: a.f32(ArrayName::TruePeak, Unit::Dbtp)?,
                max: a.f32(ArrayName::Max, Unit::Dbfs)?,
                overs: a.counts(ArrayName::Overs)?,
            })
        }
        FrameMeta::SessionGonio(meta) => FrameData::SessionGonio(gonio(&mut a, meta, h.n)?),
        FrameMeta::PreviewGonio(meta) => FrameData::PreviewGonio(gonio(&mut a, meta, h.n)?),
        FrameMeta::Timing(m) => FrameData::Timing(m),
        FrameMeta::Ka(m) => FrameData::Ka(m),
    };
    a.finish()?;
    Ok(Frame {
        stamp: c.stamp,
        data,
    })
}

/// Decode anything received on the data socket: `evt` → event, otherwise a frame.
pub fn decode_data_message(parts: &[&[u8]]) -> Result<DataMessage, DecodeError> {
    match parts.first().map(|t| Topic::parse(t)) {
        Some(Ok(Topic::Evt)) => {
            if parts.len() != 2 {
                return Err(DecodeError::Parts(parts.len()));
            }
            Ok(DataMessage::Event(decode_event(parts[1])?))
        }
        _ => super::wire::decode_frame(parts).map(DataMessage::Frame),
    }
}
