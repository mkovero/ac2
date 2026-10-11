//! Frame encoding.

use crate::PROTO_VERSION;
use crate::event::{Event, encode_event};
use crate::packing::{self, array_size};
use crate::topic::Topic;

use super::flags::MaskBits;
use super::schema::{desc, level_unit, spec_unit};
use super::{
    ArrayDesc, ArrayName, EncodeError, Frame, FrameData, FrameHeader, FrameMeta, MAX_HEADER_BYTES,
    MAX_N, MAX_PARTS, Unit,
};

enum Col<'a> {
    F(&'a [f32]),
    U(Vec<u32>),
}

fn mask_slice<T: MaskBits>(v: &[T]) -> Vec<u32> {
    v.iter().map(T::to_u32).collect()
}

/// Encode a frame into its parts `[topic, header]` or `[topic, header, payload]`.
pub fn encode_frame(frame: &Frame) -> Result<Vec<Vec<u8>>, EncodeError> {
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
            FrameMeta::Tf(f.meta.clone())
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
        FrameData::SessionLevels(f) => {
            cols.push((desc(ArrayName::Peak, Unit::Dbfs), Col::F(&f.peak)));
            cols.push((desc(ArrayName::Rms, Unit::Dbfs), Col::F(&f.rms)));
            cols.push((
                desc(ArrayName::Clip, Unit::Bitmask),
                Col::U(mask_slice(&f.clip)),
            ));
            FrameMeta::SessionLevels(f.meta.clone())
        }
        FrameData::PreviewLevels(f) => {
            cols.push((desc(ArrayName::Peak, Unit::Dbfs), Col::F(&f.peak)));
            cols.push((desc(ArrayName::Rms, Unit::Dbfs), Col::F(&f.rms)));
            cols.push((
                desc(ArrayName::Clip, Unit::Bitmask),
                Col::U(mask_slice(&f.clip)),
            ));
            FrameMeta::PreviewLevels(f.meta.clone())
        }
        FrameData::Timing(m) => FrameMeta::Timing(*m),
        FrameData::Ka(m) => FrameMeta::Ka(m.clone()),
    };

    let n = match cols.first() {
        None => 0,
        Some((_, Col::F(v))) => v.len(),
        Some((_, Col::U(v))) => v.len(),
    };
    if n > MAX_N as usize {
        return Err(EncodeError::TooManyElements(n));
    }
    for (d, c) in &cols {
        let got = match c {
            Col::F(v) => v.len(),
            Col::U(v) => v.len(),
        };
        if got != n {
            return Err(EncodeError::Length {
                name: d.name,
                expected: n,
                got,
            });
        }
    }

    let s = &frame.stamp;
    let header = FrameHeader {
        v: PROTO_VERSION,
        kind: frame.data.kind(),
        seq: s.seq,
        audio_sample: s.audio_sample,
        session_epoch: s.session_epoch,
        daemon_incarnation: s.daemon_incarnation,
        config_rev: s.config_rev,
        config_applied_at: s.config_applied_at,
        capture_wall_ns: s.capture_wall_ns,
        grid_id: s.grid_id,
        protection: s.protection,
        n: n as u32,
        arrays: cols.iter().map(|(d, _)| *d).collect(),
        meta,
    };
    let hb = rmp_serde::to_vec(&header).map_err(|e| EncodeError::Msgpack(e.to_string()))?;
    if hb.len() > MAX_HEADER_BYTES {
        return Err(EncodeError::HeaderTooLarge(hb.len()));
    }

    let mut parts = Vec::with_capacity(MAX_PARTS);
    parts.push(frame.topic().to_bytes());
    parts.push(hb);
    if !cols.is_empty() {
        let mut raw = Vec::with_capacity(cols.iter().map(|(d, _)| array_size(d.elem, n)).sum());
        for (d, c) in &cols {
            match c {
                Col::F(v) => packing::put_f32(&mut raw, d.elem, d.unit == Unit::Deg, v),
                Col::U(v) => packing::put_u32(&mut raw, v),
            }
        }
        parts.push(packing::deflate(&raw));
    }
    Ok(parts)
}

/// Encode an event as data-socket parts `[evt, body]`.
pub fn encode_event_message(e: &Event) -> Result<Vec<Vec<u8>>, EncodeError> {
    Ok(vec![Topic::Evt.to_bytes(), encode_event(e)?])
}
