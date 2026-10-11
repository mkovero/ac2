//! Fractional-octave RTA: IEC 61260-1 filterbank (design Q4). The FFT-banded RTA lives in
//! [`crate::spectrum`]; both use base-10 band edges and report band power in FS².
//!
//! Each band is a digital Butterworth band-pass: the analog low-pass prototype of order N is
//! transformed to a band-pass whose −3 dB edges are the pre-warped IEC band edges, then
//! mapped by the bilinear transform, so the digital −3 dB points land exactly on the band
//! edges. The order per side is 3 (which meets class 1 with margin in the analog domain).
//! Near Nyquist the bilinear mapping compresses the lower skirt, and an order-3 band can
//! miss the class 1 minimum attenuation there (e.g. the 1/3-octave bands at 12.5 kHz and
//! above at 44.1/48 kHz). For such bands the order is raised one step at a time until the
//! response, evaluated against the class 1 mask on a dense grid below Nyquist, passes.
//! [`BandInfo::order`] reports what was used.
//!
//! # Multirate
//!
//! A band's filter only needs a sample rate a few times its upper edge, so the input is
//! halved repeatedly by half-band decimators and each band runs at the lowest rate that
//! keeps it accurate; the whole bank then costs about as much as its top two octaves at the
//! full rate. A band runs at stage k (rate fs/2^k) only if
//! - its upper edge is at most fs_k/4, so it lies well inside the decimators' pass band
//!   (flat to 0.2 of their input rate, i.e. 0.4 fs_k),
//! - its full response — decimator chain times band filter, including what aliases into it —
//!   meets the class 1 mask, and
//! - its group delay at the mid-band frequency is at most [`LATENCY_SLACK`] above that of
//!   the same band designed at the full rate, so a decimated band responds as fast as a
//!   full-rate one would (decimation filters add delay, and the narrow bands have plenty).
//!
//! Otherwise it runs at a higher rate (ultimately the full rate).
//!
//! The decimators are polyphase IIR half-bands (two branches of first-order all-pass
//! sections in z², elliptic design): pass band to 0.2 fs, stop band from 0.3 fs at
//! ≥ [`HALF_BAND_STOP_DB`] dB. Being power-complementary, their pass-band deviation is
//! below 10⁻⁹ dB, so band levels are unaffected; their non-linear phase is irrelevant to
//! band power. Any frequency at or above 0.6 fs_k lies in the stop band of some earlier
//! decimator before it can alias, so a stage-k band is attenuated by at least that much
//! there and the class 1 check only has to cover frequencies below 0.6 fs_k.
//!
//! All filtering is f64. The bands of one rate and order are run several at a time with
//! their states side by side, so the feedback latency of one band's recursion is hidden
//! behind the others' arithmetic and the compiler can use SIMD. f32 is not used even for the
//! decimated bands: their rounding error, amplified by the noise gain of high-Q sections,
//! sits about 117 dB below the band's own level for full-scale white noise at that rate
//! (≈ −140 dB FS² in a 1/24-octave band), which a 24-bit input can resolve next to a loud
//! neighbouring band; at the full rate it would be far worse. The decimated stages are also
//! the minority of the work, so f32 would buy little.
//!
//! Band power is the mean square of each band's output samples at its own rate over the
//! averaging interval. Bands at a decimated rate get one output per 2^k input samples; a
//! band that has not produced an output sample since the last reset repeats its previous
//! interval's power.
//!
//! Bands whose upper edge is at or above Nyquist are not built.

mod class1;
mod design;
mod group;
mod half_band;

pub use class1::class1_limits;
pub use design::butterworth_bandpass;
pub use half_band::HALF_BAND_STOP_DB;

use crate::grid::{iec_band_centres, iec_band_edges};
use crate::weighting::Biquad;
use class1::meets_class1;
use design::{cascade_response, group_delay};
use group::{Group, LANES};
use half_band::{HALF_BAND_COEFS, HalfBand, chain_magnitude, chain_response, half_band_coefs};
use num_complex::Complex64;
use std::fmt;

/// Bandwidth designator 1/b of a fractional-octave band set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BandFraction {
    /// 1/1 octave.
    Octave,
    /// 1/3 octave.
    Third,
    /// 1/6 octave.
    Sixth,
    /// 1/12 octave.
    Twelfth,
    /// 1/24 octave.
    TwentyFourth,
}

impl BandFraction {
    /// The designator b.
    pub fn b(self) -> u32 {
        match self {
            BandFraction::Octave => 1,
            BandFraction::Third => 3,
            BandFraction::Sixth => 6,
            BandFraction::Twelfth => 12,
            BandFraction::TwentyFourth => 24,
        }
    }

    /// Exact IEC mid-band frequencies within `[f_lo, f_hi]`.
    pub fn centres(self, f_lo: f64, f_hi: f64) -> Vec<f64> {
        iec_band_centres(self.b(), f_lo, f_hi)
    }

    /// IEC band edges of the band centred at `fm`.
    pub fn edges(self, fm: f64) -> (f64, f64) {
        iec_band_edges(fm, self.b())
    }
}

/// One band of the filterbank.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandInfo {
    /// Exact mid-band frequency f_m in Hz.
    pub centre_hz: f64,
    /// Lower band edge in Hz.
    pub lower_hz: f64,
    /// Upper band edge in Hz.
    pub upper_hz: f64,
    /// Butterworth order per side actually used (3 unless raised near Nyquist).
    pub order: usize,
    /// Whether the band's full response (decimators and band filter) meets the IEC 61260-1
    /// class 1 mask below Nyquist.
    pub meets_class1: bool,
    /// Decimation factor 2^k of the rate the band filter runs at (1 = full rate).
    pub decimation: u32,
}

/// Lowest and highest order tried when designing a band.
const BASE_ORDER: usize = 3;
const MAX_ORDER: usize = 6;

/// Allowed relative increase of a decimated band's mid-band group delay over the same band
/// designed at the full rate.
pub const LATENCY_SLACK: f64 = 0.1;

/// Deepest decimation stage (2^16 is far below any audio band at audio rates).
const MAX_STAGE: u32 = 16;

/// A band runs at stage k only if its upper edge is at most this fraction of fs_k; the
/// decimators are flat to 0.4 fs_k, which leaves the band's upper skirt room to fall.
const MAX_UPPER_EDGE_FRACTION: f64 = 0.25;

/// One rate of the bank: the bands that run at fs/2^k and the decimator to the next rate.
#[derive(Debug, Clone)]
struct Stage {
    groups: Vec<Group>,
    /// Output samples at this rate since the last reset of the powers.
    samples: u64,
    /// Decimator from this rate to the next; `None` for the last stage.
    down: Option<HalfBand>,
    /// This stage's input at its own rate (unused for the full-rate stage).
    buf: Vec<f64>,
}

/// Filterbank construction failure.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RtaError {
    /// Sample rate is not positive and finite.
    InvalidRate(f64),
    /// No complete band lies within the requested range below Nyquist.
    NoBands,
}

impl fmt::Display for RtaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RtaError::InvalidRate(fs) => write!(f, "invalid sample rate {fs} Hz"),
            RtaError::NoBands => write!(f, "no band fits in the requested range below Nyquist"),
        }
    }
}

impl std::error::Error for RtaError {}

#[derive(Debug, Clone)]
struct Band {
    info: BandInfo,
    /// Decimation stage k.
    stage: u32,
    /// Unit-gain sections at fs/2^k (for the response).
    sections: Vec<Biquad>,
    /// Square of the product of the section gains, factored out of the running filter.
    gain_sq: f64,
    /// Where the band's running energy lives: (stage, group, lane).
    slot: (usize, usize, usize),
    /// Mean square of the last interval that produced output, if any.
    last_power: f64,
}

/// The lowest order in `BASE_ORDER..=MAX_ORDER` whose full response at stage `stage` meets
/// class 1, or `MAX_ORDER` if none does.
fn design_band(
    hb: &[f64; HALF_BAND_COEFS],
    fm: f64,
    (lower, upper): (f64, f64),
    b: u32,
    fs: f64,
    stage: u32,
) -> (usize, Vec<Biquad>, bool) {
    let fs_k = fs / f64::from(1u32 << stage);
    // Above 0.6 fs_k the decimator chain alone exceeds the far stop-band minimum.
    let f_max = if stage == 0 {
        0.5 * fs
    } else {
        (0.6 * fs_k).min(0.5 * fs)
    };
    let mut last = None;
    for order in BASE_ORDER..=MAX_ORDER {
        let sections = butterworth_bandpass(lower, upper, fs_k, order);
        let mag = |f: f64| {
            chain_magnitude(hb, stage, f, fs) * cascade_response(&sections, f, fs_k).norm()
        };
        let ok = meets_class1(mag, fm, b, f_max);
        if ok {
            return (order, sections, true);
        }
        last = Some((order, sections, false));
    }
    last.expect("at least one order tried")
}

/// One band of `fraction` centred at `fm`, designed to run at the full rate `fs`: the
/// bank's design without decimation (order 3 per side, raised near Nyquist until the
/// class 1 mask holds). `None` when the band's upper edge is at or above Nyquist. For
/// offline filtering of a whole record (room acoustics), where the bank's multirate saving
/// buys nothing and its decimators' phase would only add delay.
pub fn full_rate_band(fraction: BandFraction, fm: f64, fs: f64) -> Option<(BandInfo, Vec<Biquad>)> {
    let (lower, upper) = fraction.edges(fm);
    if !(fs.is_finite() && fs > 0.0) || upper >= 0.5 * fs {
        return None;
    }
    let (order, sections, ok) =
        design_band(&half_band_coefs(), fm, (lower, upper), fraction.b(), fs, 0);
    Some((
        BandInfo {
            centre_hz: fm,
            lower_hz: lower,
            upper_hz: upper,
            order,
            meets_class1: ok,
            decimation: 1,
        },
        sections,
    ))
}

/// IEC 61260-1 fractional-octave filterbank. Accumulates the mean square of each band's
/// output (band power in FS²) between reads.
#[derive(Debug, Clone)]
pub struct OctaveFilterBank {
    fs: f64,
    fraction: BandFraction,
    half_band: [f64; HALF_BAND_COEFS],
    bands: Vec<Band>,
    stages: Vec<Stage>,
    samples: u64,
}

impl OctaveFilterBank {
    /// Builds the bands whose exact mid-band frequency lies in `[f_lo, f_hi]` and whose upper
    /// edge is below Nyquist.
    pub fn new(fraction: BandFraction, fs: f64, f_lo: f64, f_hi: f64) -> Result<Self, RtaError> {
        if !(fs.is_finite() && fs > 0.0) {
            return Err(RtaError::InvalidRate(fs));
        }
        let b = fraction.b();
        let hb = half_band_coefs();
        let mut bands: Vec<Band> = fraction
            .centres(f_lo, f_hi)
            .into_iter()
            .filter_map(|fm| {
                let (lower, upper) = fraction.edges(fm);
                if upper >= 0.5 * fs {
                    return None;
                }
                // Orders are only raised near Nyquist, far above the fs/8 below which a band
                // can be decimated, so the base-order full-rate design sets the delay budget.
                let full_base = butterworth_bandpass(lower, upper, fs, BASE_ORDER);
                let budget = (1.0 + LATENCY_SLACK)
                    * group_delay(|f| cascade_response(&full_base, f, fs), fm);
                let deepest = (1..=MAX_STAGE)
                    .take_while(|&k| upper <= MAX_UPPER_EDGE_FRACTION * fs / f64::from(1u32 << k))
                    .last()
                    .unwrap_or(0);
                let (stage, (order, sections, ok)) = (1..=deepest)
                    .rev()
                    .find_map(|k| {
                        let fs_k = fs / f64::from(1u32 << k);
                        let delay = |sections: &[Biquad]| {
                            group_delay(
                                |f| {
                                    chain_response(&hb, k, f, fs)
                                        * cascade_response(sections, f, fs_k)
                                },
                                fm,
                            )
                        };
                        // Latency first: it only needs the base-order design and is cheap.
                        let base = butterworth_bandpass(lower, upper, fs_k, BASE_ORDER);
                        if delay(&base) > budget {
                            return None;
                        }
                        let d = design_band(&hb, fm, (lower, upper), b, fs, k);
                        (d.2 && (d.0 == BASE_ORDER || delay(&d.1) <= budget)).then_some((k, d))
                    })
                    .unwrap_or_else(|| (0, design_band(&hb, fm, (lower, upper), b, fs, 0)));
                let gain_sq = sections.iter().map(|s| s.b[0] * s.b[0]).product();
                Some(Band {
                    info: BandInfo {
                        centre_hz: fm,
                        lower_hz: lower,
                        upper_hz: upper,
                        order,
                        meets_class1: ok,
                        decimation: 1 << stage,
                    },
                    stage,
                    sections,
                    gain_sq,
                    slot: (0, 0, 0),
                    last_power: f64::NAN,
                })
            })
            .collect();
        if bands.is_empty() {
            return Err(RtaError::NoBands);
        }
        let n_stages = bands.iter().map(|b| b.stage).max().unwrap_or(0) as usize + 1;
        let mut stages: Vec<Stage> = (0..n_stages)
            .map(|k| Stage {
                groups: Vec::new(),
                samples: 0,
                down: (k + 1 < n_stages).then(|| HalfBand::new(hb)),
                buf: Vec::new(),
            })
            .collect();
        for (k, stage) in stages.iter_mut().enumerate() {
            for order in BASE_ORDER..=MAX_ORDER {
                let members: Vec<usize> = (0..bands.len())
                    .filter(|&i| bands[i].stage as usize == k && bands[i].info.order == order)
                    .collect();
                for chunk in members.chunks(LANES) {
                    let lanes: Vec<(usize, &[Biquad])> = chunk
                        .iter()
                        .map(|&i| (i, bands[i].sections.as_slice()))
                        .collect();
                    let g = stage.groups.len();
                    stage.groups.push(Group::new(&lanes));
                    for (l, &i) in chunk.iter().enumerate() {
                        bands[i].slot = (k, g, l);
                    }
                }
            }
        }
        Ok(Self {
            fs,
            fraction,
            half_band: hb,
            bands,
            stages,
            samples: 0,
        })
    }

    /// Sample rate.
    pub fn fs(&self) -> f64 {
        self.fs
    }

    /// Band fraction.
    pub fn fraction(&self) -> BandFraction {
        self.fraction
    }

    /// Number of bands.
    pub fn len(&self) -> usize {
        self.bands.len()
    }

    /// True if there are no bands (never for a constructed bank).
    pub fn is_empty(&self) -> bool {
        self.bands.is_empty()
    }

    /// Description of band `i`.
    pub fn band(&self, i: usize) -> BandInfo {
        self.bands[i].info
    }

    /// Descriptions of all bands, low to high.
    pub fn bands(&self) -> impl Iterator<Item = BandInfo> + '_ {
        self.bands.iter().map(|b| b.info)
    }

    /// Complex response of band `i` to a sinusoid at `f` Hz (below Nyquist), decimators
    /// included: for a band at a decimated rate the magnitude is that of whatever alias of
    /// `f` reaches the band filter, so it gives the band power a steady sine produces.
    pub fn response(&self, i: usize, f: f64) -> Complex64 {
        let band = &self.bands[i];
        let fs_k = self.fs / f64::from(1u32 << band.stage);
        chain_response(&self.half_band, band.stage, f, self.fs)
            * cascade_response(&band.sections, f, fs_k)
    }

    /// Filters a block through every band and accumulates the output energy.
    pub fn process(&mut self, block: &[f64]) {
        for k in 0..self.stages.len() {
            let (head, tail) = self.stages.split_at_mut(k + 1);
            let stage = &mut head[k];
            let x = if k == 0 { block } else { stage.buf.as_slice() };
            for g in &mut stage.groups {
                g.process(x);
            }
            stage.samples += x.len() as u64;
            if let (Some(down), Some(next)) = (&mut stage.down, tail.first_mut()) {
                next.buf.clear();
                down.process(x, &mut next.buf);
            }
        }
        self.samples += block.len() as u64;
    }

    /// Samples accumulated since the last [`OctaveFilterBank::reset_powers`].
    pub fn samples(&self) -> u64 {
        self.samples
    }

    /// Mean square of band `band` since the last reset; its previous interval's when its
    /// rate has produced no output yet.
    fn power(&self, band: &Band) -> f64 {
        let (k, g, l) = band.slot;
        let stage = &self.stages[k];
        if stage.samples == 0 {
            band.last_power
        } else {
            band.gain_sq * stage.groups[g].energy[l] / stage.samples as f64
        }
    }

    /// Mean-square output per band (FS²) since the last reset; NaN if nothing was accumulated.
    ///
    /// # Panics
    /// If `out.len() != self.len()`.
    pub fn band_powers(&self, out: &mut [f64]) {
        assert_eq!(out.len(), self.bands.len(), "output length");
        for (o, b) in out.iter_mut().zip(&self.bands) {
            *o = if self.samples == 0 {
                f64::NAN
            } else {
                self.power(b)
            };
        }
    }

    /// Starts a new averaging interval; the filter state is kept so there is no new transient.
    pub fn reset_powers(&mut self) {
        if self.samples > 0 {
            for i in 0..self.bands.len() {
                let p = self.power(&self.bands[i]);
                self.bands[i].last_power = p;
            }
        }
        for s in &mut self.stages {
            for g in &mut s.groups {
                g.energy = [0.0; LANES];
            }
            s.samples = 0;
        }
        self.samples = 0;
    }

    /// Clears filter state and accumulated power.
    pub fn reset(&mut self) {
        for s in &mut self.stages {
            for g in &mut s.groups {
                g.reset();
            }
            if let Some(d) = &mut s.down {
                d.reset();
            }
            s.samples = 0;
        }
        for b in &mut self.bands {
            b.last_power = f64::NAN;
        }
        self.samples = 0;
    }
}

#[cfg(test)]
mod tests;
