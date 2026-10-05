//! Room acoustic parameters (ISO 3382-1) from an impulse response: EDT, T20, T30, C50, C80
//! and D50 per octave and one-third-octave band and broadband. Design:
//! `docs/design/room-metrics.md`.
//!
//! # Decay (EDT, T20, T30)
//!
//! Each band is the IR through the IEC 61260-1 band filter run **backwards in time**
//! (Jacobsen & Rindel 1987): a causal filter rings after every arrival and stretches the
//! decay by its own, which a narrow low band makes as long as a short room decay; run
//! backwards, the ringing lands before the arrivals and the decay after the onset is the
//! room's. The band's own onset is where its energy first reaches [`TRIGGER_DB`] re its
//! peak. Noise is handled by Lundeby's iterative method (Lundeby et al. 1995,
//! [`lundeby`]): the IR is truncated where its decay meets the noise, and the energy the
//! decay would have had after that point is added back to the Schroeder integral
//! ([`schroeder_db`]) as an exponential continuation. The decay times are least-squares fits
//! to that curve over 0…−10, −5…−25 and −5…−35 dB.
//!
//! # Refusals
//!
//! The curve reaches `−range` at the truncation point ([`BandMetrics::decay_range_db`]):
//! below it the curve is the extrapolation, not measurement. A decay time is given only
//! when the bottom of its evaluation range lies [`RANGE_MARGIN_DB`] above that (ISO 3382-1:
//! 35 dB for T20, 45 dB for T30), and only when the band's filter can follow it
//! ([`MIN_BANDWIDTH_DECAY`]); otherwise it is a [`Refusal`], never a number.
//!
//! # Energy ratios (C50, C80, D50)
//!
//! Window before filtering, as ISO 3382-1 suggests: the broadband IR is cut at its own
//! onset + 50 / 80 ms, each piece is band-filtered forwards with room for the filter's tail,
//! and all of each piece's output energy is counted, so the filter's delay and ringing
//! cannot move early energy into the late part. The late part ends at the band's truncation
//! point plus the same correction as the decay.

use crate::rta::{BandFraction, full_rate_band};
use crate::weighting::Biquad;

/// Onset trigger: the first sample whose energy is within this of the peak (ISO 3382-1:
/// the start of the response lies at least 20 dB below the peak of the direct sound).
pub const TRIGGER_DB: f64 = -20.0;
/// The bottom of a decay's evaluation range must lie this far above the level where the
/// decay meets the noise (ISO 3382-1: T20 needs 35 dB, T30 45 dB of decay range).
pub const RANGE_MARGIN_DB: f64 = 10.0;
/// Decay range the energy ratios need: the early decay must be measured (EDT's need).
pub const CLARITY_RANGE_DB: f64 = 20.0;
/// Smallest product of a band's bandwidth and a decay time that the band reports. Run
/// backwards, the filter leaves a single exponential decay exactly exponential from the
/// onset on, but the energy it moved before the onset (counted there) is a step at the top
/// of the curve: at B·T = 8 it changes EDT by ≤ 1 % (≤ 6 % with a direct sound as strong as
/// the whole reverberation), at 4 by up to 45 % (`room/tests.rs`). T20 and T30 start below
/// that step, but one band of one response has only about B·T degrees of freedom, so they
/// scatter as widely there and share the limit.
pub const MIN_BANDWIDTH_DECAY: f64 = 8.0;
/// A non-straight decay: T30 more than this much longer than T20, percent (ISO 3382-2
/// curvature).
pub const CURVATURE_LIMIT_PCT: f64 = 10.0;
/// Octave bands analysed: mid-band frequencies, Hz.
pub const OCTAVE_SPAN_HZ: (f64, f64) = (63.0, 8000.0);
/// One-third-octave bands analysed: mid-band frequencies, Hz.
pub const THIRD_SPAN_HZ: (f64, f64) = (50.0, 10_000.0);

/// Lundeby: first averaging interval, s (the method's 10–50 ms).
const LUNDEBY_INTERVAL_S: f64 = 0.02;
/// Lundeby: averaging intervals per 10 dB of decay once the slope is known (3–10).
const LUNDEBY_INTERVALS_PER_10DB: f64 = 5.0;
/// Lundeby: the first noise estimate uses this tail fraction of the response, and every
/// later one at least this much.
const LUNDEBY_TAIL_FRACTION: f64 = 0.1;
/// Lundeby: the first decay fit ends this far above the noise, dB.
const LUNDEBY_FIRST_FIT_DB: f64 = 10.0;
/// Lundeby: the noise is read from where the decay line is this far below the noise, dB
/// (5–10 dB past the crossing point).
const LUNDEBY_NOISE_AFTER_DB: f64 = 5.0;
/// Lundeby: the late decay fit spans these levels above the noise, dB (a 10–20 dB range
/// starting 5–10 dB above the noise).
const LUNDEBY_LATE_FIT_DB: (f64, f64) = (5.0, 25.0);
/// Lundeby: most iterations.
const LUNDEBY_ITERATIONS: usize = 5;
/// The ratios' band pieces are filtered with this many time constants of tail, in units of
/// 1/bandwidth: a sixth-order band-pass rings down by far more than 100 dB in that time.
const TAIL_PER_BANDWIDTH: f64 = 20.0;

/// A decay time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decay {
    /// Early decay time: 0 … −10 dB, ×6.
    Edt,
    /// −5 … −25 dB, ×3.
    T20,
    /// −5 … −35 dB, ×2.
    T30,
}

impl Decay {
    /// Evaluation range (top, bottom) on the decay curve, dB.
    pub fn range_db(self) -> (f64, f64) {
        match self {
            Decay::Edt => (0.0, -10.0),
            Decay::T20 => (-5.0, -25.0),
            Decay::T30 => (-5.0, -35.0),
        }
    }

    /// Decay range this time needs: its bottom plus [`RANGE_MARGIN_DB`].
    pub fn needed_range_db(self) -> f64 {
        -self.range_db().1 + RANGE_MARGIN_DB
    }
}

/// Why a parameter is not given.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Refusal {
    /// The band carries no decay (no energy, or none falling).
    NoDecay,
    /// The decay meets the noise too soon.
    InsufficientRange {
        /// Decay range measured, dB.
        range_db: f64,
        /// Decay range needed, dB.
        needed_db: f64,
    },
    /// The decay is too short for the band's filter to follow.
    FilterLimited {
        /// Bandwidth × decay time found.
        bandwidth_decay: f64,
    },
}

/// A parameter or why it is not given.
pub type Metric = Result<f64, Refusal>;

/// One band's parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct BandMetrics {
    /// Mid-band frequency, Hz; `None` = broadband (the IR as captured).
    pub centre_hz: Option<f64>,
    /// The band's onset (trigger), s on the caller's time axis.
    pub onset_s: f64,
    /// Where the decay meets the noise (truncation), s on the caller's time axis.
    pub truncation_s: f64,
    /// Depth of the decay curve at the truncation point, dB.
    pub decay_range_db: f64,
    /// Early decay time, s.
    pub edt_s: Metric,
    /// T20, s.
    pub t20_s: Metric,
    /// T30, s.
    pub t30_s: Metric,
    /// Clarity C50, dB.
    pub c50_db: Metric,
    /// Clarity C80, dB.
    pub c80_db: Metric,
    /// Definition D50 (ratio 0…1).
    pub d50: Metric,
    /// 100·(T30/T20 − 1), when both are given.
    pub curvature_pct: Option<f64>,
}

impl BandMetrics {
    fn none(centre_hz: Option<f64>, onset_s: f64) -> Self {
        Self {
            centre_hz,
            onset_s,
            truncation_s: onset_s,
            decay_range_db: 0.0,
            edt_s: Err(Refusal::NoDecay),
            t20_s: Err(Refusal::NoDecay),
            t30_s: Err(Refusal::NoDecay),
            c50_db: Err(Refusal::NoDecay),
            c80_db: Err(Refusal::NoDecay),
            d50: Err(Refusal::NoDecay),
            curvature_pct: None,
        }
    }

    /// The decay time `d`.
    pub fn decay(&self, d: Decay) -> Metric {
        match d {
            Decay::Edt => self.edt_s,
            Decay::T20 => self.t20_s,
            Decay::T30 => self.t30_s,
        }
    }
}

/// Every band and broadband.
#[derive(Debug, Clone, PartialEq)]
pub struct RoomAnalysis {
    /// The IR as captured (its band is the sweep's).
    pub broadband: BandMetrics,
    /// Octave bands inside the excited range.
    pub octave: Vec<BandMetrics>,
    /// One-third-octave bands inside the excited range.
    pub third: Vec<BandMetrics>,
}

/// Filters `x` through `sections`, forwards with `pad` zeros of tail, or backwards in time
/// (the time-reversed signal filtered and reversed back; `pad` zeros before it).
pub fn filter(x: &[f64], sections: &[Biquad], reversed: bool, pad: usize) -> Vec<f64> {
    let mut s = sections.to_vec();
    s.iter_mut().for_each(Biquad::reset);
    let mut run = |v: f64| s.iter_mut().fold(v, |v, q| q.process_sample(v));
    let n = x.len() + pad;
    let at = |i: usize| x.get(i).copied().unwrap_or(0.0);
    if reversed {
        let mut y = vec![0.0; n];
        for i in (0..n).rev() {
            // Output index i holds input index i − pad (the zeros come first in time).
            y[i] = run(i.checked_sub(pad).map_or(0.0, at));
        }
        y
    } else {
        (0..n).map(|i| run(at(i))).collect()
    }
}

/// Index of the first sample of `e` (energy) within `trigger_db` of its peak.
pub fn onset(e: &[f64], trigger_db: f64) -> Option<usize> {
    let peak = e.iter().copied().fold(0.0f64, f64::max);
    if peak.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
        return None;
    }
    let threshold = peak * 10f64.powf(trigger_db / 10.0);
    e.iter().position(|v| *v >= threshold)
}

/// Schroeder's backward integral of the energy `e` (from the onset to the truncation
/// point) plus `correction` (the energy after it), in dB re its value at the start.
pub fn schroeder_db(e: &[f64], correction: f64) -> Vec<f64> {
    let mut edc = vec![0.0; e.len()];
    let mut acc = correction;
    for (d, v) in edc.iter_mut().zip(e).rev() {
        acc += v;
        *d = acc;
    }
    let start = edc.first().copied().unwrap_or(0.0);
    edc.iter().map(|v| 10.0 * (v / start).log10()).collect()
}

/// Least-squares line `a + b·x` through the points; `None` with fewer than two distinct x.
fn fit(points: impl Iterator<Item = (f64, f64)>) -> Option<(f64, f64)> {
    let (mut n, mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (x, y) in points {
        n += 1.0;
        sx += x;
        sy += y;
        sxx += x * x;
        sxy += x * y;
    }
    let den = n * sxx - sx * sx;
    if n < 2.0 || den.abs() <= f64::EPSILON * n * sxx {
        return None;
    }
    let b = (n * sxy - sx * sy) / den;
    Some(((sy - b * sx) / n, b))
}

/// Decay time from a decay curve sampled at `fs`: the least-squares slope over the samples
/// from where it first falls to `top` dB to where it first falls to `bottom` dB, as the
/// time a 60 dB fall takes. `None` when the curve does not reach `bottom` or does not fall.
pub fn decay_time(edc_db: &[f64], fs: f64, top: f64, bottom: f64) -> Option<f64> {
    let i0 = edc_db.iter().position(|v| *v <= top)?;
    let i1 = edc_db.iter().position(|v| *v <= bottom)?;
    if i1 <= i0 + 1 {
        return None;
    }
    let (_, b) = fit((i0..=i1).map(|i| (i as f64 / fs, edc_db[i])))?;
    (b < 0.0).then(|| -60.0 / b)
}

/// Where a decay meets the noise, by Lundeby's method.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Truncation {
    /// Sample index (into the energy given) of the crossing point.
    pub index: usize,
    /// Energy the decay would have had from there on (the late decay line continued).
    pub correction: f64,
    /// Slope of the late decay line, dB/s.
    pub slope_db_s: f64,
    /// Noise energy per sample.
    pub noise: f64,
}

fn db(e: f64) -> f64 {
    if e > 0.0 { 10.0 * e.log10() } else { -400.0 }
}

/// Mean energy per sample over consecutive intervals of `len` samples: (centre time s, dB).
fn smooth(e: &[f64], fs: f64, len: usize) -> Vec<(f64, f64)> {
    let len = len.max(1);
    e.chunks(len)
        .enumerate()
        .map(|(k, c)| {
            let t = (k * len) as f64 / fs + 0.5 * c.len() as f64 / fs;
            (t, db(c.iter().sum::<f64>() / c.len() as f64))
        })
        .collect()
}

fn mean(e: &[f64]) -> f64 {
    if e.is_empty() {
        0.0
    } else {
        e.iter().sum::<f64>() / e.len() as f64
    }
}

/// Lundeby et al. (1995): the point where the decay of the energy `e` (starting at the
/// onset) meets the background noise, and the energy the decay would have had after it.
/// `None` when no decay above the noise is found.
///
/// 1. average `e` over 20 ms intervals; take the noise from the last 10 %;
/// 2. fit a line from the peak to 10 dB above the noise; its crossing with the noise is
///    the first crossing point;
/// 3. re-average with five intervals per 10 dB of that slope; then up to five times: read
///    the noise from 5 dB of decay past the crossing point (at least the last 10 %), fit
///    the late decay 5…25 dB above the noise, and move the crossing point to where that
///    line meets the noise, until it stops moving.
pub fn lundeby(e: &[f64], fs: f64) -> Option<Truncation> {
    let n = e.len();
    let tail = ((LUNDEBY_TAIL_FRACTION * n as f64) as usize).max(1);
    if n < 4 * tail.max(2) {
        return None;
    }
    let end_s = n as f64 / fs;
    let mut noise = mean(&e[n - tail..]);
    let curve = smooth(e, fs, (LUNDEBY_INTERVAL_S * fs).round() as usize);
    let peak = curve
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.1.total_cmp(&b.1.1))
        .map(|(i, _)| i)?;
    let stop = curve[peak..]
        .iter()
        .position(|p| p.1 <= db(noise) + LUNDEBY_FIRST_FIT_DB)
        .map(|i| peak + i)?;
    let (mut a, mut b) = fit(curve[peak..stop].iter().copied())?;
    if b >= 0.0 {
        return None;
    }
    let crossing = |a: f64, b: f64, noise: f64| ((db(noise) - a) / b).clamp(0.0, end_s);
    let mut tc = crossing(a, b, noise);
    let interval = (10.0 / -b / LUNDEBY_INTERVALS_PER_10DB * fs)
        .round()
        .clamp(1.0, tail as f64) as usize;
    let curve = smooth(e, fs, interval);
    let peak_db = curve.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    for _ in 0..LUNDEBY_ITERATIONS {
        let from_s = tc + LUNDEBY_NOISE_AFTER_DB / -b;
        let from = ((from_s * fs) as usize).min(n - tail);
        noise = mean(&e[from..]);
        let nd = db(noise);
        let top = (nd + LUNDEBY_LATE_FIT_DB.1).min(peak_db);
        let bottom = nd + LUNDEBY_LATE_FIT_DB.0;
        let i0 = curve.iter().position(|p| p.1 <= top);
        let i1 = curve.iter().position(|p| p.1 <= bottom);
        let Some((na, nb)) = i0
            .zip(i1)
            .filter(|(i0, i1)| i1 > i0)
            .and_then(|(i0, i1)| fit(curve[i0..i1].iter().copied()))
            .filter(|l| l.1 < 0.0)
        else {
            break;
        };
        (a, b) = (na, nb);
        let next = crossing(a, b, noise);
        let moved = (next - tc).abs();
        tc = next;
        if moved < interval as f64 / fs {
            break;
        }
    }
    let index = ((tc * fs).round() as usize).min(n);
    // The late decay line continued from the crossing point: a geometric series per sample.
    let at = 10f64.powf((a + b * index as f64 / fs) / 10.0);
    let ratio = 10f64.powf(b / (10.0 * fs));
    Some(Truncation {
        index,
        correction: at / (1.0 - ratio),
        slope_db_s: b,
        noise,
    })
}

/// The band energy `e` from its onset on: from the trigger, but never before `earliest`
/// (the broadband onset). Run backwards, the band filter is anticausal: what it shows before
/// the broadband onset is sound that arrived after it (the direct sound above all), moved
/// earlier by the filter's pre-ringing. Its energy belongs to the response, so it is
/// counted in the first sample; its time does not, so the decay starts there.
pub fn from_onset(e: &[f64], earliest: Option<usize>) -> Option<(usize, Vec<f64>)> {
    let trigger = onset(e, TRIGGER_DB)?;
    let on = trigger
        .max(earliest.unwrap_or(0))
        .min(e.len().checked_sub(1)?);
    let mut band = e[on..].to_vec();
    band[0] += e[trigger..on].iter().sum::<f64>();
    Some((on, band))
}

/// Decay part of one band: from the energy of the (backwards-filtered) band over the whole
/// span.
struct DecayPart {
    onset: usize,
    end: usize,
    correction: f64,
    range_db: f64,
    times: [Metric; 3],
}

fn decay_part(
    e: &[f64],
    fs: f64,
    bandwidth: Option<f64>,
    earliest: Option<usize>,
) -> Option<DecayPart> {
    let (on, from_onset) = from_onset(e, earliest)?;
    let t = lundeby(&from_onset, fs)?;
    let end = on + t.index.max(1);
    let band = &from_onset[..t.index.max(1)];
    let edc = schroeder_db(band, t.correction);
    let total = band.iter().sum::<f64>() + t.correction;
    let range_db = 10.0 * (total / t.correction).log10();
    let decays = [Decay::Edt, Decay::T20, Decay::T30];
    let fits = decays.map(|d| {
        let (top, bottom) = d.range_db();
        decay_time(&edc, fs, top, bottom)
    });
    // A decay too short for the filter can leave the early curve without a fit at all (the
    // energy counted at the onset is a step past −10 dB); the other times still say why.
    let longest = fits
        .iter()
        .flatten()
        .copied()
        .fold(None, |a: Option<f64>, t| Some(a.map_or(t, |a| a.max(t))));
    let times = decays.map(|d| {
        let need = d.needed_range_db();
        if range_db < need {
            return Err(Refusal::InsufficientRange {
                range_db,
                needed_db: need,
            });
        }
        let time = fits[d as usize];
        match (bandwidth, time.or(longest)) {
            (Some(bw), Some(t)) if bw * t < MIN_BANDWIDTH_DECAY => Err(Refusal::FilterLimited {
                bandwidth_decay: bw * t,
            }),
            _ => time.ok_or(Refusal::NoDecay),
        }
    });
    Some(DecayPart {
        onset: on,
        end,
        correction: t.correction,
        range_db,
        times,
    })
}

/// Energy of `x` (band-filtered forwards with its tail when `band` is given).
fn energy(x: &[f64], band: Option<(&[Biquad], usize)>) -> f64 {
    match band {
        Some((s, pad)) => filter(x, s, false, pad).iter().map(|v| v * v).sum(),
        None => x.iter().map(|v| v * v).sum(),
    }
}

/// C50, C80, D50 by window-before-filtering: `start` = the broadband onset, `end` and
/// `correction` = the band's truncation.
fn ratios(
    ir: &[f64],
    fs: f64,
    start: usize,
    end: usize,
    correction: f64,
    band: Option<(&[Biquad], usize)>,
) -> [Metric; 3] {
    let split = |ms: f64| {
        let k = (start + (ms * 1e-3 * fs).round() as usize).min(end.max(start));
        let early = energy(&ir[start..k], band);
        let late = energy(&ir[k..end.max(k)], band) + correction;
        (early, late)
    };
    let (e50, l50) = split(50.0);
    let (e80, l80) = split(80.0);
    let ratio_db = |e: f64, l: f64| {
        if e > 0.0 && l > 0.0 {
            Ok(10.0 * (e / l).log10())
        } else {
            Err(Refusal::NoDecay)
        }
    };
    let d50 = if e50 + l50 > 0.0 {
        Ok(e50 / (e50 + l50))
    } else {
        Err(Refusal::NoDecay)
    };
    [ratio_db(e50, l50), ratio_db(e80, l80), d50]
}

fn band_metrics(
    ir: &[f64],
    fs: f64,
    t0_s: f64,
    broadband_onset: Option<usize>,
    band: Option<(f64, f64, &[Biquad])>,
) -> BandMetrics {
    let centre = band.map(|b| b.0);
    let pad = band.map_or(0, |b| (TAIL_PER_BANDWIDTH / b.1 * fs).ceil() as usize);
    let e: Vec<f64> = match band {
        Some((_, _, s)) => filter(ir, s, true, 0).iter().map(|v| v * v).collect(),
        None => ir.iter().map(|v| v * v).collect(),
    };
    let time = |i: usize| t0_s + i as f64 / fs;
    let Some(d) = decay_part(&e, fs, band.map(|b| b.1), broadband_onset) else {
        return BandMetrics::none(centre, broadband_onset.map_or(t0_s, time));
    };
    let [edt_s, t20_s, t30_s] = d.times;
    let [c50_db, c80_db, d50] = match broadband_onset {
        Some(_) if d.range_db < CLARITY_RANGE_DB => {
            [Err(Refusal::InsufficientRange {
                range_db: d.range_db,
                needed_db: CLARITY_RANGE_DB,
            }); 3]
        }
        Some(start) => ratios(ir, fs, start, d.end, d.correction, band.map(|b| (b.2, pad))),
        None => [Err(Refusal::NoDecay); 3],
    };
    let curvature_pct = match (t20_s, t30_s) {
        (Ok(a), Ok(b)) => Some(100.0 * (b / a - 1.0)),
        _ => None,
    };
    BandMetrics {
        centre_hz: centre,
        onset_s: time(d.onset),
        truncation_s: time(d.end),
        decay_range_db: d.range_db,
        edt_s,
        t20_s,
        t30_s,
        c50_db,
        c80_db,
        d50,
        curvature_pct,
    }
}

/// Bands of `fraction` whose mid-band frequency is in `span` and whose edges lie inside the
/// excited range `excited` (Hz): (mid-band, bandwidth, filter).
fn bands(
    fraction: BandFraction,
    span: (f64, f64),
    excited: (f64, f64),
    fs: f64,
) -> Vec<(f64, f64, Vec<Biquad>)> {
    fraction
        .centres(span.0, span.1)
        .into_iter()
        .filter_map(|fm| {
            let (info, sections) = full_rate_band(fraction, fm, fs)?;
            (info.lower_hz >= excited.0 && info.upper_hz <= excited.1).then_some((
                info.centre_hz,
                info.upper_hz - info.lower_hz,
                sections,
            ))
        })
        .collect()
}

/// Parameters of the bands of `fraction` within `span` (mid-band, Hz) whose edges lie inside
/// `excited` (Hz). `ir[0]` is at `t0_s` on the caller's time axis.
pub fn analyse_bands(
    ir: &[f64],
    fs: f64,
    t0_s: f64,
    fraction: BandFraction,
    span: (f64, f64),
    excited: (f64, f64),
) -> Vec<BandMetrics> {
    let e: Vec<f64> = ir.iter().map(|v| v * v).collect();
    let start = onset(&e, TRIGGER_DB);
    bands(fraction, span, excited, fs)
        .iter()
        .map(|(fm, bw, s)| band_metrics(ir, fs, t0_s, start, Some((*fm, *bw, s))))
        .collect()
}

/// Every parameter of the impulse response `ir` at `fs`: broadband, octave bands 63 Hz …
/// 8 kHz and one-third-octave bands 50 Hz … 10 kHz, each band only where its edges lie
/// inside the excited range `excited` (Hz). `ir[0]` is at `t0_s` on the caller's time axis
/// (onsets and truncation points are reported on it).
pub fn analyse(ir: &[f64], fs: f64, t0_s: f64, excited: (f64, f64)) -> RoomAnalysis {
    let e: Vec<f64> = ir.iter().map(|v| v * v).collect();
    let start = onset(&e, TRIGGER_DB);
    RoomAnalysis {
        broadband: band_metrics(ir, fs, t0_s, start, None),
        octave: analyse_bands(ir, fs, t0_s, BandFraction::Octave, OCTAVE_SPAN_HZ, excited),
        third: analyse_bands(ir, fs, t0_s, BandFraction::Third, THIRD_SPAN_HZ, excited),
    }
}

#[cfg(test)]
mod tests;
