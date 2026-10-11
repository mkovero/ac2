//! Array payload of a data frame: each array in its element encoding, concatenated in the
//! header's order, then deflated (RFC 1951, raw) into one part.
//!
//! Precision budget of the 16-bit encodings, against what a display shows of them:
//!
//! - `i16`: hundredths of the array's unit (dB, dBFS, dB SPL, degrees), so at most
//!   0.005 dB or 0.005° off. Level readouts show 0.1 dB and phase readouts whole degrees;
//!   a group delay fitted to the phase over even the narrowest low-frequency span moves by
//!   well under its 0.01 ms readout step. Degrees are wrapped to [−180°, 180°), which a
//!   transfer function's phase already is. Range ±327.66; a level beyond it is clamped (a
//!   meter reads nothing below −300 dB), NaN and ±∞ have codes of their own.
//! - `u16`: coherence γ² in ten-thousandths (0 … 10 000), at most 5·10⁻⁵ off; its readout
//!   shows hundredths. NaN has a code of its own; other codes above 10 000 are refused.
//!
//! A 16-bit array goes out as the differences between neighbouring columns (wrapping), low
//! bytes first, then high bytes: neighbouring columns of a curve are close, so most high
//! bytes are 0 or 0xff and deflate takes them to almost nothing.
//!
//! `f32` and `u32` arrays (meters, Leq windows, IR samples, bitmasks) go as they are,
//! little-endian: they are small or must stay exact, and deflate still takes out runs such
//! as all-zero bitmasks.

use crate::frame::Elem;

/// Deflate level: the fastest that still finds the runs the 16-bit planes are made of;
/// higher levels gain about 1 % on frame data at several times the CPU.
const LEVEL: u8 = 1;

/// `i16` code of NaN.
pub(crate) const I16_NAN: i16 = i16::MIN;
/// `i16` code of −∞.
pub(crate) const I16_NEG_INF: i16 = i16::MIN + 1;
/// `i16` code of +∞.
pub(crate) const I16_POS_INF: i16 = i16::MAX;
/// Largest finite `i16` code.
const I16_MAX_FINITE: f64 = (i16::MAX - 1) as f64;
/// `u16` code of NaN.
pub(crate) const U16_NAN: u16 = u16::MAX;
/// `u16` code of 1.
pub(crate) const U16_ONE: u16 = 10_000;

/// Hundredths per unit of an `i16` array.
const I16_PER_UNIT: f64 = 100.0;

/// Bytes per element.
pub(crate) fn elem_size(e: Elem) -> usize {
    match e {
        Elem::F32 | Elem::U32 => 4,
        Elem::I16 | Elem::U16 => 2,
    }
}

/// Round half up: the same in every language (Python's `round` is half-to-even).
fn round_half_up(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// `i16` code of an angle in degrees, wrapped to [−180, 180): the hundredths are wrapped,
/// not the degrees, so no angle lands on +180.00 and a decoded phase re-encodes to itself.
/// ±∞ has no angle: NaN.
pub(crate) fn deg_to_i16(x: f64) -> i16 {
    if !x.is_finite() {
        return I16_NAN;
    }
    // Exact for any finite f32 angle: the remainder of floats is exact.
    let h = round_half_up(x * I16_PER_UNIT);
    ((h + 18_000.0).rem_euclid(36_000.0) - 18_000.0) as i16
}

/// `i16` code of `x` in the array's unit.
pub(crate) fn to_i16(x: f64) -> i16 {
    if x.is_nan() {
        I16_NAN
    } else if x == f64::INFINITY {
        I16_POS_INF
    } else if x == f64::NEG_INFINITY {
        I16_NEG_INF
    } else {
        round_half_up(x * I16_PER_UNIT).clamp(-I16_MAX_FINITE, I16_MAX_FINITE) as i16
    }
}

/// Value of an `i16` code.
pub(crate) fn from_i16(q: i16) -> f32 {
    match q {
        I16_NAN => f32::NAN,
        I16_NEG_INF => f32::NEG_INFINITY,
        I16_POS_INF => f32::INFINITY,
        // Division, not a multiplication by 0.01: a value with two decimals comes back as
        // the f32 nearest to it, so re-encoding a decoded frame gives the same codes.
        q => f32::from(q) / 100.0,
    }
}

/// `u16` code of a coherence.
pub(crate) fn to_u16(x: f64) -> u16 {
    if x.is_nan() {
        U16_NAN
    } else {
        round_half_up(x * f64::from(U16_ONE)).clamp(0.0, f64::from(U16_ONE)) as u16
    }
}

/// Value of a `u16` code; `None` for an undefined code.
pub(crate) fn from_u16(q: u16) -> Option<f32> {
    match q {
        U16_NAN => Some(f32::NAN),
        q if q <= U16_ONE => Some(f32::from(q) / f32::from(U16_ONE)),
        _ => None,
    }
}

/// Appends 16-bit codes as neighbour differences, low-byte plane then high-byte plane.
fn put_planes(out: &mut Vec<u8>, codes: impl Iterator<Item = u16> + Clone) {
    let deltas = codes.scan(0u16, |prev, q| {
        let d = q.wrapping_sub(*prev);
        *prev = q;
        Some(d)
    });
    out.extend(deltas.clone().map(|d| d.to_le_bytes()[0]));
    out.extend(deltas.map(|d| d.to_le_bytes()[1]));
}

/// The 16-bit codes of a plane-encoded array of `b.len() / 2` elements.
fn get_planes(b: &[u8]) -> impl Iterator<Item = u16> + '_ {
    let (lo, hi) = b.split_at(b.len() / 2);
    lo.iter().zip(hi).scan(0u16, |prev, (l, h)| {
        *prev = prev.wrapping_add(u16::from_le_bytes([*l, *h]));
        Some(*prev)
    })
}

/// Appends a float array in `elem` (`F32`, `I16` or `U16`); `deg` wraps an `I16` array.
pub(crate) fn put_f32(out: &mut Vec<u8>, elem: Elem, deg: bool, v: &[f32]) {
    match elem {
        Elem::I16 => put_planes(
            out,
            v.iter().map(|x| {
                let x = f64::from(*x);
                (if deg { deg_to_i16(x) } else { to_i16(x) }) as u16
            }),
        ),
        Elem::U16 => put_planes(out, v.iter().map(|x| to_u16(f64::from(*x)))),
        Elem::F32 | Elem::U32 => {
            if cfg!(target_endian = "little") {
                out.extend_from_slice(bytemuck::cast_slice(v));
            } else {
                for x in v {
                    out.extend_from_slice(&x.to_le_bytes());
                }
            }
        }
    }
}

/// Appends a `u32` array.
pub(crate) fn put_u32(out: &mut Vec<u8>, v: &[u32]) {
    if cfg!(target_endian = "little") {
        out.extend_from_slice(bytemuck::cast_slice(v));
    } else {
        for x in v {
            out.extend_from_slice(&x.to_le_bytes());
        }
    }
}

/// A float array from its bytes in `elem`; `None` if a code is undefined. `b` holds exactly
/// the array (its length was checked against `n` and the element size).
pub(crate) fn get_f32(b: &[u8], elem: Elem) -> Option<Vec<f32>> {
    match elem {
        Elem::I16 => Some(get_planes(b).map(|q| from_i16(q as i16)).collect()),
        Elem::U16 => get_planes(b).map(from_u16).collect(),
        Elem::F32 | Elem::U32 => Some(
            b.chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
        ),
    }
}

/// A `u32` array from its bytes.
pub(crate) fn get_u32(b: &[u8]) -> Vec<u32> {
    b.chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Deflates the concatenated arrays into the payload part.
pub(crate) fn deflate(raw: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec(raw, LEVEL)
}

/// Inflates a payload part that must hold exactly `expected` bytes; `None` if it is not
/// valid deflate data or holds any other amount. Never grows its buffer past `expected`.
pub(crate) fn inflate(part: &[u8], expected: usize) -> Option<Vec<u8>> {
    let raw = miniz_oxide::inflate::decompress_to_vec_with_limit(part, expected).ok()?;
    (raw.len() == expected).then_some(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i16_codes() {
        assert_eq!(to_i16(f64::NAN), I16_NAN);
        assert_eq!(to_i16(f64::INFINITY), I16_POS_INF);
        assert_eq!(to_i16(f64::NEG_INFINITY), I16_NEG_INF);
        assert_eq!(to_i16(1e9), 32_766);
        assert_eq!(to_i16(-1e9), -32_766);
        assert_eq!(to_i16(-0.005), 0);
        assert_eq!(to_i16(0.005), 1);
        assert_eq!(from_i16(-325), -3.25);
        assert_eq!(from_i16(10), 0.1);
    }

    #[test]
    fn wrapping_is_half_open() {
        assert_eq!(deg_to_i16(180.0), -18_000);
        assert_eq!(deg_to_i16(-180.0), -18_000);
        assert_eq!(deg_to_i16(540.0), -18_000);
        assert_eq!(deg_to_i16(-190.0), 17_000);
        assert_eq!(deg_to_i16(179.996), -18_000);
        assert_eq!(deg_to_i16(179.994), 17_999);
        assert_eq!(deg_to_i16(f64::INFINITY), I16_NAN);
        assert!((-18_000..18_000).contains(&deg_to_i16(3.0e38)));
    }

    #[test]
    fn u16_codes() {
        assert_eq!(to_u16(1.2), U16_ONE);
        assert_eq!(to_u16(-0.1), 0);
        assert_eq!(from_u16(U16_ONE), Some(1.0));
        assert_eq!(from_u16(U16_ONE + 1), None);
        assert!(from_u16(U16_NAN).is_some_and(f32::is_nan));
    }

    #[test]
    fn planes_round_trip() {
        let codes = [0u16, 1, 65_535, 32_768, 7, 7, 300];
        let mut b = Vec::new();
        put_planes(&mut b, codes.iter().copied());
        assert_eq!(get_planes(&b).collect::<Vec<_>>(), codes);
    }

    #[test]
    fn inflate_refuses_other_lengths() {
        let p = deflate(&[1, 2, 3, 4]);
        assert_eq!(inflate(&p, 4), Some(vec![1, 2, 3, 4]));
        assert_eq!(inflate(&p, 3), None);
        assert_eq!(inflate(&p, 5), None);
        assert_eq!(inflate(&[0xff, 0x00], 4), None);
    }
}
