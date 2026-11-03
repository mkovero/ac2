//! Array payload of a data frame: each array quantised to its element encoding's codes,
//! concatenated in the header's order; on the wire the codes go as a keyframe's intra form or
//! as a delta against the frame before, then deflated (RFC 1951, raw).
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
//! - `i16_mag_offset`: an `i16` code minus the same column's `mag` code, wrapping in 16 bits:
//!   the same precision as `i16`, and exact for every pair of codes (NaN and ±∞ included),
//!   since adding the `mag` code back is the inverse in that ring. An average's range band
//!   lies a few dB around its curve, so its offsets are small numbers even where the levels
//!   are not.
//! - `u8`: γ² in hundredths rounded down (0 … 100), 255 = NaN; other codes are refused. A
//!   range band's coherence is only compared with a coherence mask, whose steps are
//!   hundredths: the code is the largest `k` whose value `k / 100` (as `f32`) is at most γ²,
//!   so `value < t` holds for exactly the values a mask at `t = j / 100` blanks (0.2996
//!   reads 0.29 and stays below 0.3). Rounding to nearest would lift it to 0.30 and unmask
//!   it.
//!
//! - `bfp16`: block floating point, for the linear IR: one exponent `e` per array and a
//!   16-bit mantissa per sample, value = mantissa · 2^e, with `e` the smallest that puts the
//!   array's largest finite magnitude at or below 32 765. So every sample is within 2^(e−1)
//!   of its value, less than 1/32 765 of the peak (−90.3 dB re peak; −96 dB when the peak
//!   fills the mantissa). The IR view draws the linear IR on a linear axis, where that is
//!   a thousandth of a pixel at the default zoom, and its log view (dB re peak) is 0.03 dB
//!   off at −40 dB and 0.3 dB at −60 dB, the default floor. A measured IR's noise floor is
//!   higher still; a deeper floor (an electrical loopback) reads on the ETC, which goes in
//!   `i16` dB. Mantissas are the `i16` codes (NaN, ±∞ alike), so small tail samples are
//!   small integers that deflate well, unlike the full mantissas of a float of any width.
//!   The exponent is in [−128, 113]: 32 766 · 2^113 is still a finite f32.
//!
//! The codes of an array are little-endian, a `bfp16` array's exponent byte first: the form
//! the CRC of a frame covers.
//!
//! Intra (keyframe): a 16-bit array goes out as the differences between neighbouring columns
//! (wrapping), low bytes first, then high bytes: neighbouring columns of a curve are close,
//! so most high bytes are 0 or 0xff and deflate takes them to almost nothing.
//!
//! Delta: a 16-bit array goes out as its codes minus the base frame's codes of the same
//! column (wrapping; an angle wrapped to ±180°, so a phase crossing the wrap moves by a small
//! step), low bytes then high bytes, not differenced between neighbours: a column of a
//! deeper analysis stage repeats bit for bit until that stage hops, and its 0 stays 0. A
//! `bfp16` array is a mantissa delta only on the base's exponent, otherwise intra. Bitmasks
//! XOR the base's; `f32` and `i8` arrays go intra in both forms. A `u8` array goes as its
//! codes minus the base's (wrapping in 8 bits): a column a deeper analysis stage holds keeps
//! its γ², so its code repeats and its difference is 0, which deflate takes to almost
//! nothing; the codes as they are would cost a literal per column even where nothing moved.
//! Wrapping makes the difference a bijection on all 256 codes, so NaN (255) going in or out
//! is one more byte and exact. The goniometer's `i8` points, in contrast, are fresh every
//! frame and gain nothing from a difference.
//!
//! `f32` and `u32` arrays (meters, Leq windows, bitmasks) go as they are,
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
/// `u8` code of NaN.
pub(crate) const U8_NAN: u8 = u8::MAX;
/// `u8` code of 1.
pub(crate) const U8_ONE: u8 = 100;

/// Smallest `bfp16` exponent: one byte, and 2^−128 is far below any IR sample.
const BFP_EXP_MIN: i32 = -128;
/// Largest `bfp16` exponent: the largest whose mantissas all decode to finite f32.
const BFP_EXP_MAX: i32 = 113;
/// Largest magnitude a `bfp16` mantissa is scaled to. Odd, so that a decoded array picks
/// the same exponent again: its peak mantissa, rounded from above 32 765 / 2, is at least
/// 16 383, and 2 · 16 383 > 32 765.
const BFP_PEAK: f64 = 32_765.0;

/// Hundredths per unit of an `i16` array.
const I16_PER_UNIT: f64 = 100.0;

/// Bytes per element.
fn elem_size(e: Elem) -> usize {
    match e {
        Elem::F32 | Elem::U32 => 4,
        Elem::I16 | Elem::I16MagOffset | Elem::U16 | Elem::Bfp16 => 2,
        Elem::I8 | Elem::U8 => 1,
    }
}

/// Bytes of an `n`-element array in `e`: a `bfp16` array leads with its exponent byte.
pub(crate) fn array_size(e: Elem, n: usize) -> usize {
    n * elem_size(e) + usize::from(e == Elem::Bfp16)
}

/// `bfp16` exponent of an array: the smallest `e` in range with every finite |x| · 2^−e at
/// most [`BFP_PEAK`]; 0 for an array with no finite non-zero value.
pub(crate) fn bfp_exponent(v: &[f32]) -> i32 {
    let peak = v
        .iter()
        .filter(|x| x.is_finite())
        .fold(0.0f64, |m, x| m.max(f64::from(x.abs())));
    if peak == 0.0 {
        return 0;
    }
    // peak = m · 2^k with m in [0.5, 1) (frexp); every f32, subnormals included, is a
    // normal f64, so k comes from the f64 exponent bits.
    let k = ((peak.to_bits() >> 52) & 0x7ff) as i32 - 1022;
    // peak · 2^−(k−15) = m · 2^15 is in [16 384, 32 768): exact, a power-of-two scaling.
    let e = if peak * 2f64.powi(15 - k) > BFP_PEAK {
        k - 14
    } else {
        k - 15
    };
    e.clamp(BFP_EXP_MIN, BFP_EXP_MAX)
}

/// `bfp16` mantissa of `x` with `down` = 2^−e: the `i16` code of x · 2^−e.
fn to_bfp(x: f32, down: f64) -> i16 {
    // Exact scaling: 2^−e is at most 2^128, x · 2^−e stays far inside the f64 range.
    code_i16(f64::from(x) * down)
}

/// Value of a `bfp16` mantissa with `up` = 2^e.
fn from_bfp(q: i16, up: f64) -> f32 {
    match q {
        I16_NAN => f32::NAN,
        I16_NEG_INF => f32::NEG_INFINITY,
        I16_POS_INF => f32::INFINITY,
        // Exact in f64 (so is 2^e over the exponent range); finite in f32 for
        // e ≤ BFP_EXP_MAX, and exact there unless it falls among the f32 subnormals, where
        // it rounds the same way everywhere.
        q => (f64::from(q) * up) as f32,
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
    code_i16(x * I16_PER_UNIT)
}

/// `i16` code of `x` counted in steps of 1: rounded, clamped, NaN and ±∞ coded.
fn code_i16(x: f64) -> i16 {
    if x.is_nan() {
        I16_NAN
    } else if x == f64::INFINITY {
        I16_POS_INF
    } else if x == f64::NEG_INFINITY {
        I16_NEG_INF
    } else {
        round_half_up(x).clamp(-I16_MAX_FINITE, I16_MAX_FINITE) as i16
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

/// `u8` code of a coherence: the largest `k` in 0 … 100 with [`from_u8`]`(k) ≤ x` (0 below
/// 0, 100 above 1), so a comparison with a mask in hundredths reads the same on the code as
/// on `x`.
pub(crate) fn to_u8(x: f64) -> u8 {
    if x.is_nan() {
        return U8_NAN;
    }
    let mut q = (x * f64::from(U8_ONE))
        .floor()
        .clamp(0.0, f64::from(U8_ONE)) as u8;
    // The product is within 10⁻¹⁴ of x · 100 and each step's f32 within 10⁻⁸ of k / 100, so
    // the floor is at most one code off the f32 steps either way.
    if q < U8_ONE && f64::from(u8_value(q + 1)) <= x {
        q += 1;
    } else if q > 0 && f64::from(u8_value(q)) > x {
        q -= 1;
    }
    q
}

/// `k / 100` as the nearest `f32`.
fn u8_value(q: u8) -> f32 {
    f32::from(q) / f32::from(U8_ONE)
}

/// Value of a `u8` code; `None` for an undefined code.
pub(crate) fn from_u8(q: u8) -> Option<f32> {
    match q {
        U8_NAN => Some(f32::NAN),
        q if q <= U8_ONE => Some(u8_value(q)),
        _ => None,
    }
}

/// The `i16_mag_offset` codes of an array of `i16` codes `arr`: each minus `mag`'s code of
/// the same column (both little-endian, same length), wrapping.
pub(crate) fn to_mag_offsets(arr: &mut [u8], mag: &[u8]) {
    for (a, m) in arr.chunks_exact_mut(2).zip(mag.chunks_exact(2)) {
        let q = u16::from_le_bytes([a[0], a[1]]).wrapping_sub(u16::from_le_bytes([m[0], m[1]]));
        a.copy_from_slice(&q.to_le_bytes());
    }
}

/// Inverse of [`to_mag_offsets`]: the `i16` codes of offsets `arr` on `mag`.
pub(crate) fn from_mag_offsets(arr: &mut [u8], mag: &[u8]) {
    for (a, m) in arr.chunks_exact_mut(2).zip(mag.chunks_exact(2)) {
        let q = u16::from_le_bytes([a[0], a[1]]).wrapping_add(u16::from_le_bytes([m[0], m[1]]));
        a.copy_from_slice(&q.to_le_bytes());
    }
}

/// Appends 8-bit codes as neighbour differences (wrapping).
fn put_planes8(out: &mut Vec<u8>, codes: &[u8]) {
    let mut prev = 0u8;
    out.extend(codes.iter().map(|q| {
        let d = q.wrapping_sub(prev);
        prev = *q;
        d
    }));
}

/// The 8-bit codes of [`put_planes8`] bytes.
fn get_planes8(out: &mut Vec<u8>, b: &[u8]) {
    let mut prev = 0u8;
    out.extend(b.iter().map(|d| {
        prev = prev.wrapping_add(*d);
        prev
    }));
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

/// Appends 16-bit codes little-endian.
fn put_le16(out: &mut Vec<u8>, codes: impl Iterator<Item = u16>) {
    for q in codes {
        out.extend_from_slice(&q.to_le_bytes());
    }
}

/// The 16-bit codes of little-endian code bytes.
fn le16(b: &[u8]) -> impl Iterator<Item = u16> + Clone + '_ {
    b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]))
}

/// Appends 16-bit values as a low-byte plane then a high-byte plane, not differenced.
fn put_raw_planes(out: &mut Vec<u8>, v: impl Iterator<Item = u16> + Clone) {
    out.extend(v.clone().map(|d| d.to_le_bytes()[0]));
    out.extend(v.map(|d| d.to_le_bytes()[1]));
}

/// The 16-bit values of [`put_raw_planes`] bytes.
fn get_raw_planes(b: &[u8]) -> impl Iterator<Item = u16> + '_ {
    let (lo, hi) = b.split_at(b.len() / 2);
    lo.iter().zip(hi).map(|(l, h)| u16::from_le_bytes([*l, *h]))
}

/// Appends a float array's codes in `elem` (`F32`, `I16`, `U16`, `U8` or `Bfp16`); `deg`
/// wraps an `I16` array. An `I16MagOffset` array is appended as its `i16` codes, which
/// [`to_mag_offsets`] then takes relative to the frame's `mag`.
pub(crate) fn put_f32(out: &mut Vec<u8>, elem: Elem, deg: bool, v: &[f32]) {
    match elem {
        Elem::I16 | Elem::I16MagOffset => put_le16(
            out,
            v.iter().map(|x| {
                let x = f64::from(*x);
                (if deg { deg_to_i16(x) } else { to_i16(x) }) as u16
            }),
        ),
        Elem::U16 => put_le16(out, v.iter().map(|x| to_u16(f64::from(*x)))),
        Elem::U8 => out.extend(v.iter().map(|x| to_u8(f64::from(*x)))),
        Elem::Bfp16 => {
            let e = bfp_exponent(v);
            out.push(e as i8 as u8);
            let down = 2f64.powi(-e);
            put_le16(out, v.iter().map(|x| to_bfp(*x, down) as u16));
        }
        Elem::F32 | Elem::U32 => {
            if cfg!(target_endian = "little") {
                out.extend_from_slice(bytemuck::cast_slice(v));
            } else {
                for x in v {
                    out.extend_from_slice(&x.to_le_bytes());
                }
            }
        }
        // Not a float encoding; the goniometer goes through `put_i8`.
        Elem::I8 => out.extend(v.iter().map(|x| *x as i8 as u8)),
    }
}

/// Appends an `i8` array.
pub(crate) fn put_i8(out: &mut Vec<u8>, v: &[i8]) {
    out.extend(v.iter().map(|x| *x as u8));
}

/// An `i8` array from its bytes.
pub(crate) fn get_i8(b: &[u8]) -> Vec<i8> {
    b.iter().map(|x| *x as i8).collect()
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

/// A float array from its codes in `elem`; `None` if a code or exponent is undefined. `b`
/// holds exactly the array (its length was checked against [`array_size`]); an
/// `I16MagOffset` array as its `i16` codes ([`from_mag_offsets`]).
pub(crate) fn get_f32(b: &[u8], elem: Elem) -> Option<Vec<f32>> {
    match elem {
        Elem::I16 | Elem::I16MagOffset => Some(le16(b).map(|q| from_i16(q as i16)).collect()),
        Elem::U16 => le16(b).map(from_u16).collect(),
        Elem::U8 => b.iter().map(|q| from_u8(*q)).collect(),
        Elem::Bfp16 => {
            let (&e, m) = b.split_first()?;
            let e = i32::from(e as i8);
            let up = 2f64.powi(e);
            (e <= BFP_EXP_MAX).then(|| le16(m).map(|q| from_bfp(q as i16, up)).collect())
        }
        Elem::F32 | Elem::U32 => Some(
            b.chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
        ),
        Elem::I8 => Some(get_i8(b).into_iter().map(f32::from).collect()),
    }
}

/// A `u32` array from its bytes.
pub(crate) fn get_u32(b: &[u8]) -> Vec<u32> {
    b.chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Appends one array's codes in its intra (keyframe) form.
pub(crate) fn put_intra(out: &mut Vec<u8>, elem: Elem, codes: &[u8]) {
    match elem {
        Elem::I16 | Elem::I16MagOffset | Elem::U16 => put_planes(out, le16(codes)),
        Elem::U8 => put_planes8(out, codes),
        Elem::Bfp16 => {
            if let Some((e, m)) = codes.split_first() {
                out.push(*e);
                put_planes(out, le16(m));
            }
        }
        Elem::F32 | Elem::U32 | Elem::I8 => out.extend_from_slice(codes),
    }
}

/// Appends one array's codes from its intra form `wire` (exactly the array's size).
pub(crate) fn get_intra(out: &mut Vec<u8>, elem: Elem, wire: &[u8]) {
    match elem {
        Elem::I16 | Elem::I16MagOffset | Elem::U16 => put_le16(out, get_planes(wire)),
        Elem::U8 => get_planes8(out, wire),
        Elem::Bfp16 => {
            if let Some((e, m)) = wire.split_first() {
                out.push(*e);
                put_le16(out, get_planes(m));
            }
        }
        Elem::F32 | Elem::U32 | Elem::I8 => out.extend_from_slice(wire),
    }
}

/// Half a turn and a turn in `i16` hundredths of a degree.
const DEG_HALF: i32 = 18_000;
const DEG_TURN: i32 = 36_000;

/// Whether an `i16` code is an angle [`deg_to_i16`] can produce (not NaN).
fn is_angle(q: i16) -> bool {
    (-DEG_HALF..DEG_HALF).contains(&i32::from(q))
}

/// Delta of an angle code `x` against `b`: the short way round when both are angles, the
/// NaN code when `x` becomes NaN, the plain wrapping difference from a NaN base. `None`:
/// `x` is no code an angle array holds.
fn angle_delta(x: i16, b: i16) -> Option<u16> {
    if !is_angle(b) {
        return Some((x as u16).wrapping_sub(b as u16));
    }
    if x == I16_NAN {
        return Some(I16_NAN as u16);
    }
    if !is_angle(x) {
        return None;
    }
    let d = (i32::from(x) - i32::from(b) + DEG_HALF).rem_euclid(DEG_TURN) - DEG_HALF;
    Some(d as i16 as u16)
}

/// Inverse of [`angle_delta`].
fn angle_apply(d: u16, b: i16) -> u16 {
    if !is_angle(b) {
        return (b as u16).wrapping_add(d);
    }
    let d = d as i16;
    if d == I16_NAN {
        return I16_NAN as u16;
    }
    ((i32::from(b) + i32::from(d) + DEG_HALF).rem_euclid(DEG_TURN) - DEG_HALF) as i16 as u16
}

/// Appends one array's codes as a delta against the base's codes of the same array (same
/// size). `false`: an angle array holds a code no angle has, so it goes in a keyframe.
pub(crate) fn put_delta(
    out: &mut Vec<u8>,
    elem: Elem,
    deg: bool,
    codes: &[u8],
    base: &[u8],
) -> bool {
    match elem {
        Elem::I16 if deg => {
            let start = out.len();
            let mut d = Vec::with_capacity(codes.len() / 2);
            for (x, b) in le16(codes).zip(le16(base)) {
                match angle_delta(x as i16, b as i16) {
                    Some(v) => d.push(v),
                    None => {
                        out.truncate(start);
                        return false;
                    }
                }
            }
            put_raw_planes(out, d.into_iter());
        }
        Elem::I16 | Elem::I16MagOffset | Elem::U16 => {
            put_raw_planes(
                out,
                le16(codes).zip(le16(base)).map(|(x, b)| x.wrapping_sub(b)),
            );
        }
        // Differences, not the codes as they are: a held column's γ² repeats, so its code
        // minus the base's is 0 (see the module docs); mod 256 it is a bijection, NaN too.
        Elem::U8 => out.extend(codes.iter().zip(base).map(|(x, b)| x.wrapping_sub(*b))),
        Elem::Bfp16 => {
            if let (Some((e, m)), Some((be, bm))) = (codes.split_first(), base.split_first()) {
                out.push(*e);
                if e == be {
                    put_raw_planes(out, le16(m).zip(le16(bm)).map(|(x, b)| x.wrapping_sub(b)));
                } else {
                    put_planes(out, le16(m));
                }
            }
        }
        Elem::U32 => {
            for (x, b) in codes.chunks_exact(4).zip(base.chunks_exact(4)) {
                out.extend(x.iter().zip(b).map(|(x, b)| x ^ b));
            }
        }
        Elem::F32 | Elem::I8 => out.extend_from_slice(codes),
    }
    true
}

/// Appends one array's codes from its delta form `wire` and the base's codes (both exactly
/// the array's size).
pub(crate) fn get_delta(out: &mut Vec<u8>, elem: Elem, deg: bool, wire: &[u8], base: &[u8]) {
    match elem {
        Elem::I16 if deg => put_le16(
            out,
            get_raw_planes(wire)
                .zip(le16(base))
                .map(|(d, b)| angle_apply(d, b as i16)),
        ),
        Elem::I16 | Elem::I16MagOffset | Elem::U16 => put_le16(
            out,
            get_raw_planes(wire)
                .zip(le16(base))
                .map(|(d, b)| b.wrapping_add(d)),
        ),
        Elem::U8 => out.extend(wire.iter().zip(base).map(|(d, b)| b.wrapping_add(*d))),
        Elem::Bfp16 => {
            if let (Some((e, m)), Some((be, bm))) = (wire.split_first(), base.split_first()) {
                out.push(*e);
                if e == be {
                    put_le16(
                        out,
                        get_raw_planes(m)
                            .zip(le16(bm))
                            .map(|(d, b)| b.wrapping_add(d)),
                    );
                } else {
                    put_le16(out, get_planes(m));
                }
            }
        }
        Elem::U32 => out.extend(wire.iter().zip(base).map(|(x, b)| x ^ b)),
        Elem::F32 | Elem::I8 => out.extend_from_slice(wire),
    }
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
    fn u8_codes_round_down_onto_the_f32_hundredths() {
        assert_eq!(to_u8(f64::NAN), U8_NAN);
        assert_eq!(to_u8(1.2), U8_ONE);
        assert_eq!(to_u8(-0.1), 0);
        assert_eq!(to_u8(f64::INFINITY), U8_ONE);
        assert_eq!(to_u8(f64::NEG_INFINITY), 0);
        assert_eq!(to_u8(f64::from(0.2996f32)), 29);
        assert_eq!(to_u8(f64::from(0.3f32)), 30);
        assert_eq!(from_u8(U8_ONE + 1), None);
        assert_eq!(from_u8(U8_NAN - 1), None);
        assert!(from_u8(U8_NAN).is_some_and(f32::is_nan));
        // Every code's value re-encodes to itself, though 0.29 as f32 is below 0.29 (a plain
        // floor of x · 100 would read 28), and the f32 just below a step reads one lower.
        for q in 0..=U8_ONE {
            let v = from_u8(q).expect("defined");
            assert_eq!(to_u8(f64::from(v)), q, "{q}");
            if q > 0 {
                assert_eq!(to_u8(f64::from(v.next_down())), q - 1, "{q}");
            }
        }
    }

    #[test]
    fn mag_offsets_invert_for_every_code() {
        let codes = [i16::MIN, i16::MIN + 1, -32_766, -1, 0, 1, 32_766, i16::MAX];
        for m in codes {
            for x in codes {
                let mut a = x.to_le_bytes();
                let mag = m.to_le_bytes();
                to_mag_offsets(&mut a, &mag);
                from_mag_offsets(&mut a, &mag);
                assert_eq!(i16::from_le_bytes(a), x);
            }
        }
    }

    #[test]
    fn bfp_exponent_is_the_smallest_that_fits() {
        assert_eq!(bfp_exponent(&[]), 0);
        assert_eq!(bfp_exponent(&[0.0, f32::NAN, f32::INFINITY]), 0);
        // 1.0 = 0.5 · 2^1: 1.0 · 2^14 = 16 384 fits, 2^15 would not.
        assert_eq!(bfp_exponent(&[0.25, -1.0]), -14);
        // Just below 2^0: 0.99999 · 2^15 = 32 767.7 > 32 765, so 2^−14.
        assert_eq!(bfp_exponent(&[0.99999]), -14);
        assert_eq!(bfp_exponent(&[0.999]), -15);
        assert_eq!(bfp_exponent(&[f32::MAX]), BFP_EXP_MAX);
        assert_eq!(bfp_exponent(&[f32::from_bits(1)]), BFP_EXP_MIN);
    }

    #[test]
    fn bfp_round_trip_is_within_half_a_step() {
        let v = [
            1.0e-3f32,
            -0.73,
            0.5,
            2.0e-7,
            -0.0,
            f32::NAN,
            f32::NEG_INFINITY,
            0.123_456,
        ];
        let mut b = Vec::new();
        put_f32(&mut b, Elem::Bfp16, false, &v);
        assert_eq!(b.len(), array_size(Elem::Bfp16, v.len()));
        let e = i32::from(b[0] as i8);
        assert_eq!(e, -15);
        let back = get_f32(&b, Elem::Bfp16).expect("decodes");
        for (x, y) in v.iter().zip(&back) {
            if x.is_finite() {
                assert!(
                    (f64::from(*x) - f64::from(*y)).abs() <= 2f64.powi(e - 1),
                    "{x} {y}"
                );
            } else {
                assert_eq!(x.is_nan(), y.is_nan());
            }
        }
        // A decoded array encodes to the same bytes.
        let mut again = Vec::new();
        put_f32(&mut again, Elem::Bfp16, false, &back);
        assert_eq!(again, b);
        // Exponents beyond the finite range are refused.
        b[0] = 114;
        assert_eq!(get_f32(&b, Elem::Bfp16), None);
        assert_eq!(get_f32(&[], Elem::Bfp16), None);
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
