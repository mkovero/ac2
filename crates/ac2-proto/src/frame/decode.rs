//! Frame decoding, every size validated before parsing.

use crate::PROTO_VERSION;
use crate::event::decode_event;
use crate::packing::{self, array_size};
use crate::topic::{Stream, Topic};
use crate::units::MeasId;

use super::flags::MaskBits;
use super::schema::{desc, level_unit, spec_unit};
use super::{
    ArrayDesc, ArrayName, BandLeqFrame, DataMessage, DecodeError, Frame, FrameData, FrameHeader,
    FrameKind, FrameMeta, FrameStamp, IrFrame, LeqFrame, LevelsFrame, MAX_ARRAYS, MAX_FRAME_BYTES,
    MAX_HEADER_BYTES, MAX_N, MAX_PARTS, PreviewLevelsFrame, RtaFrame, SessionLevelsFrame,
    SpecFrame, SplFrame, TfFrame, Unit,
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

fn stream_kind(s: Stream) -> FrameKind {
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

/// `v`, the first element of a header array: after the array marker (fixarray, array 16 or
/// array 32), a positive fixint, uint 8 or uint 16. `None` if the bytes are not that.
fn header_version(h: &[u8]) -> Option<u16> {
    let skip = match *h.first()? {
        0x90..=0x9f => 1,
        0xdc => 3,
        0xdd => 5,
        _ => return None,
    };
    let v = h.get(skip..)?;
    match *v.first()? {
        x @ 0x00..=0x7f => Some(u16::from(x)),
        0xcc => v.get(1).map(|x| u16::from(*x)),
        0xcd => Some(u16::from_be_bytes([*v.get(1)?, *v.get(2)?])),
        _ => None,
    }
}

/// Decode a data frame from its parts. Sizes are validated before anything is parsed.
pub fn decode_frame(parts: &[&[u8]]) -> Result<Frame, DecodeError> {
    if parts.len() < 2 || parts.len() > MAX_PARTS {
        return Err(DecodeError::Parts(parts.len()));
    }
    let total = parts.iter().fold(0usize, |a, p| a.saturating_add(p.len()));
    if total > MAX_FRAME_BYTES {
        return Err(DecodeError::TooLarge(total));
    }
    let topic = Topic::parse(parts[0])?;
    if parts[1].len() > MAX_HEADER_BYTES {
        return Err(DecodeError::HeaderTooLarge(parts[1].len()));
    }
    // The version comes first and is read on its own: another version may lay the rest of
    // the header out differently, and must be refused as such, not as malformed.
    if let Some(theirs) = header_version(parts[1])
        && theirs != PROTO_VERSION
    {
        return Err(DecodeError::VersionMismatch {
            ours: PROTO_VERSION,
            theirs,
        });
    }
    let h: FrameHeader =
        rmp_serde::from_slice(parts[1]).map_err(|e| DecodeError::Header(e.to_string()))?;
    if h.v != PROTO_VERSION {
        return Err(DecodeError::VersionMismatch {
            ours: PROTO_VERSION,
            theirs: h.v,
        });
    }
    if h.n > MAX_N {
        return Err(DecodeError::TooManyElements(h.n));
    }
    let payload = parts.get(2);
    if h.arrays.len() > MAX_ARRAYS || h.arrays.is_empty() != payload.is_none() {
        return Err(DecodeError::ArrayCount {
            listed: h.arrays.len(),
            parts: parts.len(),
        });
    }
    let sizes: Vec<usize> = h
        .arrays
        .iter()
        .map(|d| array_size(d.elem, h.n as usize))
        .collect();
    let expected = sizes.iter().sum();
    let raw = match payload {
        Some(p) => packing::inflate(p, expected).ok_or(DecodeError::Payload { expected })?,
        None => Vec::new(),
    };
    let topic_kind = match topic {
        Topic::Data { stream, .. } => stream_kind(stream),
        Topic::SessionLevels => FrameKind::SessionLevels,
        Topic::PreviewLevels => FrameKind::PreviewLevels,
        Topic::Timing => FrameKind::Timing,
        Topic::Ka => FrameKind::Ka,
        Topic::Evt => return Err(DecodeError::KindMismatch),
    };
    if h.kind != topic_kind || h.meta.kind() != h.kind {
        return Err(DecodeError::KindMismatch);
    }
    if !h.protection.is_known() {
        return Err(DecodeError::Schema("undefined protection bits".into()));
    }

    let meas = match topic {
        Topic::Data { meas, .. } => meas,
        _ => MeasId(0),
    };
    let mut rest = raw.as_slice();
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
    let data = match h.meta {
        FrameMeta::Tf(meta) => FrameData::Tf(TfFrame {
            meas,
            meta,
            mag: a.f32(ArrayName::Mag, Unit::Db)?,
            phase: a.f32(ArrayName::Phase, Unit::Deg)?,
            coh: a.f32(ArrayName::Coh, Unit::Coherence)?,
            validity: a.mask(ArrayName::Validity)?,
            align: align_curves(&mut a)?,
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
            })
        }
        FrameMeta::Timing(m) => FrameData::Timing(m),
        FrameMeta::Ka(m) => FrameData::Ka(m),
    };
    a.finish()?;
    Ok(Frame {
        stamp: FrameStamp {
            seq: h.seq,
            audio_sample: h.audio_sample,
            session_epoch: h.session_epoch,
            daemon_incarnation: h.daemon_incarnation,
            config_rev: h.config_rev,
            config_applied_at: h.config_applied_at,
            capture_wall_ns: h.capture_wall_ns,
            grid_id: h.grid_id,
            protection: h.protection,
        },
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
        _ => decode_frame(parts).map(DataMessage::Frame),
    }
}
