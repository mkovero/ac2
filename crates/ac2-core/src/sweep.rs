//! Swept-sine (Farina) analysis: the linear response and harmonic distortion of a system
//! from synchronised exponential sweeps recorded on a reference (loopback) and a measurement
//! input. Design: `docs/design/sweep-distortion.md`.
//!
//! # Deconvolution
//!
//! Each repeat is deconvolved twice, by regularised spectral division of a record by a
//! sweep, `H = X·S* / (|S|² + ε)`, and repeats are averaged as complex spectra.
//!
//! The **linear response** (magnitude, phase, IR, arrival, room) divides the measurement
//! record by the reference record of the same capture. Dividing by the measured reference
//! cancels every latency and the converters' own response, so t = 0 is "when the loopback
//! heard it" (the transfer function's time base).
//!
//! The **harmonics** divide each input by the emitted sweep itself, synthesised sample for
//! sample as the generator played it (fades included). With `f1·L` an integer,
//! `sin(k·φ(t)) = sin φ(t + L·ln k)`: the k-th harmonic of the sweep is the sweep advanced by
//! `L·ln k`, and its impulse response appears at `−L·ln k` (Farina). That holds only for a
//! divisor that is the clean sweep. The loopback is not one: its converters distort too, and a
//! divisor `s + ε·s²` puts its H2 into the quotient with the opposite sign, so distortion both
//! paths share (the interface's DAC) cancels and distortion on the reference input alone is
//! added. The emitted sweep is aligned to the capture by the reference's own delay against it,
//! its linear impulse's peak to a fraction of a sample (as the arrival is found): the two time
//! bases then coincide and the measurement's harmonic impulses sit at `d − L·ln k`, `d` the
//! arrival found re the reference. The reference lends its timing, not its distortion. A
//! drift ε between the output and the input clock is not cancelled against the emitted sweep:
//! it smears each harmonic impulse over ε times the sweep's length (0.5 ms at 50 ppm over
//! 10 s), far inside a harmonic window, so the band powers do not change.
//!
//! Both terms of the distortion ratio come from the same division. The fundamental `P_1` is
//! the measurement's linear response re the emitted sweep, not re the reference: the
//! interface's output response then sits in `P_1(f)` and `P_k(k·f)` alike and cancels in the
//! ratio wherever it is flat over `f … k·f`, so the dBr is the measured chain's own (as a
//! steady sine's harmonics re its own fundamental), whatever the loopback's gain. A `P_1` re
//! the reference would carry the loopback's level and response into every ratio.
//!
//! The reference input's own harmonics are separated the same way
//! ([`SweepAnalysis::reference_harmonics`]). Where the loopback shares the measured path's
//! converters, its distortion bounds what the measurement can claim: a harmonic within a few
//! dB of the reference's own is the interface's, not the system's.
//!
//! # Windows
//!
//! Orders `k` and `k+1` lie `L·ln((k+1)/k)` apart; the gap shrinks with `k`, so the highest
//! order analysed sets one window (10 % of the gap above it before `t_k`, 90 % of the gap
//! below it after; once capped, at least a third before `t_k`) used for every order, for the
//! fundamental's distortion reference and for the noise estimate. Equal windows give equal
//! noise and time resolution, so ratios compare like with like.
//!
//! # Distortion and validity
//!
//! Power is averaged over 1/24 octave ([`DISTORTION_BAND_OCT`]), widened to a few of the
//! window's resolution cells at low frequencies. Harmonic `k` at fundamental `f` is
//! `P_k(k·f) / P_1(f)`, `P_1` averaged over the fundamentals whose harmonic lies in `P_k`'s
//! band (`1/k` of its width in hertz); THD is the sum of those ratios over the orders in band.
//! An order is reported only where its whole band lies below the fade-out, where the emitted
//! sweep still plays at full level. The
//! noise floor is the same window cut from the deconvolved silence after the response; a
//! point counts when it is [`FLOOR_MARGIN_DB`] above that floor ([`is_valid`]).
//!
//! # Room parameters
//!
//! The averaged IR from just after H2's window to the end of the silence after the sweep
//! goes through [`crate::room::analyse`] (ISO 3382-1 per band). The span ends there because
//! a lag beyond the silence would pair the late part of the sweep with samples the record
//! does not have: the deconvolved noise would thin out with the lag and read as decay.

use crate::generator::{EssConfig, EssPlan, GeneratorError};
use crate::grid::LogGrid;
use crate::room::RoomAnalysis;

mod analyse;
mod locate;
mod spectral;

pub use analyse::analyse_recording;
pub(crate) use spectral::{column_mean, fft_forward, fine_peak, taper, wrap_deg};

/// Highest harmonic order analysed by default (H2 … H5).
pub const DEFAULT_MAX_ORDER: u8 = 5;
/// Highest harmonic order the analysis accepts.
pub const MAX_ORDER: u8 = 9;
/// Fraction of the gap to the next-higher order a harmonic window starts before `t_k`.
pub const PRE_FRACTION: f64 = 0.1;
/// A distortion point is valid when it is this far above the noise in its window: the noise
/// then adds at most 1.25 dB (10·log10(4/3)).
pub const FLOOR_MARGIN_DB: f64 = 6.0;
/// Width of the power average around each frequency of a distortion curve, octaves.
pub const DISTORTION_BAND_OCT: f64 = 1.0 / 24.0;
/// The power average spans at least this many resolution cells (1/window) of the
/// harmonic window: one cell of noise is exponentially distributed and exceeds four times
/// its mean 2 % of the time, three cells averaged almost never.
pub const DISTORTION_MIN_CELLS: f64 = 3.0;
/// The noise floor is averaged over this many octaves (and at least twice the cells of a
/// distortion point): noise is smooth in frequency, and a steady floor keeps one noisy
/// estimate from being compared with another.
pub const FLOOR_BAND_OCT: f64 = 1.0 / 3.0;
/// A harmonic's floor is the noise read beside its own window, in the gaps of the impulse
/// response on either side, where that is more than this factor (3 dB) above the post-roll's.
/// The post-roll noise windows read each frequency some 0.5–1 s after the sweep passed it; the
/// gaps read it within a fraction of a second of the harmonic's own moment. Noise that comes
/// in bursts (hum switching on and off) can fill the harmonic's moment while the post-roll is
/// quiet, and then reads as distortion over a floor that never saw it. Stationary noise reads
/// the same in both, and a single gap window scatters about ±2 dB around it.
pub const GAP_NOISE_FACTOR: f64 = 2.0;
/// Longest harmonic window, seconds: it still resolves fundamentals down to 2/W = 20 Hz.
pub const MAX_WINDOW_S: f64 = 0.1;
/// The emitted sweep starts at least this factor (two octaves) below the asked start
/// frequency, unless held up by [`MIN_EMITTED_START_HZ`], and rises to full level over those
/// octaves. A path answers a sweep's switch-on with a
/// transient over roughly its first two octaves; that transient does not follow the sweep's
/// phase, so the deconvolution cannot tell it from the harmonic impulses and books it as
/// distortion of the lowest fundamentals (on an electrical path, H2 up to 22 dB above the
/// steady-sine value). Starting lower puts it below the analysed band; the rising level keeps
/// the added low frequencies, where a loudspeaker's excursion grows fastest, below full level.
pub const ONSET_EXTENSION: f64 = 4.0;
/// The noise floor is averaged over the windows that fit from this fraction of the post-roll
/// to its end: the post-roll is long enough for the system's decay (see
/// [`SweepSpec::tail_s`]), which is meant to be over by its second half.
const NOISE_REGION_START: f64 = 0.5;
/// The emitted sweep never starts below this, Hz.
pub const MIN_EMITTED_START_HZ: f64 = 1.0;
/// Audio kept before each sweep's onset when the repeats are cut apart, seconds.
pub const PRE_ROLL_S: f64 = 0.1;
/// Shortest silence after each sweep, seconds.
pub const MIN_POST_ROLL_S: f64 = 1.0;
/// Longest silence after each sweep the analysis accepts, seconds (a cathedral's decay with
/// room for its noise).
pub const MAX_TAIL_S: f64 = 20.0;
/// Harmonics are analysed up to this fraction of the sample rate (anti-alias filters roll
/// off above it).
pub const HARMONIC_FS_FRACTION: f64 = 0.45;
/// A reference whose sweep is weaker than this re the emitted level is refused, dB.
pub const MIN_REFERENCE_DB: f64 = -40.0;
/// Most points of the stored IR (the protocol's `TraceIr::MAX_POINTS`): 1.37 s at 48 kHz is
/// kept sample for sample, so a sweep's IR exports as a WAV; longer spans are decimated
/// peak-preserving.
pub const IR_POINTS: usize = 65_536;
/// Corner of the harmonic windows' high-pass re the lowest analysed fundamental: H2's band
/// at twice that fundamental is then 0.002 dB down (see `high_passed`).
const HARMONIC_HIGH_PASS: f64 = 0.75;

/// Regularisation of the spectral division, relative to the reference's peak bin power.
const REGULARISATION: f64 = 1e-6;
/// Earliest arrival searched, seconds before the reference.
const EARLIEST_ARRIVAL_S: f64 = 0.02;
/// The noise window ends this long before the end of the post-roll.
const NOISE_MARGIN_S: f64 = 0.01;
/// The room parameters' IR starts at most this long before the arrival, seconds.
const ROOM_PRE_S: f64 = 0.1;
/// Samples at or above this magnitude count as clipped.
const CLIP: f64 = 0.999;
/// Samples either side of the whole-sample peak that the fine arrival is interpolated from.
/// The span cuts off the band-limited peak's tails, which decay only as 1/t; the cut biases
/// the vertex by ≈ 0.002 sample at ±16 and ≈ 0.0004 at ±32.
const FINE_HALF: usize = 32;
/// Interpolation factor before the parabolic vertex: a parabola through raw samples of a
/// sinc-shaped peak is biased by up to ≈ 0.05 sample; on a ×16 band-limited grid the peak is
/// that much closer to a parabola that its own bias falls below 0.001 sample.
const FINE_UPSAMPLE: usize = 16;

/// What was played and how to analyse it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SweepSpec {
    /// The sweep as asked: its band is the one analysed. What is emitted is
    /// [`SweepTiming::emitted`].
    pub ess: EssConfig,
    /// Emitted level, dBFS RMS of the constant-envelope part.
    pub level_dbfs: f64,
    /// Sample rate, Hz.
    pub sample_rate: f64,
    /// Highest harmonic order analysed (2 … [`MAX_ORDER`]).
    pub max_order: u8,
    /// Linear-response gate after the arrival, seconds; `None` = up to the noise window.
    pub gate_s: Option<f64>,
    /// Silence recorded after each sweep, seconds (at least [`MIN_POST_ROLL_S`]); `None` =
    /// the shortest the analysis needs. The room's decay and its noise must fit in it.
    pub tail_s: Option<f64>,
    /// Grid of the reported curves.
    pub grid: LogGrid,
    /// Harmonic windows at the lowest columns.
    pub lf_harmonics: LfHarmonics,
}

/// Harmonic windows where 1/24 octave is narrower than a few resolution cells (the lowest
/// columns, harmonics below about 1 kHz).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LfHarmonics {
    /// Every order in one window, short enough for the highest order and capped at
    /// [`MAX_WINDOW_S`]: the lowest floor, but at 25 Hz a resolution cell is 40 % of the
    /// fundamental, so a harmonic that changes steeply with frequency (behind a roll-off
    /// ahead of the distortion) reads its mean over the cell, a few tenths of a dB high.
    #[default]
    Standard,
    /// Each order in the longest window that fits between its neighbours' impulses (on a
    /// 5.5 s sweep about 0.47 s for H2, 0.16 s for H5; twice that on an 11 s sweep), with the
    /// fundamental and the floor in the same window: finer resolution and columns reported
    /// from 2/window up, for a floor raised by the window's length over the standard one.
    /// The post-roll grows to four of the longest window; above these columns everything reads
    /// as `Standard`. Its floor comes from later in the silence, where a digital path that
    /// aliases (a nonlinearity without oversampling) leaves its folded products at hundreds
    /// of hertz, and then reads them rather than the noise.
    Fine,
}

/// Why an analysis was refused.
#[derive(Debug, Clone, PartialEq)]
pub enum SweepError {
    /// The sweep parameters are invalid.
    Sweep(GeneratorError),
    /// `max_order` outside 2 … [`MAX_ORDER`].
    BadOrder,
    /// The gate is not positive and finite.
    BadGate,
    /// The tail is not finite or longer than [`MAX_TAIL_S`].
    BadTail,
    /// No repeat, or the two inputs differ in length.
    BadRecording,
    /// The reference carries no sweep (or a much weaker one than emitted).
    NoReference {
        /// Strongest sweep found re the emitted level, dB.
        level_db: f64,
    },
    /// Fewer sweeps than repeats were found, or the recording stops before a sweep's
    /// post-roll ends.
    Incomplete {
        /// Sweeps found whole.
        found: usize,
        /// Sweeps expected.
        expected: usize,
    },
    /// The measurement input is silent.
    NoSignal,
}

impl std::fmt::Display for SweepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sweep(e) => write!(f, "sweep: {e}"),
            Self::BadOrder => write!(f, "harmonic order must be 2 … {MAX_ORDER}"),
            Self::BadGate => write!(f, "the gate must be positive"),
            Self::BadTail => write!(
                f,
                "the silence after the sweep must be at most {MAX_TAIL_S} s"
            ),
            Self::BadRecording => write!(f, "the recording is empty or its inputs differ"),
            Self::NoReference { level_db } => write!(
                f,
                "the reference input carries no sweep (strongest {level_db:.1} dB re emitted): \
                 check the loopback cable and that the reference output plays the sweep"
            ),
            Self::Incomplete { found, expected } => write!(
                f,
                "found {found} of {expected} sweeps with their silence after them"
            ),
            Self::NoSignal => write!(f, "the measurement input is silent"),
        }
    }
}

impl std::error::Error for SweepError {}

impl From<GeneratorError> for SweepError {
    fn from(e: GeneratorError) -> Self {
        Self::Sweep(e)
    }
}

/// Timing derived from a [`SweepSpec`]: the synchronised sweep, its windows and the silence
/// each repeat needs after it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SweepTiming {
    /// The sweep as emitted: [`SweepSpec::ess`] at the same rate, started up to
    /// [`ONSET_EXTENSION`] lower and fading in up to the asked start.
    pub emitted: EssConfig,
    /// The emitted sweep as generated.
    pub plan: EssPlan,
    /// Harmonic window before `t_k`, seconds.
    pub pre_s: f64,
    /// Harmonic window after `t_k`, seconds.
    pub post_s: f64,
    /// Silence after each sweep, seconds.
    pub post_roll_s: f64,
    /// The leading part of the post-roll that the shared noise windows, the linear gate and the room
    /// response read, seconds: the post-roll without [`LfHarmonics::Fine`]'s extension. Past
    /// the response, products that alias land at lower frequencies the later their lag, so
    /// windows moved later would read another floor than the same sweep without Fine.
    pub shared_roll_s: f64,
}

impl SweepTiming {
    /// Validates `spec` and derives the timing.
    pub fn new(spec: &SweepSpec) -> Result<Self, SweepError> {
        if !(2..=MAX_ORDER).contains(&spec.max_order) {
            return Err(SweepError::BadOrder);
        }
        if spec.gate_s.is_some_and(|g| !(g.is_finite() && g > 0.0)) {
            return Err(SweepError::BadGate);
        }
        if spec
            .tail_s
            .is_some_and(|t| !(t.is_finite() && t <= MAX_TAIL_S))
        {
            return Err(SweepError::BadTail);
        }
        if !spec.level_dbfs.is_finite() {
            return Err(SweepError::Sweep(GeneratorError::NonFiniteLevel));
        }
        let emitted = extended(&spec.ess, spec.sample_rate)?;
        let plan = EssPlan::new(&emitted, spec.sample_rate)?;
        let k = f64::from(spec.max_order);
        let l = plan.rate_s;
        let pre_s = PRE_FRACTION * l * ((k + 1.0) / k).ln();
        let post_s = (1.0 - PRE_FRACTION) * l * (k / (k - 1.0)).ln();
        // The noise in a window grows with its length while the sweep's energy per hertz
        // grows with L: capping the window lets a longer sweep lower the floor.
        let shrink = (MAX_WINDOW_S / (pre_s + post_s)).min(1.0);
        let (mut pre_s, mut post_s) = (pre_s * shrink, post_s * shrink);
        // The k-th harmonic's IR is band-limited from k·f_lo ≥ 4/W up, and that band edge
        // rings about W/4 on each side of t_k: a rise of W/3 keeps it, where the next-higher
        // order still lies a whole window away so the windows do not overlap.
        let window = pre_s + post_s;
        if l * ((k + 1.0) / k).ln() >= window {
            pre_s = pre_s.max(window / 3.0);
            post_s = window - pre_s;
        }
        let lf_max = match spec.lf_harmonics {
            LfHarmonics::Standard => 0.0,
            LfHarmonics::Fine => (2..=spec.max_order)
                .map(|k| {
                    let (pre, post) = lf_window(l, k);
                    pre + post
                })
                .fold(0.0, f64::max),
        };
        let shared_roll_s = MIN_POST_ROLL_S
            .max(4.0 * (pre_s + post_s))
            .max(spec.tail_s.unwrap_or(0.0));
        Ok(Self {
            emitted,
            plan,
            pre_s,
            post_s,
            post_roll_s: shared_roll_s.max(4.0 * lf_max),
            shared_roll_s,
        })
    }

    /// Sweep length, samples.
    pub fn sweep_samples(&self) -> usize {
        self.plan.len
    }

    /// Silence after each sweep, samples.
    pub fn post_roll_samples(&self, fs: f64) -> usize {
        (self.post_roll_s * fs).ceil() as usize
    }

    /// One repeat (sweep + silence), samples.
    pub fn period_samples(&self, fs: f64) -> usize {
        self.sweep_samples() + self.post_roll_samples(fs)
    }

    /// Length of the harmonic window, seconds.
    pub fn window_s(&self) -> f64 {
        self.pre_s + self.post_s
    }

    /// Lowest frequency the emitted sweep plays at full level, Hz.
    pub fn full_level_hz(&self) -> f64 {
        self.emitted.start_hz * (self.emitted.fade_in_s / self.plan.rate_s).exp()
    }

    /// Time of the k-th harmonic's impulse response re the linear one, seconds (negative).
    pub fn harmonic_time_s(&self, k: u8) -> f64 {
        -self.plan.rate_s * f64::from(k).ln()
    }
}

/// The sweep emitted for `asked`: the same rate, started up to [`ONSET_EXTENSION`] lower and
/// fading in (half-cosine in time, so raised-cosine in log-frequency) up to the asked start,
/// which replaces the asked fade-in. The start is a whole number of cycles per rate constant
/// (`f·L` an integer, which keeps the harmonics' impulses in phase), at least
/// [`MIN_EMITTED_START_HZ`] and one cycle; when no such start lies below the asked one, the
/// asked sweep is emitted unchanged.
fn extended(asked: &EssConfig, fs: f64) -> Result<EssConfig, SweepError> {
    let rate = EssPlan::new(asked, fs)?.rate_s;
    let cycles = (asked.start_hz * rate).round();
    let lowest = (cycles / ONSET_EXTENSION)
        .floor()
        .max((MIN_EMITTED_START_HZ * rate).ceil())
        .max(1.0);
    if lowest >= cycles {
        return Ok(*asked);
    }
    let start_hz = lowest / rate;
    Ok(EssConfig {
        start_hz,
        duration_s: rate * (asked.end_hz / start_hz).ln(),
        fade_in_s: rate * (asked.start_hz / start_hz).ln(),
        ..*asked
    })
}

/// One harmonic order's distortion curve.
#[derive(Debug, Clone, PartialEq)]
pub struct HarmonicCurve {
    /// Order (2 = second harmonic).
    pub order: u8,
    /// Level re the fundamental at each grid column's fundamental frequency, dB; NaN where
    /// this order is not measured (outside its band).
    pub level_db: Vec<f64>,
    /// Noise in the same window, dB re the fundamental; NaN where not measured.
    pub floor_db: Vec<f64>,
}

/// Result of [`analyse_recording`].
#[derive(Debug, Clone, PartialEq)]
pub struct SweepAnalysis {
    /// Sample rate, Hz.
    pub sample_rate: f64,
    /// Rate constant L of the sweep, s.
    pub rate_s: f64,
    /// Actual sweep duration, s.
    pub duration_s: f64,
    /// Repeats averaged.
    pub repeats: usize,
    /// Arrival of the linear response re the reference, s: the peak of the impulse
    /// response, to a fraction of a sample.
    pub arrival_s: f64,
    /// The reference's sweep re the emitted level (loopback gain), dB.
    pub reference_db: f64,
    /// Onset of the first sweep in the recording, s, to a fraction of a sample (output →
    /// reference latency plus whatever preceded the sweep in the recording): where the
    /// emitted sweep aligns with the reference.
    pub first_onset_s: f64,
    /// Harmonic window before and after `t_k`, s.
    pub harmonic_window_s: (f64, f64),
    /// Linear window before and after the arrival, s.
    pub linear_window_s: (f64, f64),
    /// Start of the earliest noise window re the arrival, s (the windows tile from there to
    /// the end of the post-roll).
    pub noise_window_s: f64,
    /// Grid column frequencies (fundamental), Hz.
    pub frequencies: Vec<f64>,
    /// Fundamental response magnitude, dB re the reference; NaN outside the sweep's band.
    pub magnitude_db: Vec<f64>,
    /// Fundamental phase, degrees in (−180, 180], referred to the arrival; NaN outside.
    pub phase_deg: Vec<f64>,
    /// H2 … H`max_order`.
    pub harmonics: Vec<HarmonicCurve>,
    /// Total harmonic distortion (power sum of the orders in band) re the fundamental, dB.
    pub thd_db: Vec<f64>,
    /// Power sum of the orders' noise floors re the fundamental, dB.
    pub thd_floor_db: Vec<f64>,
    /// The reference input's own H2 … H`max_order`, separated the same way: the distortion
    /// of the loopback path, which bounds what the measurement's harmonics can claim where
    /// that path is also part of the measured one.
    pub reference_harmonics: Vec<HarmonicCurve>,
    /// Impulse response from the highest order's window to the end of the linear window,
    /// decimated to at most [`IR_POINTS`] (signed extreme per bucket).
    pub ir: Vec<f64>,
    /// Hilbert envelope of the same span, dB (maximum per bucket).
    pub ir_etc_db: Vec<f64>,
    /// Time of `ir[0]` re the arrival, s.
    pub ir_t0_s: f64,
    /// Spacing of `ir`, s.
    pub ir_dt_s: f64,
    /// A sample of either input reached full scale.
    pub clipped: bool,
    /// ISO 3382-1 room parameters of the IR (times re the arrival).
    pub room: RoomAnalysis,
    /// End of the IR the room parameters were computed from, s re the arrival.
    pub room_end_s: f64,
    /// Ranges (Hz) where the response's columns are narrower than one cell of the linear
    /// window ([`crate::resolution`]).
    pub unresolved_response: Vec<(f64, f64)>,
    /// Ranges (Hz, fundamental) where the distortion curves' columns are narrower than the
    /// coarsest cell of a harmonic window that serves them: order k's cell `1/W` at `k·f`
    /// spans `1/(k·W)` of fundamental.
    pub unresolved_harmonics: Vec<(f64, f64)>,
}

/// Whether a distortion point counts: [`FLOOR_MARGIN_DB`] above its noise floor.
pub fn is_valid(level_db: f64, floor_db: f64) -> bool {
    level_db.is_finite() && (!floor_db.is_finite() || level_db >= floor_db + FLOOR_MARGIN_DB)
}

/// dB re fundamental as percent.
pub fn db_to_percent(db: f64) -> f64 {
    100.0 * 10f64.powf(db / 20.0)
}

/// Order `k`'s longest window (pre, post) at rate `l`, seconds: from a tenth of the way to
/// the next-higher order's impulse to nine tenths of the way to the next-lower one, the rise
/// widened to a third of the window where the next-higher order still lies a window away
/// (as for the shared window).
fn lf_window(l: f64, k: u8) -> (f64, f64) {
    let k = f64::from(k);
    let up = l * ((k + 1.0) / k).ln();
    let down = l * (k / (k - 1.0)).ln();
    let pre = PRE_FRACTION * up;
    let post = (1.0 - PRE_FRACTION) * down;
    let w = pre + post;
    if up >= w {
        (w / 3.0, w - w / 3.0)
    } else {
        (pre, post)
    }
}

#[cfg(test)]
mod tests;
