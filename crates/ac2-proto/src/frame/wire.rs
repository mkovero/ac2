//! The two wire forms of a frame, `[topic][body]`: a keyframe (the full header and the
//! intra payload) and a delta (a short header and the payload against the frame before).
//!
//! The body is the msgpack header followed by the deflated payload in the same part; a frame
//! without arrays has no payload bytes. The header's first element is `v`, its second the
//! [`Form`]. A delta inherits from its base everything but the per-frame fields: kind,
//! incarnation, epoch, config, grid, `n`, the arrays and the static metadata. A frame whose
//! static fields differ from the base's goes as a keyframe.
//!
//! Decoding validates every size before it parses anything: part count, total length, the
//! header (parsed from at most [`MAX_HEADER_BYTES`]), `n`, the array count, and the payload,
//! inflated to exactly the size the header (or the base) implies and no further. A
//! malformed frame is an error value, never a panic.

use std::io::Cursor;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::PROTO_VERSION;
use crate::grid::GridId;
use crate::packing::{self, array_size};
use crate::topic::Topic;
use crate::units::{DaemonIncarnation, Rev, SampleIndex, Seconds, SessionEpoch, WallNs};

use super::decode::topic_kind;
use super::{
    ArrayDesc, CodedFrame, DecodeError, EncodeError, Frame, FrameHeader, FrameKind, FrameMeta,
    MAX_ARRAYS, MAX_FRAME_BYTES, MAX_HEADER_BYTES, MAX_N, MAX_PARTS, ProtectionFlags, Unit,
};

/// Which of the two forms a body is; on the wire a small integer, the header's second
/// element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Form {
    /// The full header and the intra payload: needs no base.
    Key,
    /// The short header and the payload against the base frame of `seq` `base`.
    Delta,
}

impl Form {
    fn code(self) -> u8 {
        match self {
            Self::Key => 0,
            Self::Delta => 1,
        }
    }

    fn from_code(c: u8) -> Option<Self> {
        match c {
            0 => Some(Self::Key),
            1 => Some(Self::Delta),
            _ => None,
        }
    }
}

impl Serialize for Form {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u8(self.code())
    }
}

impl<'de> Deserialize<'de> for Form {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let c = u8::deserialize(d)?;
        Self::from_code(c).ok_or_else(|| serde::de::Error::custom(format!("form {c}")))
    }
}

/// The per-frame metadata of a `tf` delta.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TfPerFrame {
    filled: f32,
}

/// The per-frame metadata of an `spl` delta: its readings.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SplPerFrame {
    level: f64,
    lmax: f64,
    lmin: f64,
    leq: f64,
    lpeak: f64,
    duration: Seconds,
}

/// What a delta carries of the metadata, fixed per kind: `tf` its `filled`, `spl` its
/// readings, every other kind nothing (any change of its metadata goes as a keyframe).
#[derive(Debug, Clone, Copy, PartialEq)]
enum PerFrame {
    Tf(TfPerFrame),
    Spl(SplPerFrame),
    Static,
}

fn per_frame(m: &FrameMeta) -> PerFrame {
    match m {
        FrameMeta::Tf(t) => PerFrame::Tf(TfPerFrame { filled: t.filled }),
        FrameMeta::Spl(s) => PerFrame::Spl(SplPerFrame {
            level: s.level,
            lmax: s.lmax,
            lmin: s.lmin,
            leq: s.leq,
            lpeak: s.lpeak,
            duration: s.duration,
        }),
        _ => PerFrame::Static,
    }
}

/// `base` with the per-frame fields `p`; `None` if `p` is not of `base`'s kind.
fn with_per_frame(base: &FrameMeta, p: PerFrame) -> Option<FrameMeta> {
    let mut m = base.clone();
    match (&mut m, p) {
        (FrameMeta::Tf(t), PerFrame::Tf(p)) => t.filled = p.filled,
        (FrameMeta::Spl(s), PerFrame::Spl(p)) => {
            s.level = p.level;
            s.lmax = p.lmax;
            s.lmin = p.lmin;
            s.leq = p.leq;
            s.lpeak = p.lpeak;
            s.duration = p.duration;
        }
        (FrameMeta::Tf(_) | FrameMeta::Spl(_), _) | (_, PerFrame::Tf(_) | PerFrame::Spl(_)) => {
            return None;
        }
        (_, PerFrame::Static) => {}
    }
    Some(m)
}

/// Whether `f` can go as a delta on `base`: same topic and every static field the same.
/// The metadata compare by their msgpack bytes, so a NaN in a static field compares equal
/// to itself.
fn fits(f: &CodedFrame, base: &CodedFrame) -> bool {
    let (a, b) = (&f.stamp, &base.stamp);
    if f.topic != base.topic
        || a.session_epoch != b.session_epoch
        || a.daemon_incarnation != b.daemon_incarnation
        || a.config_rev != b.config_rev
        || a.config_applied_at != b.config_applied_at
        || a.grid_id != b.grid_id
        || f.n != base.n
        || f.arrays != base.arrays
        || f.codes.len() != base.codes.len()
    {
        return false;
    }
    let Some(as_base) = with_per_frame(&base.meta, per_frame(&f.meta)) else {
        return false;
    };
    match (rmp_serde::to_vec(&f.meta), rmp_serde::to_vec(&as_base)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// A keyframe header, borrowed for encoding; field for field [`FrameHeader`].
#[derive(Serialize)]
struct KeyHeaderRef<'a> {
    v: u16,
    form: Form,
    kind: FrameKind,
    seq: u64,
    audio_sample: SampleIndex,
    session_epoch: SessionEpoch,
    daemon_incarnation: DaemonIncarnation,
    config_rev: Rev,
    config_applied_at: SampleIndex,
    capture_wall_ns: WallNs,
    grid_id: Option<GridId>,
    protection: ProtectionFlags,
    n: u32,
    arrays: &'a [ArrayDesc],
    meta: &'a FrameMeta,
    crc: u32,
}

/// A delta's short header: a msgpack array of these fields in this order; `meta` is the
/// kind's per-frame metadata (nil for a kind without any).
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeltaHeader<M> {
    v: u16,
    form: Form,
    seq: u64,
    base: u64,
    audio_sample: SampleIndex,
    capture_wall_ns: WallNs,
    protection: ProtectionFlags,
    crc: u32,
    meta: M,
}

fn msgpack<T: Serialize>(x: &T) -> Result<Vec<u8>, EncodeError> {
    rmp_serde::to_vec(x).map_err(|e| EncodeError::Msgpack(e.to_string()))
}

/// `[topic, header ++ payload]`.
fn parts(f: &CodedFrame, mut body: Vec<u8>, payload: Option<Vec<u8>>) -> Vec<Vec<u8>> {
    if let Some(p) = payload {
        body.extend_from_slice(&p);
    }
    vec![f.topic.to_bytes(), body]
}

/// A keyframe of `f`: its parts `[topic, body]`.
pub fn encode_key(f: &CodedFrame) -> Result<Vec<Vec<u8>>, EncodeError> {
    let s = &f.stamp;
    let header = KeyHeaderRef {
        v: PROTO_VERSION,
        form: Form::Key,
        kind: f.kind(),
        seq: s.seq,
        audio_sample: s.audio_sample,
        session_epoch: s.session_epoch,
        daemon_incarnation: s.daemon_incarnation,
        config_rev: s.config_rev,
        config_applied_at: s.config_applied_at,
        capture_wall_ns: s.capture_wall_ns,
        grid_id: s.grid_id,
        protection: s.protection,
        n: f.n,
        arrays: &f.arrays,
        meta: &f.meta,
        crc: f.crc(),
    };
    let hb = msgpack(&header)?;
    if hb.len() > MAX_HEADER_BYTES {
        return Err(EncodeError::HeaderTooLarge(hb.len()));
    }
    let payload = (!f.arrays.is_empty()).then(|| {
        let mut raw = Vec::with_capacity(f.codes.len());
        let mut at = 0;
        for d in &f.arrays {
            let k = array_size(d.elem, f.n as usize);
            packing::put_intra(&mut raw, d.elem, &f.codes[at..at + k]);
            at += k;
        }
        packing::deflate(&raw)
    });
    Ok(parts(f, hb, payload))
}

/// A delta of `f` on `base`; `None` if `base` does not fit (another topic, `n`, grid, array
/// set, epoch or static field), so `f` must go as a keyframe.
pub fn encode_delta(
    f: &CodedFrame,
    base: &CodedFrame,
) -> Result<Option<Vec<Vec<u8>>>, EncodeError> {
    if !fits(f, base) {
        return Ok(None);
    }
    let mut raw = Vec::with_capacity(f.codes.len());
    let mut at = 0;
    for d in &f.arrays {
        let k = array_size(d.elem, f.n as usize);
        let deg = d.unit == Unit::Deg;
        if !packing::put_delta(
            &mut raw,
            d.elem,
            deg,
            &f.codes[at..at + k],
            &base.codes[at..at + k],
        ) {
            return Ok(None);
        }
        at += k;
    }
    let s = &f.stamp;
    let head = DeltaHeader {
        v: PROTO_VERSION,
        form: Form::Delta,
        seq: s.seq,
        base: base.stamp.seq,
        audio_sample: s.audio_sample,
        capture_wall_ns: s.capture_wall_ns,
        protection: s.protection,
        crc: f.crc(),
        meta: (),
    };
    let hb = match per_frame(&f.meta) {
        PerFrame::Tf(p) => msgpack(&head.with(p))?,
        PerFrame::Spl(p) => msgpack(&head.with(p))?,
        PerFrame::Static => msgpack(&head)?,
    };
    let payload = (!f.arrays.is_empty()).then(|| packing::deflate(&raw));
    Ok(Some(parts(f, hb, payload)))
}

impl<M> DeltaHeader<M> {
    fn with<N>(self, meta: N) -> DeltaHeader<N> {
        DeltaHeader {
            v: self.v,
            form: self.form,
            seq: self.seq,
            base: self.base,
            audio_sample: self.audio_sample,
            capture_wall_ns: self.capture_wall_ns,
            protection: self.protection,
            crc: self.crc,
            meta,
        }
    }
}

/// A delta, decoded as far as it can be without its base.
#[derive(Debug, Clone, PartialEq)]
pub struct DeltaFrame {
    /// Topic.
    pub topic: Topic,
    /// This frame's `seq`.
    pub seq: u64,
    /// The `seq` of the frame it applies on.
    pub base: u64,
    audio_sample: SampleIndex,
    capture_wall_ns: WallNs,
    protection: ProtectionFlags,
    crc: u32,
    per_frame: PerFrame,
    /// Deflated delta payload; sized by the base.
    payload: Vec<u8>,
}

impl DeltaFrame {
    /// The frame this delta makes of `base`: refused unless `base` is the frame of `seq`
    /// [`DeltaFrame::base`] on this topic and the result's codes match the CRC.
    pub fn apply(&self, base: &CodedFrame) -> Result<CodedFrame, DecodeError> {
        if base.topic != self.topic || base.stamp.seq != self.base {
            return Err(DecodeError::WrongBase {
                held: base.stamp.seq,
                base: self.base,
            });
        }
        let expected = base.codes.len();
        let raw = if base.arrays.is_empty() {
            if !self.payload.is_empty() {
                return Err(DecodeError::Payload { expected: 0 });
            }
            Vec::new()
        } else {
            packing::inflate(&self.payload, expected).ok_or(DecodeError::Payload { expected })?
        };
        let mut codes = Vec::with_capacity(expected);
        let mut at = 0;
        for d in &base.arrays {
            let k = array_size(d.elem, base.n as usize);
            packing::get_delta(
                &mut codes,
                d.elem,
                d.unit == Unit::Deg,
                &raw[at..at + k],
                &base.codes[at..at + k],
            );
            at += k;
        }
        let got = crc32fast::hash(&codes);
        if got != self.crc {
            return Err(DecodeError::Crc {
                expected: self.crc,
                got,
            });
        }
        let meta = with_per_frame(&base.meta, self.per_frame).ok_or(DecodeError::KindMismatch)?;
        let mut stamp = base.stamp;
        stamp.seq = self.seq;
        stamp.audio_sample = self.audio_sample;
        stamp.capture_wall_ns = self.capture_wall_ns;
        stamp.protection = self.protection;
        Ok(CodedFrame {
            topic: base.topic,
            stamp,
            n: base.n,
            arrays: base.arrays.clone(),
            meta,
            codes,
        })
    }
}

/// A frame as received: a keyframe whole, or a delta that needs its base.
#[derive(Debug, Clone, PartialEq)]
#[expect(
    clippy::large_enum_variant,
    reason = "moved once, from the decoder into the frame held; a box would cost an allocation per keyframe"
)]
pub enum WireFrame {
    /// Keyframe, CRC checked.
    Key(CodedFrame),
    /// Delta.
    Delta(DeltaFrame),
}

impl WireFrame {
    /// Topic.
    pub fn topic(&self) -> Topic {
        match self {
            Self::Key(k) => k.topic,
            Self::Delta(d) => d.topic,
        }
    }

    /// `seq`.
    pub fn seq(&self) -> u64 {
        match self {
            Self::Key(k) => k.stamp.seq,
            Self::Delta(d) => d.seq,
        }
    }

    /// The frame this makes, given the frame of the topic the receiver holds.
    pub fn apply(self, held: Option<&CodedFrame>) -> Result<CodedFrame, DecodeError> {
        match self {
            Self::Key(k) => Ok(k),
            Self::Delta(d) => d.apply(held.ok_or(DecodeError::NoBase)?),
        }
    }
}

/// An element of a header array after the array marker (fixarray, array 16 or array 32):
/// element `i` (0 or 1) as a positive fixint, uint 8 or uint 16. `None` if the bytes are not
/// that.
fn header_uint(h: &[u8], i: usize) -> Option<u16> {
    let skip = match *h.first()? {
        0x90..=0x9f => 1,
        0xdc => 3,
        0xdd => 5,
        _ => return None,
    };
    let mut v = h.get(skip..)?;
    for at in 0..=i {
        let (x, len) = match *v.first()? {
            x @ 0x00..=0x7f => (u16::from(x), 1),
            0xcc => (u16::from(*v.get(1)?), 2),
            0xcd => (u16::from_be_bytes([*v.get(1)?, *v.get(2)?]), 3),
            _ => return None,
        };
        if at == i {
            return Some(x);
        }
        v = v.get(len..)?;
    }
    None
}

/// A header of type `T` from the front of `body`, and the bytes after it.
fn header<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<(T, &[u8]), DecodeError> {
    let head = &body[..body.len().min(MAX_HEADER_BYTES)];
    let mut de = rmp_serde::Deserializer::new(Cursor::new(head));
    let h = T::deserialize(&mut de).map_err(|e| {
        use rmp_serde::decode::Error as E;
        let eof = matches!(&e, E::InvalidMarkerRead(io) | E::InvalidDataRead(io)
            if io.kind() == std::io::ErrorKind::UnexpectedEof);
        // Bytes ran out at the bound, not at the end of the body: the header is longer.
        if eof && head.len() < body.len() {
            DecodeError::HeaderTooLarge(body.len())
        } else {
            DecodeError::Header(e.to_string())
        }
    })?;
    let at = usize::try_from(de.position())
        .unwrap_or(usize::MAX)
        .min(body.len());
    Ok((h, &body[at..]))
}

/// Decode a data frame's parts into its form. Sizes are validated before anything is parsed;
/// a keyframe's CRC is checked here, a delta's when it is applied.
pub fn decode_wire(parts: &[&[u8]]) -> Result<WireFrame, DecodeError> {
    if parts.len() != MAX_PARTS {
        return Err(DecodeError::Parts(parts.len()));
    }
    let total = parts.iter().fold(0usize, |a, p| a.saturating_add(p.len()));
    if total > MAX_FRAME_BYTES {
        return Err(DecodeError::TooLarge(total));
    }
    let topic = Topic::parse(parts[0])?;
    let kind = topic_kind(topic).ok_or(DecodeError::KindMismatch)?;
    let body = parts[1];
    // The version comes first and is read on its own: another version may lay the rest of
    // the header out differently, and must be refused as such, not as malformed.
    if let Some(theirs) = header_uint(body, 0)
        && theirs != PROTO_VERSION
    {
        return Err(DecodeError::VersionMismatch {
            ours: PROTO_VERSION,
            theirs,
        });
    }
    let form = header_uint(body, 1)
        .and_then(|c| u8::try_from(c).ok())
        .and_then(Form::from_code)
        .ok_or_else(|| DecodeError::Header("no form".into()))?;
    match form {
        Form::Key => decode_key(topic, kind, body).map(WireFrame::Key),
        Form::Delta => decode_delta(topic, kind, body).map(WireFrame::Delta),
    }
}

fn version(v: u16) -> Result<(), DecodeError> {
    if v == PROTO_VERSION {
        Ok(())
    } else {
        Err(DecodeError::VersionMismatch {
            ours: PROTO_VERSION,
            theirs: v,
        })
    }
}

fn decode_key(topic: Topic, kind: FrameKind, body: &[u8]) -> Result<CodedFrame, DecodeError> {
    let (h, payload): (FrameHeader, _) = header(body)?;
    version(h.v)?;
    if h.n > MAX_N {
        return Err(DecodeError::TooManyElements(h.n));
    }
    if h.arrays.len() > MAX_ARRAYS || h.arrays.is_empty() != payload.is_empty() {
        return Err(DecodeError::ArrayCount {
            listed: h.arrays.len(),
            payload: payload.len(),
        });
    }
    if h.form != Form::Key || h.kind != kind || h.meta.kind() != kind {
        return Err(DecodeError::KindMismatch);
    }
    let sizes: Vec<usize> = h
        .arrays
        .iter()
        .map(|d| array_size(d.elem, h.n as usize))
        .collect();
    let expected = sizes.iter().sum();
    let mut codes = Vec::with_capacity(expected);
    if !h.arrays.is_empty() {
        let raw = packing::inflate(payload, expected).ok_or(DecodeError::Payload { expected })?;
        let mut at = 0;
        for (d, k) in h.arrays.iter().zip(&sizes) {
            packing::get_intra(&mut codes, d.elem, &raw[at..at + k]);
            at += k;
        }
    }
    let got = crc32fast::hash(&codes);
    if got != h.crc {
        return Err(DecodeError::Crc {
            expected: h.crc,
            got,
        });
    }
    Ok(CodedFrame {
        topic,
        stamp: super::FrameStamp {
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
        n: h.n,
        arrays: h.arrays,
        meta: h.meta,
        codes,
    })
}

fn decode_delta(topic: Topic, kind: FrameKind, body: &[u8]) -> Result<DeltaFrame, DecodeError> {
    let (h, per_frame, payload) = match kind {
        FrameKind::Tf => {
            let (h, p): (DeltaHeader<TfPerFrame>, _) = header(body)?;
            let m = PerFrame::Tf(h.meta);
            (h.with(()), m, p)
        }
        FrameKind::Spl => {
            let (h, p): (DeltaHeader<SplPerFrame>, _) = header(body)?;
            let m = PerFrame::Spl(h.meta);
            (h.with(()), m, p)
        }
        _ => {
            let (h, p): (DeltaHeader<()>, _) = header(body)?;
            (h, PerFrame::Static, p)
        }
    };
    version(h.v)?;
    if h.form != Form::Delta {
        return Err(DecodeError::KindMismatch);
    }
    Ok(DeltaFrame {
        topic,
        seq: h.seq,
        base: h.base,
        audio_sample: h.audio_sample,
        capture_wall_ns: h.capture_wall_ns,
        protection: h.protection,
        crc: h.crc,
        per_frame,
        payload: payload.to_vec(),
    })
}

/// Decode a keyframe into its values; a delta is refused ([`DecodeError::NoBase`]), as it
/// needs the frame before it ([`decode_wire`]).
pub fn decode_frame(parts: &[&[u8]]) -> Result<Frame, DecodeError> {
    match decode_wire(parts)? {
        WireFrame::Key(k) => k.to_frame(),
        WireFrame::Delta(_) => Err(DecodeError::NoBase),
    }
}
