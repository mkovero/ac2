//! Harmonics of a steady sine of known frequency, read from an averaged power spectrum.
//!
//! # Tone power by main-lobe sum
//! A windowed sine A·sin(2πf·t) puts the window's transform, centred on c = f·N/fs bins, into
//! the spectrum. By Parseval the folded powers F_k of that tone sum to N·S₂·A²/2 over all
//! bins wherever c sits between bin centres, so
//!
//! ```text
//! power = Σ_lobe F_k / (N·S₂) / captured
//! ```
//!
//! is the tone's mean square, where the sum runs over the main lobe (the bins between the
//! first nulls either side of c, [`Window::lobe_half_width`]) and `captured` is the fraction
//! of the window's energy those bins hold: Σ_lobe |W(k − c)|² / (N·S₂), from the window's own
//! transform ([`Window::transform_power`]). On a bin every other bin is in a null and
//! `captured` is exactly 1.
//!
//! The same scale reads noise correctly: white noise of variance σ² has a mean F_k of 2·σ²·S₂, so
//! a sum over m bins reads σ²·m/(N/2), its power in the band those bins span.
//!
//! # What cannot be read
//! - The rectangular window's transform falls only as 1/Δ off a bin: a sine between bins
//!   spreads over the whole spectrum and no lobe sum means anything
//!   ([`Unreadable::RectangularOffBin`]).
//! - Harmonics closer together than a lobe and the bins between lobes that the floor needs
//!   ([`Unreadable::TooLow`]).
//! - A harmonic whose lobe reaches fs/2 has folded back onto other bins; the list stops
//!   before it.
//!
//! # Floor
//! Each harmonic comes with the power its lobe would hold with no harmonic there: the level
//! of the bins just outside the lobe (noise, or the skirt of the fundamental) times the
//! lobe's bin count. A harmonic is [`Harmonic::resolved`] when it stands
//! [`RESOLVED_MARGIN_DB`] above that; otherwise its `power` is the floor's, and only
//! "below `floor`" can be said of it.

use crate::spectrum::PowerSpectrum;
use crate::sweep::FLOOR_MARGIN_DB;
use crate::window::Window;
use std::fmt;
use std::ops::RangeInclusive;

#[cfg(test)]
mod tests;

/// A harmonic counts as read when its lobe holds this much more than its floor. At 6 dB the
/// floor's own power adds at most 10·lg(4/3) = 1.25 dB to the reading, the same rule the
/// sweep's distortion curves use, so a distortion figure means the same from either
/// measurement.
pub const RESOLVED_MARGIN_DB: f64 = FLOOR_MARGIN_DB;

/// Lobe widths of spectrum on each side of a harmonic that estimate its floor: enough bins
/// for a median to ignore a stray line, few enough to stay local on a sloping skirt.
const FLOOR_LOBES: usize = 3;

/// A tone is on a bin when f·N/fs is within this many bins of an integer, per sample of the
/// frame. An f64 phase accumulator stepping by f/fs carries a relative error of 2⁻⁵³ in the
/// step and as much again per addition, so over N samples a sine meant for a bin drifts by
/// about N·ε of a bin; the product f·N/fs itself rounds by at most (N/2)·ε. Eight times N·ε
/// covers both, and at N = 2²⁰ is 2·10⁻⁹ bin, whose leakage (amplitude ∝ offset) lies
/// 170 dB below the tone.
const ON_BIN_EPS_PER_SAMPLE: f64 = 8.0 * f64::EPSILON;

/// Why a spectrum has no reading for the sine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Unreadable {
    /// The rectangular window reads a sine only on a bin; between bins its leakage falls as
    /// 1/Δ and covers every harmonic.
    RectangularOffBin,
    /// The frequency is so low for this FFT length that the lobes of DC, the sine and its
    /// harmonics leave no bins between them.
    TooLow,
    /// The frequency is not a positive finite number, or the sine's own lobe reaches fs/2.
    OutOfBand,
    /// The lobe of the sine holds no power: there is nothing to refer the harmonics to.
    NoFundamental,
}

impl fmt::Display for Unreadable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Unreadable::RectangularOffBin => "rectangular window needs the sine on a bin",
            Unreadable::TooLow => "fundamental too low for this FFT length",
            Unreadable::OutOfBand => "sine frequency outside the analysed band",
            Unreadable::NoFundamental => "no signal at the sine frequency",
        })
    }
}

impl std::error::Error for Unreadable {}

/// One harmonic of the sine. Powers are mean squares (FS²).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Harmonic {
    /// Order k: the harmonic at k·f0.
    pub order: u32,
    /// Power in the harmonic's lobe.
    pub power: f64,
    /// Power the lobe would hold from the noise or skirt around it.
    pub floor: f64,
    /// `power` stands [`RESOLVED_MARGIN_DB`] above `floor`.
    pub resolved: bool,
}

/// The reading of a steady sine and its harmonics. Powers are mean squares (FS²), ratios are
/// power ratios.
#[derive(Debug, Clone, PartialEq)]
pub struct Harmonics {
    /// Frequency of the sine, Hz, as given.
    pub f0: f64,
    /// The sine sits on a bin centre ([`is_on_bin`]).
    pub on_bin: bool,
    /// Power of the fundamental.
    pub fundamental: f64,
    /// Harmonics 2, 3, … in order, up to the requested order or the last whose lobe lies
    /// below fs/2.
    pub harmonics: Vec<Harmonic>,
    /// Σ resolved harmonic powers / fundamental. Zero when none is resolved; then
    /// `thd_bound` is what can be said.
    pub thd: f64,
    /// Σ max(power, floor) over all harmonics read / fundamental: the distortion is not
    /// above this.
    pub thd_bound: f64,
    /// Power of every bin outside the lobes of DC and the fundamental / fundamental:
    /// harmonics, noise and whatever else is there. White noise of variance σ² alone reads
    /// σ² / fundamental.
    pub thd_n: f64,
    /// Fraction of the fundamental's power that its window spreads outside its lobe (zero on
    /// a bin). It is part of `thd_n`, which therefore cannot read below it.
    pub leakage: f64,
}

/// The frequency of the bin centre nearest `f`: k·fs/n with 1 ≤ k < n/2, so that DC and
/// fs/2 are never chosen. `n` must be at least 3 for such a bin to exist.
pub fn snap_to_bin(f: f64, fs: f64, n: usize) -> f64 {
    let last = (n.saturating_sub(1) / 2).max(1) as f64;
    // NaN and negative frequencies land on the first bin.
    let k = (f * n as f64 / fs).round().max(1.0).min(last);
    k * fs / n as f64
}

/// Whether a sine of frequency `f` sits on a bin centre of an `n`-point FFT at rate `fs`,
/// to within what an f64 generator keeps over the frame.
pub fn is_on_bin(f: f64, fs: f64, n: usize) -> bool {
    let c = f * n as f64 / fs;
    (c - c.round()).abs() <= ON_BIN_EPS_PER_SAMPLE * n as f64
}

/// The bins that hold a tone and how much of it they hold.
#[derive(Debug, Clone)]
struct Lobe {
    bins: RangeInclusive<usize>,
    on_bin: bool,
    /// Fraction of the tone's power inside `bins`.
    captured: f64,
}

/// Everything about the spectrum that the readings share.
struct Reader<'a> {
    folded: &'a [f64],
    n: usize,
    fs: f64,
    window: Window,
    /// Lobe half-width K, bins.
    half: usize,
    /// 1 / (N·S₂): folded power to mean square.
    scale: f64,
}

impl<'a> Reader<'a> {
    fn new(spectrum: &PowerSpectrum<'a>, window: Window) -> Self {
        let n = spectrum.n();
        Self {
            folded: spectrum.folded(),
            n,
            fs: spectrum.fs(),
            window,
            half: window.lobe_half_width(),
            scale: 1.0 / (n as f64 * spectrum.gains().sum_sq),
        }
    }

    /// The lobe of a tone at `c` bins, or `None` when it would start below bin 0. On a bin
    /// the transform is zero from ±K outwards, so the lobe is the 2K − 1 bins that are not
    /// nulls; between bins it is the 2K bins between the nulls at c ± K.
    fn lobe(&self, c: f64, on_bin: bool) -> Option<Lobe> {
        let k = self.half as f64;
        let (lo, hi) = if on_bin {
            (c.round() - (k - 1.0), c.round() + (k - 1.0))
        } else {
            ((c - k).ceil(), (c + k).floor())
        };
        if lo < 0.0 {
            return None;
        }
        let (lo, hi) = (lo as usize, hi as usize);
        let captured = if on_bin {
            1.0
        } else {
            (lo..=hi)
                .map(|b| self.window.transform_power(b as f64 - c, self.n))
                .sum::<f64>()
                * self.scale
        };
        Some(Lobe {
            bins: lo..=hi,
            on_bin,
            captured,
        })
    }

    /// Whether the lobe lies wholly below fs/2.
    fn below_nyquist(&self, lobe: &Lobe) -> bool {
        2 * lobe.bins.end() < self.n
    }

    /// The lobe of a tone at frequency `f`, checked for everything that makes a single tone
    /// unreadable.
    fn tone_lobe(&self, f: f64) -> Result<Lobe, Unreadable> {
        if !(f.is_finite() && f > 0.0) {
            return Err(Unreadable::OutOfBand);
        }
        let on_bin = is_on_bin(f, self.fs, self.n);
        if self.window == Window::Rectangular && !on_bin {
            return Err(Unreadable::RectangularOffBin);
        }
        let lobe = self
            .lobe(f * self.n as f64 / self.fs, on_bin)
            .ok_or(Unreadable::TooLow)?;
        // The window puts DC into bins 0..K, which no tone may share.
        if *lobe.bins.start() < self.half {
            return Err(Unreadable::TooLow);
        }
        if !self.below_nyquist(&lobe) {
            return Err(Unreadable::OutOfBand);
        }
        Ok(lobe)
    }

    fn power(&self, lobe: &Lobe) -> f64 {
        self.folded[lobe.bins.clone()].iter().sum::<f64>() * self.scale / lobe.captured
    }

    /// Median folded power of `bins`; `None` for an empty range.
    fn median(&self, bins: RangeInclusive<usize>, scratch: &mut Vec<f64>) -> Option<f64> {
        scratch.clear();
        if bins.start() <= bins.end() {
            scratch.extend_from_slice(&self.folded[bins]);
        }
        if scratch.is_empty() {
            return None;
        }
        scratch.sort_unstable_by(f64::total_cmp);
        let mid = scratch.len() / 2;
        Some(if scratch.len() % 2 == 1 {
            scratch[mid]
        } else {
            0.5 * (scratch[mid - 1] + scratch[mid])
        })
    }

    /// The power `lobe` would hold from what surrounds it: the per-bin level either side,
    /// between the neighbouring lobes that end at `below` and start at `above`, times the
    /// lobe's bin count. A median, so that a line or a neighbour's edge among the bins does
    /// not set the level. A skirt falls as a power of the distance, i.e. along a straight
    /// line in log power over the short span of a lobe, so the level under the lobe is the
    /// geometric mean of the two sides; for a flat noise floor that is the same number.
    fn floor(&self, lobe: &Lobe, below: usize, above: usize, scratch: &mut Vec<f64>) -> f64 {
        let reach = FLOOR_LOBES * 2 * self.half;
        let (lo, hi) = (*lobe.bins.start(), *lobe.bins.end());
        let left = self.median((below + 1).max(lo.saturating_sub(reach))..=lo - 1, scratch);
        let right = self.median(hi + 1..=(above.saturating_sub(1)).min(hi + reach), scratch);
        let level = match (left, right) {
            (Some(l), Some(r)) => (l * r).sqrt(),
            (Some(v), None) | (None, Some(v)) => v,
            (None, None) => 0.0,
        };
        let count = (hi - lo + 1) as f64;
        level * count * self.scale / lobe.captured
    }
}

/// Mean-square power (FS²) of the tone at frequency `f` in `spectrum`, which an analyzer
/// with `window` produced: the main-lobe sum of the module documentation. It reads whatever
/// the lobe holds, so with no tone there it is the noise in the lobe's bandwidth.
pub fn tone_power(spectrum: &PowerSpectrum<'_>, window: Window, f: f64) -> Result<f64, Unreadable> {
    let reader = Reader::new(spectrum, window);
    let lobe = reader.tone_lobe(f)?;
    Ok(reader.power(&lobe))
}

/// Reads the sine of frequency `f0` and its harmonics 2..=`max_order` from `spectrum`,
/// which an analyzer with `window` produced.
///
/// One pass over the spectrum for `thd_n`; everything else touches only the lobes and the
/// bins beside them.
pub fn read_harmonics(
    spectrum: &PowerSpectrum<'_>,
    window: Window,
    f0: f64,
    max_order: u32,
) -> Result<Harmonics, Unreadable> {
    let reader = Reader::new(spectrum, window);
    let first = reader.tone_lobe(f0)?;
    let c = f0 * reader.n as f64 / reader.fs;
    // Lobes are at most 2K bins wide and c apart: with c ≥ 2K + 2 at least one bin lies
    // between any two of them (and between DC's and the fundamental's) for the floor.
    if c < (2 * reader.half + 2) as f64 {
        return Err(Unreadable::TooLow);
    }
    let fundamental = reader.power(&first);
    if fundamental.is_nan() || fundamental <= 0.0 {
        return Err(Unreadable::NoFundamental);
    }

    let margin = 10f64.powf(RESOLVED_MARGIN_DB / 10.0);
    let last_bin = reader.folded.len() - 1;
    // Harmonics of a sine on a bin are on bins; between bins each has its own offset.
    let lobe_of = |order: u32| reader.lobe(order as f64 * c, first.on_bin);
    let mut harmonics = Vec::new();
    let mut scratch = Vec::new();
    let (mut thd, mut thd_bound) = (0.0, 0.0);
    let mut below = first.clone();
    let mut here = lobe_of(2);
    for order in 2..=max_order {
        let Some(lobe) = here.filter(|l| reader.below_nyquist(l)) else {
            break;
        };
        let next = lobe_of(order + 1);
        // The next harmonic bounds the floor's bins even when it is not read; past the
        // spectrum's end the last bin does.
        let above = next
            .as_ref()
            .map_or(last_bin + 1, |l| (*l.bins.start()).min(last_bin + 1));
        let power = reader.power(&lobe);
        let floor = reader.floor(&lobe, *below.bins.end(), above, &mut scratch);
        let resolved = power > floor * margin;
        if resolved {
            thd += power;
        }
        thd_bound += power.max(floor);
        harmonics.push(Harmonic {
            order,
            power,
            floor,
            resolved,
        });
        below = lobe;
        here = next;
    }

    // Summed directly, not as total − lobes: the residual may lie 14 decades under the
    // fundamental and would not survive the subtraction.
    let residual = reader.folded[reader.half..*first.bins.start()]
        .iter()
        .chain(&reader.folded[first.bins.end() + 1..])
        .sum::<f64>()
        * reader.scale;

    Ok(Harmonics {
        f0,
        on_bin: first.on_bin,
        fundamental,
        harmonics,
        thd: thd / fundamental,
        thd_bound: thd_bound / fundamental,
        thd_n: residual / fundamental,
        leakage: (1.0 - first.captured).max(0.0),
    })
}
